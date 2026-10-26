"""Terminal output for `patchtrack list`: a coloured circle per state.

The circle's shape carries the meaning without colour too: filled for a
state, half for partially applied, hollow for nothing known yet or
superseded.
"""

import os
import re
import sys

from . import news

RESET, DIM, BOLD = "\033[0m", "\033[2m", "\033[1m"

# state -> (circle, ANSI colour); roughly GitHub's: purple merged, green in, grey waiting
STYLE = {
    "merged": ("●", "\033[35m"),
    "in-next": ("●", "\033[32m"),
    "queued": ("●", "\033[32m"),
    "applied": ("●", "\033[32m"),
    "partially-applied": ("◐", "\033[32m"),
    "commented": ("●", "\033[33m"),
    "reviewed": ("●", "\033[36m"),
    "discussed": ("●", "\033[34m"),
    "bot-only": ("●", "\033[90m"),
    "no-reply": ("○", ""),
    "unfetched": ("○", "\033[90m"),
    "superseded": ("○", "\033[90m"),
    "closed": ("⊘", "\033[90m"),
    "draft": ("◌", ""),  # a b4 branch never sent
}
# In the default view addresses and message-ids are noise; -v keeps them.
ADDR_RE = re.compile(r" <[^<>\s]+@[^<>\s]+>")
FLAG = "unanswered"


def use_color(choice, stream=sys.stdout):
    if choice == "always":
        return True
    if choice == "never" or os.environ.get("NO_COLOR"):
        return False
    return stream.isatty()


def _paint(text, code, color):
    return f"{code}{text}{RESET}" if color and code else text


def _fit(text, width):
    return text if len(text) <= width else text[:max(width - 1, 0)] + "…"


def circle(state, color):
    shape, code = STYLE.get(state, ("●", ""))
    return _paint(shape, code, color)


def _meta(s):
    n = len([i for i in s.patches if i]) or 1
    return f"v{s.version} · {n} patch{'es' if n != 1 else ''} · {s.last_activity[:10]}"


def render(rows, color, width, verbose=False):
    """Lines for (series, state, evidence) rows, in aligned columns."""
    if not rows:
        return []
    state_w = max(len(state) for _, state, _ in rows)
    meta_w = max(len(_meta(s)) for s, _, _ in rows)
    flag_w = len(FLAG) + 2 if any(s.unanswered for s, _, _ in rows) else 0
    title_w = max(width - (2 + state_w + 2) - (2 + meta_w) - flag_w, 20)
    indent = " " * (2 + state_w + 2)
    out = []
    for s, state, evidence in rows:
        code = STYLE.get(state, ("", ""))[1]
        title = _fit(s.title + (" [RFC]" if s.rfc else ""), title_w).ljust(title_w)
        flag = ("  " + _paint(FLAG, "\033[33m" + BOLD, color)) if s.unanswered else ""
        out.append(f"{circle(state, color)} {_paint(state.ljust(state_w), code, color)}  "
                   f"{_paint(title, BOLD, color)}  {_paint(_meta(s).rjust(meta_w), DIM, color)}{flag}")
        detail = evidence if verbose else ADDR_RE.sub("", evidence)
        if s.pending and not verbose:
            detail = (detail + "; " if detail else "") + "a fresher fetch is pending"
        if detail:
            out.append(indent + _paint(_fit(detail, width - len(indent)), DIM, color))
        if verbose:
            out.append(indent + _paint(f"<{s.root}>  {s.as_of_text}", DIM, color))
    return out


def summary(states, color, width=None):
    """The count of each state, wrapped to width between items."""
    counts = {}
    for st in states:
        counts[st] = counts.get(st, 0) + 1
    order = [st for st in STYLE if st in counts] + sorted(set(counts) - set(STYLE))
    lines, line, used = [], [], 0
    for st in order:
        plain = f"{STYLE.get(st, ('●', ''))[0]} {counts[st]} {st}"
        if line and width and used + 3 + len(plain) > width:
            lines.append("   ".join(line))
            line, used = [], 0
        line.append(f"{circle(st, color)} {counts[st]} {st}")
        used += (3 if used else 0) + len(plain)
    lines.append("   ".join(line))
    return "\n".join(lines)


def change(c, color):
    """Lines for one news.Change: a headline, then what happened."""
    s = c.fam.head
    title = _paint(s.title, BOLD, color)
    lines = [f"{circle(c.fam.state, color)} {title}  {_paint(f'v{s.version}', DIM, color)}"]
    if c.new_series:
        lines.append(f"    new: {c.fam.state}")
    if c.new_version:
        lines.append(f"    v{c.new_version} posted")
    if c.state:
        before, after = c.state
        lines.append(f"    {circle(before, color)} {before} → {circle(after, color)} {after}")
    for ser, r in c.replies:
        lines.append("    " + news.describe(ser, r))
    return lines
