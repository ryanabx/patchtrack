"""The data a web frontend reads: JSON documents derived from the store.

patchtrack has no web frontend of its own, and none lives in this
repository. :doc:`commands/export` writes these documents as files, and
:doc:`commands/serve` answers with them over HTTP. A frontend reads one or
the other and needs nothing else from patchtrack: not its code, not its
store. Like every other view, the documents are derived from the mail on
each export or request, and every state carries its evidence.

Paths are relative to a base: the directory ``export`` wrote, or the
address ``serve`` listens on. The layout is the same for both, so a
frontend takes the base as its only setting.

Every document starts with the same four fields::

    {"schema": 1, "generator": "patchtrack 0.1.0",
     "generated": "2026-10-28T12:00:00Z", "archive": "https://lore.kernel.org", ...}

``index.json``
    ``authors``; ``states``, every state, most decided first (the order
    ``list`` uses); and ``families``, a FAMILY for each, most recent
    activity first, closed ones included.

``families/ID.json``
    ``family``: one FAMILY, with these added:

    - ``series``: each version, oldest first. Each is a SERIES with
      ``messages``, a MESSAGE for each mail in its conversation, in reply
      order;
    - ``parts``: each patch still in play, ``{"version", "root", "patch",
      "stage"}``, the patches the family's state is judged on;
    - ``diffs``: from each version to the next (a RESEND replaces its
      version), ``{"from", "to", "changelog", "pairs"}``.
      ``changelog`` is the author's "Changes in vN" lines from the cover.
      Each pair is ``{"marker", "old", "new", "message", "diff"}``:
      ``marker`` is ``=`` unchanged, ``!`` changed, ``<`` dropped or
      ``>`` added, as in ``git range-diff``. ``old`` and ``new`` are
      ``{"patch", "title"}`` or null. ``message`` and ``diff`` are
      unified diff lines of the commit message and of the diffs (see
      :doc:`commands/diff`).

``people.json``
    ``people``: everyone who replied, most recent first, bots included.
    Each is ``{"who", "bot", "series", "replies", "tags", "comments",
    "applied", "last"}`` (see :doc:`commands/people`).

FAMILY
    ``id`` (twelve hex digits, stable for one author's family, and the ID
    in its path), ``key`` (the b4 change-id, or ``title:`` and the
    normalised title), ``author``, ``title`` and ``label`` of the latest
    version, ``state``, ``evidence``, ``evidence_msgids``, ``open``
    (what ``list --open`` shows), ``closed`` (the reason you gave, ``""``
    for none, or null if open), ``versions`` (version numbers),
    ``head`` (the root message-id of the latest), ``patch_count``,
    ``unanswered``, ``last_activity``, ``lists``, ``as_of``,
    ``as_of_text`` and ``url`` (the latest version on the archive).

SERIES
    What ``list --json`` gives for each series, and also ``label``,
    ``rfc``, ``resend``, ``titles`` (by patch number, 0 the cover),
    ``url``, ``as_of_text``, ``pending``, ``local_as_of``,
    ``last_activity``, ``to`` (the addresses it was sent to),
    ``written_by`` (patch authors other than the sender) and
    ``evidence_msgids``.

MESSAGE
    ``msgid``, ``url``, ``in_reply_to``, ``depth`` (0 for a message with
    no parent in the conversation), ``from_name``, ``from_addr``,
    ``date``, ``subject``, ``kind`` (``cover``, ``patch`` or ``reply``),
    ``patch`` (its number, or the number of the patch replied to; 0 is
    the cover), ``mine`` (sent by a tracked author), ``bot`` and ``body``
    (as stored). A patch adds ``message`` and ``diff`` (the body split at
    its ``---`` line or first diff), a cover adds ``text`` (up to b4's
    footer). A reply adds ``direct`` (to the posting itself, not to
    another reply), ``tags`` (``{"kind", "who"}``), ``applied`` (the
    line that says so, or null) and ``comments`` (inline prose under
    quoted text, which is what makes a series ``commented``).

Evidence is text, as ``list`` prints it; ``evidence_msgids`` lists the
message-ids it cites, each written in it as ``<msgid>``, so a frontend can
turn them into links.

Dates are UTC, ISO 8601. ``schema`` goes up when a field is removed or
changes meaning; adding a field does not, so a frontend should ignore
fields it does not know.

The documents hold people's names, addresses and mail. ``serve`` listens
on localhost only, unless told otherwise; publish an export only where
that mail is already public.
"""

import hashlib
import re
from dataclasses import dataclass

from . import __version__, lore, news, series as series_, thread, vdiff

SCHEMA = 1

# The message-id that closes an evidence clause: "... <id>" or "... <id>; ..."
EVIDENCE_MSGID_RE = re.compile(r" <([^<>\s]+)>(?=;|$)")
FAMILY_PATH_RE = re.compile(r"families/([0-9a-f]{12})\.json")


@dataclass
class Data:
    """What the documents are made from: everything a command loads, once."""
    found: dict  # series by root
    families: list  # series_.families(found, closed)
    messages: dict  # stored message dicts by message-id
    authors: list
    archive: str  # public-inbox base URL
    generated: str  # UTC, ISO 8601


def family_id(fam):
    return hashlib.sha1(news.key(fam).encode()).hexdigest()[:12]


def evidence_msgids(text):
    return EVIDENCE_MSGID_RE.findall(text or "")


