"""patchtrack: track an author's patches on public-inbox mailing lists."""

import argparse
import inspect
import json
import os
import re
import shutil
import sys
from datetime import datetime, timezone


import shlex
import subprocess
import urllib.parse
import webbrowser

from . import __version__, api, b4local, board, config, local, lore, news, nudge, parse, pretty, series, serve, store, thread, trees, vdiff


def cmd_sync(args, db):
    """Fetch the authors' mail from the archive, and the threads of their series.

    One search per author and list, ``(f:ADDR OR tc:ADDR) AND rt:SINCE..``:
    ``tc:`` finds other people's replies, which nearly always copy the
    author. Then the whole thread of every series the search touched is
    fetched, because a reply that does not copy the author is only in the
    thread.

    - The **first sync** of an author covers ``--since`` (``[sync] since``,
      by default three months). **Later syncs** search from the last one,
      with a day's overlap, and refetch only the threads new mail landed in,
      plus open series not fetched for ``--refresh-days``. With nothing new,
      that is a single request. ``--full`` refetches everything in the
      window.
    - Searches run in windows of ``--window-days``; a window with 1000 or
      more results is searched again in halves, so no export cap can
      truncate the result.
    - Timeouts and HTTP 429/5xx are retried with backoff, honouring
      ``Retry-After``. A failed search records nothing, so the next sync
      covers the same window. A thread whose fetch failed (or that
      ``--no-threads`` skipped) stays owed until a sync fetches it; until
      then its series says "a fresher fetch is pending". The exit status is
      non-zero when anything failed.
    - The mailboxes in ``[sources]`` are read first (see ``import``). New
      local mail makes the archive fetch target its thread. ``--offline``
      reads only them; ``--no-local`` skips them.

    ``rt:`` is always sent as epoch seconds: lore misreads some date forms
    without complaint (``rt:20261023..`` matches far too much).
    """
    conf = args.settings
    authors = args.author or list(conf.authors)
    if not authors:
        sys.exit(NO_AUTHORS)
    lists = [args.list] if args.list else list(conf.lists) or ["all"]
    base = args.lore or conf.lore or lore.DEFAULT_BASE
    window_days = args.window_days or conf.window_days or 30
    refresh_days = args.refresh_days if args.refresh_days is not None else conf.refresh_days or 7
    now = parse.now_utc()
    refresh = parse.since_epoch(f"{refresh_days}.days.ago")
    roots, status = {}, 0  # roots: root -> why it is refetched
    local_new = set()
    if conf.sources and not args.no_local:
        local_new = _read_local(args, db, list(conf.sources), authors, now)
    if args.offline:
        return 0
    for lst in lists:
        remote = lore.Lore(base=base, inbox=lst, delay=args.delay)
        status |= _search_list(args, db, remote, lst, authors, now, refresh, refresh_days, window_days, roots,
                               local_new)
    remote = lore.Lore(base=base, delay=args.delay)
    return _fetch_threads(args, db, remote, roots, now) or status


def _read_local(args, db, paths, authors, now):
    """Read local mail into the store; returns the message-ids it added."""
    missing = [p for p in paths if not os.path.exists(p)]
    for p in missing:
        print(f"{p}: no such file or directory", file=sys.stderr)
    paths = [p for p in paths if p not in missing]
    count, new = local.read(db, paths, authors, db.last_read)
    for p in paths:
        db.mark_read(p, now)
    # Every thread with mail from here is "seen locally" as of now.
    local_roots = set(db.local_times())
    for s in series.build(db.messages(), authors, profiles=args.settings.profiles).values():
        ids = {s.root} | set(s.patches.values()) | {r.msgid for r in s.replies}
        if ids & new or s.root in local_roots:
            db.mark_local(s.root, now)
    db.commit()
    print(f"local mail: {count} messages read, {len(new)} new", file=sys.stderr)
    return new


def cmd_import(args, db):
    """Read your own mail: Maildirs, directories of ``.eml`` files, and mbox files.

    Paths come from the command line, or from ``[sources]`` in the config
    (which ``sync`` also reads first). Only what matters is kept: patches
    the authors sent or wrote, and replies in their threads. Headers are
    read first, so a big mailbox stays cheap; Maildirs are read
    incrementally by file time, and an unchanged mbox is skipped.

    Local mail holds only what was sent to you: a reply that dropped you
    from Cc is not in it. A thread seen only locally says so ("as of …,
    from your mailbox only").

    patchtrack does not speak IMAP, so it never holds a mail password: sync
    the account to a Maildir with ``mbsync``, ``offlineimap`` or ``getmail``
    and name that. notmuch and ``lei`` keep Maildirs already.
    """
    paths = [os.path.abspath(os.path.expanduser(p)) for p in args.paths] or list(args.settings.sources)
    if not paths:
        sys.exit("nothing to read: pass paths, or set [sources] in the config")
    authors = args.author or list(args.settings.authors) or db.authors()
    if not authors:
        sys.exit(NO_AUTHORS)
    _read_local(args, db, paths, authors, parse.now_utc())
    return 0


