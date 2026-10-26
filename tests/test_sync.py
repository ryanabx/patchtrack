import io
import os
import tempfile
import unittest
from unittest import mock

from patchtrack import cli, lore
from test_patchtrack import ME, mail, posting


class FakeLore:
    """An archive in memory: search returns everything, thread() its thread."""
    mails = []
    fetched = []
    broken_search = False
    broken_threads = set()

    def __init__(self, **kwargs):
        pass

    def search(self, query):
        if FakeLore.broken_search:
            raise lore.LoreError("HTTP 503")
        return list(self.mails)

    def thread(self, msgid):
        if msgid in FakeLore.broken_threads:
            raise lore.LoreError("HTTP 503")
        FakeLore.fetched.append(msgid)
        return [m for m in self.mails if msgid in (m["Message-ID"] + str(m["References"] or ""))]


def setUpModule():
    # Never read the user's real config: its authors and trees would leak in.
    global _xdg
    _xdg = mock.patch.dict(os.environ, {"XDG_CONFIG_HOME": tempfile.mkdtemp()})
    _xdg.start()


def tearDownModule():
    _xdg.stop()


class Sync(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.db = os.path.join(self.tmp.name, "t.sqlite3")
        self.a, ma = posting("a", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        self.b, mb = posting("b", 1, 1, "Mon, 05 Oct 2026 11:00:00 +0000")
        FakeLore.mails = ma + mb
        FakeLore.fetched = []
        FakeLore.broken_search = False
        FakeLore.broken_threads = set()
        patcher = mock.patch.object(lore, "Lore", FakeLore)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(self.tmp.cleanup)

    def sync(self, *extra):
        FakeLore.fetched = []
        with mock.patch("sys.stderr"):
            self.status = cli.main(["--db", self.db, "sync", "-a", ME, *extra])
        return sorted(FakeLore.fetched)

    def test_first_full_then_only_what_changed(self):
        self.assertEqual(self.sync(), sorted([self.a, self.b]))
        self.assertEqual(self.sync(), [])
        FakeLore.mails.append(mail("rev@x", "Re: [PATCH 0/1] foo: do the thing", "R <r@x>", irt=self.b,
                                   body="Why?\n", date="Tue, 06 Oct 2026 10:00:00 +0000"))
        self.assertEqual(self.sync(), [self.b])

    def test_stale_open_series_are_refreshed(self):
        self.sync()
        self.assertEqual(self.sync("--refresh-days", "0"), sorted([self.a, self.b]))

    def test_failed_search_records_nothing(self):
        FakeLore.broken_search = True
        self.assertEqual(self.sync(), [])
        self.assertEqual(self.status, 1)
        FakeLore.broken_search = False
        # Not incremental: the failed run did not count as a sync.
        self.assertEqual(self.sync(), sorted([self.a, self.b]))

    def test_failed_thread_is_retried(self):
        FakeLore.broken_threads = {self.b}
        self.assertEqual(self.sync(), [self.a])
        self.assertEqual(self.status, 1)
        FakeLore.broken_threads = set()
        self.assertEqual(self.sync(), [self.b])  # nothing new, but b is still owed
        self.assertEqual(self.sync(), [])

    def test_failed_refetch_is_retried(self):
        self.sync()  # both fetched
        FakeLore.mails.append(mail("rev@x", "Re: [PATCH 0/1] foo: do the thing", "R <r@x>", irt=self.b,
                                   body="Why?\n", date="Tue, 06 Oct 2026 10:00:00 +0000"))
        FakeLore.broken_threads = {self.b}
        self.assertEqual(self.sync(), [])  # the reply was seen; fetching its thread failed
        FakeLore.broken_threads = set()
        # No new mail now, and b was fetched recently: only the pending mark brings it back.
        self.assertEqual(self.sync(), [self.b])

    def test_no_threads_defers(self):
        self.assertEqual(self.sync("--no-threads"), [])
        self.assertEqual(self.sync(), sorted([self.a, self.b]))

    def test_config_supplies_authors_lists_and_trees(self):
        conf = os.path.join(self.tmp.name, "c.toml")
        with open(conf, "w") as f:
            f.write(f'[sync]\nauthors = ["{ME}"]\nlists = ["one", "two"]\n\n'
                    '[[trees]]\nname = "t"\nrepo = "/r"\nref = "main"\n')
        searched = []
        orig = FakeLore.__init__

        def init(fake, **kw):
            searched.append(kw.get("inbox"))
        FakeLore.__init__ = init
        try:
            with mock.patch("sys.stderr"):
                self.assertEqual(cli.main(["--config", conf, "--db", self.db, "sync"]), 0)
        finally:
            FakeLore.__init__ = orig
        self.assertEqual(searched[:2], ["one", "two"])
        out = io.StringIO()
        with mock.patch("sys.stdout", out):
            cli.main(["--config", conf, "--db", self.db, "config"])
        self.assertIn("lists = ['one', 'two']", out.getvalue())
        self.assertIn("name = 't'", out.getvalue())
        with mock.patch("sys.stderr"), self.assertRaises(SystemExit):
            cli.main(["--config", conf, "--db", self.db, "tree", "rm", "t"])

    def test_no_authors_anywhere(self):
        with mock.patch("sys.stderr"), self.assertRaises(SystemExit):
            cli.main(["--db", self.db, "--config", os.devnull, "sync"])

    def test_full(self):
        self.sync()
        self.assertEqual(self.sync("--full"), sorted([self.a, self.b]))


if __name__ == "__main__":
    unittest.main()
