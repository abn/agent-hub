#!/usr/bin/env python3
"""Local release server for the install script tests.

Usage: server.py ROOT PORT_FILE

ROOT/download/... is served as-is. /releases/latest redirects to the tag named
in ROOT/LATEST, the way github.com does. Every request path is appended to
ROOT/requests.log. The port is chosen by the OS and written to PORT_FILE.
"""
import http.server
import os
import sys

root, port_file = sys.argv[1], sys.argv[2]


def note(line):
    with open(os.path.join(root, "requests.log"), "a") as log:
        log.write(line + "\n")


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=os.path.join(root, "download"), **kwargs)

    def log_message(self, fmt, *args):
        note("%s %s" % (self.command, self.path))

    def translate_path(self, path):
        return super().translate_path(path.removeprefix("/download"))

    def do_HEAD(self):
        if self.path == "/releases/latest":
            self.redirect()
        else:
            super().do_HEAD()

    def do_GET(self):
        if self.path == "/releases/latest":
            self.redirect()
        else:
            super().do_GET()

    def redirect(self):
        with open(os.path.join(root, "LATEST")) as f:
            tag = f.read().strip()
        self.send_response(302)
        self.send_header("Location", "/releases/tag/agent-hub-" + tag)
        self.send_header("Content-Length", "0")
        self.end_headers()


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(port_file, "w") as f:
    f.write(str(server.server_address[1]))
server.serve_forever()
