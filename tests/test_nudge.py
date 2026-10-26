import io
import os
import tempfile
import unittest
from datetime import datetime, timezone
from unittest import mock

from patchtrack import cli, nudge, series, store
from test_patchtrack import ME, mail, posting

NOW = datetime(2026, 10, 27, tzinfo=timezone.utc)


def family(*replies, sent="Mon, 05 Oct 2026 10:00:00 +0000"):
    root, msgs = posting("ng", 1, 1, sent)
    msgs[0]["To"] = "Maint <maint@x>"
    msgs[0]["Cc"] = f"list@x, {ME}"
    for n, (frm, body, date) in enumerate(replies):
        msgs.append(mail(f"r{n}@x", "Re: [PATCH 0/1] foo: do the thing", frm, irt=root, body=body, date=date))
    db = store.Store(":memory:")
    for m in msgs:
        db.add(m)
    stored = db.messages()
    db.close()
    found = series.build(stored, [ME], {root: "2026-10-26T00:00:00Z"})
    return series.families(found)[0], {m["msgid"]: m for m in stored}


class Classify(unittest.TestCase):
    def kind(self, *replies, **kw):
        n = nudge.classify(family(*replies, **kw)[0], NOW)
        return n and n.kind

    def test_silence(self):
        self.assertEqual(self.kind(), "resend")  # 21 days, no human
        self.assertEqual(self.kind(sent="Sat, 17 Oct 2026 10:00:00 +0000"), "ping")  # 9 days
        self.assertIsNone(self.kind(sent="Mon, 26 Oct 2026 10:00:00 +0000"))

    def test_question_owes_a_reply_tag_does_not(self):
        q = ("R <r@x>", "> +x\nWhy?\n", "Mon, 26 Oct 2026 10:00:00 +0000")
        tag = ("R <r@x>", "Reviewed-by: R <r@x>\n", "Tue, 13 Oct 2026 10:00:00 +0000")
        self.assertEqual(self.kind(q), "reply owed")
        self.assertEqual(self.kind(tag), "ping")  # reviewed, then two weeks of nothing

    def test_own_ping_restarts_the_clock(self):
        mine = (f"Dev <{ME}>", "Ping?\n", "Mon, 26 Oct 2026 10:00:00 +0000")
        self.assertIsNone(self.kind(mine))

    def test_draft(self):
        fam, msgs = family()
        d = nudge.draft(nudge.classify(fam, NOW), msgs, ME, "https://lore.kernel.org")
        self.assertEqual(d["In-Reply-To"], f"<{fam.head.root}>")
        self.assertEqual((d["To"], d["Cc"]), ("maint@x", "list@x"))
        self.assertEqual(d["Content-Transfer-Encoding"], "7bit")
        self.assertIn("b4 send --resend v1", d.get_content())


class MergeWindow(unittest.TestCase):
    def repo(self, *tags):
        import subprocess
        from test_trees import ENV
        d = tempfile.mkdtemp()
        env = dict(os.environ, **ENV)
        git = lambda *a, **kw: subprocess.run(["git", "-C", d, *a], check=True, capture_output=True,
                                              env=dict(env, **kw))
        git("init", "-q")
        for tag, date in tags:
            git("commit", "-q", "--allow-empty", "-m", tag, GIT_COMMITTER_DATE=date, GIT_AUTHOR_DATE=date)
            git("tag", "-a", "-m", tag, tag, GIT_COMMITTER_DATE=date)
        return d

    def test_open_closed_stale(self):
        w = nudge.merge_window(self.repo(("v7.3-rc7", "2026-10-11T20:00:00Z"), ("v7.3", "2026-10-18T20:00:00Z")), NOW)
        self.assertEqual((w.open, w.tag), (True, "v7.3"))
        w = nudge.merge_window(self.repo(("v7.3", "2026-10-18T20:00:00Z"), ("v7.4-rc1", "2026-10-25T20:00:00Z")), NOW)
        self.assertIs(w.open, False)
        w = nudge.merge_window(self.repo(("v7.2", "2026-08-01T20:00:00Z")), NOW)
        self.assertIsNone(w.open)
        self.assertIn("stale", w.why)
        self.assertIsNone(nudge.merge_window(self.repo(("release-1", "2026-10-20T00:00:00Z")), NOW).open)

    def test_open_window_holds_pings_not_replies(self):
        win = nudge.Window(True, "v7.3", "2026-10-18")
        fam = family()[0]
        self.assertEqual(nudge.classify(fam, NOW, window=win).kind, "resend after -rc1")
        q = ("R <r@x>", "> +x\nWhy?\n", "Mon, 26 Oct 2026 10:00:00 +0000")
        self.assertEqual(nudge.classify(family(q)[0], NOW, window=win).kind, "reply owed")


class Close(unittest.TestCase):
    def test_close_hides_and_reopen_restores(self):
        tmp = tempfile.mkdtemp()
        db_path = os.path.join(tmp, "t.sqlite3")
        db = store.Store(db_path)
        for m in posting("cl", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")[1]:
            db.add(m)
        db.commit()
        db.close()

        def run(*argv):
            out = io.StringIO()
            with mock.patch("sys.stdout", out):
                cli.main(["--config", os.devnull, "--db", db_path, *argv])
            return out.getvalue()
        base = ("-a", ME)
        self.assertIn("do the thing", run("list", *base, "--color", "never"))
        run("close", "do the thing", *base, "--reason", "went elsewhere")
        self.assertNotIn("do the thing", run("list", *base, "--color", "never"))
        self.assertIn("closed by you: went elsewhere", run("list", *base, "--closed", "--color", "never"))
        self.assertIn("Nothing to nudge", run("nudge", *base, "--color", "never"))
        run("reopen", "do the thing", *base)
        self.assertIn("resend", run("nudge", *base, "--color", "never"))


if __name__ == "__main__":
    unittest.main()
