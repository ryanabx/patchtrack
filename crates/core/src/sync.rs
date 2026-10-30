//! Syncing with a public-inbox archive, as a state machine that does no I/O.
//!
//! The core decides what to fetch and when; the host fetches it, with
//! whatever it has (the binary's HTTP client; an archive in memory in the
//! tests), and hands the answer back. Every politeness rule lives here, so every host obeys
//! it: one search per subscription (an author, a list or a query, in an
//! inbox), searched in windows a result cap cannot truncate; one request per
//! thread; a delay between requests;
//! backoff on 429 and 5xx, honouring `Retry-After`; and a bot check
//! reported, never worked around.
//!
//! ```text
//! let mut sync = Sync::new(plan, known);
//! while let Some(effect) = sync.next(now) {
//!     match effect {
//!         Effect::Fetch(req) => { wait until req.not_before; sync.receive(req.id, fetch(&req), now) }
//!         other => apply(other),   // store mail and facts, print logs
//!     }
//! }
//! ```

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Read;

use crate::mail::{self, Message};
use crate::mbox;
use crate::rules::Rules;
use crate::series::{self, Facts, Series};
use crate::settings::{Follow, Subscription};

/// A window with this many results is searched again in halves. Exports of
/// 1652 messages have been complete (measured 2026-10-26), so no cap can
/// bite below it.
pub const SPLIT_AT: usize = 1000;
const RETRY_STATUS: [u16; 5] = [429, 500, 502, 503, 504];
const DAY: i64 = 86_400;

/// What to sync.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The archive's base URL, e.g. https://lore.kernel.org.
    pub archive: String,
    /// How subscriptions and facts name this archive: "" for the archive
    /// setting's, else its URL. One sync serves one archive.
    pub archive_key: String,
    /// What to sync, in order. Authors, lists and queries are searched; a
    /// thread is fetched whole.
    pub searches: Vec<Subscription>,
    /// An explicit start (epoch seconds): search from here, not incrementally.
    pub since: Option<i64>,
    /// Where a first sync of an author starts.
    pub first_since: i64,
    /// Where a first sync of a list or a query starts: they find far more.
    pub first_since_wide: i64,
    /// Refetch every thread with mail in the window, not only what changed.
    pub full: bool,
    pub window_days: i64,
    /// Incremental syncs also refetch open series not fetched for this long.
    pub refresh_days: i64,
    /// Fetch the threads now; otherwise they stay owed to the next sync.
    pub threads: bool,
    /// Fetch the threads a list's or a query's search finds, as an author's
    /// are. Otherwise what the search returned is all there is of them
    /// until one is asked for (opened, or followed as a thread).
    pub wide_threads: bool,
    /// Threads to fetch by any of their messages, each from its inbox:
    /// (message-id, inbox), as a listed thread is opened.
    pub messages: Vec<(String, String)>,
    /// One page of an inbox's threads, and nothing else.
    pub listing: Option<Listing>,
    /// Read the archive's list of inboxes (its `manifest.js.gz`), and
    /// nothing else.
    pub inboxes: bool,
    pub delay_ms: u64,
    pub retries: u32,
    /// Search a window again in halves when it returns this many.
    pub split_at: usize,
}

impl Plan {
    /// Searches every list for each author.
    pub fn new(archive: &str, authors: Vec<String>, first_since: i64) -> Plan {
        let author = |a: String| Subscription {
            kind: Follow::Author,
            value: a.to_lowercase(),
            list: "all".into(),
            archive: String::new(),
        };
        Plan {
            archive: archive.trim_end_matches('/').to_string(),
            archive_key: String::new(),
            searches: authors.into_iter().map(author).collect(),
            since: None,
            first_since,
            first_since_wide: first_since,
            full: false,
            window_days: 30,
            refresh_days: 7,
            threads: true,
            wide_threads: false,
            messages: vec![],
            listing: None,
            inboxes: false,
            delay_ms: 1000,
            retries: 3,
            split_at: SPLIT_AT,
        }
    }
}

/// What the store knew when the sync started.
#[derive(Debug, Clone, Default)]
pub struct Known {
    pub messages: Vec<Message>,
    /// When each subscription was last synced.
    pub last_sync: BTreeMap<Subscription, String>,
    pub facts: Facts,
    /// Message-ids new from local mail just now: their threads are refetched.
    pub local_new: HashSet<String>,
    pub rules: Rules,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    /// The search export answers POST.
    Post,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub method: Method,
    pub url: String,
    /// Milliseconds since the epoch; send it no earlier.
    pub not_before: u64,
}

