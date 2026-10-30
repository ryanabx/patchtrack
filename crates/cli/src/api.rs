//! What the web app asks of `patchtrack bridge` and `patchtrack serve`.
//!
//! The store is this machine's, and the app keeps nothing of its own: every
//! answer is derived here, from the store, as the command line derives it,
//! and every change the app makes is made here. Reads are `GET`s, with
//! their arguments in the query; changes are `POST`s of JSON. Answers are
//! JSON of the core's model, whose TypeScript types ts-rs generates
//! (web/src/core/), or of the shapes web/src/protocol.ts names.
//!
//! The store is opened anew for each request (a request is a thread), and
//! what it derives to is kept until the store changes: [`Store::data_version`]
//! on a connection of its own says when another one, this server's or a
//! command's in a terminal, committed. The same number tells the app when to
//! ask again (`/v1/status`).
//!
//! It also syncs on its own ([`Live`]): when it starts, every `sync_every`
//! minutes, when the app asks, and after you follow something.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use patchtrack_core::compose;
use patchtrack_core::mail::{self, Head};
use patchtrack_core::series::{self, Facts, Family, Series};
use patchtrack_core::settings::{self, Follow, Settings, Subscription};
use patchtrack_core::{Message, local, mbox, news, nudge, projects, sync, thread, vdiff, views};
use patchtrack_store::{Draft, Export, Store};
use serde_json::{Value, json};

/// What a request gets back.
pub enum Answer {
    Json(u16, Value),
    /// A file to download: its content type and name.
    File(&'static str, &'static str, Vec<u8>),
}

/// Why a request failed: an HTTP status and a sentence for the person.
pub struct Fail(pub u16, pub String);

impl<E: std::fmt::Display> From<E> for Fail {
    fn from(e: E) -> Fail {
        Fail(400, e.to_string())
    }
}

type Result<T> = std::result::Result<T, Fail>;

/// What the store's mail derives to, kept until the store changes.
struct Derived {
    messages: Vec<Message>,
    facts: Facts,
    settings: Settings,
    subs: Vec<Subscription>,
    series: BTreeMap<String, Series>,
    families: Vec<Family>,
}

/// Whose series a view shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Who {
    Everyone,
    /// What you follow, and your identities' series.
    Followed,
    /// Your identities' series.
    Mine,
}

pub struct Api {
    db: PathBuf,
    /// Repositories b4 may run in.
    repos: Vec<PathBuf>,
    /// It sends drafts (`--allow-send`).
    can_send: bool,
    /// A connection that only reads `data_version`.
    watch: Mutex<Store>,
    derived: Mutex<Option<(i64, Arc<Derived>)>>,
    pub live: Live,
}

fn now_secs() -> i64 {
    i64::try_from(crate::now_ms() / 1000).unwrap_or(i64::MAX)
}

/// A draft for the page: its fields, and the message read back.
fn draft_json(d: &Draft) -> Value {
    let m = Message::parse(&d.raw);
    json!({
        "msgid": d.msgid, "family": d.family, "kind": d.kind, "created_at": d.created_at, "sent_at": d.sent_at,
        "subject": m.as_ref().map(|m| m.subject.clone()), "to": m.as_ref().map(|m| m.to.clone()),
        "cc": m.as_ref().map(|m| m.cc.clone()), "in_reply_to": m.as_ref().and_then(|m| m.in_reply_to.clone()),
        "body": m.as_ref().map(|m| m.body.clone()), "raw": String::from_utf8_lossy(&d.raw),
    })
}

/// "a%40b+c" to "a@b c".
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match (b[i], hex) {
            (_, Some(v)) => {
                out.push(v);
                i += 2;
            }
            (b'+', _) => out.push(b' '),
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A request's path and its query's arguments.
fn split_query(path: &str) -> (&str, HashMap<String, String>) {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let args = query
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (unescape(k), unescape(v))
        })
        .collect();
    (path, args)
}

impl Api {
    pub fn new(db: &Path, repos: Vec<PathBuf>, can_send: bool) -> std::result::Result<Api, String> {
        let watch = Store::open(db).map_err(|e| format!("{}: {e}", db.display()))?;
        Ok(Api {
            db: db.to_path_buf(),
            repos,
            can_send,
            watch: Mutex::new(watch),
            derived: Mutex::new(None),
            live: Live::default(),
        })
    }

    pub fn db(&self) -> &Path {
        &self.db
    }

    pub fn can_b4(&self) -> bool {
        !self.repos.is_empty()
    }

    fn open(&self) -> Result<(Store, Settings)> {
        let store = Store::open(&self.db)?;
        let (settings, _) = Settings::load(store.settings()?)?;
        Ok((store, settings))
    }

    /// Changes whenever the store does.
    fn generation(&self) -> Result<i64> {
        Ok(self.watch.lock().unwrap_or_else(|e| e.into_inner()).data_version()?)
    }

    fn derived(&self) -> Result<Arc<Derived>> {
        // Read before the store is, so a change made meanwhile is seen next time.
        let generation = self.generation()?;
        let mut cache = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((g, d)) = cache.as_ref()
            && *g == generation
        {
            return Ok(d.clone());
        }
        let (store, settings) = self.open()?;
        let messages = store.messages()?;
        let facts = store.facts()?;
        let subs = store.subscriptions()?;
        let series = series::build(&messages, &facts, &settings.rules);
        let families = series::families(series.clone(), &facts);
        let d = Arc::new(Derived { messages, facts, settings, subs, series, families });
        *cache = Some((generation, d.clone()));
        Ok(d)
    }

    /// Answer one request, the server having checked who asks.
    pub fn handle(&self, method: &str, path: &str, body: &[u8]) -> Answer {
        let (path, q) = split_query(path);
        match self.route(method, path, &q, body) {
            Ok(a) => a,
            Err(Fail(status, why)) => Answer::Json(status, json!({ "error": why })),
        }
    }

