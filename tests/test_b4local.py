import io
import json
import os
import subprocess
import tempfile
import unittest
from unittest import mock

from patchtrack import cli, store
from test_patchtrack import ME, mail
from test_trees import ENV


def tracking(revision, history):
    body = {"series": {"revision": revision, "change-id": "20261001-feat-abcdef123456", "prefixes": [],
                       "history": history}}
    return "feat: the cover\n\n--- b4-submit-tracking ---\n# internal\n" + json.dumps(body, indent=2) + "\n"


class Branches(unittest.TestCase):
    def setUp(self):
        self._env = mock.patch.dict(os.environ, ENV)
        self._env.start()
        self.addCleanup(self._env.stop)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = os.path.join(self.tmp.name, "repo")
        self.git("init", "-q", "-b", "main", self.repo, cwd=None)
        self.write("a.c", "one\n")
        self.commit("base")

    def git(self, *args, cwd=True):
        return subprocess.run(["git", *(["-C", self.repo] if cwd else []), *args], check=True,
                              capture_output=True, text=True).stdout

    def write(self, name, text):
        with open(os.path.join(self.repo, name), "w") as f:
            f.write(text)

    def commit(self, msg, empty=False):
        self.git("add", "-A")
        self.git("commit", "-q", *(["--allow-empty"] if empty else []), "-m", msg)

    def branch(self, name, history, revision):
        self.git("checkout", "-q", "-b", name, "main")
        self.commit(tracking(revision, history), empty=True)
        self.write("a.c", "one\ntwo\n")
        self.commit("a: add two")
        diff = self.git("show", "--format=", "HEAD")
        self.git("checkout", "-q", "main")
        return diff

    def run_cli(self, posted_diff):
        db_path = os.path.join(self.tmp.name, "t.sqlite3")
        db = store.Store(db_path)
        db.add(mail("feat-v1-0@x", "[PATCH 0/1] feat: the cover", ME))
        db.add(mail("feat-v1-1@x", "[PATCH 1/1] a: add two", ME, irt="feat-v1-0@x",
                    body="Add two.\n---\n" + posted_diff))
        db.commit()
        db.close()
        out = io.StringIO()
        with mock.patch("sys.stdout", out):
            cli.main(["--config", os.devnull, "--db", db_path, "branches", "-a", ME, "--repo", self.repo,
                      "--color", "never"])
        return out.getvalue()

    def test_as_sent_changed_and_draft(self):
        diff = self.branch("b4/feat", {"v1": ["feat-v1-0@x"]}, 2)
        self.branch("b4/new", {}, 1)
        out = self.run_cli(diff)
        self.assertIn("b4/feat", out)
        self.assertIn("as sent in v1", out)
        self.assertIn("◌ draft", out)
        self.assertIn("v1 never sent; 1 patch\n", out)
        out = self.run_cli(diff.replace("+two", "+TWO"))
        self.assertIn("v2 in preparation: 1 changed since v1", out)


if __name__ == "__main__":
    unittest.main()
