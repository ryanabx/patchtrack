"""Which open series need a nudge, and a drafted ping for each.

The defaults follow the kernel's Documentation/process/submitting-patches.rst:
"Wait for a minimum of one week before resubmitting or pinging reviewers",
and "it's also ok to resend the patch or the patch series after a couple of
weeks". Nothing here sends mail; drafts are files for you to send.
"""

import email.message
import email.utils
import os
import re
from dataclasses import dataclass
from datetime import datetime, timezone

from . import series, trees

PING_DAYS = 7
RESEND_DAYS = 14


# "Wait ... possibly longer during busy times like merge windows". A
# window lasts two weeks; a final tag older than this with no -rc1 after it
# means the repository's tags are stale, not that the window is open.
WINDOW_MAX_DAYS = 21
KERNEL_TAG_RE = re.compile(r"^v(\d+)\.(\d+)(-rc\d+)?$")


@dataclass
class Window:
    open: bool | None  # None: cannot tell
    tag: str = ""
    since: str = ""  # ISO date of the tag
    why: str = ""


def merge_window(repo, now=None):
    """Whether mainline's merge window is open, from the tags in repo."""
    now = now or datetime.now(timezone.utc)
    out = trees.git(repo, "for-each-ref", "--sort=-creatordate", "--format=%(refname:short) %(creatordate:iso-strict)",
                    "refs/tags/v*")
    for line in out.splitlines():
        tag, when = line.split()
        m = KERNEL_TAG_RE.match(tag)
        if not m:
            continue
        if m.group(3):  # an -rc: the window is closed
            return Window(False, tag, when[:10])
        age = (now - datetime.fromisoformat(when)).days
        if age > WINDOW_MAX_DAYS:
            return Window(None, tag, when[:10], f"{tag} is {age} days old with no -rc1 after it; "
                                                f"the tags in {repo} look stale")
        return Window(True, tag, when[:10])
    return Window(None, why=f"no vX.Y tags in {repo}")


@dataclass
class Nudge:
    fam: object
    kind: str  # "reply owed", "ping" or "resend"
    days: int  # since the last human word (or since the reply that is owed)
    why: str


def _days(since, now):
    return int((now - datetime.fromisoformat(since.replace("Z", "+00:00"))).total_seconds() // 86400)


def classify(fam, now=None, ping_days=PING_DAYS, resend_days=RESEND_DAYS, window=None):
    """The nudge a family needs, or None. During an open merge window,
    pings and RESENDs wait for -rc1."""
    n = _classify(fam, now, ping_days, resend_days)
    if n and window and window.open and n.kind in ("ping", "resend"):
        n.kind, n.why = f"{n.kind} after -rc1", f"{n.why}; merge window open"
    return n


def _classify(fam, now, ping_days, resend_days):
    now = now or datetime.now(timezone.utc)
    s = fam.head
    if fam.closed or fam.state in series.DONE or fam.state == "partially-applied":
        return None
    humans = [r for r in s.replies if not r.bot and not r.mine]
    mine = [r.date for r in s.replies if r.mine]
    # A reply is owed when the last person to write after you asked or said
    # something; a bare tag or an "Applied" asks nothing.
    after = [r for r in humans if r.applied is None and r.date > max(mine + [s.date])]
    asks = [r for r in after if not any(k in series.APPROVALS for k, _ in r.trailers)]
    if asks:
        last = asks[-1]
        return Nudge(fam, "reply owed", _days(last.date, now), f"{last.who.split(' <')[0]} wrote last")
    last_word = max([s.date] + mine + [r.date for r in humans])
    days = _days(last_word, now)
    if not humans and days >= resend_days:
        return Nudge(fam, "resend", days, f"no human reply in {days} days")
    if days >= ping_days:
        if not humans:
            why = f"no human reply in {days} days"
        elif fam.state == "reviewed":
            why = f"reviewed, then nothing for {days} days"
        else:
            why = f"quiet for {days} days"
        return Nudge(fam, "ping", days, why)
    return None


def draft(n, messages, author, lore_base):
    """A ping as an email.message.Message, threaded to the series and addressed to its recipients."""
    s = n.fam.head
    root = messages[s.root]
    m = email.message.EmailMessage()
    m["From"] = author
    m["To"] = ", ".join((root.get("to_addrs") or "").split())
    m["Cc"] = ", ".join(a for a in (root.get("cc_addrs") or "").split() if a != author)
    m["Subject"] = "Re: " + " ".join(root["subject"].split())
    m["In-Reply-To"] = f"<{s.root}>"
    m["References"] = f"<{s.root}>"
    m["Date"] = email.utils.formatdate(localtime=True)
    m["Message-ID"] = email.utils.make_msgid(domain=author.split("@")[-1])
    sent = s.date[:10]
    if n.kind == "resend":
        body = (f"[Draft only. The kernel's process documentation suggests a RESEND after a couple\n"
                f"of weeks of silence, unchanged, e.g.: b4 send --resend v{s.version}\n"
                f"Delete this note and use the ping below if you prefer to ping first.]\n\n")
    else:
        body = ""
    body += (f"Hi,\n\nGentle ping on this series, sent on {sent}"
             + (", which has had no reply so far" if n.why.startswith("no human") else "") + ".\n"
             f"Is there anything I should change?\n\n"
             f"{lore_base}/all/{s.root}/\n\nThanks,\n")
    # Plain text, as the lists expect: no quoted-printable line breaks.
    m.set_content(body, cte="7bit" if body.isascii() else "8bit")
    return m


def write_draft(n, messages, author, lore_base, directory):
    os.makedirs(directory, exist_ok=True)
    name = f"{n.fam.head.date[:10]}-{n.fam.key[:60].replace('/', '_')}.eml"
    path = os.path.join(directory, name)
    with open(path, "wb") as f:
        f.write(draft(n, messages, author, lore_base).as_bytes())
    return path