    fn route(&self, method: &str, path: &str, q: &HashMap<String, String>, body: &[u8]) -> Result<Answer> {
        let arg = |k: &str| q.get(k).map_or("", String::as_str);
        let input =
            || -> Result<Value> { serde_json::from_slice(body).map_err(|_| Fail(400, "the body must be JSON".into())) };
        let ok = |v: Value| Ok(Answer::Json(200, v));
        let field = |v: &Value, k: &str| v[k].as_str().unwrap_or_default().to_string();
        match (method, path) {
            ("GET", "/v1/hello") => ok(json!({
                "patchtrack": env!("CARGO_PKG_VERSION"),
                "can": {"send": self.can_send, "b4": self.can_b4()},
            })),
            ("GET", "/v1/status") => {
                let d = self.derived()?;
                let unread = Self::unread(&d, &Store::open(&self.db)?.seen()?).len();
                ok(json!({"generation": self.generation()?, "sync": self.live.status(), "messages": d.messages.len(),
                    "unread": unread}))
            }
            ("POST", "/v1/open") => {
                let v = input()?;
                ok(self.open_series(&field(&v, "root"), v["force"].as_bool().unwrap_or(false))?)
            }
            ("POST", "/v1/browse") => {
                let v = input()?;
                let older = v["older"].as_bool().unwrap_or(false);
                ok(self.browse(&field(&v, "project"), &field(&v, "inbox"), older)?)
            }
            ("GET", "/v1/listing") => ok(self.listing(arg("project"), arg("inbox"))?),
            ("POST", "/v1/open-message") => {
                let v = input()?;
                ok(self.open_message(&field(&v, "msgid"), &field(&v, "project"), &field(&v, "inbox"))?)
            }
            ("GET", "/v1/inboxes") => ok(self.inboxes()?),
            ("POST", "/v1/inboxes/refresh") => {
                let v = input()?;
                ok(self.refresh_inboxes(v["force"].as_bool().unwrap_or(false))?)
            }
            ("POST", "/v1/sync") => {
                self.live.ask();
                ok(json!({"sync": self.live.status()}))
            }
            ("GET", "/v1/summaries") => ok(self.summaries()?),
            ("GET", "/v1/scope") => ok(self.scope()?),
            ("GET", "/v1/people") => ok(self.people(arg("who"), arg("bots") == "1")?),
            ("GET", "/v1/family") => ok(self.family(arg("sender"), arg("key"))?),
            ("GET", "/v1/family-of") => {
                let d = self.derived()?;
                let f = d.families.iter().find(|f| f.versions.iter().any(|v| v.root == arg("root")));
                ok(json!(f.map(views::summary)))
            }
            ("GET", "/v1/conversation") => {
                let d = self.derived()?;
                let s = d.series.get(arg("root")).ok_or_else(|| Fail(404, format!("no series <{}>", arg("root"))))?;
                let by_id: HashMap<&str, &Message> = d.messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
                ok(json!(thread::conversation(s, &by_id)))
            }
            ("GET", "/v1/changes") => {
                let d = self.derived()?;
                let get = |root: &str| d.series.get(root).ok_or_else(|| Fail(404, format!("no series <{root}>")));
                ok(json!(vdiff::changes(get(arg("old"))?, get(arg("new"))?)))
            }
            ("GET", "/v1/files") => {
                let d = self.derived()?;
                let get = |root: &str| d.series.get(root).ok_or_else(|| Fail(404, format!("no series <{root}>")));
                ok(json!(vdiff::files(get(arg("old"))?, get(arg("new"))?)))
            }
            ("GET", "/v1/projects") => {
                let settings = self.derived()?.settings.clone();
                let presets: Vec<_> = projects::PRESETS.iter().filter_map(|p| projects::preset(p.0)).collect();
                ok(json!({"projects": settings.projects, "presets": presets, "archive": settings.archive}))
            }
            ("GET", "/v1/settings") => {
                let (store, _) = self.open()?;
                let set: HashMap<String, String> = store.settings()?.into_iter().collect();
                let out: Vec<Value> = settings::KEYS
                    .iter()
                    .map(|k| {
                        json!({"key": k.name, "shape": k.shape, "input": k.input, "default": k.default, "doc": k.doc,
                            "value": set.get(k.name)})
                    })
                    .collect();
                ok(json!(out))
            }
            ("POST", "/v1/settings") => {
                let v = input()?;
                let key = field(&v, "key");
                let mut store = Store::open(&self.db)?;
                match v["value"].as_str() {
                    Some(text) => store.set_setting(&key, &settings::validate(&key, text)?)?,
                    None => {
                        store.unset_setting(&key)?;
                    }
                }
                ok(json!(null))
            }
            ("GET", "/v1/nudges") => ok(self.nudges()?),
            ("GET", "/v1/notifications") => ok(self.notifications()?),
            ("POST", "/v1/notifications/read") => {
                let v = input()?;
                let one = (!v["all"].as_bool().unwrap_or(false)).then(|| (field(&v, "sender"), field(&v, "key")));
                ok(self.read(one.as_ref().map(|(s, k)| (s.as_str(), k.as_str())))?)
            }
            ("POST", "/v1/close") => {
                let v = input()?;
                let mut store = Store::open(&self.db)?;
                let family = format!("{}\0{}", field(&v, "sender"), field(&v, "key"));
                store.close_family(&family, &field(&v, "reason"), &mail::utc(now_secs()))?;
                ok(json!(null))
            }
            ("POST", "/v1/reopen") => {
                let v = input()?;
                let mut store = Store::open(&self.db)?;
                ok(json!(store.reopen_family(&format!("{}\0{}", field(&v, "sender"), field(&v, "key")))?))
            }
            ("POST", "/v1/reply") => {
                let v = input()?;
                ok(self.reply(&field(&v, "to"), &field(&v, "text"), &field(&v, "tag"))?)
            }
            ("POST", "/v1/review") => {
                let v = input()?;
                let comments: Vec<(String, usize, String)> = v["comments"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|c| {
                                (field(c, "msgid"), c["at"].as_u64().unwrap_or(u64::MAX) as usize, field(c, "text"))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                ok(self.review(&field(&v, "root"), &comments, &field(&v, "summary"), &field(&v, "tag"))?)
            }
            ("GET", "/v1/drafts") => {
                let (store, _) = self.open()?;
                ok(json!(store.drafts()?.iter().map(draft_json).collect::<Vec<_>>()))
            }
            ("POST", "/v1/drafts/sent") => {
                let v = input()?;
                let mut store = Store::open(&self.db)?;
                ok(json!(store.mark_sent(&field(&v, "msgid"), &mail::utc(now_secs()))?))
            }
            ("POST", "/v1/drafts/remove") => {
                let v = input()?;
                let mut store = Store::open(&self.db)?;
                ok(json!(store.remove_draft(&field(&v, "msgid"))?))
            }
            ("POST", "/v1/send") => {
                let v = input()?;
                ok(json!({ "sent": self.send(&field(&v, "msgid"))? }))
            }
            ("GET", "/v1/subscriptions") => {
                let (store, _) = self.open()?;
                let out: Vec<Value> = store
                    .subscriptions()?
                    .iter()
                    .map(|s| {
                        json!({"kind": s.kind.as_str(), "value": s.value, "list": s.list, "archive": s.archive,
                            "label": s.label()})
                    })
                    .collect();
                ok(json!(out))
            }
            ("POST", "/v1/follow") => {
                let v = input()?;
                let (mut store, settings) = self.open()?;
                let kind = Follow::parse(&field(&v, "kind"))
                    .ok_or_else(|| Fail(400, format!("no kind of subscription {:?}", field(&v, "kind"))))?;
                let at = mail::utc(now_secs());
                let mut any = false;
                for s in settings.follow(kind, &field(&v, "value"), &field(&v, "list"), &field(&v, "project"))? {
                    any |= store.subscribe(&s, &at)?;
                }
                // What you just followed, fetched now rather than at the next sync.
                if any {
                    self.live.ask();
                }
                ok(json!(any))
            }
            ("POST", "/v1/unfollow") => {
                let v = input()?;
                let kind = Follow::parse(&field(&v, "kind"))
                    .ok_or_else(|| Fail(400, format!("no kind of subscription {:?}", field(&v, "kind"))))?;
                let mut store = Store::open(&self.db)?;
                let (list, archive) = (field(&v, "list"), field(&v, "archive"));
                ok(json!(store.unsubscribe(kind, &field(&v, "value"), Some(&list), Some(&archive))?))
            }
            ("GET", "/v1/sources") => {
                let (store, _) = self.open()?;
                let out: Vec<Value> =
                    store.sources()?.into_iter().map(|(path, read)| json!({"path": path, "read_at": read})).collect();
                ok(json!(out))
            }
            ("POST", "/v1/import") => ok(self.import(body, arg("only_followed") == "1")?),
            ("GET", "/v1/export") => {
                let (store, _) = self.open()?;
                let file = std::env::temp_dir().join(format!(
                    "patchtrack-store-{}-{}.sqlite3",
                    std::process::id(),
                    crate::now_ms()
                ));
                store.export(&file.to_string_lossy(), Export::Full)?;
                let bytes = std::fs::read(&file);
                let _ = std::fs::remove_file(&file);
                Ok(Answer::File("application/vnd.sqlite3", "patchtrack.sqlite3", bytes?))
            }
            ("GET", "/v1/branches") if !self.can_b4() => ok(json!([])),
            ("GET", "/v1/branches") => ok(crate::machine::branches(&self.repos, &self.open()?.1)?),
            ("POST", "/v1/b4") if !self.can_b4() => {
                Err(Fail(403, "this server runs no b4: start it with --repo PATH".into()))
            }
            ("POST", "/v1/b4") => {
                let v = input()?;
                let dry = v["dry_run"].as_bool().unwrap_or(true);
                let settings = self.open()?.1;
                let (done, output) = crate::machine::b4(
                    &self.repos,
                    &settings,
                    &field(&v, "repo"),
                    &field(&v, "branch"),
                    &field(&v, "action"),
                    dry,
                )?;
                ok(json!({"ok": done, "output": output}))
            }
            (_, p) if ROUTES.contains(&p) => Err(Fail(405, "wrong method".into())),
            _ => Err(Fail(404, "no such endpoint".into())),
        }
    }

    /// The families `who` covers, by `Family::id`; None for everyone.
    fn whose(&self, d: &Derived, who: Who) -> Option<HashSet<String>> {
        let ids = &d.settings.identities;
        let theirs = d.families.iter().filter(|f| match who {
            Who::Everyone => true,
            Who::Followed => f.versions.iter().any(|s| views::follows(s, ids, &d.subs, &d.facts)),
            Who::Mine => f.versions.iter().any(|s| views::follows(s, ids, &[], &d.facts)),
        });
        (who != Who::Everyone).then(|| theirs.map(Family::id).collect())
    }

    /// The authors you follow and your identities, in lower case, sorted:
    /// whose mail a mailbox keeps when it keeps only what you follow.
    fn authors(d: &Derived) -> Vec<String> {
        let followed = d.subs.iter().filter(|s| s.kind == Follow::Author).map(|s| s.value.clone());
        let mut authors: Vec<String> = followed.chain(d.settings.identities.iter().cloned()).collect();
        authors.sort();
        authors.dedup();
        authors
    }

    /// One summary per family, latest first, with the projects it is in.
    fn summaries(&self) -> Result<Value> {
        let d = self.derived()?;
        let summary = |f: &Family| {
            let mut s = views::summary(f);
            let mut names: Vec<String> = f
                .versions
                .iter()
                .flat_map(|v| projects::of(v, &d.facts, &d.settings.projects, &d.settings.archive))
                .map(str::to_string)
                .collect();
            names.sort();
            names.dedup();
            s.projects = names;
            s
        };
        Ok(json!(d.families.iter().map(summary).collect::<Vec<_>>()))
    }

    /// `{mine, followed, authors}`: family ids of your series and of what
    /// you follow; the addresses "only what you follow" keeps mail of.
    fn scope(&self) -> Result<Value> {
        let d = self.derived()?;
        Ok(json!({"mine": self.whose(&d, Who::Mine), "followed": self.whose(&d, Who::Followed),
            "authors": Self::authors(&d)}))
    }

    fn people(&self, who: &str, bots: bool) -> Result<Value> {
        let who = match who {
            "followed" => Who::Followed,
            "mine" => Who::Mine,
            _ => Who::Everyone,
        };
        let d = self.derived()?;
        let ids = self.whose(&d, who);
        let theirs = d.families.iter().filter(|f| ids.as_ref().is_none_or(|ids| ids.contains(&f.id())));
        let mut people = views::people(theirs.flat_map(|f| &f.versions));
        if !bots {
            people.retain(|p| !p.bot);
        }
        Ok(json!(people))
    }

    /// A series is opened: fetch its thread, if it never was, or longer ago
    /// than `stale_minutes` (0: only when forced), or when `force`d (the
    /// app's Refresh). One request, made by the server's own jobs, so as
    /// politely as a sync; the app sees the thread arrive as a change.
    fn open_series(&self, root: &str, force: bool) -> Result<Value> {
        let d = self.derived()?;
        let s = d.series.get(root).ok_or_else(|| Fail(404, format!("no series <{root}>")))?;
        let age = s.as_of.as_deref().and_then(series::parse_utc).map(|t| now_secs() - t);
        let stale = match age {
            None => d.settings.stale_minutes > 0 || force,
            Some(age) => d.settings.stale_minutes > 0 && age > d.settings.stale_minutes * 60,
        };
        if !(force || stale) {
            return Ok(json!({"fetching": false, "as_of": s.as_of}));
        }
        if !s.pending {
            // Where it was found, or last fetched from: "" is the archive setting's.
            let archive = d
                .facts
                .thread_archive
                .get(root)
                .or_else(|| d.facts.archives.get(root).and_then(|a| a.iter().next()))
                .cloned()
                .unwrap_or_default();
            let want = sync::Effect::WantThread { root: root.into(), at: mail::utc(now_secs()), archive };
            Store::open(&self.db)?.apply(&want)?;
        }
        self.live.ask_threads();
        Ok(json!({"fetching": true, "as_of": s.as_of}))
    }

    /// The archive key ("" for the archive setting's) of a project, by name;
    /// "" for none.
    fn archive_key(settings: &Settings, project: &str) -> Result<String> {
        if project.is_empty() {
            return Ok(String::new());
        }
        let p = crate::project(settings, project)?;
        Ok(if p.archive == settings.archive { String::new() } else { p.archive.clone() })
    }

    /// List a page of an inbox's threads, as lore's own pages do: the newest
    /// `PAGE`, or (`older`) the page after those listed. One request, and no
    /// mail stored: a thread is fetched when it is opened.
    fn browse(&self, project: &str, inbox: &str, older: bool) -> Result<Value> {
        let settings = self.derived()?.settings.clone();
        let inbox = settings::inbox(inbox)?;
        let archive = Self::archive_key(&settings, project)?;
        let offset = if older {
            let pages = Store::open(&self.db)?.listing_pages()?;
            pages.get(&(archive.clone(), inbox.clone())).map_or(0, |p| p.1)
        } else {
            0
        };
        self.live.browse(Job::List { archive, inbox, offset });
        Ok(json!({"sync": self.live.status()}))
    }

    /// An inbox's listed threads, newest first, one row per thread (by its
    /// subject without "Re:"): each with the series it is in, where the
    /// store has it.
    fn listing(&self, project: &str, inbox: &str) -> Result<Value> {
        let d = self.derived()?;
        let archive = Self::archive_key(&d.settings, project)?;
        let store = Store::open(&self.db)?;
        let pages = store.listing_pages()?;
        let (listed, next) = pages.get(&(archive.clone(), inbox.to_string())).cloned().unwrap_or((None, 0));
        let mut seen = HashSet::new();
        let mut rows = vec![];
        for l in store.listings(&archive, inbox)? {
            let title = strip_re(&l.subject).to_string();
            if !seen.insert(title.clone()) {
                continue;
            }
            let family = series_of(&d, &l.msgid).and_then(|s| {
                d.families.iter().find(|f| f.sender == s.sender && f.key == s.family).map(|f| {
                    let mut sum = views::summary(f);
                    sum.projects = projects::of(f.head(), &d.facts, &d.settings.projects, &d.settings.archive)
                        .into_iter()
                        .map(str::to_string)
                        .collect();
                    sum
                })
            });
            rows.push(json!({"listed": l, "title": title, "family": family}));
        }
        Ok(json!({"listed": listed, "next": next, "rows": rows}))
    }

    /// A listed thread is opened: the series it is in, if the store has it;
    /// else its thread is fetched by this message, from its inbox, and the
    /// app asks again; `missing` if it was fetched and is not there (the
    /// archive no longer has it).
    fn open_message(&self, msgid: &str, project: &str, inbox: &str) -> Result<Value> {
        let msgid = msgid.trim_matches(['<', '>']);
        let d = self.derived()?;
        if let Some(s) = series_of(&d, msgid) {
            return Ok(json!({"family": {"sender": s.sender, "key": s.family, "version": s.version}}));
        }
        if d.facts.fetched.contains_key(msgid) {
            return Ok(json!({ "missing": true }));
        }
        let archive = Self::archive_key(&d.settings, project)?;
        let inbox = settings::inbox(if inbox.is_empty() { "all" } else { inbox })?;
        self.live.browse(Job::Message { archive, inbox, msgid: msgid.to_string() });
        Ok(json!({ "fetching": true }))
    }

    /// The archive key of a URL: "" for the archive setting's.
    fn key_of(settings: &Settings, url: &str) -> String {
        if url == settings.archive { String::new() } else { url.to_string() }
    }

    /// Read each archive's list of inboxes again when it is older than an hour
    /// (or `force`): your projects' archives and the archive setting's.
    fn refresh_inboxes(&self, force: bool) -> Result<Value> {
        let d = self.derived()?;
        let (_, read) = Store::open(&self.db)?.archive_inboxes()?;
        let mut asked = 0;
        for url in d.settings.archives(&d.subs) {
            let key = Self::key_of(&d.settings, url);
            let fresh = read
                .get(&key)
                .and_then(|t| series::parse_utc(t))
                .is_some_and(|t| now_secs() - t < INBOXES_MINUTES * 60);
            if force || !fresh {
                self.live.browse(Job::Inboxes { archive: key });
                asked += 1;
            }
        }
        Ok(json!({ "asked": asked }))
    }

    /// The inboxes there are to browse, newest first, as lore's front page
    /// lists them: each archive's list (see `refresh_inboxes`), each inbox
    /// under the project that names it, else the project that takes every
    /// inbox of its archive; and every list a series here was found on. With
    /// how many series each has here, when it was last listed, and how far
    /// back.
    fn inboxes(&self) -> Result<Value> {
        #[derive(Default)]
        struct Row {
            description: String,
            updated: Option<String>,
            listed: Option<String>,
            series: usize,
        }
        let d = self.derived()?;
        let store = Store::open(&self.db)?;
        let (listed_by_archive, _) = store.archive_inboxes()?;
        let projects = &d.settings.projects;
        let mut out: BTreeMap<(String, String), Row> = BTreeMap::new();
        for p in projects.iter().filter(|p| !p.lists.is_empty()) {
            for l in &p.lists {
                out.entry((p.name.clone(), l.clone())).or_default();
            }
        }
        for (key, info) in &listed_by_archive {
            let url = d.settings.archive_of(key);
            let on_archive: Vec<_> = projects.iter().filter(|p| p.archive == url).collect();
            let project = on_archive
                .iter()
                .find(|p| p.lists.contains(&info.name))
                .or_else(|| on_archive.iter().find(|p| p.lists.is_empty()))
                .map(|p| p.name.clone());
            // An archive no project claims all of: its inboxes, if it is the archive setting's.
            let project = match project {
                Some(p) => p,
                None if key.is_empty() && on_archive.iter().all(|p| !p.lists.is_empty()) => String::new(),
                None => continue,
            };
            let row = out.entry((project, info.name.clone())).or_default();
            row.description = info.description.clone();
            row.updated = info.updated.clone();
        }
        for ((archive, inbox), (listed, _)) in store.listing_pages()? {
            // Under the project of that archive that has the inbox, if any does.
            let url = d.settings.archive_of(&archive).to_string();
            let project = projects
                .iter()
                .filter(|p| p.archive == url)
                .find(|p| p.lists.contains(&inbox) || p.lists.is_empty())
                .map_or(String::new(), |p| p.name.clone());
            let row = out.entry((project, inbox.clone())).or_default();
            row.listed = listed;
            row.series += store.listings(&archive, &inbox)?.len();
        }
        let mut rows: Vec<((String, String), Row)> = out.into_iter().collect();
        // Newest first, as the archive's front page; what it does not date, by what was listed here.
        rows.sort_by(|(a, x), (b, y)| {
            let when = |r: &Row| r.updated.clone().or_else(|| r.listed.clone());
            when(y).cmp(&when(x)).then_with(|| a.cmp(b))
        });
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|((project, inbox), row)| {
                json!({"project": project, "inbox": inbox, "description": row.description,
                    "updated": row.updated, "listed": row.listed, "series": row.series})
            })
            .collect();
        Ok(json!(rows))
    }

    /// One family, all of it, with each version's archive and its page there.
    fn family(&self, sender: &str, key: &str) -> Result<Value> {
        let d = self.derived()?;
        let f = d
            .families
            .iter()
            .find(|f| f.sender == sender && f.key == key)
            .ok_or_else(|| Fail(404, "no such series".into()))?;
        let archives: BTreeMap<&str, &str> = f.versions.iter().map(|s| (s.root.as_str(), archive_of(s, &d))).collect();
        let urls: BTreeMap<&str, String> =
            archives.iter().map(|(root, archive)| (*root, views::message_url(archive, root))).collect();
        let mut v = json!(f);
        v["archives"] = json!(archives);
        v["urls"] = json!(urls);
        Ok(v)
    }

    fn nudges(&self) -> Result<Value> {
        let d = self.derived()?;
        let now = now_secs();
        let window =
            crate::views::merge_window(&Store::open(&self.db)?, &d.settings, now).map_err(|e| e.to_string())?;
        let mut found: Vec<nudge::Nudge> = d
            .families
            .iter()
            .filter_map(|f| nudge::classify(f, now, d.settings.ping_days, d.settings.resend_days, window.as_ref()))
            .collect();
        found.sort_by_key(|n| (n.kind, -n.days));
        let out: Vec<Value> = found
            .iter()
            .map(|n| {
                json!({"sender": n.family.sender, "key": n.family.key, "title": n.family.head().title,
                    "state": n.family.state, "kind": n.kind.as_str(), "days": n.days, "why": n.why})
            })
            .collect();
        Ok(json!(out))
    }

    /// The open families that changed since you last read them, as `patchtrack
    /// news` finds them, of those that notify you (`views::notifies`: they
    /// involve you, or you follow their author or thread); none before the
    /// first read, which `seen` records.
    fn unread<'a>(d: &'a Derived, seen: &HashMap<String, String>) -> Vec<news::Change<'a>> {
        if seen.is_empty() {
            return vec![];
        }
        let ids = &d.settings.identities;
        let notifies = |f: &&Family| f.versions.iter().any(|s| views::notifies(s, ids, &d.subs, &d.facts));
        news::changes(d.families.iter().filter(|f| f.closed.is_none()).filter(notifies), seen)
    }

    /// GitHub's notifications, for mail: each open family that changed since
    /// you last read it, and how; `first` before anything was ever read.
    fn notifications(&self) -> Result<Value> {
        let d = self.derived()?;
        let seen = Store::open(&self.db)?.seen()?;
        let out: Vec<Value> = Self::unread(&d, &seen)
            .iter()
            .map(|c| {
                let f = c.family;
                let mut lists: Vec<&String> = f.versions.iter().flat_map(|s| &s.lists).collect();
                lists.sort();
                lists.dedup();
                json!({"sender": f.sender, "key": f.key, "title": f.head().title, "state": f.state,
                    "version": f.head().version, "lists": lists, "updated": f.head().last_activity(),
                    "newSeries": c.new_series, "newVersion": c.new_version,
                    "stateChange": c.state.map(|(a, b)| [a, b]),
                    "replies": c.replies.iter().map(|(s, r)| news::describe(s, r)).collect::<Vec<_>>()})
            })
            .collect();
        Ok(json!({"first": seen.is_empty(), "notifications": out}))
    }

    /// Mark one family read (by sender and key), or every one: what you were
    /// shown becomes what `news` compares with. The first read marks every
    /// family, so what came before it is not news. Reading what is read
    /// already writes nothing, so it is no change to the store.
    fn read(&self, one: Option<(&str, &str)>) -> Result<Value> {
        let d = self.derived()?;
        let mut store = Store::open(&self.db)?;
        let seen = store.seen()?;
        let marks: Vec<(String, String)> = match one {
            Some((sender, key)) if !seen.is_empty() => {
                let f = d
                    .families
                    .iter()
                    .find(|f| f.sender == sender && f.key == key)
                    .ok_or_else(|| Fail(404, "no such series".into()))?;
                vec![(f.id(), news::snapshot(f))]
            }
            _ => d.families.iter().map(|f| (f.id(), news::snapshot(f))).collect(),
        };
        let marks: Vec<&(String, String)> = marks.iter().filter(|(id, snap)| seen.get(id) != Some(snap)).collect();
        if !marks.is_empty() {
            store.mark_seen(marks.iter().map(|(f, s)| (f.as_str(), s.as_str())), &mail::utc(now_secs()))?;
        }
        Ok(json!({ "marked": marks.len() }))
    }

    /// A reply to the stored message `to`, as `patchtrack reply` writes it,
    /// kept as a draft.
    fn reply(&self, to: &str, text: &str, tag: &str) -> Result<Value> {
        let d = self.derived()?;
        let me = crate::compose::me(&d.settings).map_err(|e| e.to_string())?;
        let tag = tag_named(tag)?;
        let m = message(&d, to)?;
        let body = compose::reply_body(m, text, tag, &me);
        let kind = if tag.is_some() { "tag" } else { "reply" };
        Ok(draft_json(&self.draft(&d, m, &me, &body, kind)?))
    }

    /// A review, as GitHub's "Finish your review" sends one, written as mail:
    /// for each patch with comments (by message-id, at a line of its diff),
    /// an inline review of it; `summary` and `tag` in a reply to `root`, the
    /// cover (or the only patch), or in its review if it has comments. Each
    /// is kept as a draft; returns them, in the series' order.
    fn review(&self, root: &str, comments: &[(String, usize, String)], summary: &str, tag: &str) -> Result<Value> {
        let d = self.derived()?;
        let me = crate::compose::me(&d.settings).map_err(|e| e.to_string())?;
        let tag = tag_named(tag)?;
        let s = d.series.get(root).ok_or_else(|| Fail(404, format!("no series <{root}>")))?;
        let mut targets: Vec<&str> = vec![];
        for (msgid, ..) in comments.iter().filter(|(_, _, t)| !t.trim().is_empty()) {
            if !s.patches.values().any(|p| p == msgid) {
                return Err(Fail(400, format!("<{msgid}> is not a patch of this series")));
            }
            if !targets.contains(&msgid.as_str()) {
                targets.push(msgid);
            }
        }
        if targets.is_empty() && summary.trim().is_empty() && tag.is_none() {
            return Err(Fail(400, "Nothing to send: comment on a line, write a summary or give a tag.".into()));
        }
        if !targets.contains(&root) && (!summary.trim().is_empty() || tag.is_some()) {
            targets.push(root);
        }
        let order = |id: &str| s.patches.iter().find(|(_, p)| *p == id).map_or(0, |(n, _)| *n);
        targets.sort_by_key(|id| order(id));
        // Every draft is written before any is kept, so a bad comment keeps none.
        let mut written = vec![];
        for id in targets {
            let m = message(&d, id)?;
            let mine: Vec<(usize, String)> =
                comments.iter().filter(|(c, ..)| c == id).map(|(_, at, t)| (*at, t.clone())).collect();
            let (summary, tag) = if id == root { (summary, tag) } else { ("", None) };
            let body =
                compose::review_body(m, &mine, summary, tag, &me).map_err(|e| Fail(400, format!("<{id}>: {e}")))?;
            let kind = if !mine.is_empty() {
                "review"
            } else if tag.is_some() {
                "tag"
            } else {
                "reply"
            };
            written.push((m, body, kind));
        }
        let mut out = vec![];
        for (m, body, kind) in written {
            out.push(draft_json(&self.draft(&d, m, &me, &body, kind)?));
        }
        Ok(json!(out))
    }

    /// A reply to `m` with this body, kept as a draft.
    fn draft(&self, d: &Derived, m: &Message, me: &compose::Me, body: &str, kind: &str) -> Result<Draft> {
        let to = m.msgid.as_str();
        let has = |s: &&Series| {
            s.root == to || s.patches.values().any(|p| p == to) || s.replies.iter().any(|r| r.msgid == to)
        };
        let family = d.series.values().find(has).map(|s| format!("{}\0{}", s.sender, s.family)).unwrap_or_default();
        // Two replies in one millisecond must not share a Message-ID.
        let domain = me.addr.rsplit_once('@').map_or("localhost", |(_, d)| d);
        let msgid = format!("{}.{}.patchtrack@{domain}", crate::now_ms(), &crate::bridge::new_token()[..12]);
        let secs = now_secs();
        let raw = compose::reply(m, me, body, &compose::rfc2822(secs), &format!("<{msgid}>"));
        let draft = Draft { msgid, family, kind: kind.into(), raw, created_at: mail::utc(secs), sent_at: None };
        Store::open(&self.db)?.save_draft(&draft)?;
        Ok(draft)
    }

    /// Send a stored draft with git send-email: only one from your
    /// identities, and only if the server was allowed to.
    fn send(&self, msgid: &str) -> Result<String> {
        if !self.can_send {
            return Err(Fail(403, "this server does not send mail: start it with --allow-send".into()));
        }
        let (mut store, settings) = self.open()?;
        let d = store
            .drafts()?
            .into_iter()
            .find(|d| d.msgid == msgid)
            .ok_or_else(|| Fail(404, format!("no draft <{msgid}>")))?;
        if let Some(at) = &d.sent_at {
            return Err(Fail(400, format!("{msgid} was sent already, at {at}")));
        }
        let m = Message::parse(&d.raw).ok_or("the draft is not a message")?;
        if !settings.identities.contains(&m.from.addr) {
            return Err(Fail(
                400,
                format!(
                    "{} is not one of this machine's identities: set them with `patchtrack settings set identities`",
                    m.from.addr
                ),
            ));
        }
        crate::compose::deliver(&d.raw, true)?;
        store.mark_sent(msgid, &mail::utc(now_secs()))?;
        // What you sent is mail of yours, here as on the command line.
        crate::store_local(&mut store, &settings, std::slice::from_ref(&d.raw)).map_err(|e| e.to_string())?;
        eprintln!("sent {msgid} to {}", m.to.iter().chain(&m.cc).cloned().collect::<Vec<_>>().join(", "));
        Ok(m.msgid)
    }

    /// Mail (an mbox or one message) or a store file, by what the bytes
    /// are. With `only_followed`, mail keeps only what a source would: the
    /// series of the authors you follow and your own, and their replies.
    fn import(&self, raw: &[u8], only_followed: bool) -> Result<Value> {
        if raw.starts_with(b"SQLite format 3\0") {
            let file = std::env::temp_dir().join(format!(
                "patchtrack-import-{}-{}.sqlite3",
                std::process::id(),
                crate::now_ms()
            ));
            std::fs::write(&file, raw)?;
            let merged = Store::open(&self.db)?.import(&file.to_string_lossy());
            let _ = std::fs::remove_file(&file);
            let n = merged?;
            return Ok(json!({"read": n, "kept": n, "new": n, "store": true}));
        }
        let mut msgs = if raw.starts_with(b"From ") { mbox::split(raw) } else { vec![raw.to_vec()] };
        let read = msgs.len();
        let (mut store, settings) = self.open()?;
        if only_followed {
            let authors = Self::authors(&*self.derived()?);
            if authors.is_empty() {
                return Err(Fail(
                    400,
                    "You follow no one and have no identities: follow an author, or set identities in Settings.".into(),
                ));
            }
            let keep = local::relevant(&msgs, &store.msgids()?, &authors);
            msgs = keep.into_iter().map(|i| std::mem::take(&mut msgs[i])).collect();
        }
        let known = store.msgids()?;
        let new = msgs.iter().filter_map(|r| Head::parse(r)).filter(|h| !known.contains(&h.msgid)).count();
        crate::store_local(&mut store, &settings, &msgs).map_err(|e| e.to_string())?;
        Ok(json!({"read": read, "kept": msgs.len(), "new": new, "store": false}))
    }
}

