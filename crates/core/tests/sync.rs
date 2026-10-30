//! The sync state machine against an archive in memory, on a simulated clock.
//!
//! Ported from the Python version's tests/test_sync.py and test_lore.py.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use patchtrack_core::Message;
use patchtrack_core::series::Facts;
use patchtrack_core::settings::{Follow, Subscription};
use patchtrack_core::sync::{self, Effect, Known, Method, Outcome, Plan, Request, Sync};

const ME: &str = "dev@example.org";
/// 2026-10-28T00:00:00Z
const NOW: u64 = 1_793_145_600_000;

fn mail(id: &str, subject: &str, from: &str, irt: Option<&str>, date: &str, body: &str) -> Vec<u8> {
    let irt = irt.map(|p| format!("In-Reply-To: <{p}>\nReferences: <{p}>\n")).unwrap_or_default();
    format!("Message-ID: <{id}>\nSubject: {subject}\nFrom: {from}\n{irt}Date: {date}\n\n{body}").into_bytes()
}

fn posting(tag: &str, date: &str) -> (String, Vec<Vec<u8>>) {
    let root = format!("{tag}-0@x");
    (
        root.clone(),
        vec![
            // Each its own title, so each its own family.
            mail(&root, &format!("[PATCH 0/1] {tag}: do the thing"), ME, None, date, ""),
            mail(&format!("{tag}-1@x"), &format!("[PATCH 1/1] {tag}: part 1"), ME, Some(&root), date, ""),
        ],
    )
}