/// What became of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Response {
        status: u16,
        content_type: String,
        retry_after: Option<String>,
        body: Vec<u8>,
    },
    /// No answer: a timeout, a reset connection.
    Failed(String),
}

/// What the host must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Send this request (no earlier than `not_before`), then call [`Sync::receive`].
    Fetch(Request),
    /// Add these messages, as received, to the store.
    Store(Vec<Vec<u8>>),
    /// A subscription was synced; the next sync of it is incremental.
    RecordSync { sub: Subscription, query: String, ran_at: String },
    /// A sync of this list, on this archive, found this thread.
    TagRoot { root: String, list: String, archive: String },
    /// This subscription's sync found this thread: it is one you follow.
    Found { root: String, sub: Subscription },
    /// This thread is owed a fetch from this archive until a [`Effect::Fetched`] for it.
    WantThread { root: String, at: String, archive: String },
    /// The whole thread was fetched from this archive.
    Fetched { root: String, at: String, archive: String },
    /// The archive lists these inboxes (all of them: what it listed
    /// before and no longer does is gone).
    Inboxes { archive: String, at: String, inboxes: Vec<InboxInfo> },
    /// A page of an inbox's threads, as the archive listed them, newest
    /// first, from `offset`.
    Listed { archive: String, inbox: String, offset: usize, at: String, entries: Vec<Listed> },
    /// A line of progress for the user.
    Log(String),
}

/// A page of an inbox's threads to ask for: what its search export cannot
/// do, the archive's Atom feed of a search can: the newest `limit` threads
/// (one entry each, its newest message) from `offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub inbox: String,
    pub query: String,
    pub offset: usize,
    pub limit: usize,
}

/// A thread an archive listed: its newest message that matched, as the
/// Atom feed gives it. Not mail: a listing, kept apart from the messages.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Listed {
    pub msgid: String,
    pub subject: String,
    /// "Name <address>".
    pub author: String,
    pub date: String,
    pub in_reply_to: Option<String>,
}

/// The entries of a public-inbox Atom feed (a search's, with `x=A`), in
/// its order. Its text is HTML-escaped; a message-id is the last part of
/// its link. An entry without one is skipped.
pub fn atom_entries(xml: &str) -> Vec<Listed> {
    let tag = |e: &str, name: &str| -> Option<String> {
        let open = e.find(&format!("<{name}"))?;
        let start = open + e[open..].find('>')? + 1;
        let end = start + e[start..].find(&format!("</{name}>"))?;
        Some(unescape_html(&e[start..end]))
    };
    let msgid_of = |href: &str| -> Option<String> {
        let last = href.trim_end_matches('/').rsplit('/').next().filter(|l| !l.is_empty())?;
        Some(percent_decode(last))
    };
    let attr = |e: &str, element: &str, name: &str| -> Option<String> {
        let at = e.find(&format!("<{element}"))?;
        let rest = &e[at..];
        let rest = &rest[..rest.find('>')?];
        let a = rest.find(&format!("{name}=\""))? + name.len() + 2;
        Some(rest[a..a + rest[a..].find('"')?].to_string())
    };
    let mut out = vec![];
    for e in xml.split("<entry>").skip(1) {
        let e = e.split("</entry>").next().unwrap_or(e);
        let Some(msgid) = attr(e, "link", "href").and_then(|h| msgid_of(&unescape_html(&h))) else { continue };
        let name = tag(e, "name").unwrap_or_default();
        let email = tag(e, "email").unwrap_or_default();
        let author = if name.is_empty() { email } else { format!("{name} <{email}>") };
        out.push(Listed {
            msgid,
            subject: tag(e, "title").unwrap_or_default(),
            author,
            date: tag(e, "updated").unwrap_or_default(),
            in_reply_to: attr(e, "thr:in-reply-to", "href").and_then(|h| msgid_of(&unescape_html(&h))),
        });
    }
    out
}

/// Undo HTML's escapes: the named five and numeric ones.
fn unescape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e < 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16).ok())
                .unwrap_or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out + rest
}

