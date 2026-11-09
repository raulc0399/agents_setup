#!/usr/bin/env python3
"""Probe the installed Claude mod runtime without credentials or model requests."""

import argparse
import hmac
import http.server
import json
import os
from pathlib import Path
import secrets
import shutil
import socketserver
import subprocess
import tempfile
import threading
import uuid


PLUGIN_NAME = "prism-probe"
COMMAND_NAME = "prism-native-probe"
TIMEOUT_SECONDS = 30
MOD_SOURCE = r"""
export function register(on) {
  on('session.start', async ($, e, next) => {
    await $.tool.register({
      name: 'identity', description: 'Check the current native conversation identity',
      inputSchema: {type: 'object', properties: {nonce: {type: 'string'}}, required: ['nonce'], additionalProperties: false},
      isDeferred: false
    });
    await $.command.register({name: 'prism-native-probe', description: 'Check Prism native runtime capabilities'});
    return next(e);
  });
  on('tool.call', {tool: 'mcp__prism-probe__identity'}, async ($, e) => {
    return {result: JSON.stringify({sessionId: await $.session.id(), nonce: e.nonce, eventKeys: Object.keys(e)})};
  });
  on('command.run', {command: 'prism-native-probe'}, async ($, e) => {
    const sessionId = await $.session.id();
    const nonce = await $.env.get('PRISM_PROBE_NONCE');
    const token = await $.env.get('PRISM_PROBE_TOKEN');
    const socketPath = await $.env.get('PRISM_PROBE_SOCKET');
    const response = await $.http.fetch('http://localhost/probe', {
      method: 'POST', socketPath,
      headers: {'Content-Type': 'application/json', Authorization: 'Bearer ' + token},
      body: JSON.stringify({sessionId, nonce})
    });
    const rejected = await $.http.fetch('http://localhost/probe', {
      method: 'POST', socketPath, body: JSON.stringify({sessionId, nonce})
    });
    const tools = await $.tool.list();
    const tool = await $.tool.call({tool: 'mcp__prism-probe__identity', nonce});
    return {text: JSON.stringify({sessionId, version: await $.session.version(),
      toolRegistered: tools.some(item => item.name === 'mcp__prism-probe__identity'),
      tool, response, unauthorizedStatus: rejected.status})};
  });
  on('session.end', async ($, e, next) => {
    const destination = await $.env.get('PRISM_PROBE_END');
    await $.fs.write(destination, JSON.stringify({sessionId: await $.session.id(), reason: e.reason}));
    return next(e);
  });
}
"""


class UnixHttpServer(socketserver.UnixStreamServer):
    """HTTP server bound only to the probe's private Unix socket."""

    allow_reuse_address = False


def server_handler(token, expected_session, expected_nonce, requests):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, format_string, *args):
            pass

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            if length > 4096:
                self.send_error(413)
                return

            authenticated = hmac.compare_digest(
                self.headers.get("Authorization", ""), "Bearer " + token
            )
            payload = json.loads(self.rfile.read(length))
            valid = (
                self.path == "/probe"
                and payload == {"sessionId": expected_session, "nonce": expected_nonce}
            )
            status = 200 if authenticated and valid else 401
            requests.append({"authenticated": authenticated, "valid": valid, "status": status})
            response = json.dumps({"sessionId": expected_session, "nonce": expected_nonce}).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(response)))
            self.end_headers()
            self.wfile.write(response)

    return Handler


def run_command(cli, args, project, env):
    result = subprocess.run(
        [cli, *args], cwd=project, env=env, capture_output=True,
        text=True, timeout=TIMEOUT_SECONDS, check=False,
    )
    if result.returncode:
        raise RuntimeError(f"Claude command failed (exit {result.returncode}): {result.stderr[-2000:]}")
    return result.stdout


def run_probe(cli):
    with tempfile.TemporaryDirectory(prefix="prism-claude-") as directory:
        root = Path(directory)
        config = root / "config"
        project = root / "project"
        plugin = root / "plugin"
        for path in [config, project, plugin / ".claude-plugin", plugin / "hooks"]:
            path.mkdir(parents=True, mode=0o700)

        manifest = {"name": PLUGIN_NAME, "version": "0.1.0", "description": "Prism native capability probe"}
        (plugin / ".claude-plugin/plugin.json").write_text(json.dumps(manifest))
        (plugin / "hooks/hooks.json").write_text(json.dumps({"modules": ["./register.js"]}))
        (plugin / "hooks/register.js").write_text(MOD_SOURCE)
        session_id = str(uuid.uuid4())
        nonce = secrets.token_hex(16)
        token = secrets.token_urlsafe(32)
        socket_path = root / "bridge.sock"
        end_path = root / "end.json"
        # Only system process essentials cross into this isolated CLI session.
        env = {key: value for key, value in os.environ.items()
               if key in {"PATH", "HOME", "LANG", "LC_ALL", "TMPDIR", "TERM"}}
        env.update({
            "CLAUDE_CONFIG_DIR": str(config), "DISABLE_AUTOUPDATER": "1",
            "DISABLE_TELEMETRY": "1", "PRISM_PROBE_SOCKET": str(socket_path),
            "PRISM_PROBE_TOKEN": token, "PRISM_PROBE_NONCE": nonce,
            "PRISM_PROBE_END": str(end_path),
        })
        version = run_command(cli, ["--version"], project, env).strip()
        validation = run_command(cli, ["plugin", "validate", str(plugin)], project, env)
        if "Validation passed" not in validation:
            raise RuntimeError("Native plugin validation did not pass")

        requests = []
        server = UnixHttpServer(str(socket_path), server_handler(token, session_id, nonce, requests))
        os.chmod(socket_path, 0o600)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            output = run_command(cli, [
                "-p", "/" + COMMAND_NAME, "--plugin-dir", str(plugin),
                "--session-id", session_id, "--no-session-persistence",
                "--setting-sources", "", "--strict-mcp-config", "--output-format", "json",
            ], project, env)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=TIMEOUT_SECONDS)

        result = json.loads(output)
        prefix = PLUGIN_NAME + ": "
        if result.get("is_error") or not result.get("result", "").startswith(prefix):
            raise RuntimeError("Native command did not execute successfully")
        observed = json.loads(result["result"][len(prefix):])
        tool = json.loads(observed["tool"]["result"])
        response = observed["response"]
        checks = {
            "plugin_validation": True,
            "session_identity": observed["sessionId"] == result["session_id"] == session_id,
            "native_tool_registration": observed["toolRegistered"],
            "native_tool_call": tool["sessionId"] == session_id and tool["nonce"] == nonce,
            "unix_http_transport": response["status"] == 200 and json.loads(response["text"])["nonce"] == nonce,
            "missing_token_rejected": observed["unauthorizedStatus"] == 401,
            "requests_bound_to_identity": len(requests) == 2 and all(item["valid"] for item in requests),
            "no_model_request": result["num_turns"] == 0 and result["duration_api_ms"] == 0,
        }
        lifecycle = json.loads(end_path.read_text()) if end_path.exists() else None
        return {
            "cli_version": version, "runtime_version": observed["version"], "checks": checks,
            "session_end_observation": lifecycle,
            "interactive_send_enabled": False,
            "unverified": ["interactive activation and approval routing", "prompt receipt correlation",
                           "busy queue cancellation", "clear/resume/branch/reload lifecycle"],
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--claude", default=shutil.which("claude") or str(Path.home() / ".local/bin/claude"))
    args = parser.parse_args()
    report = run_probe(args.claude)
    print(json.dumps(report, indent=2))
    return 0 if all(report["checks"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
