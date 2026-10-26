"""SQLite store of raw facts: messages, and when each thread was last fetched.

Series and their status are derived from this on every read, never stored, so
a better derivation applies to old data without a refetch.
"""

import os
import sqlite3

from . import parse, trees

SCHEMA = """
CREATE TABLE IF NOT EXISTS messages (
    msgid       TEXT PRIMARY KEY,
    in_reply_to TEXT,
    refs        TEXT,            -- space-separated, oldest first
    from_name   TEXT,
    from_addr   TEXT,
    date        TEXT,            -- UTC, ISO 8601
    subject     TEXT,
    body        TEXT
);
CREATE INDEX IF NOT EXISTS messages_from ON messages (from_addr);
CREATE INDEX IF NOT EXISTS messages_irt ON messages (in_reply_to);
CREATE TABLE IF NOT EXISTS threads (
    root        TEXT PRIMARY KEY,
    fetched_at  TEXT
);
CREATE TABLE IF NOT EXISTS root_lists (
    root        TEXT,
    list        TEXT,
    PRIMARY KEY (root, list)
);
CREATE TABLE IF NOT EXISTS trees (
    name        TEXT PRIMARY KEY,
    repo        TEXT,
    ref         TEXT,
    url         TEXT,
    branch      TEXT,
    role        TEXT
);
-- The latest check of one posted patch against one tree; commit is NULL
-- when it was not found, which is as much a fact as finding it.
CREATE TABLE IF NOT EXISTS tree_results (
    msgid       TEXT,
    tree        TEXT,
    ref_sha     TEXT,
    commit_sha  TEXT,
    how         TEXT,
    checked_at  TEXT,
    PRIMARY KEY (msgid, tree)
);
CREATE TABLE IF NOT EXISTS syncs (
    author      TEXT,
    list        TEXT,
    query       TEXT,
    ran_at      TEXT
);
"""


# Applied in order on open; PRAGMA user_version counts how many have run.
MIGRATIONS = [
    # 1: recipients, for "what did I send to X". Rows stored before this
    # have none until their thread is fetched again (`patchtrack sync --full`).
    "ALTER TABLE messages ADD COLUMN to_addrs TEXT; ALTER TABLE messages ADD COLUMN cc_addrs TEXT;",
    # 2: a found commit that the tree later reverted. Filled by the next `check`.
    "ALTER TABLE tree_results ADD COLUMN reverted_by TEXT;",
    # 3: when a sync chose a thread for fetching; cleared by the fetch. Still
    # set means the fetch has not happened yet, and the next sync retries it.
    "ALTER TABLE threads ADD COLUMN wanted_at TEXT;",
    # 4: the sender (who posted) and the author (who wrote the patch: the
    # in-body From:) are separate. Rows stored before get their author from
    # the body. A b4 relay copy names the relay as sender; its thread is
    # marked for the next sync, which stores it under X-Original-From.
    lambda db: _split_sender_and_author(db),
    # 5: your own mailbox as a source. sources: when each path was last read;
    # threads.local_at: when a thread was last seen in local mail, which
    # holds only what was sent to you.
    "CREATE TABLE sources (path TEXT PRIMARY KEY, read_at TEXT);"
    " ALTER TABLE threads ADD COLUMN local_at TEXT;",
    # 6: what `news` last showed for each family (author NUL family key).
    "CREATE TABLE seen (family TEXT PRIMARY KEY, snapshot TEXT, shown_at TEXT);",
    # 7: families you closed (abandoned, folded into someone else's series...).
    "CREATE TABLE closed (family TEXT PRIMARY KEY, reason TEXT, closed_at TEXT);",
]


def _split_sender_and_author(db):
    db.executescript("ALTER TABLE messages ADD COLUMN author_name TEXT;"
                     " ALTER TABLE messages ADD COLUMN author_addr TEXT;"
                     " UPDATE messages SET author_name = from_name, author_addr = from_addr;")
    rows = db.execute("SELECT msgid, subject, from_name, from_addr, body FROM messages"
                      " WHERE body LIKE 'From:%' OR from_addr LIKE 'devnull+%'").fetchall()
    for mid, subject, name, address, body in rows:
        n, a, b = parse.inbody_author(subject, name, address, body)
        db.execute("UPDATE messages SET author_name = ?, author_addr = ?, body = ? WHERE msgid = ?",
                   (n, a, b, mid))
        if address.startswith("devnull+"):
            # t.mbox.gz of any message is its whole thread
            db.execute("INSERT INTO threads (root, wanted_at) VALUES (?, ?) ON CONFLICT (root)"
                       " DO UPDATE SET wanted_at = excluded.wanted_at", (mid, parse.now_utc()))


def default_path():
    base = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    return os.path.join(base, "patchtrack", "patchtrack.sqlite3")