fn mbox(mails: &[&Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for m in mails {
        out.extend_from_slice(b"From mboxrd@z Thu Jan  1 00:00:00 1970\n");
        out.extend_from_slice(m);
        out.push(b'\n');
    }
    out
}

/// An archive: every search returns everything; a thread, the mail that names its root.
#[derive(Default)]
struct Archive {
    mails: Vec<Vec<u8>>,
    broken_search: bool,
    broken_threads: HashSet<String>,
    /// Answers a search's Atom feed with 400, as public-inbox does a query it cannot parse.
    bad_query: bool,
}

impl Archive {
    fn answer(&self, req: &Request) -> Outcome {
        let ok =
            |body| Outcome::Response { status: 200, content_type: "application/mbox".into(), retry_after: None, body };
        let unavailable =
            Outcome::Response { status: 503, content_type: "text/plain".into(), retry_after: None, body: vec![] };
        match req.method {
            Method::Post if self.broken_search => unavailable,
            Method::Post => ok(mbox(&self.mails.iter().collect::<Vec<_>>())),
            Method::Get if req.url.contains("&x=A&") && self.bad_query => {
                Outcome::Response { status: 400, content_type: "text/html".into(), retry_after: None, body: vec![] }
            }
            Method::Get if req.url.contains("&x=A&") => ok(ATOM.as_bytes().to_vec()),
            Method::Get if req.url.ends_with("/manifest.js.gz") => {
                let json = br#"{"/linux-usb/git/0.git": {"modified": 1792000000, "description": "USB"}}"#;
                ok(json.to_vec())
            }
            Method::Get => {
                let id = req.url.rsplit('/').nth(1).expect("a thread URL");
                if self.broken_threads.contains(id) {
                    return unavailable;
                }
                // Any message's t.mbox.gz is its whole thread: what shares its top.
                let parsed: Vec<Message> = self.mails.iter().filter_map(|m| Message::parse(m)).collect();
                let top = |id: &str| {
                    let mut root = id.to_string();
                    while let Some(p) = parsed.iter().find(|m| m.msgid == root).and_then(|m| m.in_reply_to.clone()) {
                        root = p;
                    }
                    root
                };
                let root = top(id);
                let thread = self.mails.iter().zip(&parsed).filter(|(_, m)| top(&m.msgid) == root).map(|(raw, _)| raw);
                ok(mbox(&thread.collect::<Vec<_>>()))
            }
        }
    }
}

/// The store, as far as sync is concerned, and a clock that runs on
/// from one sync to the next.
#[derive(Default)]
struct Model {
    now: u64,
    messages: BTreeMap<String, Message>,
    last_sync: BTreeMap<Subscription, String>,
    fetched: BTreeMap<String, String>,
    found: BTreeMap<String, BTreeSet<Subscription>>,
    thread_archive: BTreeMap<String, String>,
    pending: BTreeSet<String>,
    /// Read from local mail before the next sync.
    local_new: HashSet<String>,
}

struct Run {
    /// The inboxes a read of the manifest found.
    inboxes: Vec<sync::InboxInfo>,
    /// The threads a page of an inbox listed, and for which query.
    listed: Vec<sync::Listed>,
    query: Option<String>,
    /// Why a page was not listed.
    unlisted: Option<String>,
    fetched: Vec<String>,
    /// Every request's URL, in order.
    urls: Vec<String>,
    failed: bool,
    logs: Vec<String>,
    /// Seconds waited before each request.
    waits: Vec<u64>,
}

impl Model {
    fn known(&self) -> Known {
        Known {
            messages: self.messages.values().cloned().collect(),
            last_sync: self.last_sync.clone(),
            facts: Facts {
                fetched: self.fetched.clone().into_iter().collect(),
                pending: self.pending.iter().cloned().collect(),
                found: self.found.clone().into_iter().collect(),
                thread_archive: self.thread_archive.clone().into_iter().collect(),
                ..Facts::default()
            },
            local_new: self.local_new.clone(),
            ..Known::default()
        }
    }

    fn sync(&mut self, archive: &Archive, edit: impl FnOnce(&mut Plan)) -> Run {
        let mut plan = Plan::new("https://lore.example.org", vec![ME.into()], 1_790_000_000);
        plan.delay_ms = 1000;
        edit(&mut plan);
        let mut sync = Sync::new(plan, self.known());
        let mut run = Run {
            fetched: vec![],
            urls: vec![],
            failed: false,
            logs: vec![],
            waits: vec![],
            inboxes: vec![],
            listed: vec![],
            query: None,
            unlisted: None,
        };
        let mut now = self.now.max(NOW);
        while let Some(effect) = sync.next(now) {
            match effect {
                Effect::Fetch(req) => {
                    run.waits.push(req.not_before.saturating_sub(now) / 1000);
                    run.urls.push(req.url.clone());
                    now = now.max(req.not_before);
                    let answer = archive.answer(&req);
                    now += 100;
                    sync.receive(req.id, answer, now);
                }
                Effect::Store(raws) => {
                    for raw in raws {
                        let m = Message::parse(&raw).expect("a message");
                        self.messages.entry(m.msgid.clone()).or_insert(m);
                    }
                }
                Effect::RecordSync { sub, ran_at, .. } => {
                    self.last_sync.insert(sub, ran_at);
                }
                Effect::Found { root, sub } => {
                    self.found.entry(root).or_default().insert(sub);
                }
                Effect::WantThread { root, archive, .. } => {
                    self.thread_archive.insert(root.clone(), archive);
                    self.pending.insert(root);
                }
                Effect::Fetched { root, at, archive } => {
                    self.thread_archive.insert(root.clone(), archive);
                    self.pending.remove(&root);
                    run.fetched.push(root.clone());
                    self.fetched.insert(root, at);
                }
                Effect::TagRoot { .. } => {}
                Effect::Inboxes { inboxes, .. } => run.inboxes = inboxes,
                Effect::Listed { entries, query, .. } => {
                    run.listed = entries;
                    run.query = Some(query);
                }
                Effect::Unlisted { why, .. } => run.unlisted = Some(why),
                Effect::Log(line) => run.logs.push(line),
            }
        }
        run.failed = sync.failed();
        run.fetched.sort();
        self.local_new.clear();
        self.now = now + 1000;
        run
    }
}

fn two_series() -> (Archive, String, String) {
    let (a, ma) = posting("a", "Mon, 05 Oct 2026 10:00:00 +0000");
    let (b, mb) = posting("b", "Mon, 05 Oct 2026 11:00:00 +0000");
    (Archive { mails: [ma, mb].concat(), ..Archive::default() }, a, b)
}

fn reply_to(root: &str) -> Vec<u8> {
    mail(
        "rev@x",
        "Re: [PATCH 0/1] foo: do the thing",
        "R <r@x>",
        Some(root),
        "Tue, 06 Oct 2026 10:00:00 +0000",
        "Why?\n",
    )
}

#[test]
fn first_full_then_only_what_changed() {
    let (mut archive, a, b) = two_series();
    let mut store = Model::default();
    assert_eq!(store.sync(&archive, |_| {}).fetched, vec![a.clone(), b.clone()]);
    assert!(store.sync(&archive, |_| {}).fetched.is_empty());
    archive.mails.push(reply_to(&b));
    let run = store.sync(&archive, |_| {});
    assert_eq!(run.fetched, vec![b]);
    assert!(run.logs.iter().any(|l| l.contains("(1 new)") && l.contains("since the last sync")), "{:?}", run.logs);
}

#[test]
fn new_local_mail_refetches_its_thread() {
    let (archive, _, b) = two_series();
    let mut store = Model::default();
    store.sync(&archive, |_| {});
    assert!(store.sync(&archive, |_| {}).fetched.is_empty());
    // A reply in your mailbox that the search does not find (it did not copy you, say).
    let m = Message::parse(&reply_to(&b)).unwrap();
    store.local_new.insert(m.msgid.clone());
    store.messages.insert(m.msgid.clone(), m);
    assert_eq!(store.sync(&archive, |_| {}).fetched, vec![b]);
}

#[test]
fn stale_open_series_are_refreshed() {
    let (archive, a, b) = two_series();
    let mut store = Model::default();
    store.sync(&archive, |_| {});
    assert_eq!(store.sync(&archive, |p| p.refresh_days = 0).fetched, vec![a, b]);
}

#[test]
fn failed_search_records_nothing() {
    let (mut archive, a, b) = two_series();
    archive.broken_search = true;
    let mut store = Model::default();
    let run = store.sync(&archive, |p| p.retries = 0);
    assert!(run.fetched.is_empty() && run.failed);
    assert!(store.last_sync.is_empty() && store.messages.is_empty());
    archive.broken_search = false;
    // Not incremental: the failed run did not count as a sync.
    assert_eq!(store.sync(&archive, |_| {}).fetched, vec![a, b]);
}

#[test]
fn failed_thread_is_retried() {
    let (mut archive, a, b) = two_series();
    archive.broken_threads.insert(b.clone());
    let mut store = Model::default();
    let run = store.sync(&archive, |p| p.retries = 0);
    assert_eq!((run.fetched, run.failed), (vec![a], true));
    archive.broken_threads.clear();
    assert_eq!(store.sync(&archive, |_| {}).fetched, vec![b]); // nothing new, but b is still owed
    assert!(store.sync(&archive, |_| {}).fetched.is_empty());
}

#[test]
fn failed_refetch_is_retried() {
    let (mut archive, _, b) = two_series();
    let mut store = Model::default();
    store.sync(&archive, |_| {});
    archive.mails.push(reply_to(&b));
    archive.broken_threads.insert(b.clone());
    assert!(store.sync(&archive, |p| p.retries = 0).fetched.is_empty()); // the reply was seen; its thread failed
    archive.broken_threads.clear();
    // No new mail now, and b was fetched recently: only the pending mark brings it back.
    assert_eq!(store.sync(&archive, |_| {}).fetched, vec![b]);
}

#[test]
fn no_threads_defers() {
    let (archive, a, b) = two_series();
    let mut store = Model::default();
    let run = store.sync(&archive, |p| p.threads = false);
    assert!(run.fetched.is_empty());
    assert!(run.logs.contains(&"2 threads left for the next sync to fetch".to_string()));
    assert_eq!(store.sync(&archive, |_| {}).fetched, vec![a, b]);
}

#[test]
fn full_refetches_everything() {
    let (archive, a, b) = two_series();
    let mut store = Model::default();
    store.sync(&archive, |_| {});
    assert_eq!(store.sync(&archive, |p| p.full = true).fetched, vec![a, b]);
}

#[test]
fn requests_are_spaced_by_the_delay() {
    let (archive, _, _) = two_series();
    let run = Model::default().sync(&archive, |p| p.delay_ms = 3000);
    // The first request at once; each after it waits out the delay from the last answer.
    assert_eq!(run.waits.first(), Some(&0));
    assert!(run.waits.len() > 2 && run.waits[1..].iter().all(|&w| w == 3), "{:?}", run.waits);
}

// ---- answers: retries, backoff, bot checks ----

/// One search window, answered from a script of outcomes.
fn one_window(outcomes: Vec<Outcome>, retries: u32) -> (Run, Vec<Vec<u8>>) {
    let mut plan = Plan::new("https://x.org", vec![ME.into()], 1_793_100_000);
    (plan.threads, plan.retries, plan.delay_ms) = (false, retries, 0);
    let mut sync = Sync::new(plan, Known::default());
    let mut outcomes = outcomes.into_iter();
    let (mut run, mut stored, mut now) = (
        Run {
            fetched: vec![],
            urls: vec![],
            failed: false,
            logs: vec![],
            waits: vec![],
            inboxes: vec![],
            listed: vec![],
            query: None,
            unlisted: None,
        },
        vec![],
        NOW,
    );
    while let Some(effect) = sync.next(now) {
        match effect {
            Effect::Fetch(req) => {
                run.waits.push(req.not_before.saturating_sub(now) / 1000);
                now = now.max(req.not_before);
                sync.receive(req.id, outcomes.next().expect("an outcome for each request"), now);
            }
            Effect::Store(raws) => stored.extend(raws),
            Effect::Log(l) => run.logs.push(l),
            _ => {}
        }
    }
    run.failed = sync.failed();
    (run, stored)
}

fn status(code: u16, retry_after: Option<&str>) -> Outcome {
    Outcome::Response {
        status: code,
        content_type: "text/plain".into(),
        retry_after: retry_after.map(str::to_string),
        body: vec![],
    }
}

fn mboxed(body: &[u8]) -> Outcome {
    Outcome::Response { status: 200, content_type: "application/mbox".into(), retry_after: None, body: body.to_vec() }
}

#[test]
fn retries_then_succeeds() {
    let m = mail("a@x", "[PATCH] a", ME, None, "Mon, 26 Oct 2026 10:00:00 +0000", "");
    let (run, stored) = one_window(vec![status(503, None), Outcome::Failed("reset".into()), mboxed(&mbox(&[&m]))], 3);
    assert_eq!(&run.waits[1..], &[5, 10]);
    assert_eq!((stored.len(), run.failed), (1, false));
}

#[test]
fn retry_after_is_honoured() {
    let (run, _) = one_window(vec![status(429, Some("60")), mboxed(b"")], 3);
    assert_eq!(run.waits, vec![0, 60]);
}

#[test]
fn no_retry_on_403_and_404_is_empty() {
    let (run, _) = one_window(vec![status(403, None)], 3);
    assert!(
        run.failed && run.logs.iter().any(|l| l.contains("search failed, nothing recorded") && l.contains("HTTP 403"))
    );
    let (run, stored) = one_window(vec![status(404, None)], 3);
    assert!(!run.failed && stored.is_empty());
}

#[test]
fn gives_up() {
    let (run, _) = one_window(vec![status(502, None), status(502, None), status(502, None), status(502, None)], 3);
    assert!(run.failed);
    assert_eq!(run.waits, vec![0, 5, 10, 20]);
}

#[test]
fn a_bot_check_page_is_an_error() {
    let page = Outcome::Response {
        status: 200,
        content_type: "text/html".into(),
        retry_after: None,
        body: b"<html>".to_vec(),
    };
    let (run, _) = one_window(vec![page], 3);
    assert!(run.failed && run.logs.iter().any(|l| l.contains("blocked by a bot check?")), "{:?}", run.logs);
}

#[test]
fn gzip_is_unpacked() {
    use std::io::Write;
    let m = mail("a@x", "[PATCH] a", ME, None, "Mon, 26 Oct 2026 10:00:00 +0000", "");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&mbox(&[&m])).unwrap();
    let (_, stored) = one_window(vec![mboxed(&gz.finish().unwrap())], 3);
    assert_eq!(stored, vec![m]);
}

