import os
import subprocess
import tempfile
import unittest
from datetime import datetime, timedelta, timezone

from patchtrack import series, trees

AUTHOR = "dev@example.org"
ENV = {"GIT_AUTHOR_NAME": "Dev", "GIT_AUTHOR_EMAIL": AUTHOR, "GIT_COMMITTER_NAME": "Maint",
       "GIT_COMMITTER_EMAIL": "maint@example.org", "GIT_CONFIG_GLOBAL": os.devnull,
       "GIT_CONFIG_NOSYSTEM": "1"}


def when(days=0):
    d = datetime.now(timezone.utc) + timedelta(days=days)
    return d.strftime("%Y-%m-%dT%H:%M:%SZ")


class Trees(unittest.TestCase):
    def setUp(self):
        self._env = {k: os.environ.get(k) for k in ENV}
        os.environ.update(ENV)
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = self.tmp.name
        self.git("init", "-q", "-b", "main")
        for name in ("a.c", "b.c", "c.c", "d.c", "e.c"):
            self.write(name, "one\ntwo\nthree\n")
        self.commit("base")

    def tearDown(self):
        self.tmp.cleanup()
        for k, v in self._env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v

    def git(self, *args):
        return subprocess.run(["git", "-C", self.repo, *args], check=True, capture_output=True,
                              text=True).stdout

    def write(self, name, text):
        with open(os.path.join(self.repo, name), "w") as f:
            f.write(text)

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)
        return self.git("rev-parse", "HEAD").strip()

    def posting(self, msgid, title, diff):
        return trees.Posted(msgid, title, AUTHOR, when(-1), f"{title}\n\nSigned-off-by: Dev\n---\n{diff}")

    def test_matchers(self):
        # Posted patches, as their diffs were when sent.
        self.write("a.c", "one\nTWO\nthree\n")
        p_link = self.posting("link-1@x", "a: shout two", self.git("diff"))
        self.git("checkout", "--", ".")
        self.write("b.c", "one\ntwo\nTHREE\n")
        p_pid = self.posting("pid-1@x", "b: shout three", self.git("diff"))
        self.git("checkout", "--", ".")
        self.write("c.c", "ONE\ntwo\nthree\n")
        p_subj = self.posting("subj-1@x", "c: shout one", self.git("diff"))
        self.git("checkout", "--", ".")
        self.write("d.c", "one\ntwo\nthree\nfour\n")
        p_cited = self.posting("cited-1@x", "d: add four", self.git("diff"))
        self.git("checkout", "--", ".")
        self.write("e.c", "one\ntwo\nthree\nfive\n")
        p_trim = self.posting("trim-1@x", "e: add five", self.git("diff"))
        self.git("checkout", "--", ".")

        # What the maintainer committed.
        self.write("a.c", "one\nTWO\nthree\n")
        self.write("b.c", "one\ntwo\nthree\n")
        c_link = self.commit("a: shout two, reworded\n\nLink: https://patch.msgid.link/link-1@x\n")
        self.write("b.c", "one\ntwo\nTHREE\n")
        c_pid = self.commit("b: a different subject")
        # Contains trim-1's diff plus another file: a different patch. Only
        # --full-diff stops the path filter trimming it into a false match.
        self.write("e.c", "one\ntwo\nthree\nfive\n")
        self.write("README", "unrelated file in the same commit\n")
        self.commit("e: something bigger")
        self.write("c.c", "ONE\ntwo\nthree\nextra\n")
        c_subj = self.commit("c: shout one")
        self.write("a.c", "zero\none\nTWO\nthree\n")
        self.commit("a: follow-up\n\nAs discussed in [1].\n\n[1] https://lore.kernel.org/all/cited-1@x/\n")

        tree = trees.Tree("t", self.repo, "main")
        sha, got = trees.check(tree, [p_link, p_pid, p_subj, p_cited, p_trim])
        self.assertEqual(sha, self.git("rev-parse", "main").strip())
        self.assertEqual(got["link-1@x"], (c_link, "message-id", None))
        self.assertEqual(got["pid-1@x"], (c_pid, "patch-id", None))
        self.assertEqual(got["subj-1@x"], (c_subj, "subject", None))
        self.assertIsNone(got["cited-1@x"])  # cited in a body, not a trailer
        self.assertIsNone(got["trim-1@x"])

    def test_fetch_whole_branch(self):
        self.write("a.c", "one\nTWO\nthree\n")
        diff = self.git("diff")
        sha = self.commit("a: shout two\n\nLink: https://lore.kernel.org/r/f-1@x\n")
        with tempfile.TemporaryDirectory() as local:
            subprocess.run(["git", "init", "-q", local], check=True)
            tree = trees.Tree("up", local, "refs/patchtrack/up", url=self.repo, branch="main")
            trees.fetch(tree)
            self.assertFalse(trees.is_shallow(local))
            got = trees.check(tree, [self.posting("f-1@x", "a: shout two", diff)])[1]
            self.assertEqual(got["f-1@x"], (sha, "message-id", None))

    def test_revert(self):
        self.write("a.c", "one\nTWO\nthree\n")
        diff = self.git("diff")
        sha = self.commit("a: shout two\n\nLink: https://lore.kernel.org/r/rv-1@x\n")
        self.git("revert", "--no-edit", sha)
        revert = self.git("rev-parse", "HEAD").strip()
        got = trees.check(trees.Tree("t", self.repo, "main"), [self.posting("rv-1@x", "a: shout two", diff)])[1]
        self.assertEqual(got["rv-1@x"], (sha, "message-id", revert))

    def test_revert_of_the_revert(self):
        self.write("a.c", "one\nTWO\nthree\n")
        diff = self.git("diff")
        sha = self.commit("a: shout two\n\nLink: https://lore.kernel.org/r/rr-1@x\n")
        self.git("revert", "--no-edit", sha)
        first = self.git("rev-parse", "HEAD").strip()
        self.git("revert", "--no-edit", first)
        posted = [self.posting("rr-1@x", "a: shout two", diff)]
        tree = trees.Tree("t", self.repo, "main")
        self.assertEqual(trees.check(tree, posted)[1]["rr-1@x"], (sha, "message-id", None))  # back in
        self.git("revert", "--no-edit", "HEAD")
        third = self.git("rev-parse", "HEAD").strip()
        self.assertEqual(trees.check(tree, posted)[1]["rr-1@x"], (sha, "message-id", third))

    def test_nothing_after_posting(self):
        self.write("a.c", "one\nTWO\nthree\n")
        diff = self.git("diff")
        self.commit("a: shout two")
        later = trees.Posted("late@x", "a: shout two", AUTHOR, when(+30), f"x\n---\n{diff}")
        self.assertIsNone(trees.check(trees.Tree("t", self.repo, "main"), [later])[1]["late@x"])