class Store:
    def __init__(self, path=None):
        path = path or default_path()
        if path != ":memory:":
            os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
        self.db = sqlite3.connect(path)
        self.db.row_factory = sqlite3.Row
        self.db.executescript(SCHEMA)
        version = self.db.execute("PRAGMA user_version").fetchone()[0]
        for n, step in enumerate(MIGRATIONS[version:], version + 1):
            if callable(step):
                step(self.db)
            else:
                self.db.executescript(step)
            self.db.execute(f"PRAGMA user_version = {n}")
        self.db.commit()

    def add(self, msg):
        """Store one email.message.Message; returns its message-id."""
        mid = parse.msgid(msg["Message-ID"])
        if not mid:
            return None
        subject = " ".join(parse.decode(msg["Subject"]).split())  # unfold
        name, address = parse.sender(msg)
        author_name, author_addr, body = parse.inbody_author(subject, name, address, parse.text_body(msg))
        refs = parse.msgids(msg["References"])
        irt = parse.msgid(msg["In-Reply-To"]) if msg["In-Reply-To"] else (refs[-1] if refs else None)
        self.db.execute(
            "INSERT OR REPLACE INTO messages (msgid, in_reply_to, refs, from_name, from_addr, date,"
            " subject, body, to_addrs, cc_addrs, author_name, author_addr)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (mid, irt, " ".join(refs), name, address, parse.date_utc(msg["Date"]), subject, body,
             " ".join(parse.addrs(msg.get_all("To"))), " ".join(parse.addrs(msg.get_all("Cc"))),
             author_name, author_addr),
        )
        return mid

    def mark_thread(self, root, when):
        self.db.execute("INSERT INTO threads (root, fetched_at) VALUES (?, ?) ON CONFLICT (root)"
                        " DO UPDATE SET fetched_at = excluded.fetched_at, wanted_at = NULL", (root, when))

    def want_thread(self, root, when):
        self.db.execute("INSERT INTO threads (root, wanted_at) VALUES (?, ?) "
                        "ON CONFLICT (root) DO UPDATE SET wanted_at = excluded.wanted_at", (root, when))

    def pending_threads(self):
        """Roots chosen for fetching whose fetch has not happened since."""
        return {r[0] for r in self.db.execute("SELECT root FROM threads WHERE wanted_at IS NOT NULL")}

    def tag_root(self, root, lst):
        self.db.execute("INSERT OR IGNORE INTO root_lists VALUES (?, ?)", (root, lst))

    def record_sync(self, author, lst, query, when):
        self.db.execute("INSERT INTO syncs VALUES (?, ?, ?, ?)", (author, lst, query, when))

    def add_tree(self, t):
        self.db.execute("INSERT OR REPLACE INTO trees VALUES (?, ?, ?, ?, ?, ?)",
                        (t.name, t.repo, t.ref, t.url, t.branch, t.role))

    def remove_tree(self, name):
        self.db.execute("DELETE FROM trees WHERE name = ?", (name,))
        self.db.execute("DELETE FROM tree_results WHERE tree = ?", (name,))

    def trees(self):
        return [trees.Tree(**dict(r)) for r in self.db.execute("SELECT * FROM trees ORDER BY name")]

    def record_tree_result(self, msgid, tree, ref_sha, commit_sha, how, reverted_by, when):
        self.db.execute(
            "INSERT OR REPLACE INTO tree_results (msgid, tree, ref_sha, commit_sha, how, checked_at,"
            " reverted_by) VALUES (?, ?, ?, ?, ?, ?, ?)",
            (msgid, tree, ref_sha, commit_sha, how, when, reverted_by))

    def tree_results(self):
        """{msgid: [row, ...]}, each row joined with its tree's role."""
        out = {}
        for r in self.db.execute(
                "SELECT r.*, t.role FROM tree_results r JOIN trees t ON t.name = r.tree"):
            out.setdefault(r["msgid"], []).append(dict(r))
        return out

    def commit(self):
        self.db.commit()

    def close(self):
        self.db.close()

    def closed(self):
        return {r[0]: r[1] or "" for r in self.db.execute("SELECT family, reason FROM closed")}

    def close_family(self, family, reason, when):
        self.db.execute("INSERT OR REPLACE INTO closed VALUES (?, ?, ?)", (family, reason, when))

    def reopen_family(self, family):
        self.db.execute("DELETE FROM closed WHERE family = ?", (family,))

    def seen(self):
        return {r[0]: r[1] for r in self.db.execute("SELECT family, snapshot FROM seen")}

    def mark_seen(self, family, snapshot, when):
        self.db.execute("INSERT OR REPLACE INTO seen VALUES (?, ?, ?)", (family, snapshot, when))

    def known_ids(self):
        return {r[0] for r in self.db.execute("SELECT msgid FROM messages")}

    def last_read(self, path):
        r = self.db.execute("SELECT read_at FROM sources WHERE path = ?", (path,)).fetchone()
        return parse.since_epoch(r[0]) if r and r[0] else None

    def mark_read(self, path, when):
        self.db.execute("INSERT OR REPLACE INTO sources VALUES (?, ?)", (path, when))

    def mark_local(self, root, when):
        self.db.execute("INSERT INTO threads (root, local_at) VALUES (?, ?) ON CONFLICT (root)"
                        " DO UPDATE SET local_at = excluded.local_at", (root, when))

    def local_times(self):
        return {r[0]: r[1] for r in self.db.execute("SELECT root, local_at FROM threads WHERE local_at IS NOT NULL")}

    def known(self, msgids):
        """The subset of msgids already stored."""
        msgids = list(msgids)
        out = set()
        for i in range(0, len(msgids), 500):
            chunk = msgids[i:i + 500]
            q = f"SELECT msgid FROM messages WHERE msgid IN ({','.join('?' * len(chunk))})"
            out |= {r[0] for r in self.db.execute(q, chunk)}
        return out

    def last_sync(self, author, lst):
        r = self.db.execute("SELECT MAX(ran_at) FROM syncs WHERE author = ? AND list = ?",
                            (author.lower(), lst)).fetchone()
        return r[0]

    def messages(self):
        return [dict(r) for r in self.db.execute("SELECT * FROM messages")]

    def thread_times(self):
        return {r["root"]: r["fetched_at"] for r in self.db.execute("SELECT * FROM threads")
                if r["fetched_at"]}

    def root_lists(self):
        out = {}
        for r in self.db.execute("SELECT * FROM root_lists"):
            out.setdefault(r["root"], set()).add(r["list"])
        return out

    def authors(self):
        return [r[0] for r in self.db.execute("SELECT DISTINCT author FROM syncs ORDER BY author")]