#[test]
fn windows_split_and_dedupe() {
    const DAY: i64 = 86_400;
    // One message per hour over 40 days; each search answers its own window.
    let start: i64 = 1_790_000_000;
    let mails: Vec<(i64, Vec<u8>)> = (0..40 * 24)
        .map(|h| (start + h * 3600, mail(&format!("{h}@x"), "x", ME, None, "Mon, 05 Oct 2026 10:00:00 +0000", "")))
        .collect();
    let mut plan = Plan::new("https://x.org", vec![ME.into()], start);
    (plan.threads, plan.delay_ms, plan.split_at) = (false, 0, 500);
    let now = u64::try_from(start + 40 * DAY).unwrap() * 1000;
    let mut sync = Sync::new(plan, Known::default());
    let (mut seen, mut stored) = (vec![], 0);
    while let Some(effect) = sync.next(now) {
        match effect {
            Effect::Fetch(req) => {
                let q = req.url.rsplit("rt%3A").next().unwrap();
                let (lo, hi) = q.split_once("..").unwrap();
                let (lo, hi): (i64, i64) = (lo.parse().unwrap(), hi.parse().unwrap());
                seen.push((lo - start, hi - start));
                let hits: Vec<&Vec<u8>> = mails.iter().filter(|(t, _)| lo <= *t && *t <= hi).map(|(_, m)| m).collect();
                sync.receive(req.id, mboxed(&mbox(&hits)), now);
            }
            Effect::Store(raws) => stored += raws.len(),
            _ => {}
        }
    }
    assert_eq!(stored, mails.len()); // all of them, boundary duplicates merged
    assert_eq!(seen[0], (0, 30 * DAY)); // 721 results: split
    assert!(seen.contains(&(0, 15 * DAY)));
}

