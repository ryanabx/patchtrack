"""The config file: optional TOML, in ``$XDG_CONFIG_HOME/patchtrack/config.toml``.

``--config FILE`` names another. With authors and trees in it, the daily
round needs no arguments: ``patchtrack sync && patchtrack check --fetch &&
patchtrack list``. ``patchtrack config`` shows the file in use and what it
sets. Command-line options override the file, and the file overrides the
built-in defaults. An unknown section or key, or a value of the wrong
type, is an error, so a typo cannot silently do nothing.

Every table, with its defaults::

    [sync]
    authors = ["you@example.org"]        # whose patches (-a on the command line replaces this)
    lists = ["all"]                      # inboxes to search, e.g. ["linux-usb", "u-boot"]
    since = "3.months.ago"               # the window of a first sync
    lore = "https://lore.kernel.org"     # the archive
    window_days = 30                     # search this many days at a time
    refresh_days = 7                     # refetch open series not fetched for this long

    [[trees]]                            # one table per tree `check` looks in
    name = "mainline"
    repo = "~/linux"
    url = "https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git"
    branch = "master"                    # or: ref = "origin/master", without url
    role = "mainline"                    # maintainer (default), next or mainline

    [sources]                            # your own mail, read by `sync` and `import`
    maildirs = ["~/Mail/INBOX", "~/Mail/Sent"]   # Maildirs, or directories of .eml files
    mboxes = ["~/mail/sent.mbox"]

    [b4]                                 # where `branches` finds your b4 prep branches
    repos = ["~/linux"]
    prefix = "b4/"

    [nudge]                              # when `nudge` suggests a ping or a RESEND
    ping_days = 7
    resend_days = 14
    merge_window = true                  # with a mainline tree: hold pings until -rc1

    [[profiles]]                         # how a project says "approved" and "landed"
    name = "myproject"                   # "sourceware" is built in; the same name replaces it
    recipients = ["*@lists.example.org"] # globs on the posting's To:/Cc:
    approvals = ['^(ok|lgtm)([.!]|$)']   # regexes on lines that start a paragraph
    landed = ['^(pushed|committed)']     # "it is in", counted from the author too

    [bots]
    builtin = true                       # keep the built-in rule (*bot*, lkp@intel.com, ...)
    addresses = ["ci@example.org"]       # these senders are bots
    patterns = ["*@ci.example.org"]      # fnmatch globs on the address
    humans = ["abbott@example.org"]      # never bots, whatever matched
    bodies = ["^My bot found errors"]    # regexes on the unquoted body, for
                                         # people who also send bot mail

Trees in the config are written to the store on every run, so the config
is their source of truth: ``tree rm`` refuses them. Trees added with
``tree add`` sit beside them.
"""

import os
import re
import tomllib
from dataclasses import dataclass, field
from fnmatch import fnmatch

from . import parse, trees

BOT_RE = re.compile(r"(^|[.+@-])(bot|robot|noreply|no-reply)([.+@-]|$)|^lkp@intel\.com$", re.I)
KNOWN = {
    "bots": {"builtin", "addresses", "patterns", "humans", "bodies"},
    "sync": {"authors", "lists", "since", "lore", "window_days", "refresh_days"},
    "sources": {"maildirs", "mboxes"},
    "nudge": {"ping_days", "resend_days", "merge_window"},
    "b4": {"repos", "prefix"},
}
TREE_KEYS = {"name", "repo", "ref", "url", "branch", "role"}
PROFILE_KEYS = {"name", "recipients", "approvals", "landed"}
TYPES = {"authors": list, "lists": list, "since": str, "lore": str, "window_days": int, "refresh_days": int, "ping_days": int, "resend_days": int, "merge_window": bool, "repos": list, "prefix": str,
         "builtin": bool, "maildirs": list, "mboxes": list, "addresses": list, "patterns": list, "humans": list, "bodies": list}


class ConfigError(ValueError):
    pass


def default_path():
    base = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    return os.path.join(base, "patchtrack", "config.toml")