def _search_list(args, db, remote, lst, authors, now, refresh, refresh_days, window_days, roots, local_new):
    status = 0
    until = parse.since_epoch(now)
    for author in authors:
        last = db.last_sync(author, lst)
        incremental = last is not None and args.since is None and not args.full
        if incremental:
            # A day of overlap: lore indexes some mail late.
            since = parse.since_epoch(last) - 86400
        else:
            since = parse.since_epoch(args.since or args.settings.since or "3.months.ago")
        # tc: finds other people's replies, which nearly always copy the author.
        query = f"f:{author} OR tc:{author}"
        try:
            msgs = lore.windowed(remote.search, query, since, until, days=window_days)
        except lore.LoreError as e:
            # Nothing is recorded, so the next sync searches this window again.
            print(f"{author}: search failed, nothing recorded: {e}", file=sys.stderr)
            status = 1
            continue
        ids = [parse.msgid(m["Message-ID"]) for m in msgs]
        new = set(ids) - db.known(ids)
        for m in msgs:
            db.add(m)
        db.record_sync(author.lower(), lst, query, now)
        db.commit()
        found = series.build(db.messages(), [author], db.thread_times(), bots=args.bots_rules, profiles=args.settings.profiles)
        hit = set(new if incremental else ids) | local_new  # new local mail: refetch its thread too
        for s in found.values():
            mine = {s.root} | set(s.patches.values()) | {r.msgid for r in s.replies}
            if mine & hit:
                roots.setdefault(s.root, "new mail" if incremental else "in the search")
                db.tag_root(s.root, lst)
            elif incremental and s.state not in series.DONE and not s.superseded_by and (
                    s.as_of is None or parse.since_epoch(s.as_of) <= refresh):
                roots.setdefault(s.root, f"not fetched for {refresh_days} days")
        kind = f"since the last sync ({last[:16]})" if incremental else "full window"
        print(f"{author}: {len(msgs)} messages ({len(new)} new) in {lst}, {kind}", file=sys.stderr)
    db.commit()
    return status


def _fetch_threads(args, db, remote, roots, now):
    for root in db.pending_threads():
        roots.setdefault(root, "an earlier fetch did not happen")
    for root in roots:
        db.want_thread(root, now)
    db.commit()
    if args.no_threads:
        if roots:
            print(f"{len(roots)} threads left for the next sync to fetch", file=sys.stderr)
        return 0
    failed = 0
    for i, (root, why) in enumerate(sorted(roots.items()), 1):
        print(f"[{i}/{len(roots)}] {root} ({why})", file=sys.stderr)
        try:
            msgs = remote.thread(root)
        except lore.LoreError as e:
            print(f"  {e}", file=sys.stderr)
            failed += 1
            continue
        for m in msgs:
            db.add(m)
        db.mark_thread(root, parse.now_utc())
        db.commit()
    if failed:
        print(f"{failed} thread fetches failed; the next sync retries them", file=sys.stderr)
    return 1 if failed else 0


def _authors(args, db):
    authors = args.author or list(args.settings.authors) or db.authors()
    if not authors:
        sys.exit(NO_AUTHORS)
    return authors


def _load(args, db):
    found = series.build(db.messages(), _authors(args, db), db.thread_times(), db.root_lists(),
                         db.tree_results(), bots=args.bots_rules, pending=db.pending_threads(),
                         local_times=db.local_times(), profiles=args.settings.profiles)
    if args.list:
        found = {k: s for k, s in found.items() if args.list in s.lists}
    return found


def cmd_people(args, db):
    """Everyone who replied to the authors' series, and how.

    One line per person: the last time they wrote, how many series they
    touched, replies, tags given (Reviewed-by, Acked-by, a profile's
    approval), inline reviews, and applies. Bots are left out unless you
    pass ``--bots``.
    """
    rows = api.people(_load(args, db))
    if not args.bots:
        rows = [p for p in rows if not p["bot"]]
    if args.json:
        json.dump(rows, sys.stdout, indent=2)
        print()
        return 0
    print(f"{'last':<10}  {'series':>6} {'replies':>7} {'tags':>4} {'comments':>8} {'applied':>7}  who")
    for p in rows:
        print(f"{p['last'][:10]}  {len(p['series']):>6} {p['replies']:>7} {p['tags']:>4} "
              f"{p['comments']:>8} {p['applied']:>7}  {p['who']}")
    return 0


def cmd_board(args, db):
    """Write a static HTML page: a card per family, in four columns.

    The columns are waiting, in review, applied and merged. Each card has
    its state, version count, last activity and evidence, and links to the
    thread on the archive. The page follows the system's light or dark
    theme and needs no server: write it from a timer (see
    :doc:`/scheduling`) and open the file.
    """
    fams = [f for f in series.families(_load(args, db), db.closed()) if f.closed is None]
    authors = ", ".join(sorted({f.author for f in fams})) or "nobody"
    page = board.render(fams, args.title or f"Patches by {authors}", parse.now_utc()[:16].replace("T", " "),
                        args.lore or args.settings.lore or lore.DEFAULT_BASE)
    if args.output == "-":
        sys.stdout.write(page)
    else:
        with open(args.output, "w") as f:
            f.write(page)
        print(f"wrote {args.output}", file=sys.stderr)
    return 0


def _data(args, db):
    found = _load(args, db)
    return api.Data(found, series.families(found, db.closed()), {m["msgid"]: m for m in db.messages()},
                    _authors(args, db), args.settings.lore or lore.DEFAULT_BASE, parse.now_utc())


def cmd_export(args, db):
    """Write the data for a web frontend: JSON files in a directory.

    ``index.json`` lists every family, ``families/ID.json`` holds one with
    its versions, conversations and version diffs, and ``people.json``
    everyone who replied. :doc:`/api` describes them. A frontend is a
    separate project that reads these files from any static web server,
    or the same documents from ``serve``.

    Each file is replaced whole, so a server never hands out half of one.
    A family's file that is no longer exported is removed; nothing else in
    the directory is touched. Run it from the same timer as ``sync`` (see
    :doc:`/scheduling`).
    """
    data = _data(args, db)
    os.makedirs(os.path.join(args.directory, "families"), exist_ok=True)
    wanted = api.paths(data)
    for path in wanted:
        target = os.path.join(args.directory, path)
        with open(target + ".tmp", "wb") as f:
            f.write(serve.encode(api.document(data, path)))
        os.replace(target + ".tmp", target)
    fams = os.path.join(args.directory, "families")
    for name in os.listdir(fams):
        if api.FAMILY_PATH_RE.fullmatch(f"families/{name}") and f"families/{name}" not in wanted:
            os.remove(os.path.join(fams, name))
    print(f"wrote {len(wanted)} documents to {args.directory}", file=sys.stderr)
    return 0