#[test]
fn quoting() {
    assert_eq!(sync::form_quote("(f:a@x OR tc:a@x) AND rt:1..2"), "%28f%3Aa%40x+OR+tc%3Aa%40x%29+AND+rt%3A1..2");
    assert_eq!(sync::path_quote("<a/b c@x.org>"), "a%2Fb%20c@x.org");
}

#[test]
fn since() {
    let now = 1_793_145_600; // 2026-10-28T00:00:00Z
    assert_eq!(sync::since("3.months.ago", now), Ok(now - 90 * 86_400));
    assert_eq!(sync::since("1.day.ago", now), Ok(now - 86_400));
    assert_eq!(sync::since("2026-10-23", now), Ok(1_792_713_600));
    assert_eq!(sync::since("2026-10-23T12:00:00Z", now), Ok(1_792_756_800));
    assert_eq!(sync::since("2026-10-23T14:00:00+02:00", now), Ok(1_792_756_800));
    assert!(sync::since("20261023", now).is_err());
    assert!(sync::since("yesterday", now).is_err());
}

// ---- subscriptions beyond an author: a list, a query, a thread ----

fn follow(kind: Follow, value: &str, list: &str) -> Subscription {
    Subscription::new(kind, value, list).unwrap()
}

/// Someone else's series, with a reply.
fn theirs() -> (String, Vec<Vec<u8>>) {
    let root = "o-0@x".to_string();
    let date = "Mon, 05 Oct 2026 12:00:00 +0000";
    let mails = vec![
        mail(&root, "[PATCH 0/1] o: other thing", "Other <o@x>", None, date, ""),
        mail("o-1@x", "[PATCH 1/1] o: part 1", "Other <o@x>", Some(&root), date, ""),
        mail(
            "o-r@x",
            "Re: [PATCH 1/1] o: part 1",
            "R <r@x>",
            Some("o-1@x"),
            "Tue, 06 Oct 2026 10:00:00 +0000",
            "Hm.\n",
        ),
    ];
    (root, mails)
}