/// "%2F" to "/", as a message-id in an archive's URL is quoted.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match hex {
            Some(v) => {
                out.push(v);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An inbox an archive lists, as its front page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct InboxInfo {
    pub name: String,
    pub description: String,
    /// When it last changed (UTC), if the archive says.
    pub updated: Option<String>,
}

/// The inboxes in a grokmirror manifest, as public-inbox serves it
/// (`manifest.js.gz`): each repository is "/NAME/git/N.git" (an epoch) or
/// "/NAME", and an inbox changed when its latest epoch did. Sorted by name.
pub fn manifest_inboxes(json: &[u8]) -> Result<Vec<InboxInfo>, String> {
    let v: serde_json::Value = serde_json::from_slice(json).map_err(|e| format!("not a manifest: {e}"))?;
    let repos = v.as_object().ok_or("not a manifest: not an object")?;
    let mut out: BTreeMap<String, (i64, String)> = BTreeMap::new();
    for (path, repo) in repos {
        let Some(name) = path.trim_start_matches('/').split('/').next().filter(|n| !n.is_empty()) else { continue };
        let modified = repo["modified"].as_i64().unwrap_or(0);
        // public-inbox names each epoch's repository: "Linux USB [epoch 1]".
        let description = repo["description"].as_str().unwrap_or_default().trim();
        let description = match description.rsplit_once(" [epoch ") {
            Some((d, n)) if n.trim_end_matches(']').bytes().all(|b| b.is_ascii_digit()) => d.trim(),
            _ => description,
        }
        .to_string();
        let e = out.entry(name.to_string()).or_insert((i64::MIN, String::new()));
        if modified >= e.0 {
            e.0 = modified;
            if !description.is_empty() {
                e.1 = description;
            }
        } else if e.1.is_empty() {
            e.1 = description;
        }
    }
    Ok(out
        .into_iter()
        .map(|(name, (modified, description))| InboxInfo {
            name,
            description,
            updated: (modified > 0).then(|| mail::utc(modified)),
        })
        .collect())
}

enum Job {
    Search(Subscription),
    Threads,
    Inboxes,
    Listing,
}

/// What a request was for.
#[derive(Debug, Clone)]
enum Kind {
    Window,
    /// The archive's manifest.
    Manifest,
    /// A thread, by its root or by any message in it.
    Thread(String, Asked),
    /// A page of an inbox's threads.
    Listing,
}

/// How a thread was asked for.
#[derive(Debug, Clone)]
enum Asked {
    /// By its root.
    Root,
    /// By any message in it, for a followed thread not yet in the store,
    /// whose root its answer shows.
    Followed(Subscription),
    /// By any message in it, opened from a listing of an inbox.
    Message,
}

struct Pending {
    kind: Kind,
    method: Method,
    url: String,
    attempt: u32,
}

struct Searching {
    sub: Subscription,
    query: String,
    incremental: bool,
    last: Option<String>,
    windows: VecDeque<(i64, i64)>,
    window: (i64, i64),
    found: Vec<Vec<u8>>,
    ids: Vec<String>,
    seen: HashSet<String>,
}

pub struct Sync {
    plan: Plan,
    known: Known,
    jobs: VecDeque<Job>,
    out: VecDeque<Effect>,
    search: Option<Searching>,
    threads: VecDeque<(String, (String, Asked))>,
    thread_total: usize,
    /// Threads to fetch, and why; with the followed thread a root stands for.
    roots: BTreeMap<String, (String, Asked)>,
    /// The inbox a thread was found in, to fetch it from there: not every
    /// archive serves lore's "all".
    inboxes: HashMap<String, String>,
    requests: HashMap<u64, Pending>,
    inflight: Option<u64>,
    next_id: u64,
    last_request: Option<u64>,
    search_failed: bool,
    thread_failures: usize,
    fetching: bool,
    done: bool,
}

impl Sync {
    pub fn new(plan: Plan, known: Known) -> Sync {
        let mut jobs: VecDeque<Job> =
            plan.searches.iter().filter(|s| s.kind != Follow::Thread).cloned().map(Job::Search).collect();
        jobs.push_back(Job::Threads);
        if plan.inboxes {
            jobs = VecDeque::from([Job::Inboxes]);
        } else if plan.listing.is_some() {
            jobs = VecDeque::from([Job::Listing]);
        }
        Sync {
            plan,
            known,
            jobs,
            out: VecDeque::new(),
            search: None,
            threads: VecDeque::new(),
            thread_total: 0,
            roots: BTreeMap::new(),
            inboxes: HashMap::new(),
            requests: HashMap::new(),
            inflight: None,
            next_id: 1,
            last_request: None,
            search_failed: false,
            thread_failures: 0,
            fetching: false,
            done: false,
        }
    }

    /// Whether anything failed: a search (it recorded nothing) or a thread
    /// fetch (it stays owed). Meaningful once [`Sync::next`] returns None.
    pub fn failed(&self) -> bool {
        self.search_failed || self.thread_failures > 0
    }

    /// The next thing to do, or None when the sync is over. After a
    /// [`Effect::Fetch`], call [`Sync::receive`] before calling this again.
    pub fn next(&mut self, now_ms: u64) -> Option<Effect> {
        assert!(self.inflight.is_none(), "receive() the answer to the last request first");
        loop {
            if let Some(e) = self.out.pop_front() {
                if let Effect::Fetch(req) = &e {
                    self.inflight = Some(req.id);
                }
                return Some(e);
            }
            if self.done {
                return None;
            }
            self.advance(now_ms);
        }
    }

    fn request(&mut self, kind: Kind, method: Method, url: String, now_ms: u64) {
        let not_before = self.last_request.map_or(now_ms, |t| (t + self.plan.delay_ms).max(now_ms));
        let id = self.next_id;
        self.next_id += 1;
        self.requests.insert(id, Pending { kind, method, url: url.clone(), attempt: 0 });
        self.out.push_back(Effect::Fetch(Request { id, method, url, not_before }));
    }

    /// Queue the next request, or start the next job.
    fn advance(&mut self, now_ms: u64) {
        if let Some(s) = &mut self.search {
            if let Some(w) = s.windows.pop_front() {
                s.window = w;
                let q = if s.query.is_empty() {
                    format!("rt:{}..{}", w.0, w.1)
                } else {
                    format!("({}) AND rt:{}..{}", s.query, w.0, w.1)
                };
                let url = format!("{}/{}/?x=m&q={}", self.plan.archive, s.sub.list, form_quote(&q));
                self.request(Kind::Window, Method::Post, url, now_ms);
            } else {
                self.finish_search(now_ms);
            }
            return;
        }
        if self.fetching {
            if let Some((root, (why, sub))) = self.threads.pop_front() {
                let i = self.thread_total - self.threads.len();
                self.out.push_back(Effect::Log(format!("[{i}/{}] {root} ({why})", self.thread_total)));
                let followed = if let Asked::Followed(s) = &sub { Some(s) } else { None };
                let inbox = self.inbox(&root, followed);
                let url = format!("{}/{inbox}/{}/t.mbox.gz", self.plan.archive, path_quote(&root));
                self.request(Kind::Thread(root, sub), Method::Get, url, now_ms);
            } else {
                self.finish();
            }
            return;
        }
        match self.jobs.pop_front() {
            Some(Job::Search(sub)) => self.start_search(sub, now_ms),
            Some(Job::Threads) => self.start_threads(now_ms),
            Some(Job::Listing) => {
                let l = self.plan.listing.clone().expect("a listing job has a listing");
                let url = format!(
                    "{}/{}/?q={}&x=A&t=1&l={}&o={}",
                    self.plan.archive,
                    l.inbox,
                    form_quote(&l.query),
                    l.limit,
                    l.offset
                );
                self.request(Kind::Listing, Method::Get, url, now_ms);
            }
            Some(Job::Inboxes) => {
                let url = format!("{}/manifest.js.gz", self.plan.archive);
                self.request(Kind::Manifest, Method::Get, url, now_ms);
            }
            None => self.finish(),
        }
    }

    /// Where to fetch a thread: the inbox a search of this sync found it in,
    /// else one a sync found it in before, else the followed thread's, else "all".
    fn inbox(&self, root: &str, sub: Option<&Subscription>) -> String {
        let before = self.known.facts.lists.get(root).and_then(|l| l.iter().find(|l| *l != "all"));
        self.inboxes
            .get(root)
            .or(before)
            .cloned()
            .or_else(|| sub.map(|s| s.list.clone()))
            .unwrap_or_else(|| "all".into())
    }

    fn start_search(&mut self, sub: Subscription, now_ms: u64) {
        let last = self.known.last_sync.get(&sub).cloned();
        let incremental = last.is_some() && self.plan.since.is_none() && !self.plan.full;
        let first = if sub.kind == Follow::Author { self.plan.first_since } else { self.plan.first_since_wide };
        let since = match (&last, incremental) {
            // A day of overlap: lore indexes some mail late.
            (Some(l), true) => series::parse_utc(l).unwrap_or(first) - DAY,
            _ => self.plan.since.unwrap_or(first),
        };
        let until = secs(now_ms);
        let step = self.plan.window_days.max(1) * DAY;
        let mut windows = VecDeque::new();
        let mut start = since;
        while start < until {
            let end = (start + step).min(until);
            windows.push_back((start, end));
            start = end;
        }
        let query = match sub.kind {
            // tc: finds other people's replies, which nearly always copy the author.
            Follow::Author => format!("f:{0} OR tc:{0}", sub.value),
            // Everything in the inbox: the windows bound it.
            Follow::List | Follow::Thread => String::new(),
            Follow::Query => sub.value.clone(),
        };
        self.search = Some(Searching {
            sub,
            query,
            incremental,
            last,
            windows,
            window: (0, 0),
            found: vec![],
            ids: vec![],
            seen: HashSet::new(),
        });
    }

    /// Every window answered: store the mail, record the sync, choose threads.
    fn finish_search(&mut self, now_ms: u64) {
        let s = self.search.take().expect("a search");
        let known: HashSet<&str> = self.known.messages.iter().map(|m| m.msgid.as_str()).collect();
        let new: HashSet<String> = s.ids.iter().filter(|i| !known.contains(i.as_str())).cloned().collect();
        let parsed: Vec<Message> =
            s.found.iter().filter_map(|r| Message::parse(r)).filter(|m| new.contains(&m.msgid)).collect();
        self.out.push_back(Effect::Store(s.found));
        self.out.push_back(Effect::RecordSync {
            sub: s.sub.clone(),
            query: s.query.clone(),
            ran_at: mail::utc(secs(now_ms)),
        });
        self.known.messages.extend(parsed);

        let refresh = secs(now_ms) - self.plan.refresh_days * DAY;
        let hit: HashSet<&str> = if s.incremental {
            new.iter().map(String::as_str).collect()
        } else {
            s.ids.iter().map(String::as_str).collect()
        };
        let found = series::build(&self.known.messages, &self.known.facts, &self.known.rules);
        // An author's thread is one of their series; a list's or a query's,
        // any series its search found mail of, now or before.
        let sub = &s.sub;
        // A list's or a query's threads are fetched when asked for, unless the plan says otherwise.
        let fetch = sub.kind == Follow::Author || self.plan.wide_threads;
        let theirs = |x: &Series| match sub.kind {
            Follow::Author => x.is_by(&sub.value),
            Follow::List => x.lists.contains(&sub.list) || self.known.facts.found_by(&x.root, sub),
            Follow::Query | Follow::Thread => self.known.facts.found_by(&x.root, sub),
        };
        for x in found.values() {
            let ids = std::iter::once(&x.root).chain(x.patches.values()).chain(x.replies.iter().map(|r| &r.msgid));
            let mut ids = ids.into_iter();
            let touched = if sub.kind == Follow::Author {
                theirs(x) && ids.any(|id| hit.contains(id.as_str()) || self.known.local_new.contains(id))
            } else {
                ids.any(|id| hit.contains(id.as_str()))
            };
            if touched {
                let why = if s.incremental { "new mail" } else { "in the search" };
                if fetch {
                    self.roots.entry(x.root.clone()).or_insert_with(|| (why.into(), Asked::Root));
                }
                self.out.push_back(Effect::TagRoot {
                    root: x.root.clone(),
                    list: sub.list.clone(),
                    archive: self.plan.archive_key.clone(),
                });
                if sub.list != "all" {
                    self.inboxes.entry(x.root.clone()).or_insert_with(|| sub.list.clone());
                }
                if sub.kind != Follow::Author {
                    self.out.push_back(Effect::Found { root: x.root.clone(), sub: sub.clone() });
                }
            } else if fetch && s.incremental && theirs(x) && stale(x, refresh) {
                let why = format!("not fetched for {} days", self.plan.refresh_days);
                self.roots.entry(x.root.clone()).or_insert((why, Asked::Root));
            }
        }
        let kind = match (&s.last, s.incremental) {
            (Some(l), true) => format!("since the last sync ({})", l.get(..16).unwrap_or(l)),
            _ => "full window".into(),
        };
        let r#in = if sub.kind == Follow::List { String::new() } else { format!(" in {}", sub.list) };
        let label = match sub.kind {
            Follow::Author => sub.value.clone(),
            _ => sub.label(),
        };
        self.out.push_back(Effect::Log(format!("{label}: {} messages ({} new){in}, {kind}", s.ids.len(), new.len())));
    }

    /// Followed threads: fetched on every sync while open, else when stale.
    fn followed_threads(&mut self, now_ms: u64) {
        let subs: Vec<Subscription> = self.plan.searches.iter().filter(|s| s.kind == Follow::Thread).cloned().collect();
        if subs.is_empty() {
            return;
        }
        let found = series::build(&self.known.messages, &self.known.facts, &self.known.rules);
        let refresh = secs(now_ms) - self.plan.refresh_days * DAY;
        let now = mail::utc(secs(now_ms));
        for sub in subs {
            let id = &sub.value;
            let has = |x: &&Series| {
                &x.root == id || x.patches.values().any(|p| p == id) || x.replies.iter().any(|r| &r.msgid == id)
            };
            let Some(x) = found.values().find(has) else {
                // Not in the store yet: fetch it by this message, and learn its root from the answer.
                self.roots
                    .entry(id.clone())
                    .or_insert_with(|| ("a thread you follow".into(), Asked::Followed(sub.clone())));
                continue;
            };
            self.out.push_back(Effect::Found { root: x.root.clone(), sub: sub.clone() });
            let open = !x.state.done() && x.superseded_by.is_none();
            if open || stale(x, refresh) || self.plan.full || self.plan.since.is_some() {
                self.roots.entry(x.root.clone()).or_insert_with(|| ("a thread you follow".into(), Asked::Root));
            }
            self.out.push_back(Effect::RecordSync { sub, query: String::new(), ran_at: now.clone() });
        }
    }

    fn start_threads(&mut self, now_ms: u64) {
        self.followed_threads(now_ms);
        // Opened from a listing: by the message, from its inbox.
        for (msgid, inbox) in self.plan.messages.clone() {
            self.inboxes.insert(msgid.clone(), inbox);
            self.roots.entry(msgid).or_insert_with(|| ("opened".into(), Asked::Message));
        }
        let facts = &self.known.facts;
        // Owed from this archive: another archive's sync fetches its own.
        let ours = |root: &&String| facts.thread_archive.get(*root).map_or("", String::as_str) == self.plan.archive_key;
        for root in facts.pending.iter().filter(ours) {
            self.roots.entry(root.clone()).or_insert_with(|| ("an earlier fetch did not happen".into(), Asked::Root));
        }
        let now = mail::utc(secs(now_ms));
        for root in self.roots.keys() {
            let archive = self.plan.archive_key.clone();
            self.out.push_back(Effect::WantThread { root: root.clone(), at: now.clone(), archive });
        }
        if !self.plan.threads {
            if !self.roots.is_empty() {
                self.out
                    .push_back(Effect::Log(format!("{} threads left for the next sync to fetch", self.roots.len())));
            }
            self.finish();
            return;
        }
        self.threads = std::mem::take(&mut self.roots).into_iter().collect();
        self.thread_total = self.threads.len();
        self.fetching = true;
    }

    fn finish(&mut self) {
        if self.thread_failures > 0 {
            let n = self.thread_failures;
            self.out.push_back(Effect::Log(format!("{n} thread fetches failed; the next sync retries them")));
        }
        self.fetching = false;
        self.done = true;
    }

    /// The answer to the request in flight.
    pub fn receive(&mut self, id: u64, outcome: Outcome, now_ms: u64) {
        assert_eq!(self.inflight.take(), Some(id), "an answer to a request not in flight");
        let mut p = self.requests.remove(&id).expect("a request we made");
        self.last_request = Some(now_ms);
        // (retry?, why, Retry-After)
        let result: Result<Vec<u8>, (bool, String, Option<String>)> = match outcome {
            Outcome::Failed(why) => Err((true, why, None)),
            // public-inbox answers an empty search with 404.
            Outcome::Response { status: 404, .. } => Ok(vec![]),
            Outcome::Response { status, retry_after, .. } if RETRY_STATUS.contains(&status) => {
                Err((true, format!("HTTP {status}"), retry_after))
            }
            Outcome::Response { status, .. } if !(200..300).contains(&status) => {
                Err((false, format!("HTTP {status}"), None))
            }
            Outcome::Response { content_type, .. } if content_type.contains("html") => {
                // Anubis or an error page: report it, never work around it.
                Err((false, format!("got {content_type}, not an mbox (blocked by a bot check?)"), None))
            }
            Outcome::Response { body, .. } => gunzip(body).map_err(|e| (false, e, None)),
        };
        match result {
            Ok(body) => self.answered(p.kind, &body, now_ms),
            Err((true, why, retry_after)) if p.attempt < self.plan.retries => {
                let mut backoff = (5u64 << p.attempt).min(120);
                if let Some(s) = retry_after.as_deref().and_then(|r| r.trim().parse::<u64>().ok()) {
                    backoff = backoff.max(s.min(600));
                }
                p.attempt += 1;
                let req = Request { id, method: p.method, url: p.url.clone(), not_before: now_ms + backoff * 1000 };
                self.requests.insert(id, p);
                self.out.push_front(Effect::Fetch(req));
                self.out.push_front(Effect::Log(format!("  {why}; retrying in {backoff} s")));
            }
            Err((_, why, _)) => self.request_failed(p.kind, format!("{}: {why}", p.url)),
        }
    }

    fn answered(&mut self, kind: Kind, body: &[u8], now_ms: u64) {
        let raws = mbox::split(body);
        match kind {
            Kind::Window => {
                let s = self.search.as_mut().expect("a search in progress");
                let (lo, hi) = s.window;
                if raws.len() >= self.plan.split_at && hi - lo > 3600 {
                    // A cap may have cut this window short: search it again in halves.
                    let mid = (lo + hi) / 2;
                    s.windows.push_front((mid, hi));
                    s.windows.push_front((lo, mid));
                    return;
                }
                for raw in raws {
                    let Some(m) = Message::parse(&raw) else { continue };
                    if s.seen.insert(m.msgid.clone()) {
                        s.ids.push(m.msgid);
                        s.found.push(raw);
                    }
                }
            }
            Kind::Listing => {
                let l = self.plan.listing.clone().expect("a listing job has a listing");
                // 404: nothing matched.
                let entries = atom_entries(&String::from_utf8_lossy(body));
                self.out.push_back(Effect::Log(format!("{}: {} threads from {}", l.inbox, entries.len(), l.offset)));
                self.out.push_back(Effect::Listed {
                    archive: self.plan.archive_key.clone(),
                    inbox: l.inbox,
                    offset: l.offset,
                    at: mail::utc(secs(now_ms)),
                    entries,
                });
            }
            Kind::Manifest => {
                // 404: an archive that lists none; recorded, so it is not asked again at once.
                let inboxes = if body.is_empty() { Ok(vec![]) } else { manifest_inboxes(body) };
                match inboxes {
                    Ok(inboxes) => {
                        let n = inboxes.len();
                        let archive = self.plan.archive_key.clone();
                        self.out.push_back(Effect::Inboxes { archive, at: mail::utc(secs(now_ms)), inboxes });
                        self.out.push_back(Effect::Log(format!("{}: {n} inboxes", self.plan.archive)));
                    }
                    Err(why) => self.request_failed(Kind::Manifest, why),
                }
            }
            Kind::Thread(root, sub) => {
                let at = mail::utc(secs(now_ms));
                if !matches!(sub, Asked::Root) {
                    // Asked for by one of its messages: its top, the message
                    // nothing else in it answers, is the root.
                    let msgs: Vec<Message> = raws.iter().filter_map(|r| Message::parse(r)).collect();
                    let ids: HashSet<&str> = msgs.iter().map(|m| m.msgid.as_str()).collect();
                    for m in msgs.iter().filter(|m| m.in_reply_to.as_deref().is_none_or(|p| !ids.contains(p))) {
                        if m.msgid != root {
                            let archive = self.plan.archive_key.clone();
                            self.out.push_back(Effect::Fetched { root: m.msgid.clone(), at: at.clone(), archive });
                        }
                        if let Asked::Followed(sub) = &sub {
                            self.out.push_back(Effect::Found { root: m.msgid.clone(), sub: sub.clone() });
                        }
                    }
                    if let Asked::Followed(sub) = sub {
                        self.out.push_back(Effect::RecordSync { sub, query: String::new(), ran_at: at.clone() });
                    }
                }
                self.out.push_back(Effect::Store(raws));
                self.out.push_back(Effect::Fetched { root, at, archive: self.plan.archive_key.clone() });
            }
        }
    }

    fn request_failed(&mut self, kind: Kind, why: String) {
        match kind {
            Kind::Window => {
                // Nothing is recorded, so the next sync searches this window again.
                let s = self.search.take().expect("a search in progress");
                self.out.push_back(Effect::Log(format!("{}: search failed, nothing recorded: {why}", s.sub.label())));
                self.search_failed = true;
            }
            Kind::Thread(..) => {
                self.out.push_back(Effect::Log(format!("  {why}")));
                self.thread_failures += 1;
            }
            Kind::Listing => {
                self.out.push_back(Effect::Log(format!("the listing: {why}")));
                self.search_failed = true;
            }
            Kind::Manifest => {
                self.out.push_back(Effect::Log(format!("the list of inboxes: {why}")));
                self.search_failed = true;
            }
        }
    }
}

/// Open, and not fetched whole since `refresh`.
fn stale(x: &Series, refresh: i64) -> bool {
    !x.state.done()
        && x.superseded_by.is_none()
        && x.as_of.as_deref().and_then(series::parse_utc).is_none_or(|t| t <= refresh)
}

fn secs(ms: u64) -> i64 {
    i64::try_from(ms / 1000).unwrap_or(i64::MAX)
}

/// A gzip file's contents, or the bytes as they are.
fn gunzip(body: Vec<u8>) -> Result<Vec<u8>, String> {
    if !body.starts_with(&[0x1f, 0x8b]) {
        return Ok(body);
    }
    let mut out = Vec::new();
    flate2::read::MultiGzDecoder::new(body.as_slice()).read_to_end(&mut out).map_err(|e| format!("bad gzip: {e}"))?;
    Ok(out)
}

/// application/x-www-form-urlencoded, as Python's urlencode writes it.
pub fn form_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A message-id as a URL path segment: Python's quote(msgid, safe="@+-._~=").
pub fn path_quote(s: &str) -> String {
    let mut out = String::new();
    for b in s.trim_matches(['<', '>']).bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'@' | b'+' | b'-' | b'.' | b'_' | b'~' | b'=' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Epoch seconds for "3.months.ago", "2026-10-23" or "2026-10-23T12:00:00Z".
///
/// lore misreads some date forms without complaint ("20261023" matches far
/// too much), so patchtrack always sends epoch seconds, as lei does.
pub fn since(text: &str, now: i64) -> Result<i64, String> {
    let t = text.trim();
    let bad = || {
        format!(
            "cannot read {text:?} as a time: use N.days.ago (or hours, weeks, months, years) or an ISO date such as 2026-10-23"
        )
    };
    if let Some(rest) = t.strip_suffix(".ago") {
        let (n, unit) = rest.split_once('.').ok_or_else(bad)?;
        let n: i64 = n.parse().map_err(|_| bad())?;
        let secs = match unit.trim_end_matches('s') {
            "minute" => 60,
            "hour" => 3600,
            "day" => DAY,
            "week" => 7 * DAY,
            "month" => 30 * DAY,
            "year" => 365 * DAY,
            _ => return Err(bad()),
        };
        return Ok(now - n * secs);
    }
    let (date, time) = t.split_once(['T', ' ']).unwrap_or((t, ""));
    let midnight = series::parse_utc(&format!("{date}T00:00:00Z")).ok_or_else(bad)?;
    if time.is_empty() {
        return Ok(midnight);
    }
    let (clock, offset) = match time.find(['Z', '+', '-']) {
        Some(i) => time.split_at(i),
        None => (time, ""),
    };
    let mut hms = clock.split(':').map(|p| p.parse::<i64>());
    let h = hms.next().ok_or_else(bad)?.map_err(|_| bad())?;
    let m = hms.next().unwrap_or(Ok(0)).map_err(|_| bad())?;
    let s = hms.next().unwrap_or(Ok(0)).map_err(|_| bad())?;
    let off = match offset {
        "" | "Z" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let (oh, om) = o[1..].split_once(':').unwrap_or((&o[1..], "0"));
            sign * (oh.parse::<i64>().map_err(|_| bad())? * 3600 + om.parse::<i64>().map_err(|_| bad())? * 60)
        }
    };
    if !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..61).contains(&s) {
        return Err(bad());
    }
    Ok(midnight + h * 3600 + m * 60 + s - off)
}
