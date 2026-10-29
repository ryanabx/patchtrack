//! The store: one SQLite schema, for the command line and the bridge alike.
//!
//! It holds raw facts and the user's decisions, never anything derived:
//! series, families and states are recomputed from it on every read. The
//! file is also the interchange format: exporting is copying it, and any
//! patchtrack can open or import what another wrote.
//!
//! - Messages are kept as received (`raw`); the other columns are parsed
//!   from it by `patchtrack_core`, and parsed again when the parser changes.
//! - Two copies under one message-id (lore keeps two of b4 relay mail) keep
//!   the one whose bytes sort first, so merging stores gives the same
//!   result in any order.
//! - `PRAGMA user_version` counts the migrations applied; a store from a
//!   newer patchtrack is refused, not guessed at. `PRAGMA application_id`
//!   marks the file as patchtrack's.

use std::fmt;
use std::path::Path;

use std::collections::BTreeMap;

use patchtrack_core::mail::{Addr, Message, PARSER_VERSION};
use patchtrack_core::series::{Facts, Role, TreeRow};
use patchtrack_core::settings::{Follow, Subscription};
use patchtrack_core::sync::Effect;
use patchtrack_core::trees::Tree;
use rusqlite::{Connection, OptionalExtension, params};

/// "ptrk": what `PRAGMA application_id` says in a patchtrack store.
pub const APPLICATION_ID: i32 = 0x7074_726b;

/// Applied in order on open; append only, never edit a shipped entry.
/// A new column is empty on old rows unless the migration fills it, so say
/// in its comment what does.
const MIGRATIONS: &[&str] = &[
    // 1: messages, as received, and the fields parsed from them.
    "CREATE TABLE messages (
        msgid        TEXT PRIMARY KEY,
        raw          BLOB NOT NULL,     -- as received; the columns below are parsed from it
        parsed_with  INTEGER NOT NULL,  -- patchtrack_core::mail::PARSER_VERSION
        in_reply_to  TEXT,
        refs         TEXT NOT NULL,     -- space-separated, oldest first
        from_name    TEXT NOT NULL,
        from_addr    TEXT NOT NULL,     -- the sender: X-Original-From over a relay's From
        author_name  TEXT NOT NULL,
        author_addr  TEXT NOT NULL,     -- the patch's author: git's in-body From
        date         TEXT NOT NULL,     -- UTC, 2026-10-05T10:00:00Z; '' if unknown
        subject      TEXT NOT NULL,
        to_addrs     TEXT NOT NULL,     -- space-separated, lower case
        cc_addrs     TEXT NOT NULL,
        body         TEXT NOT NULL      -- text/plain parts, without an in-body From
    );
    CREATE INDEX messages_from ON messages (from_addr);
    CREATE INDEX messages_irt ON messages (in_reply_to);
    CREATE INDEX messages_date ON messages (date);",
    // 2: what syncs and tree checks learn, and the user's decisions. Empty in
    // a store from before; the next sync and check fill them.
    "CREATE TABLE threads (
        root         TEXT PRIMARY KEY,
        fetched_at   TEXT,             -- the whole thread, from the archive
        wanted_at    TEXT,             -- a sync chose it; cleared by the fetch
        local_at     TEXT              -- last seen in local mail
    );
    CREATE TABLE root_lists (
        root         TEXT NOT NULL,
        list         TEXT NOT NULL,
        PRIMARY KEY (root, list)
    );
    CREATE TABLE syncs (
        author       TEXT NOT NULL,
        list         TEXT NOT NULL,
        query        TEXT NOT NULL,
        ran_at       TEXT NOT NULL,
        PRIMARY KEY (author, list, ran_at)
    );
    -- The latest check of one posted patch against one tree; no commit is
    -- 'not found', which is as much a fact as finding it. The tree's role is
    -- kept here so the results mean the same in a store without the tree.
    CREATE TABLE tree_results (
        msgid        TEXT NOT NULL,
        tree         TEXT NOT NULL,
        role         TEXT NOT NULL,    -- maintainer, next, mainline
        ref_sha      TEXT NOT NULL,
        commit_sha   TEXT,
        how          TEXT,
        reverted_by  TEXT,
        checked_at   TEXT NOT NULL,
        PRIMARY KEY (msgid, tree)
    );
    -- The user's: families closed, and what `news` last showed.
    CREATE TABLE closed (
        family       TEXT PRIMARY KEY, -- sender NUL family key
        reason       TEXT NOT NULL,
        closed_at    TEXT NOT NULL
    );
    CREATE TABLE seen (
        family       TEXT PRIMARY KEY,
        snapshot     TEXT NOT NULL,
        shown_at     TEXT NOT NULL
    );",
    // 3: settings and what you follow, which travel with the store; and the
    // machine's own local mail and git trees, which never do (an export
    // leaves them out). Empty in a store from before.
    "CREATE TABLE settings (
        key          TEXT PRIMARY KEY,
        value        TEXT NOT NULL     -- JSON, checked by patchtrack_core::settings
    );
    CREATE TABLE subscriptions (
        kind         TEXT NOT NULL,    -- author, list, query, thread (from 5)
        value        TEXT NOT NULL,    -- an address (lower case), inbox, query or message-id
        list         TEXT NOT NULL,    -- the inbox searched; 'all' for every list
        added_at     TEXT NOT NULL,
        PRIMARY KEY (kind, value, list)
    );
    CREATE TABLE sources (
        path         TEXT PRIMARY KEY, -- a Maildir, a directory of .eml files, or an mbox
        added_at     TEXT NOT NULL,
        read_at      TEXT              -- when a sync last read it
    );
    CREATE TABLE trees (
        name         TEXT PRIMARY KEY,
        repo         TEXT NOT NULL,
        ref          TEXT NOT NULL,
        url          TEXT,             -- with branch: fetched whole into ref
        branch       TEXT,
        role         TEXT NOT NULL     -- maintainer, next, mainline
    );",
    // 4: this machine's own small facts (the bridge's pairing token); never
    // exported.
    "CREATE TABLE machine (
        key          TEXT PRIMARY KEY,
        value        TEXT NOT NULL
    );",
    // 5: syncs are kept per subscription (an author, a list, a query, a
    // thread), not per author; every older row was an author's. found_by
    // says which list, query or thread subscription's sync found a thread:
    // empty in a store from before, as author syncs never needed it.
    "CREATE TABLE syncs_by_subscription (
        kind         TEXT NOT NULL,    -- author, list, query, thread
        value        TEXT NOT NULL,
        list         TEXT NOT NULL,
        query        TEXT NOT NULL,
        ran_at       TEXT NOT NULL,
        PRIMARY KEY (kind, value, list, ran_at)
    );
    INSERT INTO syncs_by_subscription SELECT 'author', author, list, query, ran_at FROM syncs;
    DROP TABLE syncs;
    ALTER TABLE syncs_by_subscription RENAME TO syncs;
    CREATE TABLE found_by (
        root         TEXT NOT NULL,
        kind         TEXT NOT NULL,
        value        TEXT NOT NULL,
        list         TEXT NOT NULL,
        PRIMARY KEY (root, kind, value, list)
    );",
    // 6: mail you composed, as it will be sent, until and after it is: yours,
    // like closed families, so a public export leaves it out. Empty in a
    // store from before.
    "CREATE TABLE drafts (
        msgid        TEXT PRIMARY KEY, -- without <>
        family       TEXT NOT NULL,    -- sender NUL family key; '' for none
        kind         TEXT NOT NULL,    -- reply, tag, thanks, ping
        raw          BLOB NOT NULL,
        created_at   TEXT NOT NULL,
        sent_at      TEXT
    );",
    // 7: several archives (projects). What a subscription searches, what a
    // sync recorded and found, and which archive owes a thread, are kept
    // per archive: '' is the archive setting's, which every older row was.
    "ALTER TABLE threads ADD COLUMN archive TEXT NOT NULL DEFAULT '';
    CREATE TABLE subscriptions_7 (
        kind TEXT NOT NULL, value TEXT NOT NULL, list TEXT NOT NULL,
        archive      TEXT NOT NULL,    -- '' for the archive setting's
        added_at TEXT NOT NULL,
        PRIMARY KEY (kind, value, list, archive)
    );
    INSERT INTO subscriptions_7 SELECT kind, value, list, '', added_at FROM subscriptions;
    DROP TABLE subscriptions;
    ALTER TABLE subscriptions_7 RENAME TO subscriptions;
    CREATE TABLE syncs_7 (
        kind TEXT NOT NULL, value TEXT NOT NULL, list TEXT NOT NULL, archive TEXT NOT NULL,
        query TEXT NOT NULL, ran_at TEXT NOT NULL,
        PRIMARY KEY (kind, value, list, archive, ran_at)
    );
    INSERT INTO syncs_7 SELECT kind, value, list, '', query, ran_at FROM syncs;
    DROP TABLE syncs;
    ALTER TABLE syncs_7 RENAME TO syncs;
    CREATE TABLE found_by_7 (
        root TEXT NOT NULL, kind TEXT NOT NULL, value TEXT NOT NULL, list TEXT NOT NULL, archive TEXT NOT NULL,
        PRIMARY KEY (root, kind, value, list, archive)
    );
    INSERT INTO found_by_7 SELECT root, kind, value, list, '' FROM found_by;
    DROP TABLE found_by;
    ALTER TABLE found_by_7 RENAME TO found_by;
    CREATE TABLE root_lists_7 (
        root TEXT NOT NULL, list TEXT NOT NULL, archive TEXT NOT NULL,
        PRIMARY KEY (root, list, archive)
    );
    INSERT INTO root_lists_7 SELECT root, list, '' FROM root_lists;
    DROP TABLE root_lists;
    ALTER TABLE root_lists_7 RENAME TO root_lists;",
];

