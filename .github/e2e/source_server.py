"""Disposable, controlled HTTP media source for external E2E clients."""

import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit

FILES = {}
LOCK = threading.Lock()
ORIGIN = os.environ.get("SOURCE_ORIGIN", "http://sources:8000")
LIMIT = int(os.environ.get("MAX_UPLOAD_BYTES", "2097152"))


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def respond(self, status, body=b"", content_type="application/octet-stream", headers=None):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def do_PUT(self):
        path = urlsplit(self.path).path
        if not path.startswith("/media/"):
            self.respond(404)
            return
        size = int(self.headers.get("Content-Length", "0"))
        if size > LIMIT:
            self.respond(413)
            return
        name = path.removeprefix("/media/")
        content = self.rfile.read(size)
        with LOCK:
            FILES[name] = content
        urls = {route: f"{ORIGIN}/{route}/{name}" for route in [
            "redirect", "missing", "empty", "invalid", "oversized", "chunked_oversized",
        ]}
        urls.update(name=name, url=f"{ORIGIN}/media/{name}")
        self.respond(201, json.dumps(urls).encode(), "application/json")

    def do_GET(self):
        path = urlsplit(self.path).path
        if path == "/healthz":
            self.respond(200, b"ok", "text/plain")
            return
        _, route, name = (path.split("/", 2) + [""])[:3]
        if route == "redirect":
            self.respond(302, headers={"Location": f"/media/{name}"})
        elif route == "missing":
            self.respond(404)
        elif route == "empty":
            self.respond(200)
        elif route == "invalid":
            self.respond(200, b"not media")
        elif route == "oversized":
            self.respond(200, b"x" * (LIMIT + 1))
        elif route == "chunked_oversized":
            self.send_response(200)
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            try:
                for _ in range(LIMIT // 65536 + 1):
                    chunk = b"x" * 65536
                    self.wfile.write(f"{len(chunk):x}\r\n".encode() + chunk + b"\r\n")
                self.wfile.write(b"0\r\n\r\n")
            except (BrokenPipeError, ConnectionResetError):
                pass  # The application correctly stops reading at its byte limit.
        elif route == "media":
            with LOCK:
                content = FILES.get(name)
            self.respond(404 if content is None else 200, content or b"")
        else:
            self.respond(404)


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 8000), Handler).serve_forever()