/// The series a message is in: its root, a patch or a reply.
fn series_of<'a>(d: &'a Derived, msgid: &str) -> Option<&'a Series> {
    d.series.values().find(|s| {
        s.root == msgid || s.patches.values().any(|p| p == msgid) || s.replies.iter().any(|r| r.msgid == msgid)
    })
}

/// A subject without the "Re:" of a reply, as the thread it answers is titled.
fn strip_re(subject: &str) -> &str {
    let mut s = subject.trim_start();
    while s.get(..3).is_some_and(|re| re.eq_ignore_ascii_case("re:")) {
        s = s[3..].trim_start();
    }
    s
}

/// A stored message, by message-id.
fn message<'a>(d: &'a Derived, msgid: &str) -> Result<&'a Message> {
    d.messages.iter().find(|m| m.msgid == msgid).ok_or_else(|| Fail(404, format!("no message <{msgid}>")))
}

/// A tag by name, in any case; "" for none.
fn tag_named(tag: &str) -> Result<Option<&'static str>> {
    match tag {
        "" => Ok(None),
        t => Ok(Some(
            *compose::TAGS
                .iter()
                .find(|k| k.eq_ignore_ascii_case(t))
                .ok_or_else(|| Fail(400, format!("{t}: not a tag")))?,
        )),
    }
}

