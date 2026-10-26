import email.message
import io
import unittest
import unittest.mock
import urllib.error

from patchtrack import lore


class Response:
    def __init__(self, body=b"", ctype="application/mbox"):
        self.body, self.headers = body, {"Content-Type": ctype}

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False

    def read(self):
        return self.body


def http_error(code, retry_after=None):
    headers = email.message.Message()
    if retry_after:
        headers["Retry-After"] = retry_after
    return urllib.error.HTTPError("u", code, "x", headers, io.BytesIO())


class Retries(unittest.TestCase):
    def client(self, outcomes):
        self.sleeps = []
        outcomes = list(outcomes)

        def opener(req, timeout):
            o = outcomes.pop(0)
            if isinstance(o, Exception):
                raise o
            return o
        return lore.Lore(delay=0, sleep=self.sleeps.append, opener=opener)

    def test_retries_then_succeeds(self):
        c = self.client([http_error(503), urllib.error.URLError("reset"), Response(b"From x\n")])
        with unittest.mock.patch("sys.stderr"):
            self.assertEqual(c._get("https://x.org/"), b"From x\n")
        self.assertEqual(self.sleeps, [5, 10])

    def test_retry_after_is_honoured(self):
        c = self.client([http_error(429, "60"), Response(b"")])
        with unittest.mock.patch("sys.stderr"):
            c._get("https://x.org/")
        self.assertEqual(self.sleeps, [60])

    def test_no_retry_on_403_and_404_is_empty(self):
        with self.assertRaises(lore.LoreError):
            self.client([http_error(403)])._get("https://x.org/")
        self.assertEqual(self.client([http_error(404)])._get("https://x.org/"), b"")

    def test_gives_up(self):
        with unittest.mock.patch("sys.stderr"), self.assertRaises(lore.LoreError):
            self.client([http_error(502)] * 4)._get("https://x.org/")

    def test_bot_check_page_is_an_error(self):
        with self.assertRaises(lore.LoreError):
            self.client([Response(b"<html>", "text/html")])._get("https://x.org/")


class Windows(unittest.TestCase):
    def test_split_and_dedupe(self):
        day = 86400
        # One message per hour over 40 days; the query names its window.
        mails = [{"Message-ID": f"<{h}@x>", "t": h * 3600} for h in range(40 * 24)]
        seen = []

        def search(q):
            lo, hi = map(int, q.rsplit("rt:", 1)[1].split(".."))
            seen.append((lo, hi))
            return [m for m in mails if lo <= m["t"] <= hi]

        got = lore.windowed(search, "f:a", 0, 40 * day, days=30, split_at=500)
        self.assertEqual(len(got), len(mails))  # all of them, boundary duplicates merged
        self.assertEqual(seen[0], (0, 30 * day))  # 721 results: split
        self.assertIn((0, 15 * day), seen)


if __name__ == "__main__":
    unittest.main()
