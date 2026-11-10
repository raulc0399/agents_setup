"""Serves the app directory to the UI harness, plus POST /diagram, which lays a diagram out
with the `diagram` example of prism-app-host, as the shell's `diagram_layout` command does.

Usage: server.py <port> <directory> <diagram binary>
"""
import functools
import http.server
import subprocess
import sys

# A layout that takes longer is reported as failed instead of stalling the checks.
LAYOUT_TIMEOUT_SECONDS = 10


class Handler(http.server.SimpleHTTPRequestHandler):
    binary = None

    def do_POST(self):
        if self.path != "/diagram":
            self.send_error(404)
            return
        length = self.headers.get("Content-Length")
        if length is None or not length.isdigit():
            self.send_error(411, "a diagram request needs Content-Length")
            return
        request = self.rfile.read(int(length))
        try:
            result = subprocess.run(
                [self.binary],
                input=request,
                capture_output=True,
                check=False,
                timeout=LAYOUT_TIMEOUT_SECONDS,
            )
        except subprocess.TimeoutExpired:
            self.reply(504, "text/plain", b"the layout timed out")
            return
        if result.returncode != 0:
            self.reply(500, "text/plain", result.stderr)
            return
        self.reply(200, "application/json", result.stdout)

    def reply(self, status, content_type, body):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass


def main():
    port, directory, binary = int(sys.argv[1]), sys.argv[2], sys.argv[3]
    Handler.binary = binary
    handler = functools.partial(Handler, directory=directory)
    http.server.ThreadingHTTPServer(("127.0.0.1", port), handler).serve_forever()


if __name__ == "__main__":
    main()
