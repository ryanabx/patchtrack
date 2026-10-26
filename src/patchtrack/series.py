"""Group an author's patches into series and families, and derive their status.

A *series* is one posting: a cover letter (or a lone first patch) and the
patches threaded under it. A *family* is every version of one series, the
nearest thing email has to a pull request. Status is derived only from mail
in the store, and every status carries the message that proves it.
"""

from dataclasses import dataclass, field
from datetime import datetime

from . import config, parse, vdiff


# Most decided first; a series shows the first that applies.
STATES = ("merged", "in-next", "queued", "applied", "partially-applied", "superseded", "commented",
          "reviewed", "discussed", "bot-only", "no-reply", "unfetched", "closed")
APPROVALS = ("Reviewed-By", "Acked-By", "Approved")  # Approved: a profile's "OK."

# How far one patch has got: a mail says so, then each kind of tree.
STAGES = {0: "not applied", 1: "applied", 2: "queued", 3: "in-next", 4: "merged"}
ROLE_STAGE = {"maintainer": 2, "next": 3, "mainline": 4}
DONE = {"applied", "queued", "in-next", "merged", "superseded"}


@dataclass
class Reply:
    msgid: str
    who: str
    date: str
    index: int  # the patch replied to, 0 for the cover
    bot: bool
    trailers: list
    applied: parse.Applied | None
    mine: bool = False  # sent by one of the tracked authors
    direct: bool = False  # a reply to the posting itself, not to another reply
    landed: bool = False  # a profile's "Pushed as r17-1234", which counts even from the author
    comments: list = field(default_factory=list)  # inline prose under quoted text


@dataclass
class Series:
    root: str
    author: str
    version: int
    total: int
    title: str
    date: str
    rfc: bool
    resend: bool
    family: str
    patches: dict = field(default_factory=dict)  # index -> msgid, 0 is the cover
    titles: dict = field(default_factory=dict)  # index -> patch title
    bodies: dict = field(default_factory=dict)  # index -> body as stored
    written_by: set = field(default_factory=set)  # patch authors other than the sender
    replies: list = field(default_factory=list)
    lists: set = field(default_factory=set)
    to: set = field(default_factory=set)  # To: and Cc: of the posting and of the author's replies
    profile: object = None  # the config.Profile whose recipients the posting matched
    as_of: str | None = None  # when the whole thread was last fetched
    pending: bool = False  # a sync wanted a fresher copy and has not got it yet
    local_as_of: str | None = None  # when local mail was last read for this thread
    superseded_by: str | None = None
    state: str = "no-reply"
    evidence: str = ""
    applied: set = field(default_factory=set)  # indices a mail says were applied
    stages: dict = field(default_factory=dict)  # index -> stage, see STAGES
    tree_rows: dict = field(default_factory=dict)  # index -> tree check results
    unanswered: bool = False

    @property
    def human_replies(self):
        return [r for r in self.replies if not r.bot and not r.mine]

    @property
    def tags(self):
        return [(r.index, kind, who, r.msgid) for r in self.replies for kind, who in r.trailers]

    @property
    def as_of_text(self):
        when = lambda t: t[:16].replace("T", " ")
        if self.as_of and (not self.local_as_of or self.as_of >= self.local_as_of):
            text = f"as of {when(self.as_of)} UTC"
        elif self.local_as_of:
            # Local mail holds what was sent to you; a reply that did not copy you is missed.
            text = f"as of {when(self.local_as_of)} UTC, from your mailbox only"
            if self.as_of:
                text += f" (archive {when(self.as_of)})"
        else:
            text = "thread never fetched"
        return text + ("; a fresher fetch is pending" if self.pending else "")

    @property
    def last_activity(self):
        return max([self.date] + [r.date for r in self.replies])

    def label(self):
        bits = []
        if self.rfc:
            bits.append("RFC")
        bits.append(f"PATCH v{self.version}" if self.version > 1 else "PATCH")
        if self.resend:
            bits.append("RESEND")
        n = len([i for i in self.patches if i]) or 1
        return f"[{' '.join(bits)}] {self.title} ({n} patch{'es' if n != 1 else ''})"


@dataclass
class Family:
    key: str
    author: str
    versions: list  # Series, oldest first
    state: str = ""
    evidence: str = ""
    parts: list = field(default_factory=list)  # (series, index, stage) for each patch still in play
    closed: str | None = None  # why you closed it, if you did

    @property
    def head(self):
        return self.versions[-1]


