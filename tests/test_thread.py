import unittest

from patchtrack import thread
from test_patchtrack import build, mail, posting


class Collapse(unittest.TestCase):
    def test_keeps_what_a_comment_answers(self):
        body = "On Mon, X wrote:\n" + "".join(f"> line {i}\n" for i in range(10)) + "\nWhy?\n\nThanks\n"
        out = [ln for ln, _ in thread.collapse(body)]
        self.assertEqual(out[:5], ["On Mon, X wrote:", "[… 7 quoted lines]", "> line 7", "> line 8", "> line 9"])
        self.assertIn("Why?", out)

    def test_trailing_quote_and_sign_off(self):
        body = "Looks good to me overall.\n\n" + "".join(f"> l{i}\n" for i in range(20)) + "\nSam\n-- \nsig\n"
        out = [ln for ln, _ in thread.collapse(body)]
        self.assertIn("[… 20 quoted lines]", out)
        self.assertNotIn("> l19", out)
        self.assertEqual(out[-1], "Sam")
        self.assertNotIn("sig", out)

    def test_patch_and_cover_parts(self):
        body = "Fix it.\n\nSigned-off-by: D\n---\n a.c | 2 +-\ndiff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\n-x\n+y\n"
        head, summary = thread.patch_part(body, diffs=False)
        self.assertEqual(head, "Fix it.\n\nSigned-off-by: D")
        self.assertEqual(summary, "[diff: 1 file, 2 lines changed; --diffs to show]")
        self.assertIn("+y", thread.patch_part(body, diffs=True)[0])
        cover = "Intro.\n\n---\nD (1):\n  a: fix\n\nbase-commit: abc\nchange-id: x\n\nBest regards,\n-- \nD\n"
        self.assertEqual(thread.cover_part(cover), "Intro.\n\n---\nD (1):\n  a: fix")


class Render(unittest.TestCase):
    def test_reply_order_and_depth(self):
        root, msgs = posting("rd", 1, 2, "Mon, 05 Oct 2026 10:00:00 +0000")
        p2 = msgs[2]["Message-ID"].strip("<>")
        msgs += [mail("a@x", "Re: [PATCH 2/2] foo: part 2", "Rev <r@x>", irt=p2, refs=[root, p2],
                      body="> +x\nWhy is this here?\n", date="Tue, 06 Oct 2026 10:00:00 +0000"),
                 mail("b@x", "Re: [PATCH 2/2] foo: part 2", "dev@example.org", irt="a@x", refs=[root, p2, "a@x"],
                      body="> Why is this here?\nBecause of the reset.\n", date="Tue, 06 Oct 2026 11:00:00 +0000"),
                 mail("c@x", "Re: [PATCH 0/2] foo: do the thing", "Late <l@x>", irt=root,
                      body="Applied, thanks!\n", date="Wed, 07 Oct 2026 10:00:00 +0000")]
        from patchtrack import store
        db = store.Store(":memory:")
        for m in msgs:
            db.add(m)
        stored = {m["msgid"]: m for m in db.messages()}
        db.close()
        s = build(msgs)[root]
        heads = [ln for ln in thread.render(s, stored) if "── " in ln]
        self.assertEqual([h.split("──")[0] for h in heads], ["", "  ", "  ", "    ", "      ", "  "])
        self.assertIn("1/2", heads[1])
        self.assertIn("Rev", heads[3])
        self.assertIn("(you)", heads[4])
        self.assertIn("Late", heads[5])


if __name__ == "__main__":
    unittest.main()
