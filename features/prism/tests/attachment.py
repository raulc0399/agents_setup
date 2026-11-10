#!/usr/bin/env python3
"""Claude attachment through the real `claude` binary and the mod; no model requests.

The Codex MCP path and the binding lifecycle are covered by tests/attachment.rs.
"""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import uuid

BINARY = pathlib.Path(os.environ.get("PRISM_BINARY", "target/debug/prism")).resolve()
PLUGIN = pathlib.Path(__file__).resolve().parent.parent / "plugins/claude"


def wait_until(predicate, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.05)
    raise AssertionError("condition did not become true")


class Attachment(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="prism-attach-")
        self.project = pathlib.Path(self.temporary.name)
        self.environment = dict(os.environ)
        self.environment["PATH"] = str(BINARY.parent) + ":" + self.environment["PATH"]
        # A pairing file that does not exist: test bridges never dial a desktop app on the host.
        self.environment["PRISM_APP_FILE"] = str(self.project / "no-prism-app.json")
        self.processes = []

    def tearDown(self):
        for process in self.processes:
            process.kill()
            process.wait()
        time.sleep(1.5)
        self.temporary.cleanup()

    def start(self, command):
        process = subprocess.Popen(command, cwd=self.project, env=self.environment, stdin=subprocess.DEVNULL)
        self.processes.append(process)
        return process

    def bindings(self):
        found = []
        for path in self.project.glob(".prism/run/*/binding.json"):
            try:
                found.append((path, json.loads(path.read_text())))
            except (OSError, json.JSONDecodeError):
                return None
        return found

    def binding(self):
        found = self.bindings()
        return found[0] if found and len(found) == 1 else None

    @unittest.skipUnless(shutil.which("claude"), "the claude CLI is not on PATH")
    def test_claude_native_command_attaches_and_reports_the_app_link(self):
        claude = shutil.which("claude")
        thread = str(uuid.uuid4())
        script = self.project / "conversation.py"
        script.write_text('''import os, pathlib, subprocess, sys, time
root = pathlib.Path.cwd()
env = dict(os.environ, CLAUDE_CONFIG_DIR=str(root / "config"), DISABLE_AUTOUPDATER="1", DISABLE_TELEMETRY="1")
with root.joinpath("stdout").open("w") as out, root.joinpath("stderr").open("w") as err:
    claude = subprocess.Popen(sys.argv[1:], env=env, stdout=out, stderr=err)
    root.joinpath("claude-pid").write_text(str(claude.pid))
    returncode = claude.wait(timeout=60)
root.joinpath("done").write_text(str(returncode))
while True: time.sleep(1)
''')
        cli = self.start([sys.executable, str(script), claude, "--session-id", thread, "-p", "/prism", "--plugin-dir", str(PLUGIN), "--setting-sources", "", "--strict-mcp-config", "--output-format", "json", "--no-session-persistence"])
        wait_until(lambda: (self.project / "done").exists(), timeout=70)
        output = (self.project / "stdout").read_text()
        diagnostics = output + (self.project / "stderr").read_text()
        self.assertIn("published documents in this conversation", output, diagnostics)
        # No pairing file exists, so the link retries and `/prism` says why.
        self.assertIn("Not connected to the Prism app", output, diagnostics)
        self.assertIn("not found", output, diagnostics)
        path, binding = wait_until(self.binding)
        self.assertEqual(binding["agent"], "claude")
        self.assertEqual(binding["conversation"], thread)
        self.assertEqual(binding["server_pid"], int((self.project / "claude-pid").read_text()))
        wait_until(lambda: json.loads(path.read_text())["retired"])
        self.assertIsNone(cli.poll())


if __name__ == "__main__":
    unittest.main()