def _date(s):
    try:
        return datetime.fromisoformat(s.replace("Z", "+00:00"))
    except ValueError:
        return None


def _close_in_time(a, b, hours=24):
    da, db = _date(a), _date(b)
    return da is None or db is None or abs((da - db).total_seconds()) <= hours * 3600


def build(messages, authors, thread_times=None, root_lists=None, tree_results=None, bots=None,
          pending=None, local_times=None, profiles=config.BUILTIN_PROFILES):
    """Series by root message-id, from stored message dicts."""
    pending = pending or set()
    local_times = local_times or {}
    bots = bots or config.BotRules()
    authors = {a.lower() for a in authors}
    thread_times = thread_times or {}
    root_lists = root_lists or {}
    tree_results = tree_results or {}
    by_id = {m["msgid"]: m for m in messages}
    subj = {m["msgid"]: parse.parse_subject(m["subject"]) for m in messages}

    def is_author_patch(mid):
        m = by_id.get(mid)
        # Yours if you posted it or wrote it (someone may resend your patch)
        return (m is not None and (m["from_addr"] in authors or m.get("author_addr") in authors)
                and subj[mid].is_patch and not subj[mid].is_reply)

    def index_of(mid):
        s = subj[mid]
        return s.index if s.index is not None else 1

    def root_of(mid):
        cur = mid
        while True:
            s, parent = subj[cur], by_id[cur]["in_reply_to"]
            if index_of(cur) == 0 or not parent or not is_author_patch(parent):
                return cur
            ps = subj[parent]
            same = (ps.version, ps.total, ps.resend) == (s.version, s.total, s.resend)
            if not same or not _close_in_time(by_id[cur]["date"], by_id[parent]["date"]):
                return cur
            cur = parent

    series, owner = {}, {}
    for mid in sorted((m for m in by_id if is_author_patch(m)), key=lambda m: by_id[m]["date"]):
        root = root_of(mid)
        if root not in series:
            r, rs = by_id[root], subj[root]
            series[root] = Series(
                root=root, author=r["from_addr"] if r["from_addr"] in authors else r.get("author_addr"),
                version=rs.version, total=rs.total or 1,
                title=rs.title, date=r["date"], rfc=rs.rfc, resend=rs.resend,
                family=parse.change_id(root, r["body"]) or "title:" + parse.normalize_title(rs.title),
                lists=set(root_lists.get(root, ())), as_of=thread_times.get(root), pending=root in pending,
                local_as_of=local_times.get(root),
                to=set(f'{r.get("to_addrs") or ""} {r.get("cc_addrs") or ""}'.split()),
            )
        if series[root].profile is None and len(series[root].patches) == 0:
            series[root].profile = next((p for p in profiles if p.matches(series[root].to)), None)
        series[root].patches[index_of(mid)] = mid
        written_by = by_id[mid].get("author_addr")
        if written_by and written_by != by_id[mid]["from_addr"]:
            series[root].written_by.add(f'{by_id[mid].get("author_name") or ""} <{written_by}>'.strip())
        series[root].titles[index_of(mid)] = subj[mid].title
        series[root].bodies[index_of(mid)] = by_id[mid]["body"]
        owner[mid] = (root, index_of(mid))

    for m in sorted(messages, key=lambda m: m["date"]):
        if m["msgid"] in owner:
            continue
        chain = [m["in_reply_to"]] + list(reversed(m["refs"].split())) if m["in_reply_to"] else list(reversed(m["refs"].split()))
        target = next((owner[c] for c in chain if c in owner), None)
        if target is None:
            continue
        root, index = target
        s = series[root]
        landed = s.profile and parse.first_match(m["body"], s.profile.landed)
        if m["from_addr"] in authors:
            # People the author brought in later were asked too.
            s.to |= set(f'{m.get("to_addrs") or ""} {m.get("cc_addrs") or ""}'.split())
            s.replies.append(Reply(m["msgid"], m["from_addr"], m["date"], index, False, [],
                                   parse.Applied(landed, subset=parse.partial(landed)) if landed else None,
                                   mine=True, landed=bool(landed)))
            continue
        who = f'{m["from_name"]} <{m["from_addr"]}>'.strip()
        trailers = parse.trailers(m["body"])
        if s.profile and not trailers and parse.first_match(m["body"], s.profile.approvals):
            trailers = [("Approved", who)]
        s.replies.append(Reply(
            msgid=m["msgid"], who=who, date=m["date"],
            index=index, bot=bots.is_bot(m["from_addr"], m["body"]), trailers=trailers,
            applied=parse.applied(m["body"], m["subject"])
            or (parse.Applied(landed, subset=parse.partial(landed)) if landed else None),
            direct=m["in_reply_to"] == s.patches.get(index), comments=parse.inline_comments(m["body"]),
            landed=bool(landed),
        ))

    _supersede(series.values())
    for s in series.values():
        s.tree_rows = {i: tree_results.get(mid, []) for i, mid in s.patches.items() if i}
        _derive(s)
    return series


