#!/usr/bin/env python3
"""On-demand attachment to existing tmux conversations; no model requests."""
import json
import os
import pathlib
import shlex
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
        self.name = "prism-attach-" + str(uuid.uuid4())
        self.environment = dict(os.environ)
        for key in ["TMUX", "TMUX_PANE", "PRISM_BINDING_FILE"]:
            self.environment.pop(key, None)
        self.environment["PATH"] = str(BINARY.parent) + ":" + self.environment["PATH"]
        # A pairing file that does not exist: test bridges never dial a desktop app on the host.
        self.environment["PRISM_APP_FILE"] = str(self.project / "no-prism-app.json")

    def tearDown(self):
        subprocess.run(["tmux", "-L", self.name, "kill-server"], capture_output=True)
        time.sleep(1.5)
        self.temporary.cleanup()

    def tmux(self, *args):
        return subprocess.check_output(["tmux", "-L", self.name, *args], text=True).strip()

    def start(self, command):
        return subprocess.check_output(["tmux", "-L", self.name, "new-session", "-d", "-P", "-F", "#{pane_id}", "-x", "160", "-y", "48", "-c", str(self.project), shlex.join(command)], env=self.environment, text=True).strip()

    def binding(self):
        paths = list(self.project.glob(".prism/run/*/binding.json"))
        if not paths:
            return None
        try:
            return paths[0], json.loads(paths[0].read_text())
        except (OSError, json.JSONDecodeError):
            return None

    def test_codex_mcp_attaches_lazily_and_reuses_right_pane(self):
        thread = str(uuid.uuid4())
        script = self.project / "conversation.py"
        script.write_text('''import json, pathlib, subprocess, sys, time
root = pathlib.Path.cwd()
p = subprocess.Popen([sys.argv[1], "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def call(request):
    p.stdin.write(json.dumps(request) + "\\n")
    p.stdin.flush()
    return json.loads(p.stdout.readline())
call({"jsonrpc":"2.0","id":0,"method":"initialize","params":{}})
root.joinpath("ready").touch()
for index in range(4):
    request = root / ("request-" + str(index))
    while not request.exists(): time.sleep(.02)
    result = call(json.loads(request.read_text()))
    temporary = root / "reply.tmp"
    temporary.write_text(json.dumps(result))
    temporary.rename(root / ("reply-" + str(index)))
while not root.joinpath("stop").exists(): time.sleep(.02)
p.stdin.close()
p.wait(timeout=5)
root.joinpath("cli-alive").touch()
while True: time.sleep(1)
''')
        pane = self.start([sys.executable, str(script), str(BINARY)])
        wait_until(lambda: (self.project / "ready").exists())
        self.assertEqual(self.tmux("list-panes", "-F", "#{pane_id}"), pane)
        self.assertFalse((self.project / ".prism").exists())
        pid = self.tmux("display-message", "-p", "-t", pane, "#{pane_pid}")
        for index in range(4):
            request = {"jsonrpc":"2.0", "id":index+1, "method":"tools/call", "params":{"name":"prism_inspect", "arguments":{}, "_meta":{"threadId":thread}}}
            if index == 0:
                request["params"]["name"] = "prism_publish"
                request["params"]["arguments"] = {"request_id": str(uuid.uuid4()), "document_id": None, "expected_revision": None, "title": "Existing conversation", "elements": [], "relationships": [], "groups": []}
            if index == 3:
                request["params"]["_meta"]["threadId"] = str(uuid.uuid4())
            (self.project / ("request-" + str(index))).write_text(json.dumps(request))
            reply = self.project / ("reply-" + str(index))
            wait_until(reply.exists)
            response = json.loads(reply.read_text())["result"]
            if index == 3:
                self.assertTrue(response["isError"], response)
                self.assertFalse(json.loads(path.read_text())["retired"])
                continue
            self.assertFalse(response["isError"], response)
            if index > 0:
                self.assertEqual(json.loads(response["content"][0]["text"])["data"]["total"], 1)
            path, binding = wait_until(self.binding)
            self.assertEqual(binding["conversation"], thread)
            self.assertEqual(binding["cli"]["pane_id"], pane)
            self.assertEqual(self.tmux("display-message", "-p", "-t", pane, "#{pane_pid}"), pid)
            panes = self.tmux("list-panes", "-F", "#{pane_id} #{pane_left} #{pane_top}").splitlines()
            self.assertEqual(len(panes), 2)
            positions = {line.split()[0]: list(map(int, line.split()[1:])) for line in panes}
            ui = binding["prism"]["pane_id"]
            self.assertGreater(positions[ui][0], positions[pane][0])
            self.assertEqual(positions[ui][1], positions[pane][1])
            if index == 0:
                first_ui = ui
            elif index == 1:
                self.assertEqual(ui, first_ui)
                duplicate = subprocess.run([str(BINARY), "attached-bridge", "--binding", str(path)], capture_output=True, text=True, timeout=5)
                self.assertNotEqual(duplicate.returncode, 0)
                self.assertIn("already owns", duplicate.stderr)
                self.tmux("kill-pane", "-t", ui)
            else:
                self.assertNotEqual(ui, first_ui)
        (self.project / "stop").touch()
        wait_until(lambda: (self.project / "cli-alive").exists())
        wait_until(lambda: json.loads(path.read_text())["retired"])
        self.assertEqual(self.tmux("display-message", "-p", "-t", pane, "#{pane_pid}"), pid)

    def test_outside_tmux_gives_start_instruction_without_runtime(self):
        request = {"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"prism_inspect", "arguments":{}, "_meta":{"threadId":str(uuid.uuid4())}}}
        result = subprocess.run([str(BINARY), "mcp"], input=json.dumps(request)+"\n", text=True, capture_output=True, cwd=self.project, env=self.environment, check=True)
        response = json.loads(result.stdout)["result"]
        self.assertTrue(response["isError"])
        self.assertIn("Start codex or claude inside tmux", response["content"][0]["text"])
        self.assertFalse((self.project / ".prism").exists())

    def test_unrelated_process_cannot_claim_a_tmux_pane(self):
        pane = self.start(["cat"])
        tmux = self.tmux("display-message", "-p", "#{socket_path},#{pid},0")
        environment = dict(self.environment, TMUX=tmux, TMUX_PANE=pane)
        request = {"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":"prism_inspect", "arguments":{}, "_meta":{"threadId":str(uuid.uuid4())}}}
        result = subprocess.run([str(BINARY), "mcp"], input=json.dumps(request)+"\n", text=True, capture_output=True, cwd=self.project, env=environment, check=True)
        response = json.loads(result.stdout)["result"]
        self.assertTrue(response["isError"])
        self.assertIn("does not belong", response["content"][0]["text"])
        self.assertFalse((self.project / ".prism").exists())
        self.assertEqual(self.tmux("list-panes", "-F", "#{pane_id}"), pane)

    def test_claude_native_command_attaches_without_launcher(self):
        claude = shutil.which("claude")
        self.assertIsNotNone(claude)
        thread = str(uuid.uuid4())
        script = self.project / "conversation.py"
        script.write_text('''import os, pathlib, subprocess, sys, time
root = pathlib.Path.cwd()
env = dict(os.environ, CLAUDE_CONFIG_DIR=str(root / "config"), DISABLE_AUTOUPDATER="1", DISABLE_TELEMETRY="1", PRISM_APP_FILE=str(root / "no-prism-app.json"))
with root.joinpath("stdout").open("w") as out, root.joinpath("stderr").open("w") as err:
    result = subprocess.run(sys.argv[1:], env=env, stdout=out, stderr=err, timeout=60)
root.joinpath("done").write_text(str(result.returncode))
while True: time.sleep(1)
''')
        pane = self.start([sys.executable, str(script), claude, "--session-id", thread, "-p", "/prism", "--plugin-dir", str(PLUGIN), "--setting-sources", "", "--strict-mcp-config", "--output-format", "json", "--no-session-persistence"])
        wait_until(lambda: (self.project / "done").exists(), timeout=70)
        output = (self.project / "stdout").read_text()
        self.assertIn("is open beside this conversation", output, output + (self.project / "stderr").read_text())
        path, binding = wait_until(self.binding)
        self.assertEqual(binding["conversation"], thread)
        self.assertEqual(binding["cli"]["pane_id"], pane)
        wait_until(lambda: json.loads(path.read_text())["retired"])
        self.assertIn(pane, self.tmux("list-panes", "-F", "#{pane_id}"))


if __name__ == "__main__":
    unittest.main()