#[test]
fn a_list_searches_the_whole_inbox_and_keeps_what_it_finds_without_fetching_it() {
    let (mine, mut mails) = posting("a", "Mon, 05 Oct 2026 10:00:00 +0000");
    let (other, more) = theirs();
    mails.extend(more);
    let archive = Archive { mails, ..Archive::default() };
    let usb = follow(Follow::List, "linux-usb", "");
    let mut store = Model::default();
    let run = store.sync(&archive, |p| {
        p.searches = vec![usb.clone()];
        p.first_since_wide = 1_792_000_000;
    });
    // No author in the query, only the window; the first one starts at the wide start.
    assert!(run.urls[0].starts_with("https://lore.example.org/linux-usb/?x=m&q=rt%3A1792000000.."), "{:?}", run.urls);
    // One request: what the search returned is kept, and no thread is fetched
    // until it is asked for.
    assert_eq!((run.urls.len(), run.fetched.len()), (1, 0), "{:?}", run.urls);
    assert!(store.found[&other].contains(&usb) && store.found[&mine].contains(&usb));
    assert!(store.pending.is_empty());
    // Every thread, when the plan says so (the fetch_all_threads setting).
    let mut store = Model::default();
    let run = store.sync(&archive, |p| {
        p.searches = vec![usb.clone()];
        p.first_since_wide = 1_792_000_000;
        p.wide_threads = true;
    });
    assert_eq!(run.fetched, vec![mine.clone(), other.clone()]);
    assert!(store.found[&other].contains(&usb) && store.found[&mine].contains(&usb));
    assert!(run.logs.iter().any(|l| l.starts_with("every series on linux-usb: 5 messages (5 new), full window")));
    // Incremental the second time, and nothing changed.
    let run = store.sync(&archive, |p| p.searches = vec![usb.clone()]);
    assert_eq!((run.urls.len(), run.fetched.len()), (1, 0), "{:?}", run.urls);
}