/// Every endpoint, for "wrong method" rather than "no such endpoint".
const ROUTES: &[&str] = &[
    "/v1/hello",
    "/v1/status",
    "/v1/sync",
    "/v1/open",
    "/v1/browse",
    "/v1/listing",
    "/v1/open-message",
    "/v1/inboxes",
    "/v1/inboxes/refresh",
    "/v1/summaries",
    "/v1/scope",
    "/v1/people",
    "/v1/family",
    "/v1/family-of",
    "/v1/conversation",
    "/v1/changes",
    "/v1/files",
    "/v1/projects",
    "/v1/settings",
    "/v1/nudges",
    "/v1/notifications",
    "/v1/notifications/read",
    "/v1/close",
    "/v1/reopen",
    "/v1/reply",
    "/v1/review",
    "/v1/drafts",
    "/v1/drafts/sent",
    "/v1/drafts/remove",
    "/v1/send",
    "/v1/subscriptions",
    "/v1/follow",
    "/v1/unfollow",
    "/v1/sources",
    "/v1/import",
    "/v1/export",
    "/v1/branches",
    "/v1/b4",
];

/// The archive a series is read on: its first project's, or the archive setting's.
fn archive_of<'a>(s: &Series, d: &'a Derived) -> &'a str {
    let named = projects::of(s, &d.facts, &d.settings.projects, &d.settings.archive);
    named
        .first()
        .and_then(|n| d.settings.projects.iter().find(|p| p.name == *n))
        .map_or(&d.settings.archive, |p| &p.archive)
}

