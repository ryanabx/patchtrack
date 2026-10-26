"""What changed since you last looked: GitHub's notifications for email.

A snapshot of each family records what `news` last showed: its state, its
head version and the replies it had. The next `news` reports the
difference and takes a new snapshot. Snapshots are a record of what you
were shown, not of the mail, so they live in their own table.
"""

import json
from dataclasses import dataclass, field


def key(fam):
    return f"{fam.author}\x00{fam.key}"


def snapshot(fam):
    return {
        "state": fam.state,
        "head": fam.head.root,
        "version": fam.head.version,
        "replies": sorted(r.msgid for s in fam.versions for r in s.replies if not r.mine),
    }


@dataclass
class Change:
    fam: object
    new_series: bool = False
    new_version: int | None = None
    state: tuple | None = None  # (before, after)
    replies: list = field(default_factory=list)  # (series, Reply)

    def __bool__(self):
        return bool(self.new_series or self.new_version or self.state or self.replies)


def changes(families, seen):
    """A Change for every family that differs from its snapshot in seen."""
    out = []
    for fam in families:
        old = seen.get(key(fam))
        if old is None:
            out.append(Change(fam, new_series=True))
            continue
        old = json.loads(old)
        c = Change(fam)
        if fam.head.version > old["version"] and fam.head.root != old["head"]:
            c.new_version = fam.head.version
        if fam.state != old["state"]:
            c.state = (old["state"], fam.state)
        known = set(old["replies"])
        c.replies = sorted(((s, r) for s in fam.versions for r in s.replies
                            if not r.mine and r.msgid not in known), key=lambda sr: sr[1].date)
        if c:
            out.append(c)
    return out


def describe(series, reply):
    """One line saying what a reply did: applied, tagged, commented or replied."""
    who = reply.who.split(" <")[0] or reply.who
    on = f"on {reply.index}/{series.total}" if series.total > 1 else ""
    if reply.applied is not None:
        what = f'"{reply.applied.line}"'
    elif reply.trailers:
        what = ", ".join(sorted({k for k, _ in reply.trailers}))
    elif reply.direct and reply.comments:
        what = f'commented: "{reply.comments[0]}"'
    else:
        what = "replied"
    return " ".join(x for x in (reply.date[:10], who + (" (bot)" if reply.bot else ""), on, what) if x)
