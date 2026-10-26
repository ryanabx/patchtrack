import io
import os
import unittest
from unittest import mock

from patchtrack import pretty, series
from test_patchtrack import build, mail, posting


def rows():
    r1, m1 = posting("pa", 1, 2, "Mon, 05 Oct 2026 10:00:00 +0000")
    r2, m2 = posting("pb", 3, 1, "Mon, 05 Oct 2026 11:00:00 +0000")
    m2[0].replace_header("Subject", "[PATCH v3 0/1] " + "a very long title " * 8)
    m2.append(mail("q@x", "Re: [PATCH v3 0/1] x", "R <r@x>", irt=r2, body="> +x\nWhy is this needed?\n",
                   date="Tue, 06 Oct 2026 10:00:00 +0000"))
    fams = series.families(build(m1 + m2))
    return [(f.head, f.state, f.evidence) for f in fams]


class Pretty(unittest.TestCase):
    def test_plain_is_aligned_and_fits(self):
        lines = [ln for ln in pretty.render(rows(), False, 100) if ln[0] in "●○◐"]
        self.assertEqual(len(lines), 2)
        self.assertNotIn("\033", "".join(lines))
        ends = {ln.index("2026-10-") for ln in lines}
        self.assertEqual(len(ends), 1)  # the date column lines up
        self.assertTrue(all(len(ln) <= 100 for ln in lines))
        self.assertIn("…", lines[0] + lines[1])

    def test_evidence_drops_addresses_unless_verbose(self):
        plain = "\n".join(pretty.render(rows(), False, 120))
        self.assertIn('R on 0/1: "Why is this needed?"', plain)
        self.assertNotIn("<r@x>", plain)
        self.assertIn("<r@x>", "\n".join(pretty.render(rows(), False, 120, verbose=True)))

    def test_color(self):
        out = "\n".join(pretty.render(rows(), True, 100))
        self.assertIn("\033[33m●", out)  # commented is yellow
        self.assertTrue(pretty.summary(["no-reply", "commented", "no-reply"], False)
                        .startswith("● 1 commented   ○ 2 no-reply"))

    def test_summary_wraps(self):
        states = ["merged", "in-next", "applied", "commented", "reviewed", "bot-only", "no-reply"]
        lines = pretty.summary(states, False, 40).splitlines()
        self.assertGreater(len(lines), 1)
        self.assertTrue(all(len(ln) <= 40 for ln in lines))
        self.assertEqual(len(pretty.summary(states, False).splitlines()), 1)

    def test_use_color(self):
        tty, pipe = io.StringIO(), io.StringIO()
        tty.isatty = lambda: True
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("NO_COLOR", None)
            self.assertTrue(pretty.use_color("auto", tty))
            self.assertFalse(pretty.use_color("auto", pipe))
            self.assertTrue(pretty.use_color("always", pipe))
            os.environ["NO_COLOR"] = "1"
            self.assertFalse(pretty.use_color("auto", tty))



class Docs(unittest.TestCase):
    def test_every_command_documents_itself(self):
        import argparse
        from patchtrack import cli
        parser = cli.build_parser()
        sub = next(a for a in parser._actions if isinstance(a, argparse._SubParsersAction))
        helps = {a.dest: a.help for a in sub._choices_actions}
        for name, p in sub.choices.items():
            self.assertTrue(helps.get(name), f"{name} has no one-line help")
            self.assertGreater(len((p.description or "").split()), 10, f"{name} has no docstring")

    def test_help_strips_markup(self):
        from patchtrack import cli
        text = cli._Help("x")._format_text("Run ``sync``, **now**; see :doc:`/scheduling`::\n")
        self.assertNotIn("``", text)
        self.assertNotIn("**", text)
        self.assertIn("https://ryanabx.github.io/patchtrack/scheduling.html", text)


if __name__ == "__main__":
    unittest.main()
