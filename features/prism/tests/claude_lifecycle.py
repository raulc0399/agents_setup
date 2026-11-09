#!/usr/bin/env python3
"""Isolated end-to-end Claude launcher, mod bridge call and lifecycle regressions.

Run with PRISM_BINARY=/absolute/path/to/prism python3 tests/claude_lifecycle.py.
Uses the real installed `claude` CLI in print mode with an isolated CLAUDE_CONFIG_DIR,
no authentication and no model request. Standard library only.
"""
import http.client
import json
import os
import pathlib
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import unittest
import uuid


BINARY = pathlib.Path(os.environ.get("PRISM_BINARY", "target/debug/prism")).resolve()
REAL_CLAUDE = pathlib.Path(os.environ.get("PRISM_CLAUDE", shutil.which("claude") or "")).resolve()
PLUGIN = pathlib.Path(__file__).resolve().parent.parent / "plugins/claude"
CLAUDE_TIMEOUT = 90

# The wrapper is what the launcher resolves as `claude`: it records its argv, optionally runs the
# real CLI once in print mode, then stays alive until the test writes its stop file.
WRAPPER = '''
import json, os, pathlib, subprocess, sys, time
project = pathlib.Path({project!r})
project.joinpath("claude-args.json").write_text(json.dumps({{"args": sys.argv[1:], "binding": os.environ.get("PRISM_BINDING_FILE"), "pid": os.getpid()}}))
if project.joinpath("wrapper-mode").read_text().strip() == "print":
    command = [{real!r}, *sys.argv[1:], "-p", "/prism", "--plugin-dir", {plugin!r}, "--setting-sources", "",
               "--strict-mcp-config", "--output-format", "json", "--no-session-persistence"]
    env = dict(os.environ, CLAUDE_CONFIG_DIR={config!r}, DISABLE_AUTOUPDATER="1", DISABLE_TELEMETRY="1")
    with project.joinpath("claude-stdout.json").open("w") as stdout, project.joinpath("claude-stderr.txt").open("w") as stderr:
        subprocess.run(command, cwd=project, env=env, stdout=stdout, stderr=stderr, timeout={timeout})
while not project.joinpath("wrapper-stop").exists():
    time.sleep(0.1)
'''


def wait_until(predicate, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError("condition did not become true before timeout")


def alive(pid):
    try:
        return pathlib.Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] != "Z"
    except FileNotFoundError:
        return False


