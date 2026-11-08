#!/usr/bin/env python3
"""Bounded native Codex contract probe; requires websockets==15.0.1.

Creates its own CODEX_HOME, server, and threads. No user configuration is edited.
Model turns are opt-in. The simulated CLI accepts only this probe's harmless
inspect_caller approval once; the simulated Prism client never answers approvals.
"""

import argparse
import asyncio
import json
import os
from pathlib import Path
import platform
import shutil
import shlex
import subprocess
import sys
import tempfile
import time
import uuid

RPC_TIMEOUT = 20
TURN_TIMEOUT = 120
IDENTITY_ENV = ("CODEX_THREAD_ID", "CODEX_SESSION_ID", "TMUX_PANE", "TMUX")
IDENTITY_KEYS = {"threadId", "thread_id", "sessionId", "session_id", "turnId", "turn_id", "windowId"}


def identity_fields(value):
    """Record only identity fields, never arbitrary metadata or credentials."""
    found = {}
    if isinstance(value, dict):
        for key, child in value.items():
            if key in IDENTITY_KEYS and isinstance(child, str):
                found[key] = child
            elif isinstance(child, dict):
                found.update(identity_fields(child))
    return found


def serve_mcp():
    for line in sys.stdin:
        request = json.loads(line)
        if "id" not in request:
            continue
        method = request.get("method")
        if method == "initialize":
            result = {"protocolVersion": request["params"]["protocolVersion"],
                      "capabilities": {"tools": {}},
                      "serverInfo": {"name": "prism-native-probe", "version": "1"}}
        elif method == "tools/list":
            result = {"tools": [{"name": "inspect_caller", "description": "Inspect native caller identity for the Prism integration probe.",
                                 "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}}]}
        elif method == "tools/call":
            meta = request.get("params", {}).get("_meta", {})
            observation = {"meta_keys": sorted(meta) if isinstance(meta, dict) else [],
                           "identity": identity_fields(meta),
                           "environment": {key: os.environ[key] for key in IDENTITY_ENV if key in os.environ},
                           "bootstrap_present": bool(os.environ.get("PRISM_PROBE_BOOTSTRAP")),
                           "pid": os.getpid(), "parent_pid": os.getppid()}
            result = {"content": [{"type": "text", "text": json.dumps(observation)}]}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)


class Rpc:
    def __init__(self, socket):
        self.socket = socket
        self.next_id = 0
        self.pending = {}
        self.events = []
        self.reader = asyncio.create_task(self.read())

    async def read(self):
        try:
            async for raw in self.socket:
                message = json.loads(raw)
                if "method" not in message and message.get("id") in self.pending:
                    if not self.pending[message["id"]].done():
                        self.pending[message["id"]].set_result(message)
                else:
                    self.events.append(message)
        except Exception:
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(ConnectionError("native connection closed"))

    async def call(self, method, params):
        self.next_id += 1
        request_id = self.next_id
        future = asyncio.get_running_loop().create_future()
        self.pending[request_id] = future
        try:
            await self.socket.send(json.dumps({"id": request_id, "method": method, "params": params}))
            response = await asyncio.wait_for(future, RPC_TIMEOUT)
            if "error" in response:
                raise RuntimeError(f"{method}: {response['error']['code']}: {response['error']['message']}")
            return response["result"]
        finally:
            self.pending.pop(request_id, None)

    async def close(self):
        await self.socket.close()
        await self.reader


def check(report, name, passed, evidence):
    report["checks"][name] = {"status": "pass" if passed else "fail", "evidence": evidence}


async def connect(path, name):
    from websockets.asyncio.client import unix_connect

    socket = await unix_connect(str(path), open_timeout=RPC_TIMEOUT)
    rpc = Rpc(socket)
    initialized = await rpc.call("initialize", {"clientInfo": {"name": name, "version": "1"},
                                               "capabilities": {"experimentalApi": True}})
    await socket.send(json.dumps({"method": "initialized", "params": {}}))
    return rpc, initialized


async def inspect_mcp(rpc, thread_id, report, name, server="prism_probe"):
    result = await rpc.call("mcpServer/tool/call", {"threadId": thread_id, "server": server, "tool": "inspect_caller", "arguments": {}})
    observed = json.loads(result["content"][0]["text"])
    native_ids = list(observed["identity"].values()) + list(observed["environment"].values())
    check(report, name, thread_id in native_ids, observed)
    check(report, "mcp_server_bootstrap_environment", observed["bootstrap_present"], "Explicit env_vars forwards server bootstrap environment; secret value never emitted.")
    return observed


def is_probe_approval(event, thread_id, turn_id):
    params = event.get("params", {})
    meta = params.get("_meta", {})
    return (event.get("method") == "mcpServer/elicitation/request" and "id" in event
            and params.get("threadId") == thread_id and params.get("turnId") == turn_id
            and params.get("serverName") == "prism_probe" and params.get("mode") == "form"
            and meta.get("codex_approval_kind") == "mcp_tool_call"
            and meta.get("tool_name") == "inspect_caller" and meta.get("tool_params") == {})


async def model_probe(first, second, thread_id, report, args, directory, env, tmux_socket):
    request_id = str(uuid.uuid4())
    reply_marker = "COMMAND_DONE" if args.tui_command else "PROBE_DONE"
    command_file = directory / "prism-command-approved"
    command = "printf prism-probe > " + shlex.quote(str(command_file))
    prompt = ("Run exactly this shell command once using exec_command with sandbox_permissions=require_escalated and a one-time approval request: " + command + ". This writes only the probe-owned temporary file. Do not request a persistent rule. After it succeeds reply COMMAND_DONE. Do not use other tools.") if args.tui_command else "Call the prism_probe inspect_caller tool exactly once, then reply PROBE_DONE. Do not use other tools."
    started = await first.call("turn/start", {"threadId": thread_id, "clientUserMessageId": request_id,
                                              "model": args.model, "input": [{"type": "text", "text": prompt}]})
    turn_id = started["turn"]["id"]
    check(report, "turn_acknowledgement", bool(turn_id), {"turn_id": turn_id})
    resumed = await second.call("thread/resume", {"threadId": thread_id})
    check(report, "two_client_exact_resume", resumed["thread"]["id"] == thread_id, {"after_turn_start": True})
    if args.tui_approval:
        command = shlex.join(["env", "CODEX_HOME=" + env["CODEX_HOME"], args.codex, "--remote", "unix://" + str(directory / "server.sock"), "resume", thread_id])
        subprocess.run(["tmux", "-L", tmux_socket, "new-session", "-d", "-s", "probe", "-x", "120", "-y", "40", "-c", str(directory), command], check=True, timeout=RPC_TIMEOUT)
        print("Isolated TUI probe: tmux -L " + tmux_socket + " capture-pane -p -t probe", file=sys.stderr, flush=True)
    steered = await second.call("turn/steer", {"threadId": thread_id, "expectedTurnId": turn_id,
                                             "clientUserMessageId": str(uuid.uuid4()),
                                             "input": [{"type": "text", "text": "This is a native steering probe; continue the original request."}]})
    check(report, "busy_steering", steered.get("turnId") == turn_id, {"same_turn": steered.get("turnId") == turn_id})
    deadline = time.monotonic() + TURN_TIMEOUT
    approved_request = None
    while time.monotonic() < deadline:
        if approved_request is None and not args.tui_approval:
            for event in first.events:
                if is_probe_approval(event, thread_id, turn_id):
                    approved_request = event["id"]
                    await first.socket.send(json.dumps({"id": approved_request, "result": {"action": "accept", "content": {}}}))
                    break
        if any(e.get("method") == "turn/completed" and e.get("params", {}).get("threadId") == thread_id and e.get("params", {}).get("turn", {}).get("id") == turn_id for e in second.events):
            break
        await asyncio.sleep(0.05)
    else:
        await second.call("turn/interrupt", {"threadId": thread_id, "turnId": turn_id})

    for label, client in [("first", first)]:
        matched = [event for event in client.events if event.get("method") == "turn/started" and event.get("params", {}).get("threadId") == thread_id and event.get("params", {}).get("turn", {}).get("id") == turn_id]
        check(report, f"turn_started_{label}", bool(matched), {"matched_events": len(matched)})
    completed = [e["params"]["turn"] for e in second.events if e.get("method") == "turn/completed" and e.get("params", {}).get("threadId") == thread_id and e.get("params", {}).get("turn", {}).get("id") == turn_id]
    check(report, "model_turn_completed", bool(completed) and completed[-1]["status"] == "completed", {"status": completed[-1]["status"] if completed else "timeout", "error": completed[-1].get("error") if completed else None})
    completed_items = [e["params"]["item"] for e in first.events if e.get("method") == "item/completed" and e.get("params", {}).get("threadId") == thread_id and e.get("params", {}).get("turnId") == turn_id]
    tool_items = [item for item in completed_items if item.get("type") == "mcpToolCall" and item.get("server") == "prism_probe" and item.get("tool") == "inspect_caller" and item.get("status") == "completed" and not item.get("error")]
    observations = []
    for item in tool_items:
        for content in (item.get("result") or {}).get("content", []):
            if content.get("type") == "text":
                try:
                    observation = json.loads(content["text"])
                    identity = observation.get("identity", {})
                    if "meta_keys" in observation and identity.get("threadId") == thread_id and identity.get("thread_id") == thread_id and identity.get("turn_id") == turn_id:
                        observations.append(observation)
                except (ValueError, TypeError):
                    continue
    if not args.tui_command:
        check(report, "model_generated_mcp_caller", bool(observations), observations)
    check(report, "simulated_cli_mcp_approval", approved_request is not None and bool(observations), {"cli_replied_once": approved_request is not None, "prism_replied": False, "real_tui": False})
    if args.tui_approval:
        report["checks"].pop("simulated_cli_mcp_approval", None)
        approval_ids = {event["id"] for event in first.events if is_probe_approval(event, thread_id, turn_id)}
        if args.tui_command:
            approval_ids = {event["id"] for event in first.events if event.get("method") == "item/commandExecution/requestApproval" and event.get("params", {}).get("threadId") == thread_id and event.get("params", {}).get("turnId") == turn_id and str(command_file) in event.get("params", {}).get("command", "")}
        resolved = any(event.get("method") == "serverRequest/resolved" and event.get("params", {}).get("threadId") == thread_id and event.get("params", {}).get("requestId") in approval_ids for event in first.events)
        assistant_replied = any(item.get("type") == "agentMessage" and item.get("text", "").strip() == reply_marker for item in completed_items)
        pane_deadline = time.monotonic() + 2
        while True:
            pane = subprocess.run(["tmux", "-L", tmux_socket, "capture-pane", "-p", "-t", "probe"], capture_output=True, text=True, timeout=RPC_TIMEOUT)
            visible_reply = pane.returncode == 0 and any(line.strip().removeprefix("•").strip() == reply_marker for line in pane.stdout.splitlines())
            if visible_reply or time.monotonic() >= pane_deadline:
                break
            await asyncio.sleep(0.05)
        tool_completed = bool(observations)
        if args.tui_command:
            tool_completed = any(item.get("type") == "commandExecution" and item.get("exitCode") == 0 and item.get("status") == "completed" and str(command_file) in item.get("command", "") for item in completed_items) and command_file.exists() and command_file.read_text() == "prism-probe"
        passed = tool_completed and resolved and assistant_replied and visible_reply and bool(completed) and completed[-1]["status"] == "completed"
        check(report, "real_tui_command_approval" if args.tui_command else "real_tui_mcp_approval", passed, {"tool_completed": tool_completed, "approval_requested": bool(approval_ids), "approval_resolved": resolved, "assistant_replied": assistant_replied, "tui_shows_assistant_reply": visible_reply, "rpc_clients_replied": False})
    report["native_event_methods"] = {label: sorted({e.get("method", "") for e in client.events}) for label, client in [("first", first), ("second", second)]}
    report["approval_requests"] = {label: [e.get("method") for e in client.events if "id" in e and "method" in e] for label, client in [("first", first), ("second", second)]}
    report["native_errors"] = [e.get("params", {}).get("error") for e in first.events if e.get("method") == "error"]
    if args.tui_switch:
        await probe_tui_switch(first, second, thread_id, tmux_socket, report)


async def probe_tui_switch(first, second, thread_id, tmux_socket, report):
    before = await first.call("thread/loaded/list", {})
    first_offset, second_offset = len(first.events), len(second.events)
    subprocess.run(["tmux", "-L", tmux_socket, "send-keys", "-t", "probe", "/new", "Enter"], check=True, timeout=RPC_TIMEOUT)
    await asyncio.sleep(0.5)
    gap_events = first.events[first_offset:] + second.events[second_offset:]
    gap_lifecycle = [event for event in gap_events if event.get("method") in {"thread/started", "thread/closed", "thread/unsubscribed", "thread/resumed"}]
    report["checks"]["tui_new_before_input"] = {"status": "untestable", "evidence": {"observer_event_methods": sorted({event.get("method", "") for event in gap_events}), "lifecycle_events": gap_lifecycle, "reason": "Native observers do not establish which thread the TUI currently focuses before its next input; sends remain disabled."}}
    subprocess.run(["tmux", "-L", tmux_socket, "send-keys", "-t", "probe", "Reply SWITCH_DONE only; do not use tools.", "Enter"], check=True, timeout=RPC_TIMEOUT)
    after = before
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        await asyncio.sleep(0.1)
        after = await first.call("thread/loaded/list", {})
        if set(after["data"]) - set(before["data"]):
            break
    await asyncio.sleep(0.3)
    observations = first.events[first_offset:] + second.events[second_offset:]
    new_threads = set(after["data"]) - set(before["data"])
    lifecycle = [e for e in observations if e.get("method") in {"thread/started", "thread/closed", "thread/unsubscribed", "thread/resumed"}]
    authoritative = [e for e in lifecycle if e.get("params", {}).get("thread", {}).get("id") in new_threads or e.get("params", {}).get("threadId") == thread_id]
    check(report, "tui_switch_observable", bool(new_threads) and bool(authoritative), {"new_loaded_threads": len(new_threads), "observer_event_methods": sorted({e.get("method", "") for e in observations}), "lifecycle_events": lifecycle})


def install_probe_plugin(args, directory, env, report):
    marketplace = directory / "marketplace"
    catalog = marketplace / ".agents/plugins"
    plugin = marketplace / "plugins/prism-probe"
    catalog.mkdir(parents=True)
    (plugin / ".codex-plugin").mkdir(parents=True)
    (plugin / "hooks").mkdir()
    manifest = {"name": "prism-probe", "version": "0.1.0", "description": "Native integration contract probe", "hooks": "./hooks/hooks.json", "mcpServers": "./.mcp.json"}
    (plugin / ".codex-plugin/plugin.json").write_text(json.dumps(manifest))
    plugin_mcp = {"mcpServers": {"plugin_probe": {"command": sys.executable, "args": [str(Path(__file__).resolve()), "--mcp-server"], "env_vars": ["PRISM_PROBE_BOOTSTRAP"]}}}
    (plugin / ".mcp.json").write_text(json.dumps(plugin_mcp))
    marker = directory / "hook-observed"
    hook_script = plugin / "hooks/session_start.py"
    hook_script.write_text("from pathlib import Path\nPath(" + repr(str(marker)) + ").write_text('observed')\n")
    hook = {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": shlex.quote(sys.executable) + ' "${PLUGIN_ROOT}/hooks/session_start.py"'}]}]}}
    (plugin / "hooks/hooks.json").write_text(json.dumps(hook))
    entry = {"name": "prism-probe", "source": {"source": "local", "path": "./plugins/prism-probe"}, "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"}, "category": "Productivity"}
    (catalog / "marketplace.json").write_text(json.dumps({"name": "prism-probe-local", "plugins": [entry]}))
    for name, command in [("native_marketplace_add", ["plugin", "marketplace", "add", str(marketplace), "--json"]),
                          ("native_plugin_install", ["plugin", "add", "prism-probe@prism-probe-local", "--json"])]:
        result = subprocess.run([args.codex, *command], cwd=directory, env=env, capture_output=True, text=True, timeout=RPC_TIMEOUT)
        check(report, name, result.returncode == 0, {"exit_code": result.returncode, "stderr": result.stderr[:500] if result.returncode else ""})
    return marker


async def cleanup_native(server, clients, tmux_socket, report):
    errors = []
    try:
        try:
            if tmux_socket:
                subprocess.run(["tmux", "-L", tmux_socket, "kill-server"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=RPC_TIMEOUT)
        except Exception as error:
            errors.append("tmux: " + type(error).__name__)
        finally:
            for client in clients:
                try:
                    await asyncio.wait_for(client.close(), RPC_TIMEOUT)
                except Exception as error:
                    errors.append("client: " + type(error).__name__)
    finally:
        try:
            server.terminate()
        except ProcessLookupError:
            pass
        finally:
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
    check(report, "native_cleanup", not errors, errors)


async def run_native(args, directory, report):
    home = directory / "codex-home"
    home.mkdir(mode=0o700)
    env = dict(os.environ)
    env["CODEX_HOME"] = str(home)
    env["PRISM_PROBE_BOOTSTRAP"] = uuid.uuid4().hex
    for key in IDENTITY_ENV:
        env.pop(key, None)
    if args.auth_file:
        shutil.copyfile(args.auth_file, home / "auth.json")
        (home / "auth.json").chmod(0o600)
    config = '\n'.join(['[mcp_servers.prism_probe]', f'command = {json.dumps(sys.executable)}',
                        f'args = [{json.dumps(str(Path(__file__).resolve()))}, "--mcp-server"]',
                        'env_vars = ["PRISM_PROBE_BOOTSTRAP"]',
                        'startup_timeout_sec = 10', 'tool_timeout_sec = 10'])
    (home / "config.toml").write_text(config + '\n')
    marker = install_probe_plugin(args, directory, env, report)
    socket_path = directory / "server.sock"
    tmux_socket = "prism-native-" + uuid.uuid4().hex[:12]
    clients = []
    with (directory / "server.log").open("w") as log:
        server = subprocess.Popen([args.codex, "app-server", "--listen", "unix://" + str(socket_path)],
                                  env=env, cwd=directory, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + RPC_TIMEOUT
            while not socket_path.exists():
                if server.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("isolated app-server did not open its Unix socket")
                await asyncio.sleep(0.05)
            first, initialized = await connect(socket_path, "prism_probe_cli")
            clients.append(first)
            second, _ = await connect(socket_path, "prism_probe_ui")
            clients.append(second)
            check(report, "unix_websocket_initialize", initialized["codexHome"] == str(home), {"user_agent": initialized["userAgent"], "socket_mode": oct(socket_path.stat().st_mode & 0o777)})
            diagnostics = await first.call("server/diagnostics", {})
            check(report, "running_server_identity", diagnostics["process"]["id"] > 0, {"server_pid": diagnostics["process"]["id"]})
            created = await first.call("thread/start", {"cwd": str(directory), "sandbox": "read-only", "approvalPolicy": "on-request", "model": args.model})
            thread = created["thread"]
            report["checks"]["native_plugin_hooks"] = {"status": "pass" if marker.exists() else "untestable", "evidence": "Hook ran" if marker.exists() else "Installed hook did not run without trust; no trust bypass used."}
            await inspect_mcp(first, thread["id"], report, "mcp_exact_caller_first")
            servers = await first.call("mcpServerStatus/list", {})
            report["plugin_mcp_names"] = [server["name"] for server in servers["data"] if "plugin_probe" in server["name"]]
            if "plugin_probe" in report["plugin_mcp_names"]:
                await inspect_mcp(first, thread["id"], report, "native_plugin_mcp_caller", "plugin_probe")
            try:
                resumed = await second.call("thread/resume", {"threadId": thread["id"]})
                check(report, "two_client_exact_resume", resumed["thread"]["id"] == thread["id"], {"thread_id": thread["id"], "session_id": thread["sessionId"]})
            except RuntimeError as error:
                expected_error = "thread/resume: -32600: no rollout found for thread id " + thread["id"]
                check(report, "empty_thread_resume", str(error) == expected_error, {"supported": False, "error": str(error)})
            other = await second.call("thread/start", {"cwd": str(directory), "sandbox": "read-only", "approvalPolicy": "on-request", "model": args.model})
            await inspect_mcp(second, other["thread"]["id"], report, "mcp_exact_caller_second_thread")
            try:
                await second.call("turn/steer", {"threadId": thread["id"], "expectedTurnId": "prism-invalid-turn", "input": [{"type": "text", "text": "Native protocol precondition probe."}]})
                check(report, "idle_steer_rejected", False, "Unexpected success")
            except RuntimeError as error:
                check(report, "idle_steer_rejected", "No active turn" in str(error) or "no active turn" in str(error), str(error))
            if args.model_turns:
                await model_probe(first, second, thread["id"], report, args, directory, env, tmux_socket)
                forked = await first.call("thread/fork", {"threadId": thread["id"]})
                check(report, "fork_identity", forked["thread"]["id"] != thread["id"], {"different_thread": forked["thread"]["id"] != thread["id"], "shared_session": forked["thread"]["sessionId"] == thread["sessionId"]})
        finally:
            await cleanup_native(server, clients, tmux_socket if args.tui_approval else None, report)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex", default="codex")
    parser.add_argument("--mcp-server", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--model-turns", action="store_true", help="Run one bounded model turn in a new isolated thread")
    parser.add_argument("--tui-approval", action="store_true", help="Require manual approval in an isolated tmux Codex TUI; JSON-RPC clients never approve")
    parser.add_argument("--tui-switch", action="store_true", help="After TUI approval, test whether /new is observable by other clients")
    parser.add_argument("--tui-command", action="store_true", help="Require actual TUI approval of one command writing a probe-owned temporary file")
    parser.add_argument("--model", default="gpt-6.1-sol")
    parser.add_argument("--auth-file", type=Path, help="Copy auth into the disposable home; never print credentials")
    args = parser.parse_args()
    if args.tui_switch or args.tui_command:
        args.tui_approval = True
    if args.tui_approval:
        args.model_turns = True
    if args.mcp_server:
        serve_mcp()
        return 0
    report = {"host": platform.platform(), "date": time.strftime("%Y-%m-%d", time.gmtime()), "checks": {}}
    report["cli_version"] = subprocess.check_output([args.codex, "--version"], text=True, timeout=RPC_TIMEOUT).strip()
    for name in ["model_turn_completed", "approval_ownership", "busy_steering", "queue_cancellation", "native_plugin_hooks", "trusted_pane_bootstrap", "model_generated_mcp_caller"]:
        report["checks"][name] = {"status": "untestable", "evidence": "Not exercised by this run; capability remains disabled."}
    try:
        with tempfile.TemporaryDirectory(prefix="prism-codex-") as temporary:
            asyncio.run(run_native(args, Path(temporary), report))
    except Exception as error:
        check(report, "probe_execution", False, str(error))
    print(json.dumps(report, indent=2))
    return 1 if any(item["status"] == "fail" for item in report["checks"].values()) else 0


if __name__ == "__main__":
    sys.exit(main())