#[test]
fn a_manifest_lists_each_inbox_once_by_its_latest_epoch() {
    let json = br#"{
        "/linux-usb/git/0.git": {"modified": 1790000000, "description": "Linux USB"},
        "/linux-usb/git/1.git": {"modified": 1792000000, "description": "Linux USB development [epoch 1]"},
        "/u-boot/git/0.git": {"modified": 1791000000, "description": ""},
        "/v1inbox": {"modified": 0, "description": "Old style"},
        "/": {"modified": 5}
    }"#;
    let got = sync::manifest_inboxes(json).unwrap();
    let names: Vec<(&str, &str, Option<&str>)> =
        got.iter().map(|i| (i.name.as_str(), i.description.as_str(), i.updated.as_deref())).collect();
    assert_eq!(
        names,
        [
            ("linux-usb", "Linux USB development", Some("2026-10-14T17:46:40Z")),
            ("u-boot", "", Some("2026-10-03T04:00:00Z")),
            ("v1inbox", "Old style", None),
        ]
    );
    assert!(sync::manifest_inboxes(b"<html>Anubis</html>").is_err());
}

#[test]
fn reading_the_inboxes_is_one_request_and_nothing_else() {
    let (_, mails) = theirs();
    let archive = Archive { mails, ..Archive::default() };
    let mut store = Model::default();
    store.pending.insert("o-0@x".into());
    let run = store.sync(&archive, |p| p.inboxes = true);
    // Not the author's search, not the thread owed.
    assert_eq!(run.urls, ["https://lore.example.org/manifest.js.gz"]);
    assert_eq!(run.inboxes.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["linux-usb"]);
}

/// A page of an Atom feed, as public-inbox writes a search's: escaped, an
/// entry's message-id the last part of its link.
const ATOM: &str = r#"<?xml version="1.0" encoding="us-ascii"?>
<feed xmlns="http://www.w3.org/2005/Atom"><title>s:PATCH - search results</title><link
rel="alternate" href="https://lore.example.org/linux-usb/?q=s:PATCH"/><updated>2026-10-29T23:46:22Z</updated>
<entry><author><name>Ann &amp; Co</name><email>ann@x.org</email></author><title>Re: [PATCH] o: other &lt;thing&gt;</title><updated>2026-10-29T23:46:22Z</updated><link
href="https://lore.example.org/linux-usb/o-r@x/"/><id>urn:uuid:1</id><thr:in-reply-to
ref="urn:uuid:2"
href="https://lore.example.org/linux-usb/o-1@x/"/><content
type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><pre style="white-space:pre-wrap">&gt; quoted
<a href="https://example.org/">a link</a></pre></div></content></entry>
<entry><author><name>Dev</name><email>dev@x.org</email></author><title>[PATCH v2 0/3] a: caf&#233;</title><updated>2026-10-28T10:00:00Z</updated><link
href="https://lore.example.org/linux-usb/a%2Fb@x/"/><id>urn:uuid:3</id><content
type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><pre>Cover.</pre></div></content></entry>
</feed>"#;

