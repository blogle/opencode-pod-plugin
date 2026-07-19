#!/usr/bin/env python3
import os
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


ROOT = "/srv/git"


class GitHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _serve(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else b""
        env = os.environ.copy()
        env.update(
            {
                "GIT_HTTP_EXPORT_ALL": "1",
                "GIT_PROJECT_ROOT": ROOT,
                "PATH_INFO": self.path.split("?", 1)[0],
                "QUERY_STRING": self.path.partition("?")[2],
                "REQUEST_METHOD": self.command,
                "CONTENT_TYPE": self.headers.get("Content-Type", ""),
                "CONTENT_LENGTH": str(length),
                "REMOTE_ADDR": self.client_address[0],
            }
        )
        result = subprocess.run(
            ["git", "http-backend"], input=body, env=env, capture_output=True, check=False
        )
        header_blob, separator, response_body = result.stdout.partition(b"\r\n\r\n")
        if not separator:
            self.send_error(500, result.stderr.decode("utf-8", "replace"))
            return
        status = 200
        headers = []
        for line in header_blob.decode("latin-1").split("\r\n"):
            name, value = line.split(":", 1)
            if name.lower() == "status":
                status = int(value.strip().split(" ", 1)[0])
            else:
                headers.append((name, value.strip()))
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        self.send_header("Content-Length", str(len(response_body)))
        self.end_headers()
        self.wfile.write(response_body)

    do_GET = _serve
    do_POST = _serve

    def log_message(self, fmt, *args):
        print(fmt % args, flush=True)


ThreadingHTTPServer(("0.0.0.0", 8000), GitHandler).serve_forever()