/// Lines of a sync's log kept for the app.
const LOG_LINES: usize = 200;

/// What a listing of an inbox searches for: its patches, and replies to them.
pub const BROWSE_QUERY: &str = "s:PATCH";

/// How many threads one page of an inbox lists.
pub const PAGE: usize = 50;

/// How old an archive's list of inboxes may get before it is read again, in minutes.
pub const INBOXES_MINUTES: i64 = 60;

/// What the server's own syncs do, one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Job {
    /// Everything you follow, as `patchtrack sync` does.
    Sync,
    /// The threads asked for since (opened in the app): one request each.
    Threads,
    /// A page of an inbox's threads, from `offset`: one request, no mail.
    List { archive: String, inbox: String, offset: usize },
    /// A listed thread, opened: fetched by this message, from its inbox.
    Message { archive: String, inbox: String, msgid: String },
    /// The archive's list of inboxes.
    Inboxes { archive: String },
}

impl Job {
    fn describe(&self) -> String {
        match self {
            Job::Sync => "Syncing".into(),
            Job::Threads => "Fetching threads".into(),
            Job::List { inbox, offset: 0, .. } => format!("Listing the latest on {inbox}"),
            Job::List { inbox, .. } => format!("Listing older threads on {inbox}"),
            Job::Message { .. } => "Fetching a thread".into(),
            Job::Inboxes { .. } => "Reading the list of inboxes".into(),
        }
    }
}

