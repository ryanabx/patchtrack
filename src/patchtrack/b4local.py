"""Your local `b4 prep` branches, linked to what you posted.

b4 keeps its state in an empty cover commit at the base of each prep
branch, after a "--- b4-submit-tracking ---" line: the change-id, the next
revision to send, and the cover message-id of every version sent. The
patches are the commits on top of it. So a branch can say which posted
series it is, and whether it has changed since the last version went out.
"""

import json
from dataclasses import dataclass, field

from . import trees

MARKER = "--- b4-submit-tracking ---"


@dataclass
class Branch:
    repo: str
    name: str
    tip: str
    cover: str  # the tracking commit
    change_id: str
    revision: int  # the next version b4 will send
    history: dict  # "v5" -> [cover message-ids]
    patch_ids: list = field(default_factory=list)  # of the commits on top of the cover

    @property
    def sent(self):
        """Sent versions, oldest first, as (version, cover message-id)."""
        out = []
        for v, mids in self.history.items():
            if v[1:].isdigit() and mids:
                out.append((int(v[1:]), mids[-1]))
        return sorted(out)


def _tracking(body):
    text = body.split(MARKER, 1)[1]
    text = "\n".join(ln for ln in text.splitlines() if not ln.startswith("#"))
    return json.loads(text).get("series", {})


def branches(repo, prefix="b4/"):
    """Every b4 prep branch in repo."""
    refs = trees.git(repo, "for-each-ref", "--format=%(refname:short) %(objectname)", f"refs/heads/{prefix}")
    out = []
    for line in refs.splitlines():
        name, tip = line.split()
        found = trees.git(repo, "log", "-n1", "--format=%H%x00%B", f"--grep=^{MARKER}$", tip).strip("\n")
        if not found:
            continue
        cover, body = found.split("\x00", 1)
        try:
            t = _tracking(body)
        except (ValueError, IndexError):
            continue
        b = Branch(repo, name, tip, cover, t.get("change-id", ""), int(t.get("revision", 1)),
                   t.get("history", {}) or {})
        log = trees.git(repo, "log", "-p", "--no-merges", "--format=commit %H", f"{cover}..{tip}", raw=True)
        pids = trees.git(repo, "patch-id", "--stable", data=log)
        b.patch_ids = [ln.split()[0] for ln in pids.splitlines()]
        out.append(b)
    return out


def compare(branch, posted_bodies, repo):
    """(changed, added, dropped) between the branch and the posted patch bodies."""
    posted = [p for p in trees.patch_ids(posted_bodies, repo) if p]
    mine, theirs = set(branch.patch_ids), set(posted)
    same = mine & theirs
    changed = min(len(mine - same), len(theirs - same))
    return changed, len(mine - same) - changed, len(theirs - same) - changed