def cmd_serve(args, db):
    """Answer a web frontend with the same documents as ``export``, over HTTP.

    Every request rebuilds its document from the store, so a ``sync`` in
    another terminal shows on the next request. It is read-only: GET
    only, and nothing in the store changes. :doc:`/api` describes the
    documents; ``/`` is ``index.json``.

    It listens on 127.0.0.1 unless ``--host`` says otherwise, and answers
    only requests addressed to that host or to localhost. A browser lets
    no other site read the answers unless ``--cors`` names that site's
    origin, e.g. a frontend's development server.
    """
    _authors(args, db)  # fail now, not on the first request
    server = serve.make_server(lambda: _data(args, db), args.host, args.port, args.cors)
    host, port = server.server_address[:2]
    print(f"serving http://{host}:{port}/ (Ctrl-C stops)", file=sys.stderr)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


def _baseline(args, db, when):
    """Snapshots of every family from the mail dated up to `when`.

    Only mail carries its own dates. What the store learned later (tree
    checks, fetches) is used on both sides, or a store created after `when`
    would report everything it ever checked as news.
    """
    msgs = [m for m in db.messages() if m["date"] <= when]
    found = series.build(msgs, _authors(args, db), db.thread_times(), db.root_lists(), db.tree_results(),
                         bots=args.bots_rules, pending=db.pending_threads(), local_times=db.local_times(), profiles=args.settings.profiles)
    if args.list:
        found = {k: s for k, s in found.items() if args.list in s.lists}
    return {news.key(f): json.dumps(news.snapshot(f)) for f in series.families(found, db.closed())}


