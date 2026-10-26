"""Turn raw mail into the facts the rest of the tool reasons about."""

import email.header
import email.utils
import re
from dataclasses import dataclass, field
from datetime import datetime, timezone

REPLY_RE = re.compile(r"^\s*(?:(?:re|aw|sv|fwd?)\s*:\s*)+", re.I)
BRACKET_RE = re.compile(r"^\s*\[([^\]]*)\]\s*")
COUNT_RE = re.compile(r"^(\d+)/(\d+)$")
VERSION_RE = re.compile(r"^v(\d+)$", re.I)
TRAILER_RE = re.compile(
    r"^\s*(Reviewed-by|Acked-by|Tested-by|Reported-by|Suggested-by|Nacked-by):\s*(.+?)\s*$",
    re.I | re.M,
)
CHANGE_ID_RE = re.compile(r"^change-id:\s*(\S+)\s*$", re.M)
# b4 message-ids: 20261025-<slug>-v5-0-0123456789ab@example.org; the date and
# hash change with every version, the slug does not
B4_MSGID_RE = re.compile(r"^\d{8}-(.+?)-v\d+-\d+-[0-9a-f]{12}@")
# b4 change-id trailer: 20260915-<slug>-d35d2ac3d6f0
B4_CHANGE_ID_RE = re.compile(r"^\d{8}-(.+?)(?:-[0-9a-f]{12})?$")
# b4 ty: "[1/2] subject\n      commit: <sha>"
B4_TY_RE = re.compile(r"^\s*\[(\d+)/(\d+)\]\s.*\n\s+commit:\s*([0-9a-f]{7,40})", re.M)
APPLIED_LINE_RE = re.compile(
    r"^\s*(?:"
    r"(?:thanks,?\s+)?(?:applied|queued|picked up)\b"
    r"|(?:i'?ve|i have|i)\s+(?:now\s+)?(?:applied|queued|picked up)\b.*\b(?:to|into|thanks)\b"
    r"|this (?:patch|series) was applied to"
    # QEMU: "This was merged into master (<sha>)."
    r"|(?:this|the) (?:patch |series |change )?(?:was|has been|is now|got) (?:merged|applied|pushed|committed)"
    r" (?:into|to|in)\b"
    r")",
    re.I,
)


@dataclass
class Subject:
    is_reply: bool
    is_patch: bool
    version: int = 1
    index: int | None = None
    total: int | None = None
    rfc: bool = False
    resend: bool = False
    subset: bool = False
    title: str = ""


def parse_subject(subject):
    s = " ".join((subject or "").split())
    out = Subject(is_reply=bool(REPLY_RE.match(s)), is_patch=False)
    s = REPLY_RE.sub("", s)
    while True:
        m = BRACKET_RE.match(s)
        if not m:
            # "(subset)" is what b4 ty puts in front of partial applies
            if s.lower().startswith("(subset)"):
                out.subset = True
                s = s[len("(subset)"):].lstrip()
                continue
            break
        s = s[m.end():]
        for tok in m.group(1).replace(",", " ").split():
            up = tok.upper()
            if up == "PATCH":
                out.is_patch = True
            elif up == "RFC":
                out.rfc = True
            elif up == "RESEND":
                out.resend = True
            elif VERSION_RE.match(tok):
                out.version = int(VERSION_RE.match(tok).group(1))
            elif COUNT_RE.match(tok):
                out.index, out.total = map(int, COUNT_RE.match(tok).groups())
    out.title = s.strip()
    return out


def normalize_title(title):
    return re.sub(r"\s+", " ", title).strip().lower()


def decode(value):
    if value is None:
        return ""
    return str(email.header.make_header(email.header.decode_header(str(value))))


def addr(value):
    name, address = email.utils.parseaddr(decode(value))
    return name, address.lower()


INBODY_FROM_RE = re.compile(r"\AFrom:[ \t]*(.+?)[ \t]*\n(?:[ \t]*\n)?")


def inbody_author(subject, name, address, body):
    """The patch's author and its body, after git's in-body From: rule.

    A patch whose body starts with "From: X <x@y>" was written by X, whoever
    sent it: `git am` records X, and the line is not part of the message.
    Otherwise the sender is the author.
    """
    s = parse_subject(subject)
    m = INBODY_FROM_RE.match(body or "")
    if not (s.is_patch and not s.is_reply and m):
        return name, address, body
    n, a = addr(m.group(1))
    if "@" not in a:
        return name, address, body
    return n, a, body[m.end():]


def sender(msg):
    """(name, address) of who wrote msg: X-Original-From when a relay or list
    rewrote From ("X via B4 Relay <devnull+...>"), else From."""
    for header in ("X-Original-From", "From"):
        name, address = addr(msg[header])
        if "@" in address:
            return name, address
    return addr(msg["From"])


def addrs(values):
    """Lower-cased addresses from a list of To/Cc header values."""
    return [a.lower() for _, a in email.utils.getaddresses([decode(v) for v in values or []]) if a]


def date_utc(value):
    try:
        d = email.utils.parsedate_to_datetime(value)
    except (TypeError, ValueError):
        return ""
    if d.tzinfo is None:
        d = d.replace(tzinfo=timezone.utc)
    return d.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def msgid(value):
    m = re.search(r"<([^>]+)>", value or "")
    return m.group(1) if m else (value or "").strip()