#[test]
fn an_atom_page_lists_each_thread_by_its_newest_message() {
    let got = sync::atom_entries(ATOM);
    assert_eq!(
        got,
        [
            sync::Listed {
                msgid: "o-r@x".into(),
                subject: "Re: [PATCH] o: other <thing>".into(),
                author: "Ann & Co <ann@x.org>".into(),
                date: "2026-10-29T23:46:22Z".into(),
                in_reply_to: Some("o-1@x".into()),
            },
            sync::Listed {
                msgid: "a/b@x".into(),
                subject: "[PATCH v2 0/3] a: café".into(),
                author: "Dev <dev@x.org>".into(),
                date: "2026-10-28T10:00:00Z".into(),
                in_reply_to: None,
            },
        ]
    );
    assert!(sync::atom_entries("<html>Anubis</html>").is_empty());
}

#[test]
fn a_page_of_an_inbox_is_one_request_and_no_mail() {
    let (_, mails) = theirs();
    let archive = Archive { mails, ..Archive::default() };
    let mut store = Model::default();
    store.pending.insert("o-0@x".into());
    let listing = sync::Listing { inbox: "linux-usb".into(), query: "s:PATCH".into(), offset: 50, limit: 50 };
    let run = store.sync(&archive, |p| p.listing = Some(listing.clone()));
    // Not the author's search, not the thread owed: the page alone, newest threads first, from 50.
    assert_eq!(run.urls, ["https://lore.example.org/linux-usb/?q=s%3APATCH&x=A&t=1&l=50&o=50"]);
    assert_eq!(run.listed.iter().map(|l| l.msgid.as_str()).collect::<Vec<_>>(), ["o-r@x", "a/b@x"]);
    assert!(store.messages.is_empty() && store.last_sync.is_empty());
    assert_eq!((run.query.as_deref(), run.unlisted), (Some("s:PATCH"), None));
}

#[test]
fn a_search_is_a_listing_of_any_query_and_a_query_the_archive_refuses_says_so() {
    let mut store = Model::default();
    let listing = sync::Listing { inbox: "all".into(), query: "f:ann \"other thing\"".into(), offset: 0, limit: 50 };
    let run = store.sync(&Archive::default(), |p| p.listing = Some(listing.clone()));
    assert_eq!(run.urls, ["https://lore.example.org/all/?q=f%3Aann+%22other+thing%22&x=A&t=1&l=50&o=0"]);
    assert_eq!(run.query.as_deref(), Some("f:ann \"other thing\""));
    assert!(!run.failed && run.unlisted.is_none());
    // 400 is not a bot check and not retried: the query is the problem, and the person is told so.
    let bad = Archive { bad_query: true, ..Archive::default() };
    let run = store.sync(&bad, |p| p.listing = Some(listing.clone()));
    assert_eq!(run.urls.len(), 1);
    assert!(run.failed && run.listed.is_empty());
    let why = run.unlisted.unwrap_or_default();
    assert!(why.ends_with("o=0: HTTP 400: the archive could not parse the query"), "{why}");
}

#[test]
fn a_listed_thread_is_fetched_by_its_message_from_its_inbox() {
    let (root, mails) = theirs();
    let archive = Archive { mails, ..Archive::default() };
    let mut store = Model::default();
    let run = store.sync(&archive, |p| {
        p.searches = vec![];
        p.messages = vec![("o-r@x".into(), "linux-usb".into())];
    });
    assert!(run.urls.ends_with(&["https://lore.example.org/linux-usb/o-r@x/t.mbox.gz".to_string()]), "{:?}", run.urls);
    // Its top is the root, fetched; the message it was asked by is no longer owed.
    assert!(run.fetched.contains(&root) && store.pending.is_empty(), "{:?}", run.fetched);
    assert_eq!(store.messages.len(), 3);
    // Not followed: nothing found it, nothing was recorded.
    assert!(store.found.is_empty() && store.last_sync.is_empty());
}

#[test]
fn a_query_is_searched_as_given_in_its_inbox() {
    let (other, mails) = theirs();
    let archive = Archive { mails, ..Archive::default() };
    let q = follow(Follow::Query, "dfn:drivers/usb/", "linux-usb");
    let mut store = Model::default();
    let run = store.sync(&archive, |p| {
        p.searches = vec![q.clone()];
        p.wide_threads = true;
    });
    let want = format!("https://lore.example.org/linux-usb/?x=m&q={}", sync::form_quote("(dfn:drivers/usb/) AND rt:"));
    assert!(run.urls[0].starts_with(&want), "{:?}", run.urls);
    assert_eq!(run.fetched, vec![other.clone()]);
    assert!(store.found[&other].contains(&q));
    // The author's own searches and the query's are recorded apart.
    assert_eq!(store.last_sync.keys().collect::<Vec<_>>(), [&q]);
}

