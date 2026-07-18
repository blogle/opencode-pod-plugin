#!/usr/bin/env python3
import hashlib
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


LOG_PATH = sys.argv[1]
PORT_PATH = sys.argv[2]
PROFILE_PATH = "/v1/projects/demo/env-profile"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, _format, *_args):
        pass

    def send_json(self, status, value=None):
        body = b"" if value is None else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def record(self, body=b""):
        entry = {
            "method": self.command,
            "path": self.path,
            "authorization": self.headers.get("Authorization"),
            "identity": self.headers.get("X-Test-Identity"),
            "bodySha256": hashlib.sha256(body).hexdigest() if body else None,
        }
        with open(LOG_PATH, "a", encoding="utf-8") as output:
            output.write(json.dumps(entry) + "\n")

    def authorized(self):
        return (
            self.headers.get("Authorization") == "Bearer test-token"
            and self.headers.get("X-Test-Identity") == "developer@example.test"
        )

    def do_GET(self):
        self.record()
        if not self.authorized():
            self.send_json(401, {"error": "unauthorized"})
        elif self.path == "/v1/projects":
            self.send_json(200, [{"key": "demo", "repository": "https://github.com/acme/demo.git"}])
        elif self.path == PROFILE_PATH + "/meta":
            self.send_json(200, metadata())
        else:
            self.send_json(404, {"error": "not found"})

    def do_PUT(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        self.record(body)
        if not self.authorized():
            self.send_json(401, {"error": "unauthorized"})
        elif self.path == PROFILE_PATH:
            self.send_json(200, metadata(hashlib.sha256(body).hexdigest()))
        else:
            self.send_json(404, {"error": "not found"})

    def do_DELETE(self):
        self.record()
        if not self.authorized():
            self.send_json(401, {"error": "unauthorized"})
        elif self.path == PROFILE_PATH:
            self.send_json(204)
        else:
            self.send_json(404, {"error": "not found"})


def metadata(digest="a" * 64):
    return {
        "projectKey": "demo",
        "owner": "developer@example.test",
        "sha256": digest,
        "updatedAt": "2026-07-17T12:00:00Z",
    }


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
with open(PORT_PATH, "w", encoding="utf-8") as output:
    output.write(str(server.server_port))
server.serve_forever()
