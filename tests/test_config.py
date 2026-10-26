import os
import tempfile
import unittest

from patchtrack import config, parse, series
from test_patchtrack import ME, build, mail


def write(text):
    f = tempfile.NamedTemporaryFile("w", suffix=".toml", delete=False)
    f.write(text)
    f.close()
    return f.name


class Bots(unittest.TestCase):
    def rules(self, text):
        path = write(text)
        try:
            return config.bot_rules(config.load(path))
        finally:
            os.unlink(path)

    def test_builtin(self):
        r = config.BotRules()
        self.assertTrue(r.is_bot("sashiko-bot@kernel.org"))
        self.assertTrue(r.is_bot("lkp@intel.com"))
        self.assertFalse(r.is_bot("abbott@example.org"))  # "bot" inside a word

    def test_configured(self):
        r = self.rules('[bots]\naddresses = ["CI@x.org"]\npatterns = ["*@ci.example.org"]\n'
                       'humans = ["review-bot@x.org"]\nbodies = ["^My bot found errors"]\n')
        self.assertTrue(r.is_bot("ci@x.org"))
        self.assertTrue(r.is_bot("runner7@ci.example.org"))
        self.assertFalse(r.is_bot("review-bot@x.org"))  # humans beat the built-in rule
        self.assertTrue(r.is_bot("reviewer@example.org", "My bot found errors running 'make'\n"))
        self.assertFalse(r.is_bot("reviewer@example.org", "> My bot found errors\nAcked-by: Rev\n"))

    def test_builtin_off(self):
        self.assertFalse(self.rules("[bots]\nbuiltin = false\n").is_bot("x-bot@y.org"))

    def test_unknown_keys_are_errors(self):
        for text in ("[bot]\n", "[bots]\naddress = []\n", "[bots]\nbodies = ['(']\n", "[bots\n"):
            with self.assertRaises(config.ConfigError, msg=text):
                self.rules(text)

    def test_missing_default_is_empty_missing_explicit_is_error(self):
        os.environ["XDG_CONFIG_HOME"] = tempfile.mkdtemp()
        try:
            self.assertEqual(config.load(), {})
            with self.assertRaises(config.ConfigError):
                config.load("/nonexistent/patchtrack.toml")
        finally:
            del os.environ["XDG_CONFIG_HOME"]

    def settings(self, text):
        path = write(text)
        try:
            return config.settings(path)
        finally:
            os.unlink(path)

    def test_sync_and_trees(self):
        c = self.settings('[sync]\nauthors = ["Me@X.org"]\nlists = ["linux-usb", "u-boot"]\nsince = "6.months.ago"\n'
                          'window_days = 14\n\n[[trees]]\nname = "next"\nrepo = "~/linux"\n'
                          'url = "https://example.org/next.git"\nbranch = "master"\nrole = "next"\n\n'
                          '[[trees]]\nname = "local"\nrepo = "/r"\nref = "origin/main"\n')
        self.assertEqual((c.authors, c.lists, c.since, c.window_days), (("me@x.org",), ("linux-usb", "u-boot"),
                                                                       "6.months.ago", 14))
        nxt, local = c.trees
        self.assertEqual((nxt.ref, nxt.role, nxt.repo), ("refs/patchtrack/next", "next",
                                                         os.path.expanduser("~/linux")))
        self.assertEqual((local.ref, local.role, local.url), ("origin/main", "maintainer", None))

    def test_bad_sync_and_trees(self):
        for text in ('[sync]\nauthors = "me@x"\n',          # a string, not a list
                     '[sync]\nwindow_days = true\n',
                     '[sync]\nsince = "20261023x"\n',
                     '[[trees]]\nname = "t"\n',               # no repo
                     '[[trees]]\nname = "t"\nrepo = "/r"\n',  # neither ref nor url
                     '[[trees]]\nname = "t"\nrepo = "/r"\nref = "x"\nrole = "upstream"\n',
                     '[[trees]]\nname = "t"\nrepo = "/r"\nref = "x"\nbrnach = "y"\n'):
            with self.assertRaises(config.ConfigError, msg=text):
                self.settings(text)

    def test_sourceware_approvals(self):
        rx = config.SOURCEWARE.approvals
        for line in ("OK.", "Ok", "LGTM.", "This is OK.", "OK for trunk.", "Ok for trunk!", "OK. No issues."):
            self.assertIsNotNone(parse.first_match(line, rx), line)
        for line in ("OK for mainline?", "ok, will do.", "OK, not really wrong, but still odd", "Okay then"):
            self.assertIsNone(parse.first_match(line, rx), line)

    def series_to(self, to, *replies, profiles=config.BUILTIN_PROFILES):
        from patchtrack import store
        from test_patchtrack import posting
        root, msgs = posting("pf", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        msgs[0]["To"] = to
        for n, (frm, body) in enumerate(replies):
            msgs.append(mail(f"r{n}@x", "Re: [PATCH 0/1] foo: do the thing", frm, irt=root, body=body,
                             date=f"Tue, 06 Oct 2026 1{n}:00:00 +0000"))
        db = store.Store(":memory:")
        for m in msgs:
            db.add(m)
        out = series.build(db.messages(), [ME], {root: "2026-10-07T00:00:00Z"}, profiles=profiles)[root]
        db.close()
        return out

    def test_gcc_series(self):
        s = self.series_to("gcc-patches@gcc.gnu.org", ("Rev <r@x>", "> +x\nOK.\n"),
                           (ME, "Pushed as r17-1234, thanks.\n"))
        self.assertEqual((s.profile.name, s.state), ("sourceware", "applied"))
        s = self.series_to("gcc-patches@gcc.gnu.org", ("Rev <r@x>", "> +x\nOK.\n"))
        self.assertEqual(s.state, "reviewed")
        self.assertIn("Approved", s.evidence)

    def test_kernel_series_have_no_profile(self):
        s = self.series_to("linux-usb@vger.kernel.org", ("Rev <r@x>", "OK.\n"), (ME, "Pushed as abc.\n"))
        self.assertIsNone(s.profile)
        self.assertEqual(s.state, "discussed")

    def test_config_profile_replaces_builtin(self):
        c = self.settings('[[profiles]]\nname = "sourceware"\nrecipients = ["*@gcc.gnu.org"]\n'
                          'approvals = ["^ship it"]\n\n[[profiles]]\nname = "x"\nrecipients = ["dev@x.org"]\n')
        self.assertEqual([p.name for p in c.profiles], ["sourceware", "x"])
        s = self.series_to("gcc-patches@gcc.gnu.org", ("Rev <r@x>", "Ship it!\n"), profiles=c.profiles)
        self.assertEqual(s.state, "reviewed")
        with self.assertRaises(config.ConfigError):
            self.settings('[[profiles]]\nname = "y"\nrecipients = ["a"]\nlanded = ["("]\n')

    def test_state_follows_rules(self):
        root = "solo@example.org"
        msgs = [mail(root, "[PATCH] bar: fix it"),
                mail("r@x", "Re: [PATCH] bar: fix it", "CI <ci@x.org>", irt=root, body="Build OK.\n",
                     date="Tue, 06 Oct 2026 10:00:00 +0000")]
        self.assertEqual(build(msgs)[root].state, "discussed")
        from patchtrack import store
        db = store.Store(":memory:")
        for m in msgs:
            db.add(m)
        rules = config.BotRules(addresses=frozenset({"ci@x.org"}))
        s = series.build(db.messages(), [ME], {root: "2026-10-07T00:00:00Z"}, bots=rules)[root]
        db.close()
        self.assertEqual(s.state, "bot-only")


if __name__ == "__main__":
    unittest.main()
