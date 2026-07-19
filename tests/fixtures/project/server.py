#!/usr/bin/env python3
import http.server
import socketserver
import sys


class Handler(http.server.SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/fixture-health":
            body = b"fixture-preview-ok\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()


port = int(sys.argv[1]) if len(sys.argv) > 1 else 18080
with socketserver.TCPServer(("0.0.0.0", port), Handler) as server:
    server.serve_forever()
