"""A series' conversation, for reading in a terminal.

Messages come in reply order, indented by depth. Long quotes collapse to
the few lines a comment answers, signatures are cut, and a patch shows
its message with its diff summarised unless diffs are asked for.
"""

import re

from . import parse
from .pretty import BOLD, DIM, RESET

CONTEXT = 3  # quoted lines kept above each comment
INDENT = "  "
MAX_DEPTH = 6
DIFF_START_RE = re.compile(r"^(---\s*$|diff --git )")


def _paint(text, code, color):
    return f"{code}{text}{RESET}" if color and code else text


def _is_prose(line):
    text = line.strip()
    if not text or parse.CLOSING_RE.search(text):
        return False
    return len(text.split()) >= 2 or text[-1] in "?.!:"


def collapse(body, context=CONTEXT):
    """(line, quoted) pairs with long quotes cut to the lines a comment answers."""
    lines = []
    for line in body.splitlines():
        if line.rstrip() == "--":  # signature
            break
        lines.append(line)
    while lines and not lines[-1].strip():
        lines.pop()
    out, i = [], 0
    while i < len(lines):
        if not lines[i].lstrip().startswith(">"):
            out.append((lines[i], False))
            i += 1
            continue
        j = i
        while j < len(lines) and (lines[j].lstrip().startswith(">") or not lines[j].strip()):
            j += 1
        block = lines[i:j]
        while block and not block[-1].strip():
            block.pop()
        # Prose follows: keep what it answers. A sign-off ("Thanks,", a bare
        # name) is not prose; "Why?" is.
        answered = any(_is_prose(ln) for ln in lines[j:] if not ln.lstrip().startswith(">"))
        keep = block[-context:] if answered else []
        if len(block) > len(keep):
            out.append((f"[… {len(block) - len(keep)} quoted lines]", True))
        out += [(q, True) for q in keep]
        if answered and lines[i:j][-1:] == [""]:
            out.append(("", False))
        i = j
    return out


def cover_part(body):
    """A cover letter up to b4's footer (base-commit:, change-id:) or the signature."""
    out = []
    for line in body.splitlines():
        if line.startswith(("base-commit:", "change-id:", "prerequisite-")) or line.rstrip() == "--":
            break
        out.append(line)
    return "\n".join(out).rstrip()


def patch_part(body, diffs):
    """A patch's message, and its diff or a one-line summary of it."""
    lines = body.splitlines()
    cut = next((k for k, ln in enumerate(lines) if DIFF_START_RE.match(ln)), len(lines))
    head = "\n".join(lines[:cut]).rstrip()
    rest = lines[cut:]
    if diffs or not rest:
        return "\n".join(lines).rstrip(), None
    files = sum(1 for ln in rest if ln.startswith("diff --git "))
    changed = sum(1 for ln in rest if ln[:1] in "+-" and not ln.startswith(("+++", "---")))
    return head, f"[diff: {files} file{'s' if files != 1 else ''}, {changed} lines changed; --diffs to show]"


def render(s, messages, color=False, diffs=False, width=100):
    """Lines for the conversation of series s, given its stored message dicts by id."""
    ids = {s.root} | set(s.patches.values()) | {r.msgid for r in s.replies}
    mail = [messages[m] for m in ids if m in messages]
    depth = {}

    def level(m, seen=()):
        if m["msgid"] in depth:
            return depth[m["msgid"]]
        parent = messages.get(m["in_reply_to"] or "")
        d = 0 if parent is None or parent["msgid"] not in ids or parent["msgid"] in seen \
            else level(parent, seen + (m["msgid"],)) + 1
        depth[m["msgid"]] = d
        return d

    # Reply order: each message after its parent; siblings by date, then
    # patch number (a series is often sent within one second).
    index = {mid: i for i, mid in s.patches.items()}
    children = {}
    for m in sorted(mail, key=lambda m: (m["date"], index.get(m["msgid"], 1 << 30), m["msgid"])):
        parent = m["in_reply_to"] if m["in_reply_to"] in ids else None
        children.setdefault(parent, []).append(m)
    order, stack = [], list(reversed(children.get(None, [])))
    while stack:
        m = stack.pop()
        order.append(m)
        stack += reversed(children.get(m["msgid"], []))

    replies = {r.msgid: r for r in s.replies}
    out = []
    for m in order:
        pad = INDENT * min(level(m), MAX_DEPTH)
        r = replies.get(m["msgid"])
        who = f'{m["from_name"] or m["from_addr"]}'
        if m["msgid"] in index:
            what = f'{index[m["msgid"]]}/{s.total}' if s.total > 1 else "patch"
            head = f'{what}  {" ".join(m["subject"].split())}'
        else:
            what = "(you)" if r and r.mine else ("[bot]" if r and r.bot else "")
            head = f"{who} {what}".strip()
        out.append(pad + _paint(f"── {head} ", BOLD, color) + _paint(f'{m["date"][:16].replace("T", " ")}', DIM, color))
        if m["msgid"] in index:
            text, summary = patch_part(m["body"], diffs) if index[m["msgid"]] else (cover_part(m["body"]), None)
            body = [(ln, False) for ln in text.splitlines()]
            if summary:
                body.append((summary, True))
        else:
            body = collapse(m["body"])
        for line, quoted in body:
            out.append(pad + "   " + (_paint(line, DIM, color) if quoted else line))
        out.append("")
    return out
