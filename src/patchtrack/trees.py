"""Find posted patches in git trees: has it really been applied, and where?

A tree is a ref in a local repository, optionally fetched from a URL into
refs/patchtrack/<name>. Fetches take the whole branch; a shallow history would make
"not found" meaningless. Only commits after the posting, touching the files
the patches touch, are considered, which keeps a check cheap on linux-next.

Matchers, strongest first:
  message-id  the posting's message-id in a Link: or Message-ID: trailer
  patch-id    `git patch-id --stable` of the commit equals the posting's
  subject     same subject and author address; the diff was changed

A found commit that a later commit in the same tree reverts ("This reverts
commit <sha>", as `git revert` writes it) is reported with the revert,
unless that revert was itself reverted.
"""

import re
import subprocess
from dataclasses import dataclass
from datetime import datetime, timedelta

from . import parse

ROLES = ("maintainer", "next", "mainline")
DIFF_RE = re.compile(r"^diff --git a/(\S+) b/(\S+)", re.M)


class GitError(RuntimeError):
    pass


@dataclass
class Tree:
    name: str
    repo: str
    ref: str
    url: str | None = None
    branch: str | None = None
    role: str = "maintainer"


@dataclass
class Posted:
    msgid: str
    title: str
    author: str
    date: str
    body: str


def git(repo, *args, data=None, raw=False):
    p = subprocess.run(["git", "-C", repo, *args], input=data, capture_output=True)
    if p.returncode:
        raise GitError(f"git {' '.join(args[:3])}: {p.stderr.decode(errors='replace').strip()}")
    return p.stdout if raw else p.stdout.decode("utf-8", "replace")


def fetch(tree):
    if not tree.url:
        return
    git(tree.repo, "fetch", "--quiet", "--no-tags", tree.url, f"+{tree.branch}:{tree.ref}")


def is_shallow(repo):
    return git(repo, "rev-parse", "--is-shallow-repository").strip() == "true"


def patch_ids(blobs, repo):
    """`git patch-id --stable` of each text in blobs, by position; None if no diff."""
    feed = "".join(f"From {i:040x} Mon Sep 17 00:00:00 2001\n{b}\n" for i, b in enumerate(blobs))
    out = {}
    for line in git(repo, "patch-id", "--stable", data=feed.encode()).splitlines():
        pid, tag = line.split()
        out[int(tag, 16)] = pid
    return [out.get(i) for i in range(len(blobs))]


def _msgid_re(msgid):
    # Only a trailer names the posting a commit came from; a reference in the
    # body ("[1] https://lore...") may cite an earlier, different patch.
    return re.compile(r"^(?:Link|Message-ID):.*(?<![^\s/<])" + re.escape(msgid) + r"(?![^\s/>])",
                      re.I | re.M)


def check(tree, posted):
    """(ref sha, {msgid: (commit, how, reverted_by) or None}) for each posted patch."""
    ref_sha = git(tree.repo, "rev-parse", "--verify", f"{tree.ref}^{{commit}}").strip()
    result = {p.msgid: None for p in posted}
    paths = sorted({f for p in posted for pair in DIFF_RE.findall(p.body) for f in pair if f != "/dev/null"})
    if not paths:
        return ref_sha, result
    first = min(datetime.fromisoformat(p.date.replace("Z", "+00:00")) for p in posted)
    since = (first - timedelta(days=2)).strftime("%Y-%m-%dT%H:%M:%SZ")
    scope = ["--no-merges", f"--since={since}", ref_sha, "--", *paths]

    commits = []
    for rec in git(tree.repo, "log", "--format=%H%x00%ae%x00%s%x00%B%x1e", *scope).split("\x1e"):
        fields = rec.strip("\n").split("\x00", 3)
        if len(fields) == 4:
            commits.append(fields)

    for p in posted:
        mre = _msgid_re(p.msgid)
        hit = next((c for c in commits if mre.search(c[3])), None)
        if hit:
            result[p.msgid] = (hit[0], "message-id", None)

    todo = [p for p in posted if result[p.msgid] is None]
    if todo:
        by_pid = {}
        # --full-diff: the path filter picks commits but must not trim their
        # diffs, or a commit that also touches other files would match falsely
        log = git(tree.repo, "log", "-p", "--full-diff", "--format=commit %H", *scope, raw=True)
        for line in git(tree.repo, "patch-id", "--stable", data=log).splitlines():
            pid, sha = line.split()
            by_pid.setdefault(pid, sha)
        for p, pid in zip(todo, patch_ids([p.body for p in todo], tree.repo)):
            if pid and pid in by_pid:
                result[p.msgid] = (by_pid[pid], "patch-id", None)

    for p in posted:
        if result[p.msgid] is None:
            want = (parse.normalize_title(p.title), p.author)
            hit = next((c for c in commits if (parse.normalize_title(c[2]), c[1].lower()) == want), None)
            if hit:
                result[p.msgid] = (hit[0], "subject", None)

    # A revert touches the same files after the commit, so it is in scope.
    for mid, hit in result.items():
        if hit:
            result[mid] = (hit[0], hit[1], undone_by(commits, hit[0]))
    return ref_sha, result


def undone_by(commits, sha):
    """The revert that last undid sha, or None if it stands: each revert of
    the previous revert flips it (a revert of a revert puts it back)."""
    cur, by, seen = sha, None, set()
    while True:
        r = next((c[0] for c in commits if f"This reverts commit {cur}" in c[3]), None)
        if r is None or r in seen:
            return by
        seen.add(r)
        by = None if by else r
        cur = r
