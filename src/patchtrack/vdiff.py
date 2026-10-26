"""What changed between two versions of a series, from the posted patches.

`git range-diff` (and `b4 diff`) need both versions applied to a base tree;
this needs only the mail. Patches are paired by title, then by similarity,
and each pair compared twice: its commit message, and its diff with hunk
line numbers and index lines removed, so a rebase alone is not a change.
Markers follow range-diff: = unchanged, ! changed, < dropped, > added.
"""

import difflib
import re
from dataclasses import dataclass

from . import parse, pretty, thread

HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@")
SIMILAR = 0.6


def split(body):
    """(message, normalised diff lines) of a posted patch."""
    lines = body.splitlines()
    cut = next((k for k, ln in enumerate(lines) if thread.DIFF_START_RE.match(ln)), len(lines))
    message = [ln for ln in lines[:cut]]
    while message and not message[-1].strip():
        message.pop()
    start = next((k for k, ln in enumerate(lines) if ln.startswith("diff --git ")), len(lines))
    diff = []
    for ln in lines[start:]:
        if ln.rstrip() == "--":  # git format-patch signature ("-- \n2.x")
            break
        if ln.startswith("index "):
            continue
        # Line numbers and git's function context in hunk headers move with a rebase.
        diff.append("@@" if HUNK_RE.match(ln) else ln)
    return message, diff


@dataclass
class Pair:
    old: tuple | None  # (index, title, body)
    new: tuple | None
    marker: str  # = ! < >
    message: list  # unified diff lines of the commit message
    diff: list  # unified diff lines of the diffs


def links(old, new):
    """{old patch index: new patch index} for the patches two versions share.

    The same title first; then the same diff (a retitled patch); then a
    similar title (a reworked, retitled patch).
    """
    olds = {i: old.titles.get(i, "") for i in old.patches if i}
    news = {i: new.titles.get(i, "") for i in new.patches if i}
    out = {}
    for n, title in sorted(news.items()):
        o = next((o for o, t in olds.items() if o not in out and parse.normalize_title(t) == parse.normalize_title(title)), None)
        if o is not None:
            out[o] = n
    diffs = {i: split(old.bodies.get(i, ""))[1] for i in olds}
    for n in sorted(set(news) - set(out.values())):
        d = split(new.bodies.get(n, ""))[1]
        o = next((o for o in sorted(olds) if o not in out and d and diffs[o] == d), None)
        if o is not None:
            out[o] = n
    for n in sorted(set(news) - set(out.values())):
        left = [o for o in olds if o not in out]
        if not left:
            break
        best = max(left, key=lambda o: difflib.SequenceMatcher(None, olds[o], news[n]).ratio())
        if difflib.SequenceMatcher(None, olds[best], news[n]).ratio() >= SIMILAR:
            out[best] = n
    return out


def pairs(old, new):
    """Pair the patches of two series (Series objects with patch bodies in .bodies)."""
    olds = {i: (i, old.titles.get(i, ""), old.bodies[i]) for i in old.patches if i}
    news = {i: (i, new.titles.get(i, ""), new.bodies[i]) for i in new.patches if i}
    back = {n: o for o, n in links(old, new).items()}
    out = []
    for i, n in sorted(news.items()):
        if i not in back:
            out.append(Pair(None, n, ">", [], []))
            continue
        o = olds.pop(back[i])
        om, od = split(o[2])
        nm, nd = split(n[2])
        message = list(difflib.unified_diff(om, nm, "old message", "new message", n=1, lineterm=""))
        diff = list(difflib.unified_diff(od, nd, "old diff", "new diff", n=2, lineterm=""))
        out.append(Pair(o, n, "!" if message or diff else "=", message, diff))
    for o in sorted(olds.values()):
        out.append(Pair(o, None, "<", [], []))
    return out


CHANGES_RE = re.compile(r"^Changes in v\d+", re.I)


def changelog(cover):
    """The "Changes in vN:" section of a cover letter, as the author wrote it."""
    lines = (cover or "").splitlines()
    start = next((k for k, ln in enumerate(lines) if CHANGES_RE.match(ln)), None)
    if start is None:
        return []
    out = [lines[start]]
    for ln in lines[start + 1:]:
        if ln.startswith(("base-commit:", "change-id:", "---")) or (not ln.strip() and out[-1].startswith("- Link to v")):
            break
        out.append(ln)
    while out and not out[-1].strip():
        out.pop()
    return out


def versions(found, fam_key):
    """The versions of one family, oldest first; a RESEND replaces its version."""
    by_v = {}
    for s in sorted((s for s in found.values() if (s.author, s.family) == fam_key), key=lambda s: (s.version, s.date)):
        by_v[s.version] = s
    return by_v


def render(old, new, color=False):
    """Lines describing the change from series old to series new."""
    paint = lambda text, code: pretty._paint(text, code, color)
    lines = [paint(f"{new.title}: v{old.version} → v{new.version}", pretty.BOLD), ""]
    log = changelog(new.bodies.get(0))
    if log:
        lines += ["  " + paint("The author's changelog:", pretty.DIM)] + ["    " + ln for ln in log] + [""]
    for p in pairs(old, new):
        o = f"{p.old[0]}/{old.total}" if p.old else "-"
        n = f"{p.new[0]}/{new.total}" if p.new else "-"
        title = (p.new or p.old)[1]
        code = {"=": pretty.DIM, "!": "\033[33m", "<": "\033[31m", ">": "\033[32m"}[p.marker]
        lines.append(paint(f"{o:>5} {p.marker} {n:<5} {title}", code))
        for d in p.message + p.diff:
            c = "\033[32m" if d.startswith("+") and not d.startswith("+++") else \
                "\033[31m" if d.startswith("-") and not d.startswith("---") else \
                "\033[36m" if d.startswith("@@") else pretty.DIM
            lines.append("      " + paint(d, c))
    return lines
