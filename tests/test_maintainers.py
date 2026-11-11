import io
import os
import subprocess
import tempfile
import unittest
from unittest import mock

from patchtrack import cli, maintainers, store
from test_patchtrack import ME, mail
from test_trees import ENV

MAINTAINERS = """\
List of maintainers
===================

Descriptions of section entries and preferred order
---------------------------------------------------

\tM: *Mail* patches to: FullName <address@domain>
\tT: *SCM* tree type and location.

FOO NET DRIVERS
M:\tNet Maint <net@example.org>
L:\tnetdev@example.org
T:\tgit git://git.example.org/pub/scm/net/net.git
T:\tgit https://git.example.org/pub/scm/net/net-next.git main
F:\tdrivers/net/
X:\tdrivers/net/vendor/

VENDOR DRIVER
M:\tVendor Person <vendor@example.org>
L:\tnetdev@example.org
T:\tgit https://github.com/vendor/wireless
F:\tdrivers/net/vendor/

ONE FILE
M:\tFile Person <file@example.org>
L:\tfile@example.org
T:\tgit git://git.example.org/pub/scm/file.git -b for-next
F:\tinclude/foo.h

BY NAME
M:\tName Person <name@example.org>
L:\tname@example.org
T:\tgit https://git.example.org/pub/scm/name.git topic
N:\tfrobnicat

GLOBBED
L:\tglob@example.org
T:\tgit https://git.example.org/pub/scm/glob.git next
F:\tarch/*/boot/dts/acme*

NO TREE
L:\tnotree@example.org
F:\tdrivers/char/

THE REST
M:\tLinus <torvalds@example.org>
L:\tlinux-kernel@example.org
T:\tgit git://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git
F:\t*
F:\t*/
"""

NEXT_TREES = """\
Trees included into this release:

Name\t\tUrl
----\t\t---
origin\t\thttps://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git#master
net\t\thttps://git.example.org/pub/scm/net/net.git#main
net-next\thttps://git.example.org/pub/scm/net/net-next.git#main
"""


class Matching(unittest.TestCase):
    def setUp(self):
        self.sections = {s.name: s for s in maintainers.parse(MAINTAINERS)}

    def test_preamble_is_not_a_section(self):
        self.assertEqual(list(self.sections), ["FOO NET DRIVERS", "VENDOR DRIVER", "ONE FILE", "BY NAME",
                                               "GLOBBED", "NO TREE", "THE REST"])

    def test_directory_and_exclude(self):
        net, vendor = self.sections["FOO NET DRIVERS"], self.sections["VENDOR DRIVER"]
        self.assertTrue(maintainers.covers(net, "drivers/net/a/b.c"))
        self.assertFalse(maintainers.covers(net, "drivers/net/vendor/x.c"))
        self.assertTrue(maintainers.covers(vendor, "drivers/net/vendor/x.c"))
        self.assertFalse(maintainers.covers(net, "drivers/netx/a.c"))

    def test_file_pattern_is_not_a_prefix_of_other_names(self):
        one = self.sections["ONE FILE"]
        self.assertTrue(maintainers.covers(one, "include/foo.h"))
        self.assertFalse(maintainers.covers(one, "include/foo.hh"))

    def test_glob_and_regex(self):
        self.assertTrue(maintainers.covers(self.sections["GLOBBED"], "arch/arm64/boot/dts/acme-board.dts"))
        self.assertFalse(maintainers.covers(self.sections["GLOBBED"], "arch/arm64/boot/dts/other.dts"))
        self.assertTrue(maintainers.covers(self.sections["BY NAME"], "lib/frobnicate.c"))
        self.assertFalse(maintainers.covers(self.sections["BY NAME"], "lib/other.c"))

    def test_tree_lines(self):
        self.assertEqual(self.sections["FOO NET DRIVERS"].trees,
                         [("git://git.example.org/pub/scm/net/net.git", None),
                          ("https://git.example.org/pub/scm/net/net-next.git", "main")])
        self.assertEqual(self.sections["ONE FILE"].trees, [("git://git.example.org/pub/scm/file.git", "for-next")])

    def test_urls_compare_across_spellings(self):
        self.assertEqual(maintainers.normalize_url("git://git.example.org/pub/scm/net/net.git"),
                         maintainers.normalize_url("https://git.example.org/pub/scm/net/net/"))
        self.assertNotEqual(maintainers.normalize_url("https://git.example.org/a/net"),
                            maintainers.normalize_url("https://git.example.org/a/net-next"))

    def test_routes(self):
        nt = maintainers.parse_next_trees(NEXT_TREES)
        rs = maintainers.routes(list(self.sections.values()), ["drivers/net/a.c"], nt)
        # T: without a branch takes Next/Trees' (git:// there, https:// here);
        # T: with one keeps it; mainline (THE REST) is left out.
        self.assertEqual([(r.section, r.branch, r.source, maintainers.tree_name(r)) for r in rs],
                         [("FOO NET DRIVERS", "main", "Next/Trees", "net"),
                          ("FOO NET DRIVERS", "main", "T:", "net-next")])

    def test_unknown_branch_stays_unknown(self):
        rs = maintainers.routes(list(self.sections.values()), ["drivers/net/vendor/x.c"],
                                maintainers.parse_next_trees(NEXT_TREES))
        self.assertEqual([(r.url, r.branch) for r in rs], [("https://github.com/vendor/wireless", None)])
        self.assertEqual(maintainers.tree_name(rs[0]), "vendor-wireless")

    def test_section_without_tree(self):
        self.assertEqual(maintainers.routes(list(self.sections.values()), ["drivers/char/x.c"]), [])

    def test_lists_only(self):
        lists = maintainers.addresses(MAINTAINERS)
        self.assertIn("netdev@example.org", lists)
        self.assertNotIn("net@example.org", lists)  # a person, who may be in another project too


