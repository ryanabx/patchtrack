import unittest
from types import SimpleNamespace

from patchtrack import vdiff

PATCH = """{msg}

Signed-off-by: D <d@x>
---
 a.c | 2 +-
 1 file changed

diff --git a/a.c b/a.c
index {idx}..1111111 100644
--- a/a.c
+++ b/a.c
@@ -{line},3 +{line},3 @@ {ctx}
 keep
-{old}
+{new}
--
2.51.0
"""


def patch(msg="Fix a.", idx="abc1234", line=10, ctx="int main(void)", old="x", new="y"):
    return PATCH.format(msg=msg, idx=idx, line=line, ctx=ctx, old=old, new=new)


def series(*patches):
    titles = {i: t for i, (t, _) in enumerate(patches, 1)}
    bodies = {i: b for i, (_, b) in enumerate(patches, 1)}
    return SimpleNamespace(patches={i: f"{i}@x" for i in titles}, titles=titles, bodies=bodies, total=len(titles))


class Pairs(unittest.TestCase):
    def test_rebase_alone_is_unchanged(self):
        old = series(("a: fix", patch()))
        new = series(("a: fix", patch(idx="fff9999", line=42, ctx="static int other(void)")))
        (p,) = vdiff.pairs(old, new)
        self.assertEqual(p.marker, "=")

    def test_changed_message_and_diff(self):
        (p,) = vdiff.pairs(series(("a: fix", patch())), series(("a: fix", patch(msg="Fix a properly.", new="z"))))
        self.assertEqual(p.marker, "!")
        self.assertIn("+Fix a properly.", p.message)
        self.assertIn("++z", p.diff)

    def test_added_dropped_and_retitled(self):
        old = series(("a: allow longer tables", patch()), ("b: drop me", patch(old="q")))
        new = series(("a: allow 101 table points", patch()), ("c: brand new", patch(old="r")))
        got = {(p.old and p.old[1], p.new and p.new[1]): p.marker for p in vdiff.pairs(old, new)}
        self.assertEqual(got, {("a: allow longer tables", "a: allow 101 table points"): "=",
                               (None, "c: brand new"): ">", ("b: drop me", None): "<"})

    def test_changelog(self):
        cover = ("Intro.\n\nChanges in v5:\n- Rework the probe.\n- Link to v4: https://x\n\n"
                 "---\nD (2):\nbase-commit: abc\n")
        self.assertEqual(vdiff.changelog(cover), ["Changes in v5:", "- Rework the probe.", "- Link to v4: https://x"])
        self.assertEqual(vdiff.changelog("No changelog here."), [])


if __name__ == "__main__":
    unittest.main()