def row(tree, role, commit, at="2026-10-26T00:00:00Z", reverted_by=None):
    return {"tree": tree, "role": role, "ref_sha": "f" * 40, "commit_sha": commit, "how": "patch-id",
            "checked_at": at, "reverted_by": reverted_by}


class Stages(unittest.TestCase):
    def series_with(self, rows_by_index, applied_mail=False):
        msgs = [{"msgid": "c@x", "in_reply_to": None, "refs": "", "from_name": "", "from_addr": AUTHOR,
                 "date": "2026-10-01T00:00:00Z", "subject": "[PATCH 0/2] t: thing", "body": ""}]
        for i in (1, 2):
            msgs.append({"msgid": f"p{i}@x", "in_reply_to": "c@x", "refs": "c@x", "from_name": "",
                         "from_addr": AUTHOR, "date": "2026-10-01T00:00:00Z",
                         "subject": f"[PATCH {i}/2] t: part {i}", "body": ""})
        if applied_mail:
            msgs.append({"msgid": "y@x", "in_reply_to": "c@x", "refs": "c@x", "from_name": "M",
                         "from_addr": "m@x", "date": "2026-10-02T00:00:00Z",
                         "subject": "Re: [PATCH 0/2] t: thing", "body": "Applied, thanks.\n"})
        results = {f"p{i}@x": rows for i, rows in rows_by_index.items()}
        return series.build(msgs, [AUTHOR], {"c@x": "2026-10-03T00:00:00Z"}, None, results)["c@x"]

    def test_lowest_stage_wins(self):
        s = self.series_with({1: [row("mainline", "mainline", "a" * 40)],
                              2: [row("next", "next", "b" * 40), row("mainline", "mainline", None)]})
        self.assertEqual((s.state, s.stages), ("in-next", {1: 4, 2: 3}))
        self.assertIn("2/2 is bbbbbbbbbbbb in next", s.evidence)

    def test_partial(self):
        s = self.series_with({1: [row("mfd", "maintainer", "a" * 40)], 2: [row("mfd", "maintainer", None)]})
        self.assertEqual(s.state, "partially-applied")
        self.assertEqual(s.evidence, "1/2 queued, 2/2 not applied")

    def test_reverted_does_not_count(self):
        s = self.series_with({1: [row("next", "next", "a" * 40, reverted_by="b" * 40)],
                              2: [row("next", "next", "c" * 40)]})
        self.assertEqual(s.state, "partially-applied")
        self.assertIn("1/2 was aaaaaaaaaaaa in next, reverted by bbbbbbbbbbbb", s.evidence)

    def test_silent_apply_is_noted(self):
        s = self.series_with({1: [row("next", "next", "a" * 40)], 2: [row("next", "next", "c" * 40)]})
        self.assertEqual(s.state, "in-next")
        self.assertIn('no "applied" mail', s.evidence)
        s = self.series_with({1: [row("next", "next", "a" * 40)], 2: [row("next", "next", "c" * 40)]},
                             applied_mail=True)
        self.assertNotIn('no "applied" mail', s.evidence)

    def test_mail_says_applied_tree_does_not(self):
        s = self.series_with({1: [row("foo", "maintainer", None)], 2: [row("foo", "maintainer", None)]},
                             applied_mail=True)
        self.assertEqual(s.state, "applied")
        self.assertIn("not in foo as of 2026-10-26 00:00", s.evidence)


if __name__ == "__main__":
    unittest.main()