@dataclass(frozen=True)
class BotRules:
    builtin: bool = True
    addresses: frozenset = frozenset()
    patterns: tuple = ()
    humans: frozenset = frozenset()
    bodies: tuple = field(default=())  # compiled

    def is_bot(self, address, body=""):
        a = (address or "").lower()
        if a in self.humans:
            return False
        if a in self.addresses or any(fnmatch(a, p) for p in self.patterns):
            return True
        if self.bodies and any(r.search(parse.unquoted(body)) for r in self.bodies):
            return True
        return self.builtin and bool(BOT_RE.search(a))


def load(path=None):
    """The parsed config; {} when the default file does not exist."""
    explicit = path is not None
    path = path or default_path()
    if not os.path.exists(path):
        if explicit:
            raise ConfigError(f"{path}: no such file")
        return {}
    with open(path, "rb") as f:
        try:
            conf = tomllib.load(f)
        except tomllib.TOMLDecodeError as e:
            raise ConfigError(f"{path}: {e}") from None
    for section, value in conf.items():
        if section == "trees":
            _check_trees(path, value)
            continue
        if section == "profiles":
            _check_profiles(path, value)
            continue
        if section not in KNOWN or not isinstance(value, dict):
            raise ConfigError(f"{path}: unknown section [{section}]")
        unknown = set(value) - KNOWN[section]
        if unknown:
            raise ConfigError(f"{path}: unknown key(s) in [{section}]: {', '.join(sorted(unknown))}")
        for key, v in value.items():
            if not isinstance(v, TYPES[key]) or (TYPES[key] is int and isinstance(v, bool)):
                raise ConfigError(f"{path}: [{section}] {key} must be a {TYPES[key].__name__}")
    return conf


def _check_profiles(path, value):
    if not isinstance(value, list):
        raise ConfigError(f"{path}: profiles must be [[profiles]] tables")
    for p in value:
        unknown = set(p) - PROFILE_KEYS
        if unknown:
            raise ConfigError(f"{path}: unknown key(s) in profile {p.get('name', '?')}: {', '.join(sorted(unknown))}")
        if not p.get("name") or not p.get("recipients"):
            raise ConfigError(f"{path}: every [[profiles]] needs a name and recipients")
        for key in ("approvals", "landed"):
            for rx in p.get(key, []):
                try:
                    re.compile(rx)
                except re.error as e:
                    raise ConfigError(f"{path}: profile {p['name']} {key}: {e}") from None


@dataclass(frozen=True)
class Profile:
    """How a project says "approved" and "landed", for the series sent to it."""
    name: str
    recipients: tuple  # fnmatch globs on To:/Cc: addresses
    approvals: tuple = ()  # regexes on a reply's paragraph-starting lines: an approval
    landed: tuple = ()  # the same, for "it is in": also counted from the author

    def matches(self, addresses):
        return any(fnmatch(a, g) for a in addresses for g in self.recipients)


# Measured on a week of gcc-patches (2026-10-26): reviewers approve with a
# bare "OK."; "OK for mainline?" and "OK, but..." are conversation. Authors
# commit themselves and say "Pushed as r17-1234".
SOURCEWARE = Profile(
    "sourceware", ("*@gcc.gnu.org", "*@sourceware.org"),
    approvals=(re.compile(r"(?i)^(?:this (?:patch |version )?is )?(?:ok|lgtm)"
                          r"(?: (?:for|to) [\w./+-]+(?: [\w./+-]+)?)?(?:[.!](?:\s|$)|$)"),),
    landed=(re.compile(r"(?i)^(?:ok and |now )?(?:pushed|committed|installed)\b"),),
)
BUILTIN_PROFILES = (SOURCEWARE,)