def msgids(value):
    return re.findall(r"<([^>]+)>", value or "")


def text_body(msg):
    parts = []
    for part in msg.walk():
        if part.get_content_type() == "text/plain":
            payload = part.get_payload(decode=True) or b""
            parts.append(payload.decode(part.get_content_charset() or "utf-8", "replace"))
    return "\n".join(parts)


def unquoted(body):
    """The body without quoted lines, and without anything after a signature."""
    out = []
    for line in body.splitlines():
        if line.rstrip() == "--":
            break
        if not line.lstrip().startswith(">"):
            out.append(line)
    return "\n".join(out)


CLOSING_RE = re.compile(
    r"^(thanks|thank you|regards|cheers|best|br|kind regards|looks good|lgtm)\b|wrote:$", re.I)


def inline_comments(body):
    """Lines of prose written below quoted text: the reviewer's comments.

    Skips the "On ..., X wrote:" attribution, trailers, sign-offs, and lines
    under three words; anything after a signature is ignored.
    """
    quoted, out = False, []
    for line in body.splitlines():
        if line.rstrip() == "--":
            break
        if line.lstrip().startswith(">"):
            quoted = True
            continue
        text = line.strip()
        if quoted and len(text.split()) >= 3 and not TRAILER_RE.match(text) and not CLOSING_RE.search(text):
            out.append(text)
    return out


def trailers(body):
    """Trailers given in a reply: (kind, who), ignoring quoted text."""
    return [(k.title(), v) for k, v in TRAILER_RE.findall(unquoted(body))]


# A line that is nothing but an announcement: "Applied.", "Queued, thanks."
BARE_APPLIED_RE = re.compile(r"^\s*(?:thanks,?\s+)?(?:applied|queued)[.!]?(?:,?\s*thanks[.!]?)?\s*$", re.I)


def _paragraph_starts(body):
    """Unquoted lines that start a paragraph (after a blank or quoted line), or are a bare announcement.

    A wrapped sentence's continuation ("...as it was\napplied first.") is
    not an announcement, however it begins.
    """
    prev = ""
    for line in body.splitlines():
        if line.rstrip() == "--":
            return
        quoted = line.lstrip().startswith(">")
        starts = not prev.strip() or prev.lstrip().startswith(">")
        if not quoted and line.strip() and (starts or BARE_APPLIED_RE.match(line)):
            yield line
        prev = line


@dataclass
class Applied:
    line: str
    indices: list = field(default_factory=list)  # empty means "the message replied to"
    subset: bool = False


def applied(body, subject=""):
    """Evidence in a reply that a maintainer applied something, or None."""
    text = unquoted(body)
    line = next((ln.strip() for ln in _paragraph_starts(body) if APPLIED_LINE_RE.match(ln)), None)
    if line is None:
        return None
    indices = sorted({int(i) for i, _, _ in B4_TY_RE.findall(text)})
    return Applied(line=line, indices=indices, subset=parse_subject(subject).subset or partial(line))


# "Committed to trunk as r17-1234, others to follow": only part of a series.
PARTIAL_RE = re.compile(r"\b(?:others?|the rest|rest|remaining(?: ones| patches)?)\b[^.;]*?\b(?:follow|later|to come)\b",
                        re.I)


def partial(line):
    return bool(PARTIAL_RE.search(line or ""))


def first_match(body, patterns):
    """The first paragraph-starting line of body that a pattern matches, or None."""
    if not patterns:
        return None
    return next((ln.strip() for ln in _paragraph_starts(body)
                 if any(p.search(ln.strip()) for p in patterns)), None)


def change_id(msg_id, body):
    """b4's series slug, from the change-id trailer or else the message-id."""
    m = CHANGE_ID_RE.search(body or "")
    if m:
        cid = B4_CHANGE_ID_RE.match(m.group(1))
        return cid.group(1) if cid else m.group(1)
    m = B4_MSGID_RE.match(msg_id or "")
    return m.group(1) if m else None


SINCE_RE = re.compile(r"^(\d+)\.(minute|hour|day|week|month|year)s?\.ago$")
SINCE_SECONDS = {"minute": 60, "hour": 3600, "day": 86400, "week": 7 * 86400,
                 "month": 30 * 86400, "year": 365 * 86400}


def since_epoch(text, now=None):
    """Epoch seconds for "3.months.ago", "2026-10-23" or "2026-10-23T12:00:00Z".

    lore misreads some date forms without complaint ("20261023" matches far
    too much), so patchtrack always sends epoch seconds, as lei does.
    """
    now = now or datetime.now(timezone.utc)
    m = SINCE_RE.match(text.strip())
    if m:
        return int(now.timestamp()) - int(m.group(1)) * SINCE_SECONDS[m.group(2)]
    try:
        d = datetime.fromisoformat(text.strip().replace("Z", "+00:00"))
    except ValueError:
        raise ValueError(f"cannot read {text!r} as a time: use N.days.ago (or hours, weeks, "
                         "months, years) or an ISO date such as 2026-10-23") from None
    if d.tzinfo is None:
        d = d.replace(tzinfo=timezone.utc)
    return int(d.timestamp())


def now_utc():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