def _supersede(all_series):
    fams = {}
    for s in all_series:
        fams.setdefault((s.author, s.family), []).append(s)
    for members in fams.values():
        members.sort(key=lambda s: (s.version, s.date))
        for older, newer in zip(members, members[1:]):
            older.superseded_by = newer.root


def _state_from_stages(stages):
    """The state a set of patch stages adds up to, or None if none got anywhere."""
    done = min(stages)
    if done >= 2:
        return STAGES[done]
    if done == 1:
        return "applied"
    if any(stages):
        return "partially-applied"
    return None


def is_review_comment(r):
    """A direct reply from a person that comments inline and gives no tag."""
    return (r.direct and not r.mine and not r.bot and r.applied is None and bool(r.comments)
            and not any(k in APPROVALS for k, _ in r.trailers))


def _open_comments(s):
    """Per patch (and the cover), the latest direct review if it commented without a tag.

    Only direct replies to a posting judge it: replies to replies are
    conversation (routing, thanks, follow-ups) more often than review.
    """
    latest = {}
    for r in s.replies:
        if r.direct and not r.mine and not r.bot and r.applied is None:
            latest[r.index] = r
    return sorted((r for r in latest.values() if is_review_comment(r)), key=lambda r: r.date)


def _derive(s):
    wanted = {i for i in s.patches if i} or {1}
    unknown_part = None
    for r in s.replies:
        if r.applied is None or (r.mine and not r.landed):
            continue
        got = set(r.applied.indices) or (wanted if r.index == 0 and not r.applied.subset else {r.index})
        if not got & wanted:
            # A subset on the cover that does not say which: some went in.
            unknown_part = r
        s.applied |= got & wanted
        if not s.evidence:
            s.evidence = f'{r.who}, {r.date[:10]}: "{r.applied.line}" <{r.msgid}>'

    misses, hits, reverts = {}, set(), []
    for i in wanted:
        s.stages[i] = 1 if i in s.applied else 0
        for row in s.tree_rows.get(i, ()):
            if row.get("reverted_by"):
                reverts.append((i, row))
                continue
            if not row["commit_sha"]:
                misses[row["tree"]] = max(misses.get(row["tree"], ""), row["checked_at"])
                continue
            hits.add(row["tree"])
            s.stages[i] = max(s.stages[i], ROLE_STAGE[row["role"]])
    misses = {t: w for t, w in misses.items() if t not in hits}

    others = [r for r in s.replies if not r.mine]
    humans = s.human_replies
    done = min(s.stages.values())
    if done >= 2:
        s.state = STAGES[done]
        i = next(i for i in sorted(wanted) if s.stages[i] == done)
        row = max((r for r in s.tree_rows[i] if r["commit_sha"] and ROLE_STAGE[r["role"]] == done),
                  key=lambda r: r["checked_at"])
        s.evidence = (f'{i}/{s.total} is {row["commit_sha"][:12]} in {row["tree"]} '
                      f'(by {row["how"]}), as of {row["checked_at"][:16].replace("T", " ")} UTC')
        if not s.applied and s.as_of:
            s.evidence += "; no \"applied\" mail in the thread"
    elif done == 1:
        s.state = "applied"
        if misses:
            # Today's most useful fact: a maintainer said so, the trees do not show it yet.
            s.evidence += "; not in " + ", ".join(
                f'{t} as of {w[:16].replace("T", " ")}' for t, w in sorted(misses.items()))
    elif any(s.stages.values()):
        s.state = "partially-applied"
        s.evidence = ", ".join(f"{i}/{s.total} {STAGES[s.stages[i]]}" for i in sorted(wanted))
    elif unknown_part is not None:
        r = unknown_part
        s.state = "partially-applied"
        s.evidence = f'{r.who}, {r.date[:10]}: "{r.applied.line}" (which patches is not said) <{r.msgid}>'
    elif s.superseded_by:
        s.state, s.evidence = "superseded", f"by <{s.superseded_by}>"
    elif commented := _open_comments(s):
        r = commented[-1]
        s.state = "commented"
        s.evidence = f'{r.who} on {r.index}/{s.total}: "{r.comments[0]}" <{r.msgid}>'
    elif any(kind in APPROVALS for _, kind, _, _ in s.tags):
        s.state = "reviewed"
        i, kind, who, mid = next(t for t in s.tags if t[1] in APPROVALS)
        s.evidence = f"{kind}: {who} on {i}/{s.total} <{mid}>"
    elif humans:
        s.state = "discussed"
        s.evidence = f"{len(humans)} repl{'y' if len(humans) == 1 else 'ies'}, last {humans[-1].who} <{humans[-1].msgid}>"
    elif others:
        s.state = "bot-only"
        s.evidence = f"{len(others)} from bots"
    elif s.as_of is None and s.local_as_of is None:
        # "No reply" is only a fact once the thread has been read somewhere.
        s.state = "unfetched"
    else:
        s.state = "no-reply"

    for i, row in reverts:
        s.evidence += (f'{"; " if s.evidence else ""}{i}/{s.total} was {row["commit_sha"][:12]} in '
                       f'{row["tree"]}, reverted by {row["reverted_by"][:12]}')

    # A person wrote after the author last did. A fact, not a judgement: a
    # bare Reviewed-by counts too. An "Applied" notice asks for nothing, and a
    # superseded version waits on nobody.
    mine = [r.date for r in s.replies if r.mine] + [s.date]
    asks = [r for r in humans if r.applied is None]
    s.unanswered = (bool(asks) and asks[-1].date > max(mine) and s.state not in DONE
                    and not s.superseded_by)


