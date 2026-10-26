import os
import re
import select
import sys
import tempfile
import time
import unittest

from patchtrack import series, store, tui
from test_patchtrack import ME, mail, posting


class Win:
    def __init__(self, h, w):
        self.h, self.w = h, w
        self.erase()

    def getmaxyx(self):
        return self.h, self.w

    def erase(self):
        self.grid = [[" "] * self.w for _ in range(self.h)]

    def addnstr(self, y, x, text, n, attr=0):
        for i, ch in enumerate(text[:n]):
            if x + i < self.w:
                self.grid[y][x + i] = ch

    def refresh(self):
        pass

    def text(self):
        return ["".join(r).rstrip() for r in self.grid]


def model():
    msgs = []
    r1, m1 = posting("ta", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
    r2, m2 = posting("ta", 2, 1, "Mon, 12 Oct 2026 10:00:00 +0000")
    r3, m3 = posting("tb", 10, 1, "Mon, 05 Oct 2026 11:00:00 +0000")  # v10: a wider column
    m3[0].replace_header("Subject", "[PATCH v10 0/1] other: a second series")
    msgs = m1 + m2 + m3 + [mail("q@x", "Re: [PATCH v10 0/1] other", "R <r@x>", irt=r3, body="> +x\nWhy?\n",
                                date="Tue, 13 Oct 2026 10:00:00 +0000")]
    db = store.Store(":memory:")
    for m in msgs:
        db.add(m)
    stored = db.messages()
    db.close()
    found = series.build(stored, [ME], {r: "2026-10-14T00:00:00Z" for r in (r1, r2, r3)})
    return tui.Model(series.families(found), found, {m["msgid"]: m for m in stored}, "https://lore.example")


class Model(unittest.TestCase):
    def test_navigation_filter_and_views(self):
        m = model()
        self.assertEqual(len(m.visible), 2)
        m.key("j")
        m.key("j")  # stops at the end
        self.assertEqual(m.sel, 1)
        for k in "/second":
            m.key(k)
        m.key("\n")
        self.assertEqual([f.head.title for f in m.visible], ["other: a second series"])
        m.key("\n")
        self.assertEqual(m.view, "read")
        self.assertTrue(any("Why?" in ln for ln in m.lines))
        m.key("q")
        self.assertEqual(m.view, "list")
        m.key("d")
        self.assertEqual(m.status, "only one version is known")
        self.assertEqual(m.key("o"), ("open", f"https://lore.example/all/{m.visible[0].head.root}/T/#u"))
        self.assertEqual(m.key("q"), "quit")

    def test_open_only_and_diff(self):
        m = model()
        m.key("f")
        self.assertTrue(all(f.state in tui.OPEN for f in m.visible))
        m.key("f")
        m.sel = [f.head.title for f in m.visible].index("foo: do the thing")
        m.key("d")
        self.assertEqual(m.view, "diff")
        self.assertIn("v1 → v2", m.lines[0])

    def test_draw_aligns_columns(self):
        m = model()
        w = Win(10, 80)
        tui.draw(w, m)
        rows = [r for r in w.text()[1:] if r and r[0] in "●○◐"]
        self.assertEqual(len(rows), 2)
        self.assertEqual(len({r.index("2026-10-") for r in rows}), 1)
        self.assertTrue(w.text()[-1].startswith("Enter read"))


@unittest.skipUnless(sys.platform.startswith("linux") or sys.platform == "darwin", "needs a pty")
class Terminal(unittest.TestCase):
    def test_runs_and_quits_in_a_real_terminal(self):
        import pty
        tmp = tempfile.mkdtemp()
        db = os.path.join(tmp, "t.sqlite3")
        s = store.Store(db)
        for m in posting("pt", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")[1]:
            s.add(m)
        s.commit()
        s.close()
        env = dict(os.environ, TERM="xterm-256color",
                   PYTHONPATH=os.pathsep.join([os.path.abspath("src"), os.environ.get("PYTHONPATH", "")]))
        pid, fd = pty.fork()
        if pid == 0:
            os.execvpe(sys.executable, [sys.executable, "-m", "patchtrack", "--config", os.devnull, "--db", db,
                                        "tui", "-a", ME], env)
        out = b""
        for key in (b"", b"\r", b"q", b"q"):
            if key:
                os.write(fd, key)
            end = time.time() + 1.0
            while time.time() < end:
                if select.select([fd], [], [], 0.05)[0]:
                    try:
                        out += os.read(fd, 65536)
                    except OSError:
                        break
        _, status = os.waitpid(pid, 0)
        self.assertEqual(os.waitstatus_to_exitcode(status), 0)
        text = re.sub(rb"\x1b\[[0-9;?]*[a-zA-Z]", b"", out).decode("utf-8", "replace")
        self.assertIn("1 series", text)
        self.assertIn("do the thing", text)


if __name__ == "__main__":
    unittest.main()
