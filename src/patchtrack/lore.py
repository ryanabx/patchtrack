"""Read-only access to a public-inbox archive such as lore.kernel.org.

Only two endpoints are used: the search export (``?x=m``, the same request
``lei q -I`` makes) and a thread's ``t.mbox.gz``. Requests identify the tool
honestly and are spaced out; nothing here ever sends mail.
"""

import gzip
import mailbox
import os
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

from . import __version__

DEFAULT_BASE = "https://lore.kernel.org"
USER_AGENT = f"patchtrack/{__version__}"
RETRY_STATUS = {429, 500, 502, 503, 504}
# A window with this many results is split. Exports of 1652 messages have
# been complete (measured 2026-10-26), so no cap can bite below it.
SPLIT_AT = 1000


def message_url(base, msgid):
    """A message's page on the archive."""
    return f"{base}/all/{urllib.parse.quote(msgid, safe='@+-._~=')}/"


class LoreError(RuntimeError):
    pass


class Lore:
    def __init__(self, base=DEFAULT_BASE, inbox="all", delay=1.0, timeout=120, retries=3,
                 sleep=time.sleep, opener=urllib.request.urlopen):
        self.base = base.rstrip("/")
        self.inbox = inbox
        self.delay = delay
        self.timeout = timeout
        self.retries = retries
        self.sleep = sleep
        self.opener = opener
        self._last = 0.0

    def _get(self, url, data=None):
        for attempt in range(self.retries + 1):
            wait = self._last + self.delay - time.monotonic()
            if wait > 0:
                self.sleep(wait)
            req = urllib.request.Request(url, data=data, headers={"User-Agent": USER_AGENT})
            retry_after = None
            try:
                with self.opener(req, timeout=self.timeout) as resp:
                    ctype = resp.headers.get("Content-Type", "")
                    raw = resp.read()
                break
            except urllib.error.HTTPError as e:
                e.close()
                # public-inbox answers an empty search with 404
                if e.code == 404:
                    return b""
                if e.code not in RETRY_STATUS or attempt == self.retries:
                    raise LoreError(f"{url}: HTTP {e.code}") from e
                retry_after = e.headers.get("Retry-After") if e.headers else None
                why = f"HTTP {e.code}"
            except (urllib.error.URLError, TimeoutError, ConnectionError) as e:
                if attempt == self.retries:
                    raise LoreError(f"{url}: {getattr(e, 'reason', e)}") from e
                why = str(getattr(e, "reason", e))
            finally:
                self._last = time.monotonic()
            backoff = min(5 * 2 ** attempt, 120)
            if retry_after and retry_after.isdigit():
                backoff = max(backoff, min(int(retry_after), 600))
            print(f"  {why}; retrying in {backoff} s", file=sys.stderr)
            self.sleep(backoff)
        if "html" in ctype:
            # Anubis or an error page, not an mbox
            raise LoreError(f"{url}: got {ctype}, not an mbox (blocked by a bot check?)")
        return gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw

    def search(self, query):
        """Every message matching a public-inbox search query, as a list."""
        q = urllib.parse.urlencode({"x": "m", "q": query})
        return parse_mbox(self._get(f"{self.base}/{self.inbox}/?{q}", data=b""))

    def thread(self, msgid):
        """The whole thread containing msgid."""
        mid = urllib.parse.quote(msgid.strip("<>"), safe="@+-._~=")
        return parse_mbox(self._get(f"{self.base}/all/{mid}/t.mbox.gz"))


def windowed(search, query, since, until, days=30, split_at=SPLIT_AT, progress=None):
    """Every message for query between two epoch times, searched window by window.

    A window that returns split_at or more messages is searched again in
    halves, so a cap on one export can never truncate the result quietly.
    """
    found, todo = {}, []
    start = since
    while start < until:
        end = min(start + days * 86400, until)
        todo.append((start, end))
        start = end
    while todo:
        lo, hi = todo.pop(0)
        msgs = search(f"({query}) AND rt:{lo}..{hi}")
        if len(msgs) >= split_at and hi - lo > 3600:
            mid = (lo + hi) // 2
            todo[:0] = [(lo, mid), (mid, hi)]
            continue
        if progress:
            progress(lo, hi, len(msgs))
        for m in msgs:
            found.setdefault(m["Message-ID"], m)
    return list(found.values())


def parse_mbox(raw):
    if not raw:
        return []
    fd, path = tempfile.mkstemp(suffix=".mbox")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(raw)
        return list(mailbox.mbox(path, create=False))
    finally:
        os.unlink(path)
