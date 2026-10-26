import mailbox
import os
import tempfile
import time
import unittest
from unittest import mock

from patchtrack import cli, local, lore, store
from test_patchtrack import ME, mail, posting
from test_sync import FakeLore


class Local(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root, self.series = posting("loc", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        self.reply = mail("rev@x", "Re: [PATCH 0/1] foo: do the thing", "R <r@x>", irt=self.root,
                          body="> +x\nWhy?\n", date="Tue, 06 Oct 2026 10:00:00 +0000")
        self.noise = mail("other@x", "[PATCH] somebody else's patch", "S <s@x>")
        self.db = store.Store(os.path.join(self.tmp.name, "t.sqlite3"))
        self.addCleanup(self.db.close)

    def maildir(self, msgs, name="md"):
        md = mailbox.Maildir(os.path.join(self.tmp.name, name))
        for m in msgs:
            md.add(m)
        return md._path

    def read(self, *paths):
        return local.read(self.db, paths, [ME], self.db.last_read)

    def test_maildir_keeps_only_whats_relevant(self):
        count, new = self.read(self.maildir(self.series + [self.reply, self.noise]))
        self.assertEqual(count, 4)
        self.assertEqual(new, {self.root, self.series[1]["Message-ID"].strip("<>"), "rev@x"})

    def test_mbox_and_eml_directory(self):
        box = mailbox.mbox(os.path.join(self.tmp.name, "sent.mbox"))
        for m in self.series:
            box.add(m)
        box.flush()
        eml = os.path.join(self.tmp.name, "eml")
        os.mkdir(eml)
        with open(os.path.join(eml, "reply.eml"), "wb") as f:
            f.write(self.reply.as_bytes())
        _, new = self.read(box._path, eml)
        self.assertIn("rev@x", new)
        self.assertIn(self.root, new)

    def test_incremental(self):
        path = self.maildir(self.series)
        self.read(path)
        self.db.mark_read(path, "2026-10-27T00:00:00Z")
        old = time.mktime((2026, 1, 1, 0, 0, 0, 0, 0, 0))
        for d in ("cur", "new"):
            for f in os.listdir(os.path.join(path, d)):
                os.utime(os.path.join(path, d, f), (old, old))
        mailbox.Maildir(path).add(self.reply)  # delivered now
        count, new = self.read(path)
        self.assertEqual((count, new), (1, {"rev@x"}))  # only the new file was opened


class SyncWithLocal(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root, msgs = posting("sl", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        md = mailbox.Maildir(os.path.join(self.tmp.name, "md"))
        for m in msgs:
            md.add(m)
        self.conf = os.path.join(self.tmp.name, "c.toml")
        with open(self.conf, "w") as f:
            f.write(f'[sync]\nauthors = ["{ME}"]\n[sources]\nmaildirs = ["{md._path}"]\n')
        self.db = os.path.join(self.tmp.name, "t.sqlite3")
        self.md, self.msgs = md, msgs
        FakeLore.mails, FakeLore.fetched = list(msgs), []
        FakeLore.broken_search, FakeLore.broken_threads = False, set()
        patcher = mock.patch.object(lore, "Lore", FakeLore)
        patcher.start()
        self.addCleanup(patcher.stop)

    def run_cli(self, *argv):
        FakeLore.fetched = []
        with mock.patch("sys.stderr"), mock.patch("sys.stdout"):
            return cli.main(["--config", self.conf, "--db", self.db, *argv])

    def test_offline_never_touches_the_archive(self):
        with mock.patch.object(FakeLore, "search", side_effect=AssertionError("searched")):
            self.assertEqual(self.run_cli("sync", "--offline"), 0)
        db = store.Store(self.db)
        self.assertIn(self.root, db.local_times())
        db.close()

    def test_new_local_mail_refetches_its_thread(self):
        self.run_cli("sync")
        self.run_cli("sync")
        self.assertEqual(FakeLore.fetched, [])  # nothing new anywhere
        self.md.add(mail("rev@x", "Re: [PATCH 0/1] foo: do the thing", "R <r@x>", irt=self.root,
                         body="Why?\n", date="Tue, 06 Oct 2026 10:00:00 +0000"))
        self.run_cli("sync")
        self.assertEqual(FakeLore.fetched, [self.root])


def setUpModule():
    global _xdg
    _xdg = mock.patch.dict(os.environ, {"XDG_CONFIG_HOME": tempfile.mkdtemp()})
    _xdg.start()


def tearDownModule():
    _xdg.stop()


if __name__ == "__main__":
    unittest.main()
