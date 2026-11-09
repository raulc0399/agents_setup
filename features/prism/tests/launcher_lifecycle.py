#!/usr/bin/env python3
"""Isolated launcher, supervisor, tmux ownership and actual UI PTY regressions.

Run with PRISM_BINARY=/absolute/path/to/prism python3 tests/launcher_lifecycle.py.
Requires websockets==15.0.1. The native-process fixture exercises lifecycle only; real Codex validation is separate.
"""
import fcntl
import http.client
import json
import os
import pathlib
import pty
import signal
import sys
import socket
import subprocess
import tempfile
import termios
import time
import unittest
import uuid


BINARY = pathlib.Path(os.environ.get("PRISM_BINARY", "target/debug/prism")).resolve()


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


class LauncherLifecycle(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="prism-launch-")
        self.project = pathlib.Path(self.temporary.name)
        self.name = f"prism-launch-test-{uuid.uuid4()}"
        self.tmux("new-session", "-d", "-x", "160", "-y", "48", "-s", "anchor", "cat")
        pane = self.tmux("display-message", "-p", "#{pane_id}")
        tmux_env = self.tmux("display-message", "-p", "#{socket_path},#{pid},0")
        self.env = dict(os.environ, TMUX=tmux_env, TMUX_PANE=pane, TERM="xterm-256color")
        self.anchor = pane
        executable = self.project / "codex"
        executable.write_text("#!" + sys.executable + '''
import asyncio, json, os, pathlib, socket, sys, time
if sys.argv[1:4] == ["plugin", "list", "--json"]:
    pathlib.Path("plugin-list-called").write_text("unexpected remote catalog dependency")
    time.sleep(60)
elif sys.argv[1] == "app-server":
    endpoint = sys.argv[sys.argv.index("--listen") + 1]
    pathlib.Path(os.environ["PRISM_BINDING_FILE"]).with_name("native-process.json").write_text(json.dumps({"pid":os.getpid(),"parent":os.getppid()}))
    time.sleep(float(os.environ.get("PRISM_TEST_STARTUP_DELAY", "0")))
    from websockets.asyncio.server import unix_serve
    async def handle(connection):
        async for message in connection:
            request = json.loads(message)
            if "id" not in request:
                continue
            method = request["method"]
            if method == "initialize":
                result = {"userAgent":"prism-lifecycle-fixture/1"}
            elif method == "server/diagnostics":
                result = {"process":{"id":os.getpid()}}
            elif method == "thread/resume":
                result = {"thread":{"id":request["params"]["threadId"]}}
            else:
                raise AssertionError("unsupported native fixture request " + method)
            await connection.send(json.dumps({"id":request["id"],"result":result}))
    async def serve():
        async with unix_serve(handle, endpoint.removeprefix("unix://")):
            await asyncio.Future()
    asyncio.run(serve())
else:
    pathlib.Path("native-started.json").write_text(json.dumps({"args":sys.argv[1:],"binding":os.environ.get("PRISM_BINDING_FILE")}))
    while sys.stdin.readline():
        pass
''')
        executable.chmod(0o700)
        self.env["PATH"] = f"{self.project}:{self.env['PATH']}"
        self.binding = None
        self.supervisor = None

    def tearDown(self):
        if self.supervisor and alive(self.supervisor):
            os.kill(self.supervisor, signal.SIGTERM)
            wait_until(lambda: not alive(self.supervisor))
        subprocess.run(["tmux", "-L", self.name, "kill-server"], capture_output=True)
        self.temporary.cleanup()

    def tmux(self, *args):
        return subprocess.check_output(["tmux", "-L", self.name, *args], text=True).strip()

    def start(self, resume=None):
        command = [str(BINARY), "--agent", "codex", "--project", str(self.project), "--detach"]
        if resume:
            command += ["--resume", resume]
        result = subprocess.run(command, env=self.env, cwd=self.project, text=True, capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((self.project / "plugin-list-called").exists())
        launch = json.loads(result.stdout)
        self.binding_path = pathlib.Path(launch["binding"])
        self.binding = json.loads(self.binding_path.read_text())
        self.supervisor = int(pathlib.Path(f"/proc/{self.binding['server_pid']}/stat").read_text().rsplit(")", 1)[1].split()[1])
        wait_until(lambda: (self.project / "native-started.json").exists())
        return json.loads((self.project / "native-started.json").read_text())

    def call(self, operation, thread):
        connection = http.client.HTTPConnection("localhost", timeout=5)
        connection.sock = socket.socket(socket.AF_UNIX)
        connection.sock.connect(str((pathlib.Path("/tmp") / ("prism-" + self.binding_path.parent.name) / "bridge.sock")))
        data = json.dumps({"version":1,"generation":self.binding["generation"],"thread_id":thread,"operation":operation})
        connection.request("POST", "/call", data, {"Authorization":f"Bearer {self.binding['token']}","Content-Type":"application/json"})
        response = connection.getresponse()
        value = json.loads(response.read())
        connection.close()
        self.assertEqual(response.status, 200, value)
        return value

    def test_detach_preserves_exact_resume_and_cli_exit_retires_only_owned_session(self):
        thread = str(uuid.uuid4())
        native = self.start(thread)
        self.assertEqual(native["args"][:2], ["resume", thread])
        self.assertEqual(native["binding"], str(self.binding_path))
        self.assertEqual(self.binding["conversation"], thread)
        self.assertEqual(self.binding_path.stat().st_mode & 0o777, 0o600)
        self.tmux("kill-pane", "-t", self.binding["cli"]["pane_id"])
        wait_until(lambda: not alive(self.binding["server_pid"]))
        wait_until(lambda: not alive(self.supervisor))
        self.assertTrue(json.loads(self.binding_path.read_text())["retired"])
        self.assertIn(self.anchor, self.tmux("list-panes", "-a", "-F", "#{pane_id}"))

    def test_repeated_exact_resume_does_not_create_another_live_cli(self):
        thread = str(uuid.uuid4())
        self.start(thread)
        before = self.tmux("list-panes", "-a", "-F", "#{pane_id}")
        runtimes = set(self.project.glob(".prism/run/*"))
        result = subprocess.run([str(BINARY), "--agent", "codex", "--project", str(self.project), "--resume", thread, "--detach"], env=self.env, text=True, capture_output=True, timeout=5)
        try:
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("already has a live Prism binding", result.stderr)
            self.assertEqual(self.tmux("list-panes", "-a", "-F", "#{pane_id}"), before)
            self.assertEqual(set(self.project.glob(".prism/run/*")), runtimes)
        finally:
            if result.returncode == 0:
                duplicate = json.loads(pathlib.Path(json.loads(result.stdout)["binding"]).read_text())
                os.kill(duplicate["server_pid"], signal.SIGTERM)

    def test_next_launch_prunes_retired_runtime_without_live_panes(self):
        self.start(str(uuid.uuid4()))
        previous = self.binding_path.parent
        self.tmux("kill-pane", "-t", self.binding["cli"]["pane_id"])
        wait_until(lambda: not alive(self.supervisor))
        self.start(str(uuid.uuid4()))
        self.assertFalse(previous.exists())

    def test_next_launch_preserves_retired_runtime_with_live_ui(self):
        thread = str(uuid.uuid4())
        self.start(thread)
        self.call({"method": "inspect", "params": {}}, thread)
        previous = self.binding_path.parent
        saved = json.loads(self.binding_path.read_text())
        self.assertIsNotNone(saved["prism"])
        self.tmux("kill-pane", "-t", self.binding["cli"]["pane_id"])
        wait_until(lambda: not alive(self.supervisor))
        self.start(str(uuid.uuid4()))
        self.assertTrue(previous.exists())
        self.assertTrue(alive(saved["prism"]["pid"]))

    def test_concurrent_exact_resumes_serialize_registration(self):
        thread = str(uuid.uuid4())
        command = [str(BINARY), "--agent", "codex", "--project", str(self.project), "--resume", thread, "--detach"]
        processes = [subprocess.Popen(command, env=dict(self.env, PRISM_TEST_STARTUP_DELAY="0.5"), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) for _ in range(2)]
        results = [(process, process.communicate(timeout=8)) for process in processes]
        successful = [(process, output) for process, output in results if process.returncode == 0]
        try:
            self.assertEqual(len(successful), 1)
            rejected = [output[1] for process, output in results if process.returncode != 0]
            self.assertIn("already has a live Prism binding", rejected[0])
            self.assertEqual(len(self.tmux("list-panes", "-a", "-F", "#{pane_id}").splitlines()), 2)
        finally:
            for _, (stdout, _) in successful:
                binding = json.loads(pathlib.Path(json.loads(stdout)["binding"]).read_text())
                if alive(binding["server_pid"]):
                    os.kill(binding["server_pid"], signal.SIGTERM)
                    wait_until(lambda: not alive(binding["server_pid"]))

    def test_startup_signals_cancel_and_reap_the_owned_supervisor(self):
        for interruption in [signal.SIGINT, signal.SIGTERM, signal.SIGHUP]:
            with self.subTest(interruption=interruption):
                existing = set(self.project.glob(".prism/run/*/native-process.json"))
                process = subprocess.Popen([str(BINARY), "--agent", "codex", "--project", str(self.project), "--detach"], env=dict(self.env, PRISM_TEST_STARTUP_DELAY="3"), stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                native_file = wait_until(lambda: next(iter(set(self.project.glob(".prism/run/*/native-process.json")) - existing), None))
                native = json.loads(native_file.read_text())
                try:
                    process.send_signal(interruption)
                    process.communicate(timeout=5)
                    self.assertNotEqual(process.returncode, 0)
                    self.assertFalse(alive(native["parent"]))
                    self.assertFalse(alive(native["pid"]))
                    self.assertEqual(self.tmux("list-panes", "-a", "-F", "#{pane_id}"), self.anchor)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
                    for pid in [native["parent"], native["pid"]]:
                        if alive(pid):
                            os.kill(pid, signal.SIGKILL)

    def test_attached_launcher_signals_detach_client_and_preserve_live_session(self):
        wrapper = self.project / "tmux"
        wrapper.write_text(f'#!/bin/sh\nexec /usr/bin/tmux -L {self.name} "$@"\n')
        wrapper.chmod(0o700)
        environment = {key: value for key, value in self.env.items() if key not in {"TMUX", "TMUX_PANE"}}
        for interruption in [signal.SIGINT, signal.SIGTERM, signal.SIGHUP]:
            with self.subTest(interruption=interruption):
                existing = set(self.project.glob(".prism/run/*/binding.json"))
                master, slave = pty.openpty()
                before = termios.tcgetattr(slave)
                process = subprocess.Popen([str(BINARY), "--agent", "codex", "--project", str(self.project)], env=environment, stdin=slave, stdout=slave, stderr=slave, start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
                try:
                    binding_path = wait_until(lambda: next(iter(set(self.project.glob(".prism/run/*/binding.json")) - existing), None))
                    binding = json.loads(binding_path.read_text())
                    self.supervisor = int(pathlib.Path(f"/proc/{binding['server_pid']}/stat").read_text().rsplit(")", 1)[1].split()[1])
                    wait_until(lambda: termios.tcgetattr(slave) != before)
                    process.send_signal(interruption)
                    process.wait(timeout=4)
                    self.assertNotEqual(process.returncode, 0)
                    self.assertTrue(alive(self.supervisor))
                    self.assertTrue(alive(binding["server_pid"]))
                    self.assertFalse(json.loads(binding_path.read_text())["retired"])
                    self.assertEqual(termios.tcgetattr(slave), before)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
                    if self.supervisor and alive(self.supervisor):
                        os.kill(self.supervisor, signal.SIGTERM)
                        wait_until(lambda: not alive(self.supervisor))
                    os.close(master)
                    os.close(slave)

    def test_attachment_rejects_a_restarted_tmux_server_with_recycled_session_ids(self):
        wrapper = self.project / "tmux"
        marker = self.project / "attachment-race-triggered"
        wrapper.write_text("#!" + sys.executable + "\n" + f'''import os, pathlib, subprocess, sys, time
arguments = sys.argv[1:]
if any("attach-session" in argument for argument in arguments):
    command = ["/usr/bin/tmux", "-L", {self.name!r}]
    subprocess.run(command + ["kill-server"], check=True)
    time.sleep(0.2)
    subprocess.run(command + ["new-session", "-d", "-s", "unrelated-anchor", "cat"], check=True)
    subprocess.run(command + ["new-session", "-d", "-s", "unrelated-session", "cat"], check=True)
    pathlib.Path({str(marker)!r}).write_text("replaced")
os.execv("/usr/bin/tmux", ["tmux", "-L", {self.name!r}, *arguments])
''')
        wrapper.chmod(0o700)
        environment = {key: value for key, value in self.env.items() if key not in {"TMUX", "TMUX_PANE"}}
        master, slave = pty.openpty()
        process = subprocess.Popen([str(BINARY), "--agent", "codex", "--project", str(self.project)], env=environment, stdin=slave, stdout=slave, stderr=slave, start_new_session=True, preexec_fn=lambda: fcntl.ioctl(0, termios.TIOCSCTTY, 0))
        try:
            wait_until(marker.exists)
            process.wait(timeout=4)
            self.assertNotEqual(process.returncode, 0)
            self.assertEqual(self.tmux("list-clients", "-F", "#{client_session}"), "")
            self.assertEqual(len(self.tmux("list-panes", "-a", "-F", "#{pane_id}").splitlines()), 2)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()
            os.close(master)
            os.close(slave)

    def test_long_project_path_uses_private_tmp_sockets(self):
        self.project = self.project / ("p" * 120)
        self.project.mkdir()
        self.start()
        socket_path = pathlib.Path(self.binding["server_endpoint"].removeprefix("unix://"))
        self.assertEqual(socket_path.parent.parent, pathlib.Path("/tmp"))
        self.assertLess(len(str(socket_path)), 108)
        self.assertEqual(socket_path.parent.stat().st_mode & 0o777, 0o700)
        self.assertEqual(socket_path.stat().st_mode & 0o777, 0o600)
        self.assertTrue((socket_path.parent / "bridge.sock").is_socket())
        self.tmux("kill-pane", "-t", self.binding["cli"]["pane_id"])
        wait_until(lambda: not alive(self.supervisor))
        self.assertFalse(socket_path.parent.exists())

    def test_supervisor_crash_terminates_private_native_server(self):
        self.start()
        os.kill(self.supervisor, signal.SIGKILL)
        wait_until(lambda: not alive(self.binding["server_pid"]))
        self.assertIn(self.anchor, self.tmux("list-panes", "-a", "-F", "#{pane_id}"))

    def test_ui_restores_terminal_on_quit_and_signals(self):
        self.start()
        thread = str(uuid.uuid4())
        self.call({"method":"publish","params":{"request_id":str(uuid.uuid4()),"document_id":None,"expected_revision":None,"title":"PTY lifecycle","elements":[],"relationships":[],"groups":[]}}, thread)
        for interruption in [None, signal.SIGINT, signal.SIGTERM, signal.SIGHUP]:
            with self.subTest(interruption=interruption):
                master, slave = pty.openpty()
                before = termios.tcgetattr(slave)
                process = subprocess.Popen([str(BINARY), "ui", "--binding", str(self.binding_path)], stdin=slave, stdout=slave, stderr=slave, env=self.env)
                try:
                    wait_until(lambda: termios.tcgetattr(slave) != before)
                    if interruption is None:
                        os.write(master, b"q")
                    else:
                        process.send_signal(interruption)
                    process.wait(timeout=8)
                    self.assertEqual(process.returncode, 0)
                    self.assertEqual(termios.tcgetattr(slave), before)
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
                    os.close(master)
                    os.close(slave)


if __name__ == "__main__":
    unittest.main()