def family(fam, archive):
    s = fam.head
    return {
        "id": family_id(fam), "key": fam.key, "author": fam.author,
        "title": s.title, "label": s.label(), "state": fam.state, "evidence": fam.evidence,
        "evidence_msgids": evidence_msgids(fam.evidence),
        "open": fam.state not in series_.DONE | {"partially-applied", "closed"}, "closed": fam.closed,
        "versions": [v.version for v in fam.versions], "head": s.root,
        "patch_count": len([i for i in s.patches if i]) or 1, "unanswered": s.unanswered,
        "last_activity": s.last_activity, "lists": sorted(set().union(*(v.lists for v in fam.versions))),
        "as_of": s.as_of, "as_of_text": s.as_of_text, "url": lore.message_url(archive, s.root),
    }


def series(s, archive=lore.DEFAULT_BASE):
    """A series as ``list --json`` gives it, and more."""
    return {
        "root": s.root, "author": s.author, "title": s.title, "version": s.version,
        "total": s.total, "date": s.date, "family": s.family, "state": s.state,
        "evidence": s.evidence, "unanswered": s.unanswered,
        "applied": sorted(s.applied), "superseded_by": s.superseded_by, "as_of": s.as_of,
        "lists": sorted(s.lists),
        "tags": [{"patch": i, "kind": k, "who": w, "msgid": m} for i, k, w, m in s.tags],
        "patches": {str(i): m for i, m in sorted(s.patches.items())},
        "stages": {str(i): series_.STAGES[v] for i, v in sorted(s.stages.items())},
        "trees": {str(i): [{k: r.get(k) for k in ("tree", "role", "ref_sha", "commit_sha", "how", "reverted_by",
                                                  "checked_at")}
                           for r in rows] for i, rows in sorted(s.tree_rows.items())},
        "label": s.label(), "rfc": s.rfc, "resend": s.resend,
        "titles": {str(i): t for i, t in sorted(s.titles.items())},
        "url": lore.message_url(archive, s.root), "as_of_text": s.as_of_text, "pending": s.pending,
        "local_as_of": s.local_as_of, "last_activity": s.last_activity, "to": sorted(s.to),
        "written_by": sorted(s.written_by), "evidence_msgids": evidence_msgids(s.evidence),
    }


def messages(s, stored, archive):
    """The conversation of series s, in reply order, from stored message dicts by id."""
    index = {mid: i for i, mid in s.patches.items()}
    replies = {r.msgid: r for r in s.replies}
    out = []
    for m, depth in thread.order(s, stored):
        d = {k: m[k] for k in ("msgid", "in_reply_to", "from_name", "from_addr", "date", "subject")}
        d.update(url=lore.message_url(archive, m["msgid"]), depth=depth, body=m["body"])
        if m["msgid"] in index:
            d.update(kind="patch" if index[m["msgid"]] else "cover", patch=index[m["msgid"]], mine=True, bot=False)
            if index[m["msgid"]]:
                message, diff = thread.split_patch(m["body"])
                d.update(message="\n".join(message).rstrip(), diff="\n".join(diff))
            else:
                d.update(text=thread.cover_part(m["body"]))
        else:
            r = replies[m["msgid"]]
            d.update(kind="reply", patch=r.index, mine=r.mine, bot=r.bot, direct=r.direct,
                     tags=[{"kind": k, "who": w} for k, w in r.trailers],
                     applied=r.applied.line if r.applied else None, comments=r.comments)
        out.append(d)
    return out


def _side(p):
    return {"patch": p[0], "title": p[1]} if p else None


def diffs(fam):
    by_v = vdiff.versions({s.root: s for s in fam.versions}, (fam.author, fam.key))
    vs = [by_v[v] for v in sorted(by_v)]
    return [{"from": old.version, "to": new.version, "changelog": vdiff.changelog(new.bodies.get(0)),
             "pairs": [{"marker": p.marker, "old": _side(p.old), "new": _side(p.new),
                        "message": p.message, "diff": p.diff} for p in vdiff.pairs(old, new)]}
            for old, new in zip(vs, vs[1:])]


def family_detail(fam, data):
    return dict(family(fam, data.archive),
                series=[dict(series(s, data.archive), messages=messages(s, data.messages, data.archive))
                        for s in fam.versions],
                parts=[{"version": s.version, "root": s.root, "patch": i, "stage": series_.STAGES[st]}
                       for s, i, st in fam.parts],
                diffs=diffs(fam))


def people(found):
    """Everyone who replied to the series in found, most recent first."""
    out = {}
    for s in found.values():
        for r in s.replies:
            if r.mine:
                continue
            p = out.setdefault(r.who.lower(), {"who": r.who, "bot": r.bot, "series": set(), "replies": 0,
                                               "tags": 0, "comments": 0, "applied": 0, "last": ""})
            p["series"].add(s.family)
            p["replies"] += 1
            p["tags"] += any(k in series_.APPROVALS for k, _ in r.trailers)
            p["comments"] += series_.is_review_comment(r)
            p["applied"] += r.applied is not None
            p["last"] = max(p["last"], r.date)
    return [dict(p, series=sorted(p["series"])) for p in sorted(out.values(), key=lambda p: p["last"], reverse=True)]


def _header(data):
    return {"schema": SCHEMA, "generator": f"patchtrack {__version__}", "generated": data.generated,
            "archive": data.archive}


def document(data, path):
    """The document at path, or None if there is none."""
    if path == "index.json":
        return dict(_header(data), authors=sorted(data.authors), states=list(series_.STATES),
                    families=[family(f, data.archive) for f in data.families])
    if path == "people.json":
        return dict(_header(data), people=people(data.found))
    m = FAMILY_PATH_RE.fullmatch(path)
    fam = m and next((f for f in data.families if family_id(f) == m.group(1)), None)
    return dict(_header(data), family=family_detail(fam, data)) if fam else None


def paths(data):
    """Every document's path."""
    return ["index.json", "people.json"] + [f"families/{family_id(f)}.json" for f in data.families]
