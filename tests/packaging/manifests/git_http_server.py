#!/usr/bin/env python3
"""Serve git repositories over smart HTTP on 127.0.0.1, for offline install tests.

Harnesses clone marketplaces shallowly, which git's "dumb" HTTP transport (a plain file
server) cannot do. This relays requests to `git http-backend`, the CGI program git ships.

    git_http_server.py ROOT [PORT]

ROOT holds bare repositories (ROOT/name.git); they are served read-only at
http://127.0.0.1:PORT/name.git. The chosen port is printed on the first line of stdout and the
server's process id on the second, so that the caller can stop exactly this process.
"""

import os
import subprocess
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.abspath(sys.argv[1])
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 0
BACKEND = os.path.join(subprocess.check_output(["git", "--exec-path"], text=True).strip(), "git-http-backend")


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.0"

    def relay(self):
        path, _, query = self.path.partition("?")
        body = b""
        if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
            while True:
                size = int(self.rfile.readline().split(b";")[0].strip() or b"0", 16)
                if size == 0:
                    self.rfile.readline()
                    break
                body += self.rfile.read(size)
                self.rfile.readline()
        else:
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "GIT_PROJECT_ROOT": ROOT,
            "GIT_HTTP_EXPORT_ALL": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_CONFIG_SYSTEM": os.devnull,
            "REQUEST_METHOD": self.command,
            "PATH_INFO": path,
            "QUERY_STRING": query,
            "CONTENT_TYPE": self.headers.get("Content-Type", ""),
            "CONTENT_LENGTH": str(len(body)),
            "REMOTE_ADDR": "127.0.0.1",
        }
        if self.headers.get("Content-Encoding"):
            env["HTTP_CONTENT_ENCODING"] = self.headers["Content-Encoding"]
        if self.headers.get("Git-Protocol"):
            env["GIT_PROTOCOL"] = self.headers["Git-Protocol"]
        out = subprocess.run([BACKEND], input=body, env=env, capture_output=True).stdout
        head, _, payload = out.partition(b"\r\n\r\n")
        status = 200
        headers = []
        for line in head.split(b"\r\n"):
            name, _, value = line.decode("latin-1").partition(":")
            if name.lower() == "status":
                status = int(value.strip().split()[0])
            elif name:
                headers.append((name, value.strip()))
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    do_GET = relay
    do_POST = relay

    def log_message(self, *args):
        pass


server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
print(server.server_address[1], flush=True)
print(os.getpid(), flush=True)
server.serve_forever()