def _check_trees(path, value):
    if not isinstance(value, list):
        raise ConfigError(f"{path}: trees must be [[trees]] tables")
    for t in value:
        name = t.get("name", "?")
        unknown = set(t) - TREE_KEYS
        if unknown:
            raise ConfigError(f"{path}: unknown key(s) in tree {name}: {', '.join(sorted(unknown))}")
        if not t.get("name") or not t.get("repo"):
            raise ConfigError(f"{path}: every [[trees]] needs a name and a repo")
        if bool(t.get("url")) != bool(t.get("branch")) or bool(t.get("url")) == bool(t.get("ref")):
            raise ConfigError(f"{path}: tree {name}: give either ref, or url and branch")
        if t.get("role", "maintainer") not in trees.ROLES:
            raise ConfigError(f"{path}: tree {name}: role must be one of {', '.join(trees.ROLES)}")


@dataclass(frozen=True)
class Settings:
    path: str
    exists: bool
    authors: tuple = ()
    lists: tuple = ()
    since: str | None = None
    lore: str | None = None
    window_days: int | None = None
    refresh_days: int | None = None
    trees: tuple = ()
    profiles: tuple = BUILTIN_PROFILES
    sources: tuple = ()  # local Maildirs, .eml directories and mbox files
    ping_days: int | None = None
    resend_days: int | None = None
    merge_window: bool = True  # read mainline's tags for an open merge window
    b4_repos: tuple = ()  # where `branches` looks for b4 prep branches
    b4_prefix: str = "b4/"
    bots: BotRules = BotRules()


def settings(path=None):
    """Everything the config file says, ready to use."""
    conf = load(path)
    sync = conf.get("sync", {})
    since = sync.get("since")
    if since:
        try:
            parse.since_epoch(since)
        except ValueError as e:
            raise ConfigError(f"[sync] since: {e}") from None
    return Settings(
        path=path or default_path(),
        exists=bool(conf) or os.path.exists(path or default_path()),
        authors=tuple(a.lower() for a in sync.get("authors", [])),
        lists=tuple(sync.get("lists", [])),
        since=since,
        lore=sync.get("lore"),
        window_days=sync.get("window_days"),
        refresh_days=sync.get("refresh_days"),
        trees=tuple(trees.Tree(t["name"], os.path.abspath(os.path.expanduser(t["repo"])),
                               t.get("ref") or f"refs/patchtrack/{t['name']}", t.get("url"), t.get("branch"),
                               t.get("role", "maintainer")) for t in conf.get("trees", [])),
        profiles=_profiles(conf),
        sources=tuple(os.path.abspath(os.path.expanduser(p)) for key in ("maildirs", "mboxes")
                      for p in conf.get("sources", {}).get(key, [])),
        b4_repos=tuple(os.path.abspath(os.path.expanduser(p)) for p in conf.get("b4", {}).get("repos", [])),
        b4_prefix=conf.get("b4", {}).get("prefix", "b4/"),
        merge_window=conf.get("nudge", {}).get("merge_window", True),
        ping_days=conf.get("nudge", {}).get("ping_days"),
        resend_days=conf.get("nudge", {}).get("resend_days"),
        bots=bot_rules(conf),
    )


def _profiles(conf):
    mine = [Profile(p["name"], tuple(a.lower() for a in p["recipients"]),
                    tuple(re.compile(r, re.I) for r in p.get("approvals", [])),
                    tuple(re.compile(r, re.I) for r in p.get("landed", []))) for p in conf.get("profiles", [])]
    names = {p.name for p in mine}
    return tuple(mine) + tuple(p for p in BUILTIN_PROFILES if p.name not in names)


def bot_rules(conf):
    b = conf.get("bots", {})
    try:
        bodies = tuple(re.compile(p, re.M) for p in b.get("bodies", []))
    except re.error as e:
        raise ConfigError(f"[bots] bodies: {e}") from None
    return BotRules(
        builtin=bool(b.get("builtin", True)),
        addresses=frozenset(a.lower() for a in b.get("addresses", [])),
        patterns=tuple(p.lower() for p in b.get("patterns", [])),
        humans=frozenset(a.lower() for a in b.get("humans", [])),
        bodies=bodies,
    )