def cmd_news(args, db):
    """What changed since you last looked: GitHub's notifications for email.

    Every family is compared with a snapshot of what ``news`` showed last
    time. It reports:

    - new series, and new versions of existing ones;
    - state changes, e.g. "reviewed → applied" or "in-next → merged";
    - each new reply, with what it did: its tag, its "Applied …" line, or
      the first line of an inline comment.

    Then it takes a new snapshot, so the same change is shown once;
    ``--peek`` leaves the snapshot alone. The first run only records a
    baseline.

    ``--since WHEN`` compares with the mail as it stood at that time
    instead of with your last look, and implies ``--peek``. Only mail
    carries its own dates, so tree movements (queued, in-next, merged)
    appear through the ordinary ``news``.
    """
    fams = series.families(_load(args, db), db.closed())
    if args.since:
        when = datetime.fromtimestamp(parse.since_epoch(args.since), timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        seen, args.peek = _baseline(args, db, when), True
        print(f"Changes since {when[:16].replace('T', ' ')} UTC:")
    else:
        seen = db.seen()
    color = pretty.use_color(args.color)
    first = not seen and not args.since
    if first:
        print(f"First look: {len(fams)} series recorded. From now on, `news` shows what changed.")
    else:
        found = news.changes([f for f in fams if f.closed is None], seen)
        for c in found:
            for line in pretty.change(c, color):
                print(line)
        if not found:
            print("Nothing new.")
    if not args.peek:
        now = parse.now_utc()
        for f in fams:
            db.mark_seen(news.key(f), json.dumps(news.snapshot(f)), now)
        db.commit()
    return 0


def _pick(args, found):
    """The series a query names: the latest version of each matching family, or all with --all."""
    hits = [s for s in found.values() if args.series in s.root or args.series.lower() in s.title.lower()]
    if not hits:
        sys.exit(f"no series matches {args.series!r}")
    if args.all:
        return sorted(hits, key=lambda s: (s.family, s.version, s.date))
    latest = {}
    for s in hits:
        if s.family not in latest or (s.version, s.date) > (latest[s.family].version, latest[s.family].date):
            latest[s.family] = s
    return sorted(latest.values(), key=lambda s: s.date)


def _page(lines):
    text = "\n".join(lines) + "\n"
    if not sys.stdout.isatty():
        sys.stdout.write(text)
        return
    pager = shlex.split(os.environ.get("PAGER") or "less -RFX")
    try:
        subprocess.run(pager, input=text.encode(), check=False)
    except FileNotFoundError:
        sys.stdout.write(text)


def cmd_read(args, db):
    """Read a series' conversation in the terminal.

    Messages come in reply order, indented by depth, through ``$PAGER``.
    The cover letter is shown up to b4's footer. Each patch shows its
    message, with its diff summarised unless ``--diffs`` is given. In
    replies, long quotes collapse to the few lines each comment answers,
    and signatures are cut.

    The series is named by part of its title or root message-id. Without
    ``--all``, the latest version of each matching family is shown.
    """
    found = _load(args, db)
    messages = {m["msgid"]: m for m in db.messages()}
    color = pretty.use_color(args.color)
    lines = []
    for s in _pick(args, found):
        lines.append(pretty._paint(f"{s.label()}  ·  {s.state}  ·  {s.as_of_text}", pretty.BOLD, color))
        lines.append("")
        lines += thread.render(s, messages, color=color, diffs=args.diffs)
    _page(lines)
    return 0


def cmd_open(args, db):
    """Open a series' thread on the archive in a browser.

    ``--print`` prints the URL instead. Series are chosen as for ``read``.
    """
    base = args.settings.lore or lore.DEFAULT_BASE
    for s in _pick(args, _load(args, db)):
        url = f"{base}/all/{urllib.parse.quote(s.root, safe='@+-._~=')}/T/#u"
        if args.print or not webbrowser.open(url):
            print(url)
    return 0


def cmd_nudge(args, db):
    """Open series that need you: a reply, a ping, or a RESEND.

    - **reply owed**: someone asked or said something after your last
      message. A bare tag or an "Applied" asks nothing.
    - **ping**: a week or more since anyone last wrote.
    - **resend**: two weeks or more with no human reply at all.

    The defaults follow the kernel's ``submitting-patches.rst``: "wait for
    a minimum of one week before resubmitting or pinging reviewers", and it
    is "ok to resend … after a couple of weeks". Your own pings restart the
    clock. ``[nudge] ping_days`` and ``resend_days`` change the thresholds.

    With a ``mainline`` tree configured, its repository's tags show
    whether the merge window is open (a final ``vX.Y`` with no ``-rc1``
    after it). Pings and RESENDs then say "after -rc1": the kernel asks
    for patience "during busy times like merge windows". A final tag over
    three weeks old reads as stale tags, not an open window.
    ``--ignore-merge-window`` or ``[nudge] merge_window = false`` turns
    this off.

    ``--draft DIR`` writes each ping as a plain-text ``.eml``, threaded to
    the series and addressed to its recipients, for you to edit and send.
    A RESEND draft names the ``b4 send --resend vN`` command. Nothing is
    ever sent.
    """
    c = args.settings
    ping = args.ping_days or c.ping_days or nudge.PING_DAYS
    resend = args.resend_days or c.resend_days or nudge.RESEND_DAYS
    window = None
    mainline = [t for t in db.trees() if t.role == "mainline"]
    if c.merge_window and mainline and not args.ignore_merge_window:
        try:
            window = nudge.merge_window(mainline[0].repo)
        except trees.GitError as e:
            window = nudge.Window(None, why=str(e))
    color = pretty.use_color(args.color)
    if window and window.open:
        print(pretty._paint(f"The merge window has been open since {window.tag} ({window.since}): "
                            "pings and RESENDs wait for -rc1.", pretty.DIM, color))
    elif window and window.open is None:
        print(pretty._paint(f"Merge window unknown: {window.why}.", pretty.DIM, color))
    found = [n for f in series.families(_load(args, db), db.closed())
             if (n := nudge.classify(f, ping_days=ping, resend_days=resend, window=window))]
    order = {"reply owed": 0, "resend": 1, "ping": 2, "resend after -rc1": 3, "ping after -rc1": 4}
    found.sort(key=lambda n: (order[n.kind], -n.days))
    messages = {m["msgid"]: m for m in db.messages()} if args.draft else {}
    for n in found:
        s = n.fam.head
        print(f"{pretty.circle(n.fam.state, color)} {pretty._paint(n.kind.ljust(17), pretty.BOLD, color)}  "
              f"{s.title}  {pretty._paint(f'v{s.version} · {n.why}', pretty.DIM, color)}")
        if args.draft and n.kind != "reply owed":
            path = nudge.write_draft(n, messages, n.fam.author, c.lore or lore.DEFAULT_BASE, args.draft)
            print(f"{'':21}draft: {path}")
    if not found:
        print(f"Nothing to nudge (ping after {ping} days, RESEND after {resend}).")
    elif args.draft:
        print("\nDrafts only: nothing was sent. Edit and send them yourself.")
    return 0


def cmd_diff(args, db):
    """What changed between two versions of a series.

    By default, the latest version against the one before; ``--from`` and
    ``--to`` choose others. The output starts with the author's own
    "Changes in vN" from the cover letter. Patches are then paired by
    title, then by an identical diff, then by a similar title, and marked
    as ``git range-diff`` does: ``=`` unchanged, ``!`` changed, ``<``
    dropped, ``>`` added. A changed pair shows the diff of its commit
    message and the diff of its diffs.

    Hunk line numbers, git's function context and ``index`` lines are
    ignored, so a rebase alone is not a change. It works from the mail
    alone; for a true range-diff against a base tree, ``b4 diff`` does
    that.
    """
    found = _load(args, db)
    args.all = True
    hits = _pick(args, found)
    fams = {(s.author, s.family) for s in hits}
    if len(fams) > 1:
        for s in {(s.author, s.family): s for s in hits}.values():
            print(f"  {s.title}  (family {s.family})")
        sys.exit(f"{len(fams)} families match {args.series!r}: narrow it")
    versions = sorted((s for s in found.values() if (s.author, s.family) in fams), key=lambda s: (s.version, s.date))
    by_v = {}
    for s in versions:
        by_v[s.version] = s  # a RESEND of the same version replaces it
    if len(by_v) < 2:
        sys.exit(f"only one version of {versions[-1].title!r} is known")
    new = by_v.get(args.to) if args.to else versions[-1]
    old = by_v.get(args.from_) if args.from_ else max((s for v, s in by_v.items() if v < new.version),
                                                      key=lambda s: s.version, default=None)
    if old is None or new is None:
        sys.exit(f"versions known: {', '.join(f'v{v}' for v in sorted(by_v))}")
    lines = vdiff.render(old, new, pretty.use_color(args.color))
    _page(lines)
    return 0


def cmd_branches(args, db):
    """Your ``b4 prep`` branches, against what you posted.

    b4 keeps its state in an empty cover commit at the base of each prep
    branch: the change-id, the next revision to send, and the cover
    message-id of every version sent. So each branch can show its
    family's state, and one of:

    - "as sent in vN";
    - "vN+1 in preparation: K changed since vN" (compared by
      ``git patch-id``);
    - "draft: never sent".

    Repositories come from ``--repo`` or ``[b4] repos`` in the config.
    """
    repos = [os.path.abspath(os.path.expanduser(r)) for r in args.repo] or list(args.settings.b4_repos)
    if not repos:
        sys.exit("no repositories: pass --repo, or set [b4] repos in the config")
    found = _load(args, db)
    fams = {(f.author, f.key): f for f in series.families(found, db.closed())}
    color = pretty.use_color(args.color)
    for repo in repos:
        try:
            bs = b4local.branches(repo, args.settings.b4_prefix)
        except trees.GitError as e:
            print(f"{repo}: {e}", file=sys.stderr)
            continue
        for b in sorted(bs, key=lambda b: b.name):
            sent = [(v, found[mid]) for v, mid in b.sent if mid in found]
            if not b.sent:
                n = len(b.patch_ids)
                state, note = "draft", f"v{b.revision} never sent; {n} patch{'es' if n != 1 else ''}"
            elif not sent:
                state, note = "unknown", f"sent up to v{b.sent[-1][0]}, but not in the store (sync further back?)"
            else:
                v, s = sent[-1]
                fam = fams.get((s.author, s.family))
                state = fam.state if fam else s.state
                changed, added, dropped = b4local.compare(b, [s.bodies[i] for i in sorted(s.bodies) if i], repo)
                if changed or added or dropped:
                    what = ", ".join(f"{n} {w}" for n, w in ((changed, "changed"), (added, "added"),
                                                             (dropped, "dropped")) if n)
                    note = f"v{b.revision} in preparation: {what} since v{v}"
                else:
                    note = f"as sent in v{v}"
            print(f"{pretty.circle(state, color)} {state:<17} {b.name}")
            print(f"{'':20}{pretty._paint(note, pretty.DIM, color)}")
    return 0


def cmd_tui(args, db):
    """Browse your series interactively.

    In the list: ``j``/``k`` move, ``/`` filters by title or state, ``f``
    shows open series only, Enter reads a series, ``d`` shows its version
    diff, ``o`` opens it on the archive, ``?`` lists the keys, and ``q``
    goes back or quits.
    """
    from . import tui
    found = _load(args, db)
    fams = [f for f in series.families(found, db.closed()) if f.closed is None or args.closed]
    if not sys.stdout.isatty():
        sys.exit("tui needs a terminal")
    model = tui.Model(fams, found, {m["msgid"]: m for m in db.messages()},
                      args.settings.lore or lore.DEFAULT_BASE)
    tui.run(model, lambda url: webbrowser.open(url))
    return 0


def cmd_close(args, db):
    """Close a series family you abandoned, or one that went in some other way.

    For example, a series folded into someone else's. A closed family
    leaves ``list``, ``news``, ``nudge``, the board and the TUI; ``list
    --closed`` shows it with your reason, and ``reopen`` undoes it. A
    query that matches several families is refused unless you pass
    ``--all``.
    """
    fams = {}
    for s in _pick(args, _load(args, db)):
        fams[(s.author, s.family)] = s
    if len(fams) > 1 and not args.all:
        for s in fams.values():
            print(f"  {s.title}  (family {s.family})")
        sys.exit(f"{len(fams)} families match {args.series!r}: narrow it, or pass --all")
    now = parse.now_utc()
    for (author, family), s in fams.items():
        if args.cmd == "close":
            db.close_family(f"{author}\x00{family}", args.reason or "", now)
            print(f"closed: {s.title}")
        else:
            db.reopen_family(f"{author}\x00{family}")
            print(f"reopened: {s.title}")
    db.commit()
    return 0


def cmd_reopen(args, db):
    """Undo ``close``: the family comes back to ``list``, ``news``, ``nudge`` and
    the board."""
    return cmd_close(args, db)


def cmd_config(args, db):
    """Show the config file in use and what it sets.

    The defaults are printed where the file sets nothing, so the output is
    the configuration in effect. See :doc:`/configuration` for every
    key.
    """
    c = args.settings
    print(f"# {c.path}{'' if c.exists else ' (does not exist; defaults below)'}")
    print("[sync]")
    print(f"authors = {list(c.authors)}")
    print(f"lists = {list(c.lists) or ['all']}")
    print(f"since = {c.since or '3.months.ago'!r}")
    print(f"lore = {c.lore or lore.DEFAULT_BASE!r}")
    print(f"window_days = {c.window_days or 30}")
    print(f"refresh_days = {c.refresh_days if c.refresh_days is not None else 7}")
    if c.sources:
        print(f"\n[sources]\npaths = {list(c.sources)}")
    for t in c.trees:
        src = f"url = {t.url!r}\nbranch = {t.branch!r}" if t.url else f"ref = {t.ref!r}"
        print(f"\n[[trees]]\nname = {t.name!r}\nrepo = {t.repo!r}\n{src}\nrole = {t.role!r}")
    others = [t for t in db.trees() if t.name not in {c.name for c in c.trees}]
    if others:
        print("\n# trees added with `patchtrack tree add`: " + ", ".join(t.name for t in others))
    return 0


def cmd_tree(args, db):
    """Name the git trees that ``check`` looks in.

    A tree is a ref in a local repository, with a role: ``maintainer`` (a
    patch found there is *queued*), ``next`` (*in-next*) or ``mainline``
    (*merged*). Give either ``--ref`` (a ref already in the repository), or
    ``--url`` and ``--branch``: ``check --fetch`` then fetches the whole
    branch into ``refs/patchtrack/NAME``. It never makes a shallow fetch,
    because a shallow history would make "not found" meaningless.

    Trees can also be declared as ``[[trees]]`` in the config, which is
    then their source of truth (``tree rm`` refuses them).
    """
    if args.action == "list":
        for t in db.trees():
            src = f"{t.url} {t.branch}" if t.url else "local"
            print(f"{t.name:<16} {t.role:<10} {t.repo} {t.ref}  ({src})")
        return 0
    if args.action == "rm":
        if any(t.name == args.name for t in args.settings.trees):
            sys.exit(f"{args.name} is in {args.settings.path}; remove it there")
        db.remove_tree(args.name)
        db.commit()
        return 0
    if bool(args.url) != bool(args.branch) or bool(args.url) == bool(args.ref):
        sys.exit("give either --ref, or --url and --branch")
    repo = os.path.abspath(os.path.expanduser(args.repo))
    ref = args.ref or f"refs/patchtrack/{args.name}"
    db.add_tree(trees.Tree(args.name, repo, ref, args.url, args.branch, args.role))
    db.commit()
    return 0


def cmd_check(args, db):
    """Look for every posted patch in the trees.

    Only commits after the posting that touch the posted files are read,
    which takes seconds even on linux-next. Matchers, strongest first:

    - **message-id**: the posting's message-id in a ``Link:`` or
      ``Message-ID:`` trailer (b4 adds one). A message-id cited elsewhere
      in a commit message names some other patch and does not count;
    - **patch-id**: equal ``git patch-id --stable``;
    - **subject**: the same subject and author, with a changed diff.

    Not finding a patch is stored as a result too, with the tree's commit
    and the time. A found commit that a later commit in the same tree
    reverts does not count (unless that revert was itself reverted).
    """
    chosen = [t for t in db.trees() if not args.tree or t.name in args.tree]
    if not chosen:
        sys.exit("no trees: add one with `patchtrack tree add`")
    msgs = {m["msgid"]: m for m in db.messages()}
    found = series.build(list(msgs.values()), _authors(args, db))
    posted = [trees.Posted(mid, parse.parse_subject(msgs[mid]["subject"]).title,
                           msgs[mid].get("author_addr") or msgs[mid]["from_addr"], msgs[mid]["date"],
                           msgs[mid]["body"])
              for s in found.values() for i, mid in s.patches.items() if i]
    if not posted:
        return 0
    status = 0
    for t in chosen:
        try:
            if args.fetch and t.url:
                print(f"{t.name}: fetching {t.url} {t.branch}", file=sys.stderr)
                trees.fetch(t)
            if trees.is_shallow(t.repo):
                print(f"{t.name}: {t.repo} is shallow; 'not found' may be wrong", file=sys.stderr)
            ref_sha, result = trees.check(t, posted)
        except trees.GitError as e:
            print(f"{t.name}: {e}", file=sys.stderr)
            status = 1
            continue
        when = parse.now_utc()
        for mid, hit in result.items():
            db.record_tree_result(mid, t.name, ref_sha, *(hit or (None, None, None)), when)
        db.commit()
        n = sum(1 for h in result.values() if h)
        print(f"{t.name} @ {ref_sha[:12]}: {n} of {len(posted)} posted patches found", file=sys.stderr)
    return status



def cmd_list(args, db):
    """One line per series family: its state, title, version and last activity.

    A family is every version of one series, the nearest email has to a
    pull request; its state spans its versions (see :doc:`/concepts`). A
    coloured circle shows the state: purple merged, green
    applied/queued/in-next, yellow commented, cyan reviewed, blue
    discussed, grey waiting. The shape says the same without colour: ● a
    state, ◐ partially applied, ○ nothing known yet or superseded, ⊘
    closed. Colour is on for a terminal unless ``NO_COLOR`` is set.

    Below each line is the evidence for its state. ``-v`` adds addresses,
    message-ids and the time the thread was last fetched; ``--json`` gives
    everything as JSON. ``--all-versions`` shows each posting's own state.

    Filters combine. "What have I put in front of this maintainer that is
    still open?" is ``patchtrack list --open --to maintainer@example.org``.
    """
    found = _load(args, db)
    rows = []  # (series, state, evidence): a family's in the default view
    for fam in series.families(found, db.closed()):
        if fam.closed is not None and not (args.closed or (args.state and "closed" in args.state)):
            continue
        if args.all_versions:
            shown = [(s, s.state, s.evidence) for s in reversed(fam.versions)]
        else:
            shown = [(fam.head, fam.state, fam.evidence)]
        for s, state, evidence in shown:
            if args.state and state not in args.state:
                continue
            if args.unanswered and not s.unanswered:
                continue
            if args.open and state in series.DONE | {"partially-applied"}:
                continue
            if args.to and not any(args.to.lower() in a for a in s.to):
                continue
            if args.replied and not any(args.replied.lower() in r.who.lower() for r in s.replies if not r.mine):
                continue
            rows.append((s, state, evidence))
    if args.json:
        json.dump([dict(api.series(s, args.settings.lore or lore.DEFAULT_BASE), state=state, evidence=evidence,
                        evidence_msgids=api.evidence_msgids(evidence)) for s, state, evidence in rows],
                  sys.stdout, indent=2)
        print()
        return 0
    color = pretty.use_color(args.color)
    width = shutil.get_terminal_size((100, 24)).columns
    for line in pretty.render(rows, color, width, args.verbose):
        print(line)
    if rows:
        print("\n" + pretty.summary([state for _, state, _ in rows], color, width))
    return 0


def cmd_show(args, db):
    """Everything about one series: patches, tags, tree results and replies.

    The series is named by part of its title or root message-id; every
    matching version is shown, after a line for its family.
    """
    found = _load(args, db)
    hits = [s for s in found.values() if args.series in s.root or args.series in s.title]
    if not hits:
        sys.exit(f"no series matches {args.series!r}")
    for fam in series.families(found, db.closed()):
        if len(fam.versions) > 1 and any(s in hits for s in fam.versions):
            print(f"family {fam.key}: {fam.state}, {len(fam.versions)} versions")
            print(f"  {fam.evidence}\n" if fam.evidence else "")
    for s in sorted(hits, key=lambda s: (s.family, s.version)):
        print(f"{s.label()}\n  {s.state}{' (unanswered)' if s.unanswered else ''}, {s.as_of_text}")
        if s.evidence:
            print(f"  {s.evidence}")
        print(f"  root <{s.root}>, family {s.family}")
        if s.written_by:
            print(f"  written by {', '.join(sorted(s.written_by))}; posted by {s.author}")
        for i in sorted(s.patches):
            stage = f" [{series.STAGES[s.stages[i]]}]" if i else ""
            print(f"  {i}/{s.total} <{s.patches[i]}>{stage}")
            for kind, who in [(k, w) for idx, k, w, _ in s.tags if idx == i]:
                print(f"      {kind}: {who}")
            for row in s.tree_rows.get(i, ()):
                where = f'{row["commit_sha"][:12]} (by {row["how"]})' if row["commit_sha"] else "not found"
                if row.get("reverted_by"):
                    where += f', reverted by {row["reverted_by"][:12]}'
                print(f'      {row["tree"]} @ {row["ref_sha"][:12]}: {where}, {row["checked_at"][:16].replace("T", " ")} UTC')
        for r in s.replies:
            who = "(you)" if r.mine else r.who + (" [bot]" if r.bot else "")
            print(f"  {r.date[:16].replace('T', ' ')} on {r.index}/{s.total}: {who}")
        print()
    return 0


DOCS_URL = "https://ryanabx.github.io/patchtrack"
NO_AUTHORS = "no authors: pass -a EMAIL, or set [sync] authors in the config (see `patchtrack config`)"


class _Help(argparse.RawDescriptionHelpFormatter):
    """--help shows the command docstrings, which are reStructuredText for the
    website, with the markup taken out."""

    def _format_text(self, text):
        text = re.sub(r"``([^`]+)``", r"\1", text)
        text = re.sub(r"\*\*([^*]+)\*\*", r"\1", text)
        text = re.sub(r"`([^`<]+?) <([^>]+)>`_+", r"\1 (\2)", text)
        text = re.sub(r":doc:`/?([^`]+)`", lambda m: f"{DOCS_URL}/{m.group(1)}.html", text)
        text = re.sub(r"::$", ":", text, flags=re.M)
        return super()._format_text(text)


def _doc(func):
    return inspect.cleandoc(func.__doc__ or "")


def build_parser():
    """The whole command line: what main() parses, and what the docs render."""
    p = argparse.ArgumentParser(prog="patchtrack", description=__doc__, formatter_class=_Help)
    p.add_argument("--version", action="version", version=f"patchtrack {__version__}")
    # Generic defaults: these strings also end up on the website.
    p.add_argument("--db", help="the store (default: $XDG_DATA_HOME/patchtrack/patchtrack.sqlite3)")
    p.add_argument("--config", help="the config file (default: $XDG_CONFIG_HOME/patchtrack/config.toml, if it exists)")
    sub = p.add_subparsers(dest="cmd", required=True, metavar="COMMAND")

    def command(name, func, helptext):
        return sub.add_parser(name, help=helptext, description=_doc(func), formatter_class=_Help)

    sp = command("sync", cmd_sync, "fetch an author's mail and the threads of their series")
    sp.add_argument("-a", "--author", action="append",
                    help="email address (repeatable; default: [sync] authors in the config)")
    sp.add_argument("-l", "--list", help="inbox to search, e.g. linux-usb (default: [sync] lists, or all)")
    sp.add_argument("--since", help="N.days.ago (hours, weeks, months, years) or an ISO date; "
                    "default: since the last sync, or [sync] since (3.months.ago) for the first")
    sp.add_argument("--full", action="store_true",
                    help="refetch every thread with mail in the window, not only what changed")
    sp.add_argument("--refresh-days", type=int,
                    help="incremental: also refetch open series not fetched for this long (default 7)")
    sp.add_argument("--lore", help=f"public-inbox base URL (default: [sync] lore, or {lore.DEFAULT_BASE})")
    sp.add_argument("--delay", type=float, default=1.0, help="seconds between requests")
    sp.add_argument("--no-local", action="store_true", help="do not read the [sources] mailboxes")
    sp.add_argument("--offline", action="store_true", help="read the [sources] mailboxes only, not the archive")
    sp.add_argument("--no-threads", action="store_true",
                    help="search only; the threads it would fetch are left for the next sync")
    sp.add_argument("--window-days", type=int,
                    help="search this many days at a time (default 30); big windows are split further")
    sp.set_defaults(func=cmd_sync)

    sp = command("import", cmd_import, "read your own mail: Maildirs, .eml directories, mbox files")
    sp.add_argument("paths", nargs="*", help="default: [sources] in the config")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.set_defaults(func=cmd_import)

    sp = command("news", cmd_news, "what changed since you last looked")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.add_argument("--peek", action="store_true", help="show the changes without marking them seen")
    sp.add_argument("--since", help="compare with the state at this time (N.days.ago or ISO date) instead "
                    "of what you last saw; implies --peek")
    sp.add_argument("--color", choices=("auto", "always", "never"), default="auto")
    sp.set_defaults(func=cmd_news)

    for name, func, helptext in (("read", cmd_read, "read a series' conversation in the terminal"),
                                 ("open", cmd_open, "open a series' thread on the archive in a browser")):
        sp = command(name, func, helptext)
        sp.add_argument("series", help="part of a root message-id or title")
        sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
        sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
        sp.add_argument("--all", action="store_true", help="every matching version, not only the latest")
        sp.set_defaults(func=func)
        if name == "read":
            sp.add_argument("--diffs", action="store_true", help="show the patches' diffs")
            sp.add_argument("--color", choices=("auto", "always", "never"), default="auto")
        else:
            sp.add_argument("--print", action="store_true", help="print the URL instead of opening it")

    sp = command("tui", cmd_tui, "browse your series interactively")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.add_argument("--closed", action="store_true", help="include families you closed")
    sp.set_defaults(func=cmd_tui)

    sp = command("branches", cmd_branches, "your b4 prep branches: what was sent, and what changed since")
    sp.add_argument("--repo", action="append", default=[], help="default: [b4] repos in the config")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.add_argument("-l", "--list", help=argparse.SUPPRESS)
    sp.add_argument("--color", choices=("auto", "always", "never"), default="auto")
    sp.set_defaults(func=cmd_branches)

    sp = command("diff", cmd_diff, "what changed between two versions of a series")
    sp.add_argument("series", help="part of a root message-id or title")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.add_argument("--from", dest="from_", type=int, metavar="N", help="older version (default: the one before --to)")
    sp.add_argument("--to", type=int, metavar="M", help="newer version (default: the latest)")
    sp.add_argument("--color", choices=("auto", "always", "never"), default="auto")
    sp.set_defaults(func=cmd_diff)

    for name, func, helptext in (("close", cmd_close, "close a series family you abandoned or that went in elsewhere"),
                                 ("reopen", cmd_reopen, "undo close")):
        sp = command(name, func, helptext)
        sp.add_argument("series", help="part of a root message-id or title")
        sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
        sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
        sp.add_argument("--all", action="store_true", help="every matching family")
        if name == "close":
            sp.add_argument("--reason", help='e.g. "folded into another series"')
        sp.set_defaults(func=func)

    sp = command("nudge", cmd_nudge, "open series that need a reply, a ping or a RESEND")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.add_argument("--ping-days", type=int, help=f"default: [nudge] ping_days, or {nudge.PING_DAYS}")
    sp.add_argument("--resend-days", type=int, help=f"default: [nudge] resend_days, or {nudge.RESEND_DAYS}")
    sp.add_argument("--draft", metavar="DIR", help="write a ping for each as an .eml file in DIR (never sent)")
    sp.add_argument("--ignore-merge-window", action="store_true",
                    help="do not hold pings for an open merge window (read from a mainline tree's tags)")
    sp.add_argument("--color", choices=("auto", "always", "never"), default="auto")
    sp.set_defaults(func=cmd_nudge)

    sp = command("config", cmd_config, "show the config file in use and what it sets")
    sp.set_defaults(func=cmd_config)

    sp = command("tree", cmd_tree, "the git trees `check` looks in")
    sp.add_argument("action", choices=("add", "list", "rm"))
    sp.add_argument("name", nargs="?")
    sp.add_argument("--repo", help="local repository (fetches go into it)")
    sp.add_argument("--ref", help="a ref already in the repository")
    sp.add_argument("--url", help="fetch from here into refs/patchtrack/NAME (whole branch, never shallow)")
    sp.add_argument("--branch", help="the branch to fetch from --url")
    sp.add_argument("--role", choices=trees.ROLES, default="maintainer",
                    help="maintainer: queued; next: in-next; mainline: merged")
    sp.set_defaults(func=cmd_tree)

    sp = command("check", cmd_check, "look for every posted patch in the trees")
    sp.add_argument("-a", "--author", action="append", help="default: every author synced")
    sp.add_argument("-t", "--tree", action="append", help="default: every tree")
    sp.add_argument("--fetch", action="store_true", help="fetch trees that have a --url first")
    sp.set_defaults(func=cmd_check)

    for name, func, helptext in (("list", cmd_list, "one line per series family"),
                                 ("show", cmd_show, "patches, tags and replies of a series")):
        sp = command(name, func, helptext)
        sp.add_argument("-a", "--author", action="append", help="default: every author synced")
        sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
        sp.set_defaults(func=func)
        if name == "show":
            sp.add_argument("series", help="part of a root message-id or title")
        else:
            sp.add_argument("--state", action="append", choices=series.STATES)
            sp.add_argument("--all-versions", action="store_true", help="every version, not only the latest")
            sp.add_argument("--closed", action="store_true", help="include families you closed")
            sp.add_argument("--unanswered", action="store_true", help="only where someone wrote after the author last did")
            sp.add_argument("--open", action="store_true", help="only series not applied, merged or superseded")
            sp.add_argument("--to", metavar="ADDR", help="only series sent (To: or Cc:) to an address containing ADDR")
            sp.add_argument("--replied", metavar="WHO", help="only series where someone matching WHO replied")
            sp.add_argument("--json", action="store_true")
            sp.add_argument("-v", "--verbose", action="store_true", help="message-ids and fetch times too")
            sp.add_argument("--color", choices=("auto", "always", "never"), default="auto",
                            help="colour the circles (auto: on a terminal, unless NO_COLOR is set)")

    sp = command("people", cmd_people, "everyone who replied: reviews, tags, comments, applies")
    sp.add_argument("-a", "--author", action="append", help="default: every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.add_argument("--bots", action="store_true", help="include bots")
    sp.add_argument("--json", action="store_true")
    sp.set_defaults(func=cmd_people)

    sp = command("board", cmd_board, "write a static HTML board of every family")
    sp.add_argument("-a", "--author", action="append", help="default: every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.add_argument("-o", "--output", default="patchtrack-board.html", help="file, or - for stdout")
    sp.add_argument("--title")
    sp.add_argument("--lore", help="archive the cards link to (default: [sync] lore, or lore.kernel.org)")
    sp.set_defaults(func=cmd_board)

    sp = command("export", cmd_export, "write JSON for a web frontend: every family, its threads and diffs")
    sp.add_argument("directory", help="written into; created if missing")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.set_defaults(func=cmd_export)

    sp = command("serve", cmd_serve, "serve the same JSON over HTTP, read-only, for a web frontend")
    sp.add_argument("-a", "--author", action="append", help="default: [sync] authors, or every author synced")
    sp.add_argument("-l", "--list", help="only series found by a sync of this inbox")
    sp.add_argument("--host", default="127.0.0.1", help="address to listen on (default 127.0.0.1: this machine only)")
    sp.add_argument("--port", type=int, default=8080, help="default 8080; 0 picks a free one")
    sp.add_argument("--cors", metavar="ORIGIN", help="let pages from this origin read the answers, "
                    "e.g. http://localhost:5173")
    sp.set_defaults(func=cmd_serve)

    return p


def main(argv=None):
    p = build_parser()
    args = p.parse_args(argv)
    if args.cmd == "tree" and args.action != "list" and not args.name:
        p.error("tree add/rm need a NAME")
    if args.cmd == "tree" and args.action == "add" and not args.repo:
        p.error("tree add needs --repo")
    try:
        args.settings = config.settings(args.config)
    except config.ConfigError as e:
        p.error(str(e))
    args.bots_rules = args.settings.bots
    if getattr(args, "since", None):
        try:
            parse.since_epoch(args.since)
        except ValueError as e:
            p.error(str(e))
    db = store.Store(args.db)
    for t in args.settings.trees:
        db.add_tree(t)  # the config is the source of truth for its trees
    db.commit()
    try:
        return args.func(args, db)
    finally:
        db.close()


if __name__ == "__main__":
    sys.exit(main())
