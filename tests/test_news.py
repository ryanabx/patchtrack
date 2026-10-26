import io
import os
import tempfile
import unittest
from unittest import mock

from patchtrack import cli, store
from test_patchtrack import ME, mail, posting


class News(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = os.path.join(self.tmp.name, "t.sqlite3")
        self.r1, msgs = posting("nw", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        self.add(*msgs)

    def add(self, *msgs):
        db = store.Store(self.db)
        for m in msgs:
            db.add(m)
        db.mark_thread(self.r1, "2026-10-05T12:00:00Z")
        db.commit()
        db.close()

    def news(self, *extra):
        out = io.StringIO()
        with mock.patch("sys.stdout", out):
            cli.main(["--config", os.devnull, "--db", self.db, "news", "-a", ME, "--color", "never", *extra])
        return out.getvalue()

    def test_first_look_then_changes(self):
        self.assertIn("First look: 1 series recorded", self.news())
        self.assertIn("Nothing new.", self.news())
        self.add(mail("c@x", "Re: [PATCH 0/1] foo: do the thing", "Rev <r@x>", irt=self.r1,
                      body="> +x\nThis needs a lock.\n", date="Tue, 06 Oct 2026 10:00:00 +0000"))
        out = self.news("--peek")
        self.assertIn("○ no-reply → ● commented", out)
        self.assertIn('2026-10-06 Rev commented: "This needs a lock."', out)
        self.assertEqual(out, self.news())  # --peek did not mark it seen
        self.assertIn("Nothing new.", self.news())

    def test_new_version_and_applied(self):
        self.news()
        r2, msgs = posting("nw", 2, 1, "Mon, 12 Oct 2026 10:00:00 +0000")
        self.add(*msgs, mail("a@x", "Re: [PATCH v2 0/1] foo: do the thing", "M <m@x>", irt=r2,
                            body="Applied, thanks!\n", date="Tue, 13 Oct 2026 10:00:00 +0000"))
        out = self.news()
        self.assertIn("v2 posted", out)
        self.assertIn("→ ● applied", out)
        self.assertIn('M "Applied, thanks!"', out)

    def test_since(self):
        self.add(mail("c@x", "Re: [PATCH 0/1] foo: do the thing", "Rev <r@x>", irt=self.r1,
                      body="Reviewed-by: Rev <r@x>\n", date="Tue, 06 Oct 2026 10:00:00 +0000"))
        out = self.news("--since", "2026-10-06T00:00:00Z")
        self.assertIn("no-reply → ● reviewed", out)
        self.assertIn("Rev Reviewed-By", out)
        self.assertIn("Nothing new.", self.news("--since", "2026-10-07T00:00:00Z"))
        self.assertIn("First look", self.news())  # --since did not record a snapshot


if __name__ == "__main__":
    unittest.main()
