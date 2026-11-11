"""Which maintainer trees a posted patch goes to, from the project's own files.

Two files say it, both read from git, never guessed:

- **MAINTAINERS** (the kernel's, and U-Boot's, which has the same format).
  A section's ``F:``, ``N:`` and ``X:`` lines say which files it covers,
  as ``scripts/get_maintainer.pl`` reads them, and its ``T:`` lines name
  its trees: ``T: git URL [BRANCH]``. Many omit the branch.
- **linux-next's Next/Trees**: every tree merged into linux-next, as
  ``NAME  URL#BRANCH``. It gives the branch a ``T:`` line leaves out, for the
  trees that feed linux-next.

A tree in neither with a branch stays without one: the branch it is
applied on is the maintainer's choice (Felix Fietkau's mt76 tree has
``mt76``, ``mt76-pending`` and ``pending``; "pending" means *not yet
applied*). ``tree discover`` lists the branches to choose from, and asks
which one when it runs in a terminal with ``--add``.
"""

import re
from dataclasses import dataclass, field

# linux-next's list of the trees it merges, read when no other copy is given
NEXT_TREES_URL = "https://git.kernel.org/pub/scm/linux/kernel/git/next/linux-next.git/plain/Next/Trees"

# Mainline is a role of its own (`merged`), not a maintainer's queue.
MAINLINE_URLS = {"git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux",
                 "github.com/torvalds/linux"}


@dataclass
class Section:
    name: str
    files: list = field(default_factory=list)      # F:
    excludes: list = field(default_factory=list)   # X:
    regexes: list = field(default_factory=list)    # N:
    trees: list = field(default_factory=list)      # (url, branch or None) from T: git


@dataclass
class Route:
    section: str
    url: str
    branch: str | None
    source: str         # "T:" or "Next/Trees"
    next_name: str | None = None

    @property
    def key(self):
        return (normalize_url(self.url), self.branch)


ADDR_RE = re.compile(r"[\w.+-]+@[\w-]+(?:\.[\w-]+)+")


def addresses(text):
    """Every list on an L: line. A series sent to none of them went to
    another project: people can be in two projects' files (a U-Boot
    custodian who maintains something in the kernel), lists are not."""
    return {a.lower() for line in text.splitlines() if re.match(r"^L:", line)
            for a in ADDR_RE.findall(line)}


def parse(text):
    """Sections of a MAINTAINERS file, in order; the preamble is skipped."""
    sections, cur = [], None
    for line in text.splitlines():
        m = re.match(r"^([A-Z]):\s*(.*?)\s*$", line)
        if not line.strip():
            cur = None
            continue
        if m is None:
            # a title line starts a section (the preamble has no tag lines)
            cur = Section(line.strip())
            sections.append(cur)
            continue
        if cur is None:
            continue
        tag, value = m.groups()
        if tag == "F":
            cur.files.append(value)
        elif tag == "X":
            cur.excludes.append(value)
        elif tag == "N":
            try:
                cur.regexes.append(re.compile(value, re.X))
            except re.error:
                pass
        elif tag == "T":
            parts = value.split()
            if len(parts) >= 2 and parts[0] == "git":
                rest = [p for p in parts[2:] if p != "-b"]
                cur.trees.append((parts[1], rest[0] if rest else None))
    return [s for s in sections if s.files or s.regexes]


def _pattern(pat):
    """get_maintainer.pl's F:/X: matching: a prefix match, with * and ? as
    wildcards that may cross '/'; a pattern ending in '/' covers the whole
    directory below it."""
    rx = re.escape(pat).replace(r"\*", ".*").replace(r"\?", ".")
    return re.compile("^" + rx)


def _file_match(pat, path):
    if not _pattern(pat).match(path):
        return False
    # Without a trailing '/' or a wildcard, a pattern names a file or a
    # directory: "drivers/foo" must not cover "drivers/foobar.c".
    if pat.endswith("/") or "*" in pat or "?" in pat:
        return True
    return path == pat or path.startswith(pat + "/")


def covers(section, path):
    if any(_file_match(x, path) for x in section.excludes):
        return False
    return any(_file_match(f, path) for f in section.files) or any(r.search(path) for r in section.regexes)


def normalize_url(url):
    """The same tree under its different spellings: scheme, .git, trailing /."""
    u = re.sub(r"^[a-z+]+://", "", url.strip().lower())
    u = re.sub(r"^[^@/]+@", "", u)
    return re.sub(r"(\.git)?/*$", "", u)


def parse_next_trees(text):
    """{normalized url: [(name, url, branch)]} from linux-next's Next/Trees."""
    out = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) < 2 or "#" not in parts[-1] or "://" not in parts[-1]:
            continue
        url, branch = parts[-1].split("#", 1)
        out.setdefault(normalize_url(url), []).append((parts[0], url, branch))
    return out


def routes(sections, paths, next_trees=None):
    """Every tree the sections covering paths name, with a branch where one is
    known. Mainline is left out."""
    next_trees = next_trees or {}
    found, seen = [], set()
    for s in sections:
        if not s.trees or not any(covers(s, p) for p in paths):
            continue
        for url, branch in s.trees:
            nurl = normalize_url(url)
            if nurl in MAINLINE_URLS:
                continue
            if branch:
                known = next((n for n, _, b in next_trees.get(nurl, ()) if b == branch), None)
                cands = [Route(s.name, url, branch, "T:", known)]
            elif nurl in next_trees:
                cands = [Route(s.name, url, b, "Next/Trees", name) for name, _, b in next_trees[nurl]]
            else:
                cands = [Route(s.name, url, None, "T:")]
            for r in cands:
                if r.key not in seen:
                    seen.add(r.key)
                    found.append(r)
    return found


def is_kernel(text):
    """Whether a MAINTAINERS file is the Linux kernel's (so linux-next's
    Next/Trees applies to it)."""
    return "torvalds/linux" in text


def tree_name(route):
    """A name for `tree add`: Next/Trees' own, else the URL's last two parts
    and the branch."""
    if route.next_name:
        return route.next_name
    parts = [p for p in normalize_url(route.url).split("/") if p][-2:]
    name = "-".join(parts + ([route.branch] if route.branch else []))
    return re.sub(r"[^A-Za-z0-9._-]+", "-", name)