class Discover(unittest.TestCase):
    def setUp(self):
        self._env = mock.patch.dict(os.environ, ENV)
        self._env.start()
        self.addCleanup(self._env.stop)
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.repo = os.path.join(self.tmp.name, "repo")
        subprocess.run(["git", "init", "-q", "-b", "main", self.repo], check=True)
        with open(os.path.join(self.repo, "MAINTAINERS"), "w") as f:
            f.write(MAINTAINERS)
        subprocess.run(["git", "-C", self.repo, "add", "-A"], check=True)
        subprocess.run(["git", "-C", self.repo, "commit", "-q", "-m", "base"], check=True)
        self.next_trees = os.path.join(self.tmp.name, "Trees")
        with open(self.next_trees, "w") as f:
            f.write(NEXT_TREES)
        self.db_path = os.path.join(self.tmp.name, "t.sqlite3")
        db = store.Store(self.db_path)
        for tag, path, to in (("net", "drivers/net/a.c", "netdev@example.org"),
                              ("vendor", "drivers/net/vendor/x.c", "netdev@example.org"),
                              ("other", "drivers/net/a.c", "other-project@example.org")):
            m = mail(f"{tag}-1@x", f"[PATCH] {tag}: change", ME,
                     body=f"Change.\n---\ndiff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n"
                          "@@ -1 +1 @@\n-a\n+b\n")
            m["To"] = to
            db.add(m)
        db.commit()
        db.close()

    def run_cli(self, *extra, where=None, next_trees=True, tty=False, answers=(), fetched=None):
        out = io.StringIO()
        args = ["--config", os.devnull, "--db", self.db_path, "tree", "discover", "-a", ME]
        args += [] if where == "cwd" else ["--maintainers", self.repo]
        args += ["--next-trees", self.next_trees] if next_trees else []
        answers = list(answers)
        self.heads_asked = []

        def heads(url):
            self.heads_asked.append(url)
            return ["main", "pending", "queue"]
        cwd = os.getcwd()
        try:
            if where == "cwd":
                os.chdir(os.path.join(self.repo))
            with mock.patch("sys.stdout", out), mock.patch("sys.stderr", io.StringIO()), \
                    mock.patch("patchtrack.trees.remote_heads", heads), \
                    mock.patch("sys.stdin.isatty", lambda: tty), \
                    mock.patch("builtins.input", lambda prompt: answers.pop(0)), \
                    mock.patch("patchtrack.lore.get_text", fetched or self.no_network):
                cli.main(args + list(extra))
        finally:
            os.chdir(cwd)
        return out.getvalue()

    def no_network(self, url):
        raise AssertionError(f"fetched {url}")

    def trees(self):
        db = store.Store(self.db_path)
        got = {t.name: (t.url, t.branch) for t in db.trees()}
        db.close()
        return got

    def test_report(self):
        out = self.run_cli()
        self.assertIn("git://git.example.org/pub/scm/net/net.git main  (from Next/Trees; new: net)", out)
        self.assertIn("https://github.com/vendor/wireless  (no branch", out)
        self.assertIn("another project's series): 1\n  [PATCH] other: change", out)
        self.assertNotIn("torvalds", out)
        self.assertIn("branches: main pending queue", out)
        self.assertIn("patchtrack tree add vendor-wireless --repo", out)

    def test_maintainers_from_the_current_directory(self):
        self.assertIn("(from Next/Trees; new: net)", self.run_cli(where="cwd"))

    def test_next_trees_default_is_linux_next_for_the_kernel(self):
        asked = []

        def fetched(url):
            asked.append(url)
            return NEXT_TREES
        self.assertIn("(from Next/Trees; new: net)", self.run_cli(next_trees=False, fetched=fetched))
        self.assertEqual(asked, [maintainers.NEXT_TREES_URL])
        out = self.run_cli("--next-trees", "none", next_trees=False)  # no_network would fail
        self.assertIn("net.git  (no branch", out)

    def test_no_linux_next_for_another_project(self):
        with open(os.path.join(self.repo, "MAINTAINERS"), "w") as f:
            f.write(MAINTAINERS.split("THE REST")[0])  # a file without the kernel's mainline
        subprocess.run(["git", "-C", self.repo, "commit", "-q", "-am", "not the kernel"], check=True)
        out = self.run_cli(next_trees=False)  # no_network fails the test if it fetches
        self.assertIn("net.git  (no branch", out)

    def test_add_asks_for_the_unknown_branch(self):
        self.run_cli("--add", tty=True, answers=["nope", "2"])
        self.assertEqual(self.trees()["vendor-wireless-pending"], ("https://github.com/vendor/wireless", "pending"))
        # once checked, the tree is neither listed nor asked about again
        out = self.run_cli("--add", tty=True, answers=[])
        self.assertIn("checked as vendor-wireless-pending)", out)
        self.assertEqual(self.heads_asked, [])

    def test_add_skips_the_unknown_branch_on_enter_or_without_a_terminal(self):
        self.run_cli("--add", tty=True, answers=[""])
        self.assertEqual(set(self.trees()), {"net", "net-next"})
        out = self.run_cli("--add")
        self.assertIn("run in a terminal to be asked", out)
        self.assertEqual(set(self.trees()), {"net", "net-next"})

    def test_add(self):
        out = self.run_cli("--add")
        self.assertIn("Next: `patchtrack check --fetch`", out)
        db = store.Store(self.db_path)
        got = {t.name: (t.url, t.branch, t.role, t.ref) for t in db.trees()}
        db.close()
        self.assertEqual(got, {
            "net": ("git://git.example.org/pub/scm/net/net.git", "main", "maintainer", "refs/patchtrack/net"),
            "net-next": ("https://git.example.org/pub/scm/net/net-next.git", "main", "maintainer",
                         "refs/patchtrack/net-next")})
        # a second run finds them checked, and adds nothing
        out = self.run_cli("--add")
        self.assertIn("checked as net)", out)
        self.assertNotIn("\nadded ", "\n" + out)


if __name__ == "__main__":
    unittest.main()