/// Mail you composed (see `patchtrack_core::compose`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    /// Its own Message-ID, without <>.
    pub msgid: String,
    /// The family it is about (sender NUL key), or "".
    pub family: String,
    /// reply, tag, thanks or ping.
    pub kind: String,
    pub raw: Vec<u8>,
    pub created_at: String,
    /// When it was sent; None while it is a draft.
    pub sent_at: Option<String>,
}

/// Tables of this machine's, which no export carries.
const MACHINE_TABLES: &[&str] = &["sources", "trees", "machine"];

const COLUMNS: &str = "msgid, raw, parsed_with, in_reply_to, refs, from_name, from_addr, author_name, author_addr, \
                       date, subject, to_addrs, cc_addrs, body";

/// The newer copy of a message-id wins only if its bytes sort first.
const UPSERT: &str = "ON CONFLICT (msgid) DO UPDATE SET raw = excluded.raw, parsed_with = excluded.parsed_with,
    in_reply_to = excluded.in_reply_to, refs = excluded.refs, from_name = excluded.from_name,
    from_addr = excluded.from_addr, author_name = excluded.author_name, author_addr = excluded.author_addr,
    date = excluded.date, subject = excluded.subject, to_addrs = excluded.to_addrs, cc_addrs = excluded.cc_addrs,
    body = excluded.body
    WHERE excluded.raw < messages.raw";

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    /// The file is an SQLite database, but not a patchtrack store.
    NotAStore,
    /// Written by a newer patchtrack: its schema version, and ours.
    TooNew(usize, usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sqlite(e) => write!(f, "{e}"),
            Error::NotAStore => write!(f, "not a patchtrack store"),
            Error::TooNew(theirs, ours) => {
                write!(f, "written by a newer patchtrack (schema {theirs}; this one knows {ours}): upgrade to read it")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Sqlite(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What an export leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export {
    /// Everything but this machine's local mail paths and trees: for
    /// another machine of yours.
    Full,
    /// Also without your settings, what `news` showed you, closed
    /// families' reasons and which threads were in your mailbox: for
    /// publishing.
    Public,
}

/// What [`Store::add`] did with a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Added {
    New(String),
    /// Already stored: the same bytes, or another copy that sorts first.
    Kept(String),
    /// Another copy was stored; this one sorts first and replaced it.
    Replaced(String),
    /// Not a message with a Message-ID.
    NotAMessage,
}

pub struct Store {
    db: Connection,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Store> {
        Store::setup(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Store> {
        Store::setup(Connection::open_in_memory()?)
    }

    fn setup(db: Connection) -> Result<Store> {
        let app: i32 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version = schema_version(&db, "main")?;
        let empty: bool = db.query_row("SELECT count(*) = 0 FROM sqlite_schema", [], |r| r.get(0))?;
        if app != APPLICATION_ID && !(app == 0 && empty) {
            return Err(Error::NotAStore);
        }
        if version > MIGRATIONS.len() {
            return Err(Error::TooNew(version, MIGRATIONS.len()));
        }
        let mut store = Store { db };
        // Only when there is something to write: a commit, even of nothing
        // new, is a change to every other connection (see `data_version`).
        if version < MIGRATIONS.len() || app != APPLICATION_ID {
            store.migrate(version)?;
        }
        Ok(store)
    }

    fn migrate(&mut self, from: usize) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        for (n, step) in MIGRATIONS.iter().enumerate().skip(from) {
            tx.execute_batch(step)?;
            tx.pragma_update(None, "user_version", i64::try_from(n + 1).expect("few migrations"))?;
        }
        tx.commit()?;
        Ok(())
    }

    /// A number that changes whenever another connection, of this process
    /// or another, commits to the store: what a server watches to know its
    /// answers are stale.
    pub fn data_version(&self) -> Result<i64> {
        Ok(self.db.query_row("PRAGMA data_version", [], |r| r.get(0))?)
    }

    /// Store one message's raw bytes.
    pub fn add(&mut self, raw: &[u8]) -> Result<Added> {
        let Some(m) = Message::parse(raw) else { return Ok(Added::NotAMessage) };
        let before: Option<Vec<u8>> =
            self.db.query_row("SELECT raw FROM messages WHERE msgid = ?1", [&m.msgid], |r| r.get(0)).optional()?;
        upsert(&self.db, raw, &m)?;
        Ok(match before {
            None => Added::New(m.msgid),
            Some(old) if raw < old.as_slice() => Added::Replaced(m.msgid),
            Some(_) => Added::Kept(m.msgid),
        })
    }

    /// Store many, in one transaction; returns how many were new.
    pub fn add_all<'a>(&mut self, raws: impl IntoIterator<Item = &'a [u8]>) -> Result<usize> {
        let tx = self.db.transaction()?;
        let mut new = 0;
        {
            let mut exists = tx.prepare("SELECT 1 FROM messages WHERE msgid = ?1")?;
            for raw in raws {
                let Some(m) = Message::parse(raw) else { continue };
                new += usize::from(!exists.exists([&m.msgid])?);
                upsert(&tx, raw, &m)?;
            }
        }
        tx.commit()?;
        Ok(new)
    }

    /// Every message, as parsed when it was stored or last re-parsed.
    pub fn messages(&self) -> Result<Vec<Message>> {
        let mut stmt = self.db.prepare(
            "SELECT msgid, in_reply_to, refs, from_name, from_addr, author_name, author_addr, date, subject,
                    to_addrs, cc_addrs, body FROM messages",
        )?;
        let words = |s: String| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let rows = stmt.query_map([], |r| {
            Ok(Message {
                msgid: r.get(0)?,
                in_reply_to: r.get(1)?,
                refs: words(r.get(2)?),
                from: Addr { name: r.get(3)?, addr: r.get(4)? },
                author: Addr { name: r.get(5)?, addr: r.get(6)? },
                date: r.get(7)?,
                subject: r.get(8)?,
                to: words(r.get(9)?),
                cc: words(r.get(10)?),
                body: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every message-id stored.
    pub fn msgids(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self.db.prepare("SELECT msgid FROM messages")?;
        Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Do what a sync asks of the store; fetches and logs are the host's.
    pub fn apply(&mut self, effect: &Effect) -> Result<()> {
        match effect {
            Effect::Store(raws) => {
                self.add_all(raws.iter().map(Vec::as_slice))?;
            }
            Effect::RecordSync { sub, query, ran_at } => {
                self.db.execute(
                    "INSERT OR IGNORE INTO syncs (kind, value, list, archive, query, ran_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![sub.kind.as_str(), sub.value, sub.list, sub.archive, query, ran_at],
                )?;
            }
            Effect::Found { root, sub } => {
                self.db.execute(
                    "INSERT OR IGNORE INTO found_by (root, kind, value, list, archive) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![root, sub.kind.as_str(), sub.value, sub.list, sub.archive],
                )?;
            }
            Effect::TagRoot { root, list, archive } => {
                self.db.execute(
                    "INSERT OR IGNORE INTO root_lists (root, list, archive) VALUES (?1, ?2, ?3)",
                    params![root, list, archive],
                )?;
            }
            Effect::WantThread { root, at, archive } => {
                self.db.execute(
                    "INSERT INTO threads (root, wanted_at, archive) VALUES (?1, ?2, ?3)
                     ON CONFLICT (root) DO UPDATE SET wanted_at = excluded.wanted_at, archive = excluded.archive",
                    params![root, at, archive],
                )?;
            }
            Effect::Fetched { root, at, archive } => {
                self.db.execute(
                    "INSERT INTO threads (root, fetched_at, archive) VALUES (?1, ?2, ?3)
                     ON CONFLICT (root) DO UPDATE SET fetched_at = excluded.fetched_at, wanted_at = NULL,
                         archive = excluded.archive",
                    params![root, at, archive],
                )?;
            }
            Effect::Fetch(_) | Effect::Log(_) => {}
        }
        Ok(())
    }

    /// When local mail last showed each of these threads.
    pub fn mark_local(&mut self, roots: impl IntoIterator<Item = impl AsRef<str>>, at: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        for root in roots {
            tx.execute(
                "INSERT INTO threads (root, local_at) VALUES (?1, ?2)
                 ON CONFLICT (root) DO UPDATE SET local_at = excluded.local_at",
                params![root.as_ref(), at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The latest check of a patch against a tree.
    pub fn record_tree_result(&mut self, msgid: &str, row: &TreeRow) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO tree_results (msgid, tree, role, ref_sha, commit_sha, how, reverted_by, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                msgid,
                row.tree,
                row.role.as_str(),
                row.ref_sha,
                row.commit_sha,
                row.how,
                row.reverted_by,
                row.checked_at
            ],
        )?;
        Ok(())
    }

    /// One of this machine's own values (never exported).
    pub fn machine(&self, key: &str) -> Result<Option<String>> {
        Ok(self.db.query_row("SELECT value FROM machine WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_machine(&mut self, key: &str, value: &str) -> Result<()> {
        self.db.execute("INSERT OR REPLACE INTO machine (key, value) VALUES (?1, ?2)", [key, value])?;
        Ok(())
    }

    /// This machine's git trees, by name.
    pub fn trees(&self) -> Result<Vec<Tree>> {
        let mut stmt = self.db.prepare("SELECT name, repo, ref, url, branch, role FROM trees ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok(Tree {
                name: r.get(0)?,
                repo: r.get(1)?,
                git_ref: r.get(2)?,
                url: r.get(3)?,
                branch: r.get(4)?,
                role: Role::parse(&r.get::<_, String>(5)?).unwrap_or(Role::Maintainer),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Add a tree, or replace the one of that name.
    pub fn add_tree(&mut self, t: &Tree) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO trees (name, repo, ref, url, branch, role) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![t.name, t.repo, t.git_ref, t.url, t.branch, t.role.as_str()],
        )?;
        Ok(())
    }

    /// Forget a tree; its results stay, as facts about the patches.
    pub fn remove_tree(&mut self, name: &str) -> Result<bool> {
        Ok(self.db.execute("DELETE FROM trees WHERE name = ?1", [name])? > 0)
    }

    /// What `news` last showed of each family: family to snapshot.
    pub fn seen(&self) -> Result<std::collections::HashMap<String, String>> {
        let mut stmt = self.db.prepare("SELECT family, snapshot FROM seen")?;
        Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Record what `news` showed: (family, snapshot) pairs, at one time.
    pub fn mark_seen<'a>(&mut self, shown: impl IntoIterator<Item = (&'a str, &'a str)>, at: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        for (family, snapshot) in shown {
            tx.execute(
                "INSERT OR REPLACE INTO seen (family, snapshot, shown_at) VALUES (?1, ?2, ?3)",
                [family, snapshot, at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn close_family(&mut self, family: &str, reason: &str, at: &str) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO closed (family, reason, closed_at) VALUES (?1, ?2, ?3)",
            params![family, reason, at],
        )?;
        Ok(())
    }

    pub fn reopen_family(&mut self, family: &str) -> Result<bool> {
        Ok(self.db.execute("DELETE FROM closed WHERE family = ?1", [family])? > 0)
    }

    /// Everything the store knows besides the mail.
    pub fn facts(&self) -> Result<Facts> {
        let mut f = Facts::default();
        let mut stmt = self.db.prepare("SELECT root, fetched_at, wanted_at, local_at, archive FROM threads")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (root, fetched, wanted, local, archive) = row?;
            if fetched.is_some() || wanted.is_some() {
                f.thread_archive.insert(root.clone(), archive);
            }
            if let Some(t) = fetched {
                f.fetched.insert(root.clone(), t);
            }
            if wanted.is_some() {
                f.pending.insert(root.clone());
            }
            if let Some(t) = local {
                f.local.insert(root, t);
            }
        }
        let mut stmt = self.db.prepare("SELECT root, list, archive FROM root_lists")?;
        for row in
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?
        {
            let (root, list, archive) = row?;
            f.lists.entry(root.clone()).or_default().insert(list);
            f.archives.entry(root).or_default().insert(archive);
        }
        let mut stmt = self.db.prepare("SELECT root, kind, value, list, archive FROM found_by")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, subscription(r, 1)?)))? {
            if let (root, Some(sub)) = row? {
                f.found.entry(root).or_default().insert(sub);
            }
        }
        let mut stmt = self.db.prepare(
            "SELECT msgid, tree, role, ref_sha, commit_sha, how, reverted_by, checked_at FROM tree_results ORDER BY tree",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                TreeRow {
                    tree: r.get(1)?,
                    role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Maintainer),
                    ref_sha: r.get(3)?,
                    commit_sha: r.get(4)?,
                    how: r.get(5)?,
                    reverted_by: r.get(6)?,
                    checked_at: r.get(7)?,
                },
            ))
        })?;
        for row in rows {
            let (msgid, row) = row?;
            f.trees.entry(msgid).or_default().push(row);
        }
        let mut stmt = self.db.prepare("SELECT family, reason FROM closed")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (family, reason) = row?;
            f.closed.insert(family, reason);
        }
        Ok(f)
    }

    /// When each subscription was last synced.
    pub fn last_syncs(&self) -> Result<BTreeMap<Subscription, String>> {
        let mut stmt = self
            .db
            .prepare("SELECT kind, value, list, archive, max(ran_at) FROM syncs GROUP BY kind, value, list, archive")?;
        let rows = stmt.query_map([], |r| Ok((subscription(r, 0)?, r.get::<_, String>(4)?)))?;
        let mut out = BTreeMap::new();
        for row in rows {
            if let (Some(sub), at) = row? {
                out.insert(sub, at);
            }
        }
        Ok(out)
    }

    /// Every author ever synced, in order.
    pub fn authors(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .db
            .prepare("SELECT DISTINCT value FROM syncs WHERE kind = 'author' AND archive = '' ORDER BY value")?;
        Ok(stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn raw(&self, msgid: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.db.query_row("SELECT raw FROM messages WHERE msgid = ?1", [msgid], |r| r.get(0)).optional()?)
    }

    /// Parse again every message parsed by another version of the parser;
    /// returns how many.
    pub fn reparse(&mut self) -> Result<usize> {
        let tx = self.db.transaction()?;
        let stale: Vec<Vec<u8>> = tx
            .prepare("SELECT raw FROM messages WHERE parsed_with != ?1")?
            .query_map([PARSER_VERSION], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for raw in &stale {
            if let Some(m) = Message::parse(raw) {
                write_parsed(&tx, raw, &m)?;
            }
        }
        tx.commit()?;
        Ok(stale.len())
    }

    /// Write a copy of the store to path, compacted.
    pub fn export(&self, path: &str, what: Export) -> Result<()> {
        self.db.execute("VACUUM INTO ?1", [path])?;
        let copy = Connection::open(path)?;
        let mut removed = 0;
        for table in MACHINE_TABLES {
            removed += copy.execute(&format!("DELETE FROM {table}"), [])?;
        }
        if what == Export::Public {
            copy.execute_batch(
                "DELETE FROM settings; DELETE FROM seen; DELETE FROM drafts; UPDATE closed SET reason = '';
                 UPDATE threads SET local_at = NULL;",
            )?;
        }
        // Deleted rows stay in free pages until the file is rebuilt; nothing
        // deleted (a store with no machine rows), nothing to rebuild.
        if removed > 0 || what == Export::Public {
            copy.execute_batch("VACUUM")?;
        }
        Ok(())
    }

    /// Keep a draft, or replace the one with its Message-ID.
    pub fn save_draft(&mut self, d: &Draft) -> Result<()> {
        self.db.execute(
            "INSERT INTO drafts (msgid, family, kind, raw, created_at, sent_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (msgid) DO UPDATE SET raw = excluded.raw, sent_at = excluded.sent_at",
            params![d.msgid, d.family, d.kind, d.raw, d.created_at, d.sent_at],
        )?;
        Ok(())
    }

    /// Every draft, oldest first.
    pub fn drafts(&self) -> Result<Vec<Draft>> {
        let mut stmt = self
            .db
            .prepare("SELECT msgid, family, kind, raw, created_at, sent_at FROM drafts ORDER BY created_at, msgid")?;
        let rows = stmt.query_map([], |r| {
            Ok(Draft {
                msgid: r.get(0)?,
                family: r.get(1)?,
                kind: r.get(2)?,
                raw: r.get(3)?,
                created_at: r.get(4)?,
                sent_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A draft was sent at this time.
    pub fn mark_sent(&mut self, msgid: &str, at: &str) -> Result<bool> {
        Ok(self.db.execute("UPDATE drafts SET sent_at = ?2 WHERE msgid = ?1", [msgid, at])? > 0)
    }

    pub fn remove_draft(&mut self, msgid: &str) -> Result<bool> {
        Ok(self.db.execute("DELETE FROM drafts WHERE msgid = ?1", [msgid])? > 0)
    }

    /// Every setting, as (key, JSON); see `patchtrack_core::settings`.
    pub fn settings(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.db.prepare("SELECT key, value FROM settings ORDER BY key")?;
        Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Set a key to a JSON value already checked by `settings::validate`.
    pub fn set_setting(&mut self, key: &str, json: &str) -> Result<()> {
        self.db.execute("INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)", [key, json])?;
        Ok(())
    }

    /// Back to the default; false if it was not set.
    pub fn unset_setting(&mut self, key: &str) -> Result<bool> {
        Ok(self.db.execute("DELETE FROM settings WHERE key = ?1", [key])? > 0)
    }

    /// Everything followed, in order; kinds this version does not know are skipped.
    pub fn subscriptions(&self) -> Result<Vec<Subscription>> {
        let mut stmt = self
            .db
            .prepare("SELECT kind, value, list, archive FROM subscriptions ORDER BY archive, kind, value, list")?;
        let rows = stmt.query_map([], |r| subscription(r, 0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().flatten().collect())
    }

    /// Follow; false if it already was.
    pub fn subscribe(&mut self, s: &Subscription, at: &str) -> Result<bool> {
        Ok(self.db.execute(
            "INSERT OR IGNORE INTO subscriptions (kind, value, list, archive, added_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![s.kind.as_str(), s.value.to_lowercase(), s.list, s.archive, at],
        )? > 0)
    }

    /// Stop following, in one list or (None) in all, on one archive or (None)
    /// on all; returns how many were removed.
    pub fn unsubscribe(
        &mut self,
        kind: Follow,
        value: &str,
        list: Option<&str>,
        archive: Option<&str>,
    ) -> Result<usize> {
        Ok(self.db.execute(
            "DELETE FROM subscriptions WHERE kind = ?1 AND value = ?2 AND (?3 IS NULL OR list = ?3)
                 AND (?4 IS NULL OR archive = ?4)",
            params![kind.as_str(), value.to_lowercase(), list, archive],
        )?)
    }

    /// This machine's local mail: (path, when a sync last read it).
    pub fn sources(&self) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.db.prepare("SELECT path, read_at FROM sources ORDER BY path")?;
        Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
    }

    /// Add a local mail path; false if it was there already.
    pub fn add_source(&mut self, path: &str, at: &str) -> Result<bool> {
        Ok(self.db.execute("INSERT OR IGNORE INTO sources (path, added_at) VALUES (?1, ?2)", [path, at])? > 0)
    }

    pub fn remove_source(&mut self, path: &str) -> Result<bool> {
        Ok(self.db.execute("DELETE FROM sources WHERE path = ?1", [path])? > 0)
    }

    /// A sync read this path at this time.
    pub fn mark_read(&mut self, path: &str, at: &str) -> Result<()> {
        self.db.execute("UPDATE sources SET read_at = ?2 WHERE path = ?1", [path, at])?;
        Ok(())
    }

    /// Merge another store into this one; returns how many messages were new.
    pub fn import(&mut self, path: &str) -> Result<usize> {
        self.db.execute("ATTACH DATABASE ?1 AS other", [path])?;
        let merged = self.merge_attached();
        self.db.execute("DETACH DATABASE other", [])?;
        let new = merged?;
        self.reparse()?;
        Ok(new)
    }

    fn merge_attached(&mut self) -> Result<usize> {
        let app: i32 = self.db.query_row("PRAGMA other.application_id", [], |r| r.get(0))?;
        if app != APPLICATION_ID {
            return Err(Error::NotAStore);
        }
        let theirs = schema_version(&self.db, "other")?;
        if theirs > MIGRATIONS.len() {
            return Err(Error::TooNew(theirs, MIGRATIONS.len()));
        }
        let tx = self.db.transaction()?;
        let count =
            |tx: &rusqlite::Transaction<'_>| tx.query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0));
        let before = count(&tx)?;
        // "WHERE true" keeps SQLite from reading ON CONFLICT as part of the SELECT.
        tx.execute(
            &format!("INSERT INTO messages ({COLUMNS}) SELECT {COLUMNS} FROM other.messages WHERE true {UPSERT}"),
            [],
        )?;
        let after = count(&tx)?;
        if theirs >= 2 {
            // Facts: the later fetch and check win; lists and syncs add up. An
            // owed fetch is the other store's business, and its local mail
            // was someone else's mailbox: not taken. The user's decisions
            // here stay as they are; theirs fill only the gaps.
            tx.execute_batch(
                "INSERT INTO threads (root, fetched_at)
                     SELECT root, fetched_at FROM other.threads WHERE fetched_at IS NOT NULL
                 ON CONFLICT (root) DO UPDATE SET fetched_at = excluded.fetched_at
                     WHERE threads.fetched_at IS NULL OR excluded.fetched_at > threads.fetched_at;
                 INSERT INTO tree_results SELECT msgid, tree, role, ref_sha, commit_sha, how, reverted_by, checked_at
                     FROM other.tree_results WHERE true
                 ON CONFLICT (msgid, tree) DO UPDATE SET role = excluded.role, ref_sha = excluded.ref_sha,
                     commit_sha = excluded.commit_sha, how = excluded.how, reverted_by = excluded.reverted_by,
                     checked_at = excluded.checked_at
                     WHERE excluded.checked_at > tree_results.checked_at;
                 INSERT OR IGNORE INTO closed SELECT family, reason, closed_at FROM other.closed;
                 INSERT OR IGNORE INTO seen SELECT family, snapshot, shown_at FROM other.seen;",
            )?;
        }
        // The facts whose columns later schemas changed, from each schema.
        if (2..7).contains(&theirs) {
            tx.execute_batch("INSERT OR IGNORE INTO root_lists SELECT root, list, '' FROM other.root_lists;")?;
        }
        if (2..5).contains(&theirs) {
            tx.execute_batch(
                "INSERT OR IGNORE INTO syncs SELECT 'author', author, list, '', query, ran_at FROM other.syncs;",
            )?;
        } else if (5..7).contains(&theirs) {
            tx.execute_batch(
                "INSERT OR IGNORE INTO syncs SELECT kind, value, list, '', query, ran_at FROM other.syncs;
                 INSERT OR IGNORE INTO found_by SELECT root, kind, value, list, '' FROM other.found_by;",
            )?;
        } else if theirs >= 7 {
            tx.execute_batch(
                "INSERT OR IGNORE INTO root_lists SELECT root, list, archive FROM other.root_lists;
                 INSERT OR IGNORE INTO syncs SELECT kind, value, list, archive, query, ran_at FROM other.syncs;
                 INSERT OR IGNORE INTO found_by SELECT root, kind, value, list, archive FROM other.found_by;
                 UPDATE threads SET archive = (SELECT o.archive FROM other.threads o WHERE o.root = threads.root)
                     WHERE archive = '' AND root IN (SELECT root FROM other.threads WHERE archive != '');",
            )?;
        }
        if theirs >= 6 {
            // Drafts are yours, keyed by their own Message-ID: theirs add up.
            tx.execute_batch(
                "INSERT OR IGNORE INTO drafts SELECT msgid, family, kind, raw, created_at, sent_at FROM other.drafts;",
            )?;
        }
        if theirs >= 3 {
            // Settings and subscriptions are decisions too: theirs fill the
            // keys you have not set, and their subscriptions are taken only
            // into a store that follows nothing (a new one), so importing a
            // published board does not make your syncs follow its authors.
            let archive = if theirs >= 7 { "archive" } else { "''" };
            tx.execute_batch(&format!(
                "INSERT OR IGNORE INTO settings SELECT key, value FROM other.settings;
                 INSERT OR IGNORE INTO subscriptions SELECT kind, value, list, {archive}, added_at FROM other.subscriptions
                     WHERE NOT EXISTS (SELECT 1 FROM main.subscriptions);"
            ))?;
        }
        tx.commit()?;
        Ok(usize::try_from(after - before).unwrap_or(0))
    }
}

/// A subscription from (kind, value, list, archive) columns starting at `at`; None
/// for a kind a newer patchtrack wrote.
fn subscription(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Option<Subscription>> {
    let (kind, value, list, archive): (String, String, String, String) =
        (r.get(at)?, r.get(at + 1)?, r.get(at + 2)?, r.get(at + 3)?);
    Ok(Follow::parse(&kind).map(|kind| Subscription { kind, value, list, archive }))
}

fn schema_version(db: &Connection, schema: &str) -> Result<usize> {
    let v: i64 = db.query_row(&format!("PRAGMA {schema}.user_version"), [], |r| r.get(0))?;
    Ok(usize::try_from(v).unwrap_or(0))
}

fn upsert(db: &Connection, raw: &[u8], m: &Message) -> Result<()> {
    db.execute(
        &format!("INSERT INTO messages ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14) {UPSERT}"),
        params![
            m.msgid, raw, PARSER_VERSION, m.in_reply_to, m.refs.join(" "), m.from.name, m.from.addr,
            m.author.name, m.author.addr, m.date, m.subject, m.to.join(" "), m.cc.join(" "), m.body
        ],
    )?;
    Ok(())
}

fn write_parsed(db: &Connection, raw: &[u8], m: &Message) -> Result<()> {
    db.execute(
        "UPDATE messages SET parsed_with = ?2, in_reply_to = ?3, refs = ?4, from_name = ?5, from_addr = ?6,
            author_name = ?7, author_addr = ?8, date = ?9, subject = ?10, to_addrs = ?11, cc_addrs = ?12, body = ?13
         WHERE msgid = ?1 AND raw = ?14",
        params![
            m.msgid,
            PARSER_VERSION,
            m.in_reply_to,
            m.refs.join(" "),
            m.from.name,
            m.from.addr,
            m.author.name,
            m.author.addr,
            m.date,
            m.subject,
            m.to.join(" "),
            m.cc.join(" "),
            m.body,
            raw
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(id: &str, from: &str, body: &str) -> Vec<u8> {
        format!("Message-ID: <{id}>\nFrom: {from}\nSubject: [PATCH] foo: bar\nDate: Mon, 05 Oct 2026 10:00:00 +0000\n\n{body}")
            .into_bytes()
    }

    fn tmp(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("patchtrack-store-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        dir.to_string_lossy().into_owned()
    }

    #[test]
    fn opening_or_reading_changes_nothing_another_connection_sees() {
        let path = tmp("data-version");
        let mut first = Store::open(&path).unwrap();
        first.add(&mail("a@x", "Dev <dev@x>", "Fix.\n")).unwrap();
        let watch = Store::open(&path).unwrap();
        let before = watch.data_version().unwrap();
        // What a server does for each request: open, read, close.
        for _ in 0..3 {
            let mut s = Store::open(&path).unwrap();
            s.reparse().unwrap();
            s.messages().unwrap();
        }
        assert_eq!(watch.data_version().unwrap(), before, "a read looked like a change");
        first.add(&mail("b@x", "Dev <dev@x>", "Fix.\n")).unwrap();
        assert_ne!(watch.data_version().unwrap(), before, "a change went unseen");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn add_and_read_back() {
        let mut s = Store::open_in_memory().unwrap();
        let raw = mail("a@x", "Dev <dev@x>", "Fix.\n");
        assert_eq!(s.add(&raw).unwrap(), Added::New("a@x".into()));
        assert_eq!(s.add(&raw).unwrap(), Added::Kept("a@x".into()));
        assert_eq!(s.add(b"no headers at all").unwrap(), Added::NotAMessage);
        let msgs = s.messages().unwrap();
        assert_eq!(msgs, vec![Message::parse(&raw).unwrap()]);
        assert_eq!(s.raw("a@x").unwrap(), Some(raw));
    }

    #[test]
    fn two_copies_keep_the_one_that_sorts_first_in_any_order() {
        // lore's two copies of b4 relay mail: the relay's and the author's.
        let relay =
            b"Message-ID: <a@x>\nFrom: Dev via B4 Relay <devnull+dev.x@kernel.org>\nX-Original-From: Dev <dev@x>\n\
                      Subject: [PATCH] foo\n\nFrom: Dev <dev@x>\n\nFix.\n"
                .to_vec();
        let own = mail("a@x", "Dev <dev@x>", "Fix.\n");
        let first = relay.clone().min(own.clone());
        for order in [[&relay, &own], [&own, &relay]] {
            let mut s = Store::open_in_memory().unwrap();
            s.add(order[0]).unwrap();
            let added = s.add(order[1]).unwrap();
            assert_eq!(s.raw("a@x").unwrap().as_ref(), Some(&first));
            assert_eq!(added == Added::Replaced("a@x".into()), order[1] == &first);
            // Either copy says the same about who sent and wrote it.
            let m = &s.messages().unwrap()[0];
            assert_eq!((m.from.addr.as_str(), m.author.addr.as_str(), m.body.as_str()), ("dev@x", "dev@x", "Fix.\n"));
        }
    }

    #[test]
    fn export_import_merge() {
        let (a, b, out) = (tmp("a"), tmp("b"), tmp("out"));
        let mut sa = Store::open(&a).unwrap();
        sa.add_all([mail("1@x", "A <a@x>", "").as_slice(), mail("2@x", "A <a@x>", "").as_slice()]).unwrap();
        let mut sb = Store::open(&b).unwrap();
        sb.add(&mail("2@x", "A <a@x>", "")).unwrap();
        sb.add(&mail("3@x", "B <b@x>", "")).unwrap();
        sb.export(&out, Export::Full).unwrap();
        assert_eq!(sa.import(&out).unwrap(), 1);
        let mut ids: Vec<_> = sa.messages().unwrap().into_iter().map(|m| m.msgid).collect();
        ids.sort();
        assert_eq!(ids, ["1@x", "2@x", "3@x"]);
        assert_eq!(sa.import(&out).unwrap(), 0);
        for p in [a, b, out] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn refuses_other_files_and_newer_stores() {
        let p = tmp("other");
        let db = Connection::open(&p).unwrap();
        db.execute_batch("CREATE TABLE t (x)").unwrap();
        drop(db);
        assert!(matches!(Store::open(&p), Err(Error::NotAStore)));
        let mut s = Store::open_in_memory().unwrap();
        assert!(matches!(s.import(&p), Err(Error::NotAStore)));

        let q = tmp("newer");
        drop(Store::open(&q).unwrap());
        Connection::open(&q).unwrap().pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(Store::open(&q), Err(Error::TooNew(99, _))));
        assert!(matches!(s.import(&q), Err(Error::TooNew(99, _))));
        for f in [p, q] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn sync_effects_become_facts() {
        let mut s = Store::open_in_memory().unwrap();
        let dev = Subscription::new(Follow::Author, "Dev@X", "").unwrap();
        let usb = Subscription::new(Follow::List, "linux-usb", "").unwrap();
        let at = "2026-10-28T00:00:00Z".to_string();
        for e in [
            Effect::Store(vec![mail("a@x", "Dev <dev@x>", "")]),
            Effect::RecordSync { sub: dev.clone(), query: "q".into(), ran_at: at.clone() },
            Effect::Found { root: "a@x".into(), sub: usb.clone() },
            Effect::TagRoot { root: "a@x".into(), list: "linux-usb".into(), archive: String::new() },
            Effect::TagRoot { root: "a@x".into(), list: "gcc-patches".into(), archive: "https://x.org".into() },
            Effect::WantThread { root: "a@x".into(), at: at.clone(), archive: String::new() },
            Effect::WantThread { root: "b@x".into(), at: at.clone(), archive: "https://x.org".into() },
            Effect::Fetched { root: "a@x".into(), at: at.clone(), archive: String::new() },
            Effect::Log("ignored".into()),
        ] {
            s.apply(&e).unwrap();
        }
        let f = s.facts().unwrap();
        assert_eq!(f.fetched.get("a@x"), Some(&at));
        // A fetch clears what was owed; what was not fetched stays owed.
        assert_eq!(f.pending.iter().collect::<Vec<_>>(), vec!["b@x"]);
        assert_eq!(f.lists["a@x"].iter().collect::<Vec<_>>(), vec!["gcc-patches", "linux-usb"]);
        assert_eq!(f.archives["a@x"].iter().collect::<Vec<_>>(), vec!["", "https://x.org"]);
        assert_eq!((f.thread_archive["a@x"].as_str(), f.thread_archive["b@x"].as_str()), ("", "https://x.org"));
        assert_eq!(s.last_syncs().unwrap()[&dev], at);
        assert!(f.found_by("a@x", &usb) && !f.found_by("a@x", &dev));
        assert_eq!(s.authors().unwrap(), vec!["dev@x"]);
        assert_eq!(s.messages().unwrap().len(), 1);
    }

    #[test]
    fn drafts_are_yours() {
        let mut s = Store::open_in_memory().unwrap();
        let d = Draft {
            msgid: "d1@x".into(),
            family: "me@x\0title:a".into(),
            kind: "tag".into(),
            raw: b"From: me@x\n\nReviewed-by: Me <me@x>\n".to_vec(),
            created_at: "2026-10-29T00:00:00Z".into(),
            sent_at: None,
        };
        s.save_draft(&d).unwrap();
        s.save_draft(&Draft { raw: b"edited".to_vec(), ..d.clone() }).unwrap();
        assert_eq!(s.drafts().unwrap(), [Draft { raw: b"edited".to_vec(), ..d.clone() }]);
        assert!(s.mark_sent("d1@x", "2026-10-29T01:00:00Z").unwrap() && !s.mark_sent("nope@x", "t").unwrap());
        assert_eq!(s.drafts().unwrap()[0].sent_at.as_deref(), Some("2026-10-29T01:00:00Z"));
        // A full export carries them, and they merge; a public one does not.
        let (full, public) = (tmp("drafts-full"), tmp("drafts-public"));
        s.export(&full, Export::Full).unwrap();
        s.export(&public, Export::Public).unwrap();
        let mut other = Store::open_in_memory().unwrap();
        other.import(&full).unwrap();
        assert_eq!(other.drafts().unwrap().len(), 1);
        let mut fresh = Store::open_in_memory().unwrap();
        fresh.import(&public).unwrap();
        assert!(fresh.drafts().unwrap().is_empty());
        assert!(s.remove_draft("d1@x").unwrap() && s.drafts().unwrap().is_empty());
        for f in [full, public] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn a_store_from_before_keeps_its_syncs_as_authors() {
        let p = tmp("v4");
        let old = Connection::open(&p).unwrap();
        old.pragma_update(None, "application_id", APPLICATION_ID).unwrap();
        for step in &MIGRATIONS[..4] {
            old.execute_batch(step).unwrap();
        }
        old.pragma_update(None, "user_version", 4).unwrap();
        old.execute("INSERT INTO syncs VALUES ('dev@x', 'linux-usb', 'q', '2026-10-01T00:00:00Z')", []).unwrap();
        drop(old);
        let dev = Subscription::new(Follow::Author, "dev@x", "linux-usb").unwrap();
        // Imported into a new store, and opened (migrated) in place, alike.
        let mut fresh = Store::open_in_memory().unwrap();
        fresh.import(&p).unwrap();
        assert_eq!(fresh.last_syncs().unwrap()[&dev], "2026-10-01T00:00:00Z");
        let s = Store::open(&p).unwrap();
        assert_eq!(s.last_syncs().unwrap()[&dev], "2026-10-01T00:00:00Z");
        assert_eq!(s.authors().unwrap(), ["dev@x"]);
        drop(s);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn import_merges_facts_and_keeps_your_decisions() {
        let (a, b) = (tmp("fa"), tmp("fb"));
        let row = |at: &str, commit: Option<&str>| TreeRow {
            tree: "next".into(),
            role: Role::Next,
            ref_sha: "f".repeat(40),
            commit_sha: commit.map(str::to_string),
            how: Some("patch-id".into()),
            reverted_by: None,
            checked_at: at.into(),
        };
        let mut mine = Store::open(&a).unwrap();
        mine.apply(&Effect::Fetched { root: "r@x".into(), at: "2026-10-01T00:00:00Z".into(), archive: String::new() })
            .unwrap();
        mine.record_tree_result("p@x", &row("2026-10-01T00:00:00Z", None)).unwrap();
        mine.close_family("dev@x\0fam", "mine", "2026-10-01T00:00:00Z").unwrap();
        let mut theirs = Store::open(&b).unwrap();
        theirs
            .apply(&Effect::Fetched { root: "r@x".into(), at: "2026-10-20T00:00:00Z".into(), archive: String::new() })
            .unwrap();
        theirs
            .apply(&Effect::WantThread {
                root: "owed@x".into(),
                at: "2026-10-20T00:00:00Z".into(),
                archive: String::new(),
            })
            .unwrap();
        theirs.record_tree_result("p@x", &row("2026-10-20T00:00:00Z", Some(&"c".repeat(40)))).unwrap();
        theirs.close_family("dev@x\0fam", "theirs", "2026-10-20T00:00:00Z").unwrap();
        theirs.close_family("dev@x\0other", "theirs", "2026-10-20T00:00:00Z").unwrap();
        drop(theirs);
        mine.import(&b).unwrap();
        let f = mine.facts().unwrap();
        assert_eq!(f.fetched["r@x"], "2026-10-20T00:00:00Z");
        assert!(f.pending.is_empty());
        assert_eq!(f.trees["p@x"][0].commit_sha.as_deref(), Some("c".repeat(40).as_str()));
        assert_eq!((f.closed["dev@x\0fam"].as_str(), f.closed["dev@x\0other"].as_str()), ("mine", "theirs"));
        for p in [a, b] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn settings_and_subscriptions() {
        let mut s = Store::open_in_memory().unwrap();
        s.set_setting("ping_days", "3").unwrap();
        s.set_setting("ping_days", "5").unwrap();
        s.set_setting("archive", "\"https://x.org\"").unwrap();
        assert_eq!(
            s.settings().unwrap(),
            [("archive".into(), "\"https://x.org\"".into()), ("ping_days".into(), "5".into())]
        );
        assert!(s.unset_setting("ping_days").unwrap());
        assert!(!s.unset_setting("ping_days").unwrap());

        let me = |list: &str| Subscription {
            kind: Follow::Author,
            value: "me@x.org".into(),
            list: list.into(),
            archive: String::new(),
        };
        assert!(s.subscribe(&Subscription { value: "Me@X.org".into(), ..me("all") }, "t").unwrap());
        assert!(!s.subscribe(&me("all"), "t").unwrap());
        s.subscribe(&me("linux-usb"), "t").unwrap();
        assert_eq!(s.subscriptions().unwrap(), [me("all"), me("linux-usb")]);
        assert_eq!(s.unsubscribe(Follow::Author, "ME@x.org", Some("all"), None).unwrap(), 1);
        assert_eq!(s.subscriptions().unwrap(), [me("linux-usb")]);
        assert_eq!(s.unsubscribe(Follow::Author, "me@x.org", None, None).unwrap(), 1);
        assert!(s.subscriptions().unwrap().is_empty());

        assert!(s.add_source("/m/INBOX", "t").unwrap());
        assert!(!s.add_source("/m/INBOX", "t").unwrap());
        s.mark_read("/m/INBOX", "u").unwrap();
        assert_eq!(s.sources().unwrap(), [("/m/INBOX".into(), Some("u".into()))]);
        assert!(s.remove_source("/m/INBOX").unwrap());
    }

    #[test]
    fn exports_leave_out_the_machine_and_public_ones_your_decisions() {
        let (a, full, public) = (tmp("ea"), tmp("efull"), tmp("epublic"));
        let mut s = Store::open(&a).unwrap();
        s.add(&mail("1@x", "A <a@x>", "")).unwrap();
        s.set_setting("identities", r#"["a@x"]"#).unwrap();
        let sub =
            Subscription { kind: Follow::Author, value: "a@x".into(), list: "all".into(), archive: String::new() };
        s.subscribe(&sub, "t").unwrap();
        s.add_source("/home/a/Mail", "t").unwrap();
        s.set_machine("bridge-token", "s3cret-token").unwrap();
        s.db.execute(
            "INSERT INTO trees VALUES ('next', '/home/a/linux', 'refs/patchtrack/next', NULL, NULL, 'next')",
            [],
        )
        .unwrap();
        s.close_family("a@x\0fam", "folded into Bob's series", "t").unwrap();
        s.mark_local(["1@x"], "t").unwrap();
        s.export(&full, Export::Full).unwrap();
        s.export(&public, Export::Public).unwrap();

        let count = |db: &Connection, table: &str| -> i64 {
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0)).unwrap()
        };
        let f = Connection::open(&full).unwrap();
        assert_eq!((count(&f, "sources"), count(&f, "trees"), count(&f, "machine")), (0, 0, 0));
        assert_eq!((count(&f, "settings"), count(&f, "subscriptions"), count(&f, "messages")), (1, 1, 1));
        let p = Connection::open(&public).unwrap();
        assert_eq!((count(&p, "settings"), count(&p, "subscriptions"), count(&p, "sources")), (0, 1, 0));
        let reason: String = p.query_row("SELECT reason FROM closed", [], |r| r.get(0)).unwrap();
        let local: Option<String> = p.query_row("SELECT local_at FROM threads", [], |r| r.get(0)).unwrap();
        assert_eq!((reason.as_str(), local), ("", None));
        // Nothing of what was deleted is left in the file's free pages.
        let bytes = std::fs::read(&public).unwrap();
        for secret in [&b"folded into Bob"[..], b"/home/a/Mail", b"/home/a/linux", b"identities", b"s3cret-token"] {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret), "{}", String::from_utf8_lossy(secret));
        }
        for path in [a, full, public] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn import_fills_settings_and_subscriptions_only_where_you_have_none() {
        let (a, b, c) = (tmp("sa"), tmp("sb"), tmp("sc"));
        let follow = |who: &str| Subscription {
            kind: Follow::Author,
            value: who.into(),
            list: "all".into(),
            archive: String::new(),
        };
        let mut theirs = Store::open(&b).unwrap();
        theirs.set_setting("ping_days", "3").unwrap();
        theirs.set_setting("archive", "\"https://theirs.org\"").unwrap();
        theirs.subscribe(&follow("them@x"), "t").unwrap();
        theirs.subscribe(&follow("other@x"), "t").unwrap();
        theirs.add_source("/their/Mail", "t").unwrap();
        drop(theirs);

        let mut mine = Store::open(&a).unwrap();
        mine.set_setting("archive", "\"https://mine.org\"").unwrap();
        mine.subscribe(&follow("me@x"), "t").unwrap();
        mine.import(&b).unwrap();
        assert_eq!(
            mine.settings().unwrap(),
            [("archive".into(), "\"https://mine.org\"".into()), ("ping_days".into(), "3".into())]
        );
        assert_eq!(mine.subscriptions().unwrap(), [follow("me@x")]);
        assert!(mine.sources().unwrap().is_empty());

        // A new store takes them all.
        let mut fresh = Store::open(&c).unwrap();
        fresh.import(&b).unwrap();
        assert_eq!(fresh.subscriptions().unwrap(), [follow("other@x"), follow("them@x")]);
        for p in [a, b, c] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn reparse_rewrites_rows_from_another_parser() {
        let mut s = Store::open_in_memory().unwrap();
        s.add(&mail("a@x", "Dev <dev@x>", "Fix.\n")).unwrap();
        s.db.execute("UPDATE messages SET parsed_with = 0, subject = 'stale', from_addr = ''", []).unwrap();
        assert_eq!(s.reparse().unwrap(), 1);
        let m = &s.messages().unwrap()[0];
        assert_eq!((m.subject.as_str(), m.from.addr.as_str()), ("[PATCH] foo: bar", "dev@x"));
        assert_eq!(s.reparse().unwrap(), 0);
    }
}
