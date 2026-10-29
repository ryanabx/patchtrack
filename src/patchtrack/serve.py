"""The api documents over HTTP: read-only, rebuilt from the store on each request.

Only GET, only the paths ``api.paths`` names. The documents hold people's
mail, so the server checks the Host header: a page elsewhere that points a
name of its own at 127.0.0.1 (DNS rebinding) gets nothing. No other origin
may read the responses unless one is named with ``cors``.
"""

import http.server
import json
import urllib.parse

from . import __version__, api

LOOPBACK = {"localhost", "127.0.0.1", "::1"}
ANY = {"", "0.0.0.0"}


def encode(doc):
    return (json.dumps(doc, indent=1, ensure_ascii=False) + "\n").encode()


def make_server(load, host="127.0.0.1", port=8080, cors=None):
    """An HTTPServer answering with api.document(load(), path). load() returns an api.Data."""
    hosts = None if host in ANY else LOOPBACK | {host.strip("[]")}

    class Handler(http.server.BaseHTTPRequestHandler):
        server_version = f"patchtrack/{__version__}"

        def do_GET(self):
            name = urllib.parse.urlsplit(f"//{self.headers.get('Host', '')}").hostname or ""
            if hosts is not None and name not in hosts:
                return self._send(403, {"error": f"unexpected Host: {self.headers.get('Host')!r}"})
            path = urllib.parse.urlsplit(self.path).path.lstrip("/") or "index.json"
            doc = api.document(load(), path)
            if doc is None:
                return self._send(404, {"error": f"no document {path!r}"})
            self._send(200, doc)

        def _send(self, status, doc):
            body = encode(doc)
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            if cors:
                self.send_header("Access-Control-Allow-Origin", cors)
                self.send_header("Vary", "Origin")
            self.end_headers()
            self.wfile.write(body)

    return http.server.HTTPServer((host, port), Handler)
