"""A minimal Marionette client for headless Firefox: no WebDriver needed.

Firefox listens for Marionette when started with --marionette; the port
comes from the profile's user.js (marionette.port).
"""
import base64
import json
import socket
import time


class Marionette:
    def __init__(self, port):
        for _ in range(150):
            try:
                self.sock = socket.create_connection(("127.0.0.1", port))
                break
            except OSError:
                time.sleep(0.2)
        else:
            raise RuntimeError(f"Firefox did not open Marionette on port {port}")
        self.buf = b""
        self.id = 0
        self._read()  # greeting
        # A test's own HTTPS (a stand-in for a bridge's tunnel) has a certificate of its own making.
        self.cmd("WebDriver:NewSession", {"capabilities": {}, "acceptInsecureCerts": True})

    def _read(self):
        while b":" not in self.buf:
            self.buf += self.sock.recv(65536)
        n, rest = self.buf.split(b":", 1)
        n = int(n)
        while len(rest) < n:
            rest += self.sock.recv(1 << 20)
        self.buf = rest[n:]
        return json.loads(rest[:n])

    def cmd(self, name, params=None):
        self.id += 1
        data = json.dumps([0, self.id, name, params or {}]).encode()
        self.sock.sendall(str(len(data)).encode() + b":" + data)
        while True:
            msg = self._read()
            if msg[0] == 1 and msg[1] == self.id:
                if msg[2]:
                    raise RuntimeError(msg[2])
                return msg[3]

    def go(self, url):
        self.cmd("WebDriver:Navigate", {"url": url})

    def reload(self, base, hash="#/"):
        """A new document for sure, through the service worker. WebDriver:Refresh
        bypasses the worker, and WebDriver:Navigate from one page of the app to
        another sometimes leaves the old document in place; a navigation the
        page makes itself, to a query string it has not seen, does neither."""
        self.loads = getattr(self, "loads", 0) + 1
        before = self.js("return performance.timeOrigin")
        self.js(f"location.href = {json.dumps(f'{base}?load={self.loads}{hash}')}; return 1")
        for _ in range(150):
            time.sleep(0.1)
            try:
                if self.js("return document.readyState === 'complete' && performance.timeOrigin") not in (False, before):
                    return
            except RuntimeError:
                pass  # between documents
        raise RuntimeError("the page did not reload")

    def js(self, script, *args):
        return self.cmd("WebDriver:ExecuteScript", {"script": script, "args": list(args)})["value"]

    def wait(self, script, seconds=30):
        """Poll a script until it returns something true; return that, or None."""
        end = time.time() + seconds
        while time.time() < end:
            v = self.js(script)
            if v:
                return v
            time.sleep(0.2)
        return None

    def shot(self, path):
        png = self.cmd("WebDriver:TakeScreenshot", {"full": True})["value"]
        with open(path, "wb") as f:
            f.write(base64.b64decode(png))