#[test]
fn a_followed_thread_is_fetched_by_any_message_then_by_its_root_while_open() {
    let (other, mails) = theirs();
    let archive = Archive { mails, ..Archive::default() };
    // Pasted: the reply's URL, not the root's.
    let t = follow(Follow::Thread, "https://lore.kernel.org/linux-usb/o-r@x/", "");
    let mut store = Model::default();
    let run = store.sync(&archive, |p| p.searches = vec![t.clone()]);
    // From the inbox its URL names.
    assert_eq!(run.urls, ["https://lore.example.org/linux-usb/o-r@x/t.mbox.gz"], "no search, one thread");
    // Its top is the root, fetched whole and found by the subscription.
    assert!(run.fetched.contains(&other), "{:?}", run.fetched);
    assert!(store.found[&other].contains(&t));
    assert_eq!(store.messages.len(), 3);
    // Next time the store knows it: by its root, as any thread.
    let run = store.sync(&archive, |p| p.searches = vec![t.clone()]);
    assert_eq!(run.urls, [format!("https://lore.example.org/all/{other}/t.mbox.gz")]);
}

#[test]
fn an_author_and_the_same_author_in_one_list_sync_apart() {
    let (archive, ..) = two_series();
    let mut store = Model::default();
    let (all, usb) = (follow(Follow::Author, ME, ""), follow(Follow::Author, ME, "linux-usb"));
    store.sync(&archive, |p| p.searches = vec![all.clone()]);
    let run = store.sync(&archive, |p| p.searches = vec![all.clone(), usb.clone()]);
    assert!(run.logs.iter().any(|l| l.contains("in all, since the last sync")), "{:?}", run.logs);
    assert!(run.logs.iter().any(|l| l.contains("in linux-usb, full window")), "{:?}", run.logs);
}

// ---- several archives ----

#[test]
fn a_thread_is_fetched_from_the_inbox_it_was_found_in() {
    let (archive, a, b) = two_series();
    let mut store = Model::default();
    let gcc = follow(Follow::Author, ME, "gcc-patches").on("https://inbox.sourceware.org").unwrap();
    let run = store.sync(&archive, |p| {
        p.archive = "https://inbox.sourceware.org".into();
        p.archive_key = gcc.archive.clone();
        p.searches = vec![gcc.clone()];
    });
    // Not every archive serves lore's "all": the list the search found it in.
    let threads: Vec<String> = run.urls.iter().filter(|u| u.ends_with("t.mbox.gz")).cloned().collect();
    assert_eq!(
        threads,
        [
            format!("https://inbox.sourceware.org/gcc-patches/{a}/t.mbox.gz"),
            format!("https://inbox.sourceware.org/gcc-patches/{b}/t.mbox.gz")
        ]
    );
    assert_eq!(store.thread_archive[&a], "https://inbox.sourceware.org");
    // An author in "all" still fetches from "all", as before.
    let run = Model::default().sync(&archive, |_| {});
    assert!(run.urls.iter().any(|u| u.ends_with(&format!("/all/{a}/t.mbox.gz"))), "{:?}", run.urls);
}

#[test]
fn a_thread_owed_by_one_archive_waits_for_that_archive() {
    let (mut archive, a, _) = two_series();
    let mut store = Model::default();
    store.sync(&archive, |p| p.threads = false); // both owed, from the archive setting's archive
    archive.broken_threads.clear();
    let other = |p: &mut Plan| {
        p.archive = "https://inbox.sourceware.org".into();
        p.archive_key = "https://inbox.sourceware.org".into();
        p.searches = vec![];
    };
    // Another archive's sync does not fetch them...
    assert!(store.sync(&archive, other).fetched.is_empty());
    // ...their own does.
    assert!(store.sync(&archive, |p| p.searches = vec![]).fetched.contains(&a));
}