def families(series, closed=None):
    """Families by latest activity; closed maps "author NUL family" to why it was closed."""
    closed = closed or {}
    out = {}
    for s in sorted(series.values(), key=lambda s: (s.version, s.date)):
        key = (s.author, s.family)
        out.setdefault(key, Family(key=s.family, author=s.author, versions=[])).versions.append(s)
    for f in out.values():
        _derive_family(f)
        why = closed.get(f"{f.author}\x00{f.key}")
        if why is not None:
            f.closed, f.state = why, "closed"
            f.evidence = f"closed by you{': ' + why if why else ''}"
    return sorted(out.values(), key=lambda f: f.head.last_activity, reverse=True)


def _derive_family(f):
    """Judge the family on the head's patches, each as far as any version of it got.

    A patch is the same patch in another version if vdiff.links pairs them:
    the same title, the same diff, or a similar title. A patch a later
    version dropped still counts if it was applied somewhere (dropped
    because it went in); one dropped and never applied was abandoned.
    """
    head = f.head
    # An older version whose commits a newer version also matched (same
    # diff, so same patch-id) is that version's history, not a second merge.
    later = set()
    for s in reversed(f.versions):
        commits = {r["commit_sha"] for rows in s.tree_rows.values() for r in rows
                   if r["commit_sha"] and not r.get("reverted_by")}
        if s is not head and commits and commits <= later:
            s.state, s.evidence = "superseded", f"by <{s.superseded_by}>; a later version has the same commits"
        later |= commits

    # Follow each patch forward version by version (vdiff.links: title, same
    # diff, similar title) to the head, or to the last version that had it.
    end = {(id(head), i): ("head", i) for i in head.stages}
    for older, newer in reversed(list(zip(f.versions, f.versions[1:]))):
        link = vdiff.links(older, newer)
        for i in older.stages:
            end[(id(older), i)] = end.get((id(newer), link.get(i)), ("gone", id(older), i))
    best = {}  # end -> (stage, series, index); later versions win ties
    for s in f.versions:
        for i, stage in s.stages.items():
            k = end[(id(s), i)]
            if k not in best or stage >= best[k][0]:
                best[k] = (stage, s, i)
    f.parts = [(best[("head", i)][1], best[("head", i)][2], best[("head", i)][0]) for i in sorted(head.stages)]
    f.parts += [(s, i, st) for k, (st, s, i) in best.items() if k[0] == "gone" and st >= 1]

    if all(s is head for s, _, _ in f.parts):
        f.state, f.evidence = head.state, head.evidence
        return
    f.state = _state_from_stages([st for _, _, st in f.parts]) or head.state
    f.evidence = ", ".join(f"v{s.version} {i}/{s.total} {STAGES[st]}" for s, i, st in f.parts)