class ClaudeLifecycle(unittest.TestCase):
    def setUp(self):
        self.assertTrue(REAL_CLAUDE.is_file(), "installed claude CLI not found; set PRISM_CLAUDE")
        self.temporary = tempfile.TemporaryDirectory(prefix="prism-claude-")
        self.project = pathlib.Path(self.temporary.name) / "project"
        self.config = pathlib.Path(self.temporary.name) / "claude-config"
        self.project.mkdir(mode=0o700)
        self.config.mkdir(mode=0o700)
        self.name = f"prism-claude-test-{uuid.uuid4()}"
        self.tmux("new-session", "-d", "-x", "160", "-y", "48", "-s", "anchor", "cat")
        pane = self.tmux("display-message", "-p", "#{pane_id}")
        tmux_env = self.tmux("display-message", "-p", "#{socket_path},#{pid},0")
        wrapper = self.project / "claude"
        wrapper.write_text("#!" + sys.executable + WRAPPER.format(project=str(self.project), real=str(REAL_CLAUDE), plugin=str(PLUGIN), config=str(self.config), timeout=CLAUDE_TIMEOUT))
        wrapper.chmod(0o700)
        (self.project / "wrapper-mode").write_text("record")
        self.env = dict(os.environ, TMUX=tmux_env, TMUX_PANE=pane, TERM="xterm-256color", CLAUDE_CONFIG_DIR=str(self.config), DISABLE_AUTOUPDATER="1", DISABLE_TELEMETRY="1")
        self.env["PATH"] = f"{self.project}:{self.env['PATH']}"
        self.env.pop("PRISM_BINDING_FILE", None)
        self.binding = None
        self.supervisors = []

    def tearDown(self):
        (self.project / "wrapper-stop").write_text("stop")
        subprocess.run(["tmux", "-L", self.name, "kill-server"], capture_output=True)
        try:
            if self.binding:
                wait_until(lambda: not alive(self.binding["server_pid"]), timeout=20)
            # The supervisor rewrites binding.json while it retires; stop it before removing the tree.
            for pid in self.supervisors:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                wait_until(lambda: not alive(pid), timeout=20)
        finally:
            self.temporary.cleanup()

    def supervisor_pids(self):
        pids = []
        for stat in pathlib.Path("/proc").glob("[0-9]*/cmdline"):
            try:
                arguments = stat.read_bytes().split(b"\0")
            except OSError:
                continue
            if b"supervise" in arguments and str(self.binding_path).encode() in arguments:
                pids.append(int(stat.parent.name))
        return pids

    def tmux(self, *args):
        return subprocess.check_output(["tmux", "-L", self.name, *args], text=True).strip()

    def start(self, resume=None):
        command = [str(BINARY), "--agent", "claude", "--project", str(self.project), "--detach"]
        if resume:
            command += ["--resume", resume]
        result = subprocess.run(command, env=self.env, cwd=self.project, text=True, capture_output=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        launch = json.loads(result.stdout)
        self.binding_path = pathlib.Path(launch["binding"])
        self.binding = json.loads(self.binding_path.read_text())
        self.supervisors = wait_until(self.supervisor_pids)
        wait_until(lambda: (self.project / "claude-args.json").exists())
        return json.loads((self.project / "claude-args.json").read_text())

    def read_binding(self):
        return json.loads(self.binding_path.read_text())

    def call(self, operation, thread):
        connection = http.client.HTTPConnection("localhost", timeout=5)
        connection.sock = socket.socket(socket.AF_UNIX)
        try:
            connection.sock.connect(str((pathlib.Path("/tmp") / ("prism-" + self.binding_path.parent.name) / "bridge.sock")))
            data = json.dumps({"version":1,"generation":self.binding["generation"],"thread_id":thread,"operation":operation})
            connection.request("POST", "/call", data, {"Authorization":f"Bearer {self.binding['token']}","Content-Type":"application/json"})
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def test_print_mode_mod_reaches_the_bridge_and_opens_the_pane(self):
        (self.project / "wrapper-mode").write_text("print")
        native = self.start()
        self.assertEqual(self.binding["agent"], "claude")
        self.assertEqual(native["args"][0], "--session-id")
        self.assertEqual(self.binding["conversation"], native["args"][1])
        self.assertEqual(native["binding"], str(self.binding_path))
        self.assertEqual(self.binding["server_pid"], native["pid"])
        self.assertEqual(self.binding["server_pid"], self.binding["cli"]["pid"])
        self.assertEqual(int(self.tmux("display-message", "-p", "-t", self.binding["cli"]["pane_id"], "#{pane_pid}")), native["pid"])
        output = wait_until(lambda: (self.project / "claude-stdout.json").read_text() if (self.project / "claude-stdout.json").exists() else None, timeout=CLAUDE_TIMEOUT)
        result = json.loads(output)
        self.assertFalse(result["is_error"], result)
        self.assertEqual(result["session_id"], self.binding["conversation"])
        self.assertEqual(result["num_turns"], 0)
        self.assertIn("documents", result["result"])
        self.assertIn("Prism pane %", result["result"])
        pane = result["result"].split("Prism pane ", 1)[1].split()[0]
        panes = {line.split()[0]: int(line.split()[1]) for line in self.tmux("list-panes", "-a", "-F", "#{pane_id} #{pane_left}").splitlines()}
        self.assertIn(pane, panes)
        self.assertGreater(panes[pane], panes[self.binding["cli"]["pane_id"]])
        # The print run ends with session.end(other); the mod reports it and the bridge retires.
        wait_until(lambda: self.read_binding()["retired"], timeout=15)
        self.assertTrue(alive(self.binding["server_pid"]))

    def test_foreign_thread_is_rejected_and_retires_the_binding(self):
        self.start()
        status, value = self.call({"method":"inspect","params":{"document_id":None,"cursor":None}}, str(uuid.uuid4()))
        self.assertNotEqual(status, 200)
        self.assertIn("error", value)
        self.assertTrue(self.read_binding()["retired"])
        self.assertTrue(alive(self.binding["server_pid"]))

    def test_session_end_lifecycle_retires_the_binding_and_keeps_the_cli_pane(self):
        native = self.start()
        status, value = self.call({"method":"lifecycle","params":{"event":"session_end","reason":"clear"}}, native["args"][1])
        self.assertEqual(status, 200, value)
        self.assertEqual(value["data"], {"retiring": True})
        wait_until(lambda: self.read_binding()["retired"], timeout=5)
        self.assertTrue(alive(self.binding["server_pid"]))
        self.assertIn(self.binding["cli"]["pane_id"], self.tmux("list-panes", "-a", "-F", "#{pane_id}"))
        # The retired bridge exits and removes its socket; the CLI pane is not cleaned up for Claude.
        wait_until(lambda: not ((pathlib.Path("/tmp") / ("prism-" + self.binding_path.parent.name) / "bridge.sock")).exists(), timeout=5)
        self.assertTrue(alive(self.binding["server_pid"]))

    def test_explicit_resume_passes_the_session_to_claude(self):
        session = str(uuid.uuid4())
        native = self.start(session)
        self.assertEqual(native["args"], ["--resume", session])
        self.assertEqual(self.binding["conversation"], session)
        self.assertEqual(self.binding["agent"], "claude")


if __name__ == "__main__":
    unittest.main()