/// The server's own syncs: one job at a time; a sync when it starts, every
/// `sync_every` minutes and when asked, threads and inbox pages when asked.
#[derive(Default)]
pub struct Live {
    state: Mutex<LiveState>,
    wake: Condvar,
}

#[derive(Default)]
struct LiveState {
    /// A sync is wanted as soon as the job running (if any) is done.
    asked: bool,
    /// Threads were asked for.
    threads: bool,
    browse: VecDeque<Job>,
    /// What runs now.
    doing: Option<Job>,
    /// When the last sync started and finished, and whether nothing failed.
    started: Option<String>,
    finished: Option<String>,
    ok: Option<bool>,
    log: VecDeque<String>,
}

impl Live {
    /// Sync as soon as possible.
    pub fn ask(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).asked = true;
        self.wake.notify_all();
    }

    /// Fetch the threads that are wanted, as soon as possible.
    pub fn ask_threads(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).threads = true;
        self.wake.notify_all();
    }

    /// List a page of an inbox, unless the same page is waiting already.
    fn browse(&self, job: Job) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !s.browse.contains(&job) && s.doing.as_ref() != Some(&job) {
            s.browse.push_back(job);
        }
        self.wake.notify_all();
    }

    pub fn status(&self) -> Value {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let inbox = |j: &Job| match j {
            Job::List { inbox, .. } => Some(inbox.clone()),
            _ => None,
        };
        let browsing: Vec<String> = s.doing.iter().chain(&s.browse).filter_map(inbox).collect();
        json!({"running": s.doing == Some(Job::Sync), "asked": s.asked, "doing": s.doing.as_ref().map(Job::describe),
            "threads": s.threads || s.doing == Some(Job::Threads), "browsing": browsing,
            "inboxes": s.doing.iter().chain(&s.browse).any(|j| matches!(j, Job::Inboxes { .. })),
            "started": s.started, "finished": s.finished, "ok": s.ok, "log": s.log})
    }

    fn say(&self, line: String) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.log.len() == LOG_LINES {
            s.log.pop_front();
        }
        s.log.push_back(line);
    }

    /// How often to sync, from the settings now.
    fn every(db: &Path) -> i64 {
        Store::open(db).ok().and_then(|st| Settings::load(st.settings().ok()?).ok()).map_or(30, |(s, _)| s.sync_every)
    }

    /// Run the store's jobs at `db` until the process ends: a sync now, then
    /// on schedule, and whatever is asked for.
    pub fn run(&self, db: &Path) -> ! {
        self.ask();
        let mut next = Instant::now();
        loop {
            let job = {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    let every = Self::every(db);
                    if s.asked || (every > 0 && Instant::now() >= next) {
                        s.asked = false;
                        s.started = Some(mail::utc(now_secs()));
                        s.log.clear();
                        break Job::Sync;
                    }
                    if s.threads {
                        s.threads = false;
                        break Job::Threads;
                    }
                    if let Some(job) = s.browse.pop_front() {
                        break job;
                    }
                    // Settings may change meanwhile: look again within a minute.
                    let wait = if every > 0 { next.saturating_duration_since(Instant::now()) } else { Duration::MAX };
                    s = self
                        .wake
                        .wait_timeout(s, wait.min(Duration::from_secs(60)))
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
                }
            };
            self.state.lock().unwrap_or_else(|e| e.into_inner()).doing = Some(job.clone());
            let ok = match self.run_job(db, &job) {
                Ok(ok) => ok,
                Err(e) => {
                    self.say(format!("{}: {e}", job.describe()));
                    false
                }
            };
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.doing = None;
            if job == Job::Sync {
                next = Instant::now() + Duration::from_secs(u64::try_from(Self::every(db).max(1)).unwrap_or(1) * 60);
                s.finished = Some(mail::utc(now_secs()));
                s.ok = Some(ok);
            }
        }
    }

    fn run_job(&self, db: &Path, job: &Job) -> std::result::Result<bool, Box<dyn std::error::Error>> {
        let mut store = Store::open(db)?;
        let (settings, _) = Settings::load(store.settings()?)?;
        let agent = crate::http_agent();
        let now = now_secs();
        match job {
            Job::Sync => {
                let subs = store.subscriptions()?;
                if subs.is_empty() && store.sources()?.is_empty() {
                    self.say("Nothing to sync: follow an author, a list, a search or a thread.".into());
                    return Ok(true);
                }
                let args = crate::SyncArgs::default();
                crate::sync_store(&mut store, &settings, &args, subs, &mut |line| self.say(line))
            }
            Job::Threads => {
                // Each archive that is owed a thread, the archive setting's first.
                let facts = store.facts()?;
                let mut keys: Vec<String> =
                    facts.pending.iter().map(|r| facts.thread_archive.get(r).cloned().unwrap_or_default()).collect();
                keys.sort();
                keys.dedup();
                let mut ok = true;
                for key in keys {
                    let mut plan = settings.plan(vec![], now)?;
                    plan.archive = settings.archive_of(&key).to_string();
                    plan.archive_key = key;
                    ok &= crate::run_plan(&mut store, &settings, plan, HashSet::new(), &agent, &mut |l| self.say(l))?;
                }
                Ok(ok)
            }
            Job::Inboxes { archive } => {
                let mut plan = settings.plan(vec![], now)?;
                plan.archive = settings.archive_of(archive).to_string();
                plan.archive_key = archive.clone();
                plan.inboxes = true;
                crate::run_plan(&mut store, &settings, plan, HashSet::new(), &agent, &mut |l| self.say(l))
            }
            Job::List { archive, inbox, offset } => {
                let mut plan = settings.plan(vec![], now)?;
                plan.archive = settings.archive_of(archive).to_string();
                plan.archive_key = archive.clone();
                plan.listing = Some(sync::Listing {
                    inbox: inbox.clone(),
                    query: BROWSE_QUERY.into(),
                    offset: *offset,
                    limit: PAGE,
                });
                crate::run_plan(&mut store, &settings, plan, HashSet::new(), &agent, &mut |l| self.say(l))
            }
            Job::Message { archive, inbox, msgid } => {
                let mut plan = settings.plan(vec![], now)?;
                plan.archive = settings.archive_of(archive).to_string();
                plan.archive_key = archive.clone();
                plan.messages = vec![(msgid.clone(), inbox.clone())];
                crate::run_plan(&mut store, &settings, plan, HashSet::new(), &agent, &mut |l| self.say(l))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(name: &str) -> Api {
        let dir = std::env::temp_dir().join(format!("patchtrack-api-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Api::new(&dir.join("s.sqlite3"), vec![], false).unwrap()
    }

    fn json_of(a: Answer) -> (u16, Value) {
        match a {
            Answer::Json(status, v) => (status, v),
            Answer::File(..) => panic!("a file"),
        }
    }

    fn get(api: &Api, path: &str) -> Value {
        let (status, v) = json_of(api.handle("GET", path, b""));
        assert_eq!(status, 200, "{path}: {v}");
        v
    }

    fn post(api: &Api, path: &str, body: Value) -> (u16, Value) {
        json_of(api.handle("POST", path, body.to_string().as_bytes()))
    }

    const MBOX: &str = "From x Thu Jan  1 00:00:00 1970\nFrom: Dev <dev@x.org>\nTo: list@x.org\n\
        Subject: [PATCH] foo: do a thing\nMessage-ID: <p1@x>\nDate: Mon, 05 Oct 2026 10:00:00 +0000\n\n\
        Body.\n\nSigned-off-by: Dev <dev@x.org>\n---\ndiff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\n\
        @@ -1 +1 @@\n-a\n+b\n\n\
        From x Thu Jan  1 00:00:00 1970\nFrom: Rev <rev@y.org>\nTo: dev@x.org\nCc: list@x.org\n\
        Subject: Re: [PATCH] foo: do a thing\nMessage-ID: <r1@y>\nIn-Reply-To: <p1@x>\nReferences: <p1@x>\n\
        Date: Tue, 06 Oct 2026 10:00:00 +0000\n\n> Body.\n\nReviewed-by: Rev <rev@y.org>\n";

    #[test]
    fn mail_in_series_out() {
        let api = api("series");
        let before = get(&api, "/v1/status")["generation"].clone();
        let (status, added) = json_of(api.handle("POST", "/v1/import", MBOX.as_bytes()));
        assert_eq!((status, &added["new"]), (200, &json!(2)), "{added}");
        // The store changed: the app is told so, and the list is derived anew.
        assert_ne!(get(&api, "/v1/status")["generation"], before);
        let list = get(&api, "/v1/summaries");
        assert_eq!(list[0]["title"], "foo: do a thing");
        assert_eq!(list[0]["state"], "reviewed");
        assert_eq!(list[0]["replies"], 1);
        let (sender, key) = (list[0]["sender"].as_str().unwrap(), list[0]["key"].as_str().unwrap());
        let q = |s: &str| s.replace('@', "%40").replace(':', "%3A").replace(' ', "+");
        let f = get(&api, &format!("/v1/family?sender={}&key={}", q(sender), q(key)));
        assert_eq!(f["urls"]["p1@x"], "https://lore.kernel.org/all/p1@x/");
        assert_eq!(f["archives"]["p1@x"], "https://lore.kernel.org");
        let files = get(&api, "/v1/files?old=p1%40x&new=p1%40x");
        assert_eq!((&files[0]["path"], &files[0]["marker"]), (&json!("a.c"), &json!("=")));
        let conv = get(&api, "/v1/conversation?root=p1%40x");
        assert_eq!(conv.as_array().unwrap().len(), 2);
        // Reading is no change: the app, polling, must not read everything again.
        let now = get(&api, "/v1/status")["generation"].clone();
        for path in [
            "/v1/summaries",
            "/v1/settings",
            "/v1/drafts",
            "/v1/people?who=everyone",
            "/v1/notifications",
            "/v1/nudges",
        ] {
            get(&api, path);
        }
        assert_eq!(get(&api, "/v1/status")["generation"], now);
        // Closing is yours, and kept.
        let (status, _) = post(&api, "/v1/close", json!({"sender": sender, "key": key, "reason": "dup"}));
        assert_eq!(status, 200);
        assert_eq!(get(&api, "/v1/summaries")[0]["closed"], "dup");
        assert_eq!(post(&api, "/v1/reopen", json!({"sender": sender, "key": key})).1, json!(true));
        assert_eq!(get(&api, "/v1/summaries")[0]["closed"], Value::Null);
        // No such series is a 404, with why.
        let (status, v) = json_of(api.handle("GET", "/v1/conversation?root=nope", b""));
        assert!(status == 404 && v["error"].as_str().unwrap().contains("nope"));
    }

    #[test]
    fn notifications_are_read_one_at_a_time_or_all() {
        let api = api("notifications");
        api.handle("POST", "/v1/import", MBOX.as_bytes());
        // Notifications are about what involves you: here, your own series.
        post(&api, "/v1/settings", json!({"key": "identities", "value": "[\"dev@x.org\"]"}));
        let list = get(&api, "/v1/summaries");
        let (sender, key) = (list[0]["sender"].clone(), list[0]["key"].clone());
        let n = get(&api, "/v1/notifications");
        assert_eq!((&n["first"], n["notifications"].as_array().unwrap().len()), (&json!(true), 0));
        // The first read, of one series or all, is where news starts from.
        assert_eq!(post(&api, "/v1/notifications/read", json!({"sender": sender, "key": key})).1["marked"], 1);
        assert_eq!(get(&api, "/v1/notifications")["first"], false);
        let reply = "From x Thu Jan  1 00:00:00 1970\nFrom: Ann <ann@z.org>\nTo: dev@x.org\n\
            Subject: Re: [PATCH] foo: do a thing\nMessage-ID: <r2@z>\nIn-Reply-To: <p1@x>\nReferences: <p1@x>\n\
            Date: Wed, 07 Oct 2026 10:00:00 +0000\n\n> Body.\n\nWhy?\n";
        api.handle("POST", "/v1/import", reply.as_bytes());
        assert_eq!(get(&api, "/v1/status")["unread"], 1);
        // A stranger's series, on a list or an inbox you browse, tells you nothing.
        let stranger = "From x Thu Jan  1 00:00:00 1970\nFrom: S <s@y.org>\nTo: list@x.org\n\
            Subject: [PATCH] bar: theirs\nMessage-ID: <s1@y>\nDate: Wed, 07 Oct 2026 10:00:00 +0000\n\nFix.\n";
        api.handle("POST", "/v1/import", stranger.as_bytes());
        assert_eq!(get(&api, "/v1/status")["unread"], 1);
        let n = get(&api, "/v1/notifications");
        assert!(n["notifications"][0]["replies"][0].as_str().unwrap().contains("Ann"), "{n}");
        assert_eq!(post(&api, "/v1/notifications/read", json!({"sender": sender, "key": key})).1["marked"], 1);
        assert_eq!(get(&api, "/v1/status")["unread"], 0);
        // All: the stranger's family is recorded too, so it has a baseline.
        assert_eq!(post(&api, "/v1/notifications/read", json!({"all": true})).1["marked"], 1);
        // Read again: nothing written, so nothing for the app to reload.
        let generation = get(&api, "/v1/status")["generation"].clone();
        assert_eq!(post(&api, "/v1/notifications/read", json!({"sender": sender, "key": key})).1["marked"], 0);
        assert_eq!(post(&api, "/v1/notifications/read", json!({"all": true})).1["marked"], 0);
        assert_eq!(get(&api, "/v1/status")["generation"], generation);
        // A closed series tells you nothing.
        post(&api, "/v1/close", json!({"sender": sender, "key": key, "reason": ""}));
        let reply = reply.replace("<r2@z>", "<r3@z>");
        api.handle("POST", "/v1/import", reply.as_bytes());
        assert_eq!(get(&api, "/v1/status")["unread"], 0);
    }

    #[test]
    fn opening_a_series_fetches_its_thread_when_stale() {
        let api = api("open");
        api.handle("POST", "/v1/import", MBOX.as_bytes());
        // Never fetched (only imported): opening it asks for its thread.
        let (status, v) = post(&api, "/v1/open", json!({"root": "p1@x"}));
        assert_eq!((status, &v["fetching"]), (200, &json!(true)), "{v}");
        assert_eq!(get(&api, "/v1/status")["sync"]["threads"], true);
        assert!(Store::open(&api.db).unwrap().facts().unwrap().pending.contains("p1@x"));
        let fetched = |at: &str| {
            let e = sync::Effect::Fetched { root: "p1@x".into(), at: at.into(), archive: String::new() };
            Store::open(&api.db).unwrap().apply(&e).unwrap();
        };
        // Fetched a minute ago: fresh, unless you ask.
        fetched(&mail::utc(now_secs() - 60));
        assert_eq!(post(&api, "/v1/open", json!({"root": "p1@x"})).1["fetching"], false);
        assert_eq!(post(&api, "/v1/open", json!({"root": "p1@x", "force": true})).1["fetching"], true);
        // An hour ago: older than stale_minutes (30).
        fetched(&mail::utc(now_secs() - 3600));
        assert_eq!(post(&api, "/v1/open", json!({"root": "p1@x"})).1["fetching"], true);
        // 0: only when you ask.
        post(&api, "/v1/settings", json!({"key": "stale_minutes", "value": "0"}));
        fetched(&mail::utc(now_secs() - 3600));
        assert_eq!(post(&api, "/v1/open", json!({"root": "p1@x"})).1["fetching"], false);
        assert_eq!(post(&api, "/v1/open", json!({"root": "nope@x"})).0, 404);
    }

    #[test]
    fn an_inbox_is_listed_by_page_and_opened_by_message() {
        let api = api("browse");
        api.handle("POST", "/v1/import", MBOX.as_bytes());
        let linux = r#"[{"name": "Linux", "archive": "https://lore.kernel.org", "lists": ["linux-usb"]}]"#;
        post(&api, "/v1/settings", json!({"key": "projects", "value": linux}));
        let inboxes = get(&api, "/v1/inboxes");
        assert_eq!((&inboxes[0]["project"], &inboxes[0]["inbox"]), (&json!("Linux"), &json!("linux-usb")), "{inboxes}");
        let (status, v) = post(&api, "/v1/browse", json!({"project": "Linux", "inbox": "linux-usb"}));
        assert_eq!(status, 200, "{v}");
        assert_eq!(get(&api, "/v1/status")["sync"]["browsing"], json!(["linux-usb"]));
        // The same page asked twice waits once.
        post(&api, "/v1/browse", json!({"project": "Linux", "inbox": "linux-usb"}));
        assert_eq!(get(&api, "/v1/status")["sync"]["browsing"], json!(["linux-usb"]));
        // A page, as its job stores it: a reply in a thread the store has, and a thread it has not.
        let entry = |id: &str, subject: &str, date: &str| sync::Listed {
            msgid: id.into(),
            subject: subject.into(),
            author: "A <a@z.org>".into(),
            date: date.into(),
            in_reply_to: None,
        };
        let page = sync::Effect::Listed {
            archive: String::new(),
            inbox: "linux-usb".into(),
            offset: 0,
            at: mail::utc(now_secs()),
            entries: vec![
                entry("r1@y", "Re: [PATCH] foo: do a thing", "2026-10-29T10:00:00Z"),
                entry("new@z", "[PATCH] bar: new", "2026-10-28T10:00:00Z"),
            ],
        };
        Store::open(&api.db).unwrap().apply(&page).unwrap();
        let listing = get(&api, "/v1/listing?project=Linux&inbox=linux-usb");
        assert_eq!(listing["next"], 2);
        let rows = listing["rows"].as_array().unwrap();
        assert_eq!(
            (&rows[0]["title"], &rows[0]["family"]["title"]),
            (&json!("[PATCH] foo: do a thing"), &json!("foo: do a thing"))
        );
        assert_eq!((&rows[1]["title"], &rows[1]["family"]), (&json!("[PATCH] bar: new"), &Value::Null));
        // An older page goes on from what was listed.
        post(&api, "/v1/browse", json!({"project": "Linux", "inbox": "linux-usb", "older": true}));
        assert_eq!(get(&api, "/v1/status")["sync"]["browsing"], json!(["linux-usb", "linux-usb"]));
        // Opened: the series it is in; one the store has not, fetched; one fetched and gone, missing.
        let open =
            |id: &str| post(&api, "/v1/open-message", json!({"msgid": id, "project": "Linux", "inbox": "linux-usb"})).1;
        assert_eq!(open("r1@y")["family"]["sender"], "dev@x.org");
        assert_eq!(open("new@z")["fetching"], true);
        let fetched = sync::Effect::Fetched { root: "new@z".into(), at: mail::utc(now_secs()), archive: String::new() };
        Store::open(&api.db).unwrap().apply(&fetched).unwrap();
        assert_eq!(open("new@z")["missing"], true);
        // Every inbox at once is one page like any other.
        assert_eq!(post(&api, "/v1/browse", json!({"inbox": "all"})).0, 200);
        assert_eq!(post(&api, "/v1/browse", json!({"project": "Nope", "inbox": "linux-usb"})).0, 400);
        assert_eq!(post(&api, "/v1/browse", json!({"inbox": "a b/c"})).0, 400);
    }

    #[test]
    fn inboxes_come_newest_first_under_their_projects() {
        let api = api("inbox-list");
        let projects = r#"[{"name": "U-Boot", "archive": "https://lore.kernel.org", "lists": ["u-boot"]},
                          {"name": "Linux", "archive": "https://lore.kernel.org", "lists": []}]"#;
        post(&api, "/v1/settings", json!({"key": "projects", "value": projects}));
        // Not read yet: one archive to read.
        assert_eq!(post(&api, "/v1/inboxes/refresh", json!({})).1["asked"], 1);
        assert_eq!(get(&api, "/v1/status")["sync"]["inboxes"], true);
        let inbox = |name: &str, updated: &str| sync::InboxInfo {
            name: name.into(),
            description: format!("{name} list"),
            updated: Some(updated.into()),
        };
        let read = sync::Effect::Inboxes {
            archive: String::new(),
            at: mail::utc(now_secs()),
            inboxes: vec![
                inbox("netdev", "2026-10-20T00:00:00Z"),
                inbox("u-boot", "2026-10-10T00:00:00Z"),
                inbox("linux-usb", "2026-10-25T00:00:00Z"),
            ],
        };
        Store::open(&api.db).unwrap().apply(&read).unwrap();
        let rows = get(&api, "/v1/inboxes");
        let got: Vec<(&str, &str)> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["project"].as_str().unwrap(), r["inbox"].as_str().unwrap()))
            .collect();
        assert_eq!(got, [("Linux", "linux-usb"), ("Linux", "netdev"), ("U-Boot", "u-boot")]);
        assert_eq!(rows[0]["description"], "linux-usb list");
        // Read just now: not again, unless forced.
        assert_eq!(post(&api, "/v1/inboxes/refresh", json!({})).1["asked"], 0);
        assert_eq!(post(&api, "/v1/inboxes/refresh", json!({"force": true})).1["asked"], 1);
    }

    #[test]
    fn following_asks_for_a_sync() {
        let api = api("follow");
        assert_eq!(get(&api, "/v1/status")["sync"]["asked"], false);
        let (status, v) =
            post(&api, "/v1/follow", json!({"kind": "author", "value": "dev@x.org", "list": "", "project": ""}));
        assert_eq!((status, v), (200, json!(true)));
        assert_eq!(get(&api, "/v1/status")["sync"]["asked"], true);
        assert_eq!(get(&api, "/v1/subscriptions")[0]["value"], "dev@x.org");
        let (status, v) = post(&api, "/v1/follow", json!({"kind": "nobody", "value": "x"}));
        assert!(status == 400 && v["error"].as_str().unwrap().contains("nobody"), "{v}");
    }

    #[test]
    fn a_reply_is_a_draft_from_you() {
        let api = api("reply");
        api.handle("POST", "/v1/import", MBOX.as_bytes());
        post(&api, "/v1/settings", json!({"key": "identities", "value": "[\"me@z.org\"]"}));
        let (status, d) = post(&api, "/v1/reply", json!({"to": "p1@x", "text": "Hm.", "tag": "acked-by"}));
        assert_eq!(status, 200, "{d}");
        assert_eq!((&d["kind"], &d["in_reply_to"]), (&json!("tag"), &json!("p1@x")));
        assert!(
            d["raw"].as_str().unwrap().contains("Message-ID: <") && d["msgid"].as_str().unwrap().ends_with("@z.org")
        );
        assert!(
            d["body"].as_str().unwrap().contains("Acked-by: ") && d["body"].as_str().unwrap().contains("me@z.org"),
            "{d}"
        );
        let (status, v) = post(&api, "/v1/reply", json!({"to": "p1@x", "text": "", "tag": "Liked-by"}));
        assert!(status == 400 && v["error"].as_str().unwrap().contains("not a tag"), "{v}");
        assert_eq!(get(&api, "/v1/drafts").as_array().unwrap().len(), 1);
        // A review: a comment on the patch's added line, and the tag, in one mail
        // (the patch is the series' root).
        let diff_line = |at: usize| json!([{"msgid": "p1@x", "at": at, "text": "Why b?"}]);
        let (status, v) = post(
            &api,
            "/v1/review",
            json!({"root": "p1@x", "comments": diff_line(6), "summary": "Else fine.", "tag": "reviewed-by"}),
        );
        assert_eq!(status, 200, "{v}");
        let body = v[0]["body"].as_str().unwrap();
        assert!(body.contains("> @@ -1 +1 @@\n> -a\n> +b\n\nWhy b?\n\nElse fine.\n\nReviewed-by: "), "{body}");
        assert_eq!((v.as_array().unwrap().len(), &v[0]["kind"]), (1, &json!("review")));
        // A line not in a hunk keeps no draft at all.
        let (status, v) =
            post(&api, "/v1/review", json!({"root": "p1@x", "comments": diff_line(0), "summary": "", "tag": ""}));
        assert!(status == 400 && v["error"].as_str().unwrap().contains("not"), "{v}");
        assert_eq!(get(&api, "/v1/drafts").as_array().unwrap().len(), 2);
        // This server was not allowed to send.
        assert_eq!(post(&api, "/v1/send", json!({"msgid": d["msgid"]})).0, 403);
    }

    #[test]
    fn queries_are_unescaped() {
        let (path, q) = split_query("/v1/family?sender=a%40b.org&key=title%3Afoo+bar&x");
        assert_eq!(path, "/v1/family");
        assert_eq!(q["sender"], "a@b.org");
        assert_eq!(q["key"], "title:foo bar");
        assert_eq!(q["x"], "");
        assert_eq!(unescape("100%"), "100%");
        assert_eq!(unescape("%zz%4"), "%zz%4");
    }
}
