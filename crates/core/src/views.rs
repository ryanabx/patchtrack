//! Shapes of the derived data that more than one frontend shows: who
//! replied, which series a query names, the message-ids evidence cites.

use std::collections::{BTreeMap, BTreeSet};

use crate::evidence::APPROVALS;
use crate::series::{Facts, Family, Series, State};
use crate::settings::{Follow, Subscription};
use crate::sync::path_quote;

/// A message's page on the archive.
pub fn message_url(archive: &str, msgid: &str) -> String {
    format!("{}/all/{}/", archive.trim_end_matches('/'), path_quote(msgid))
}

/// The message-ids an evidence line cites: each "<id>" that ends one of its
/// "; "-separated parts.
pub fn evidence_msgids(evidence: &str) -> Vec<String> {
    let mut out = vec![];
    for part in evidence.split(';') {
        let part = part.trim_end();
        if let Some(open) = part.rfind(" <")
            && let Some(id) = part[open + 2..].strip_suffix('>')
            && !id.is_empty()
            && !id.contains(['<', '>', ' ', '\t'])
        {
            out.push(id.to_string());
        }
    }
    out
}

/// What a family's row in a list shows, without its mail: a list of a
/// thousand families is a tenth the size of the families themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Summary {
    pub sender: String,
    /// Addresses of patch authors other than the sender, in any version:
    /// with `sender`, whose series it is ("sent or wrote").
    pub written_by: BTreeSet<String>,
    pub key: String,
    pub state: State,
    pub evidence: String,
    /// Why you closed it, if you did.
    pub closed: Option<String>,
    /// How many postings the family has.
    pub versions: u32,
    /// The latest version's title, version number and patch count.
    pub title: String,
    pub version: u32,
    pub patches: u32,
    pub lists: BTreeSet<String>,
    pub unanswered: bool,
    /// Replies to the latest version from people (not bots, not its sender).
    pub replies: u32,
    /// When the latest version was posted.
    pub posted: String,
    pub last_activity: String,
    /// Whether the latest version's thread was ever read whole, from the
    /// archive or your mailbox: if not, only what a search returned is known.
    pub fetched: bool,
    /// The projects it is in (see `projects::of`); filled by who knows them.
    pub projects: Vec<String>,
}

pub fn summary(f: &Family) -> Summary {
    let s = f.head();
    let address = |w: &String| w.rsplit_once('<').map_or(w.as_str(), |(_, a)| a.trim_end_matches('>')).to_string();
    Summary {
        sender: f.sender.clone(),
        written_by: f.versions.iter().flat_map(|v| v.written_by.iter().map(address)).collect(),
        key: f.key.clone(),
        state: f.state,
        evidence: f.evidence.clone(),
        closed: f.closed.clone(),
        versions: u32::try_from(f.versions.len()).unwrap_or(u32::MAX),
        title: s.title.clone(),
        version: s.version,
        patches: u32::try_from(s.patches.keys().filter(|&&i| i > 0).count().max(1)).unwrap_or(u32::MAX),
        lists: s.lists.clone(),
        unanswered: s.unanswered,
        replies: u32::try_from(s.replies.iter().filter(|r| !r.bot && !r.mine).count()).unwrap_or(u32::MAX),
        posted: s.date.clone(),
        last_activity: s.last_activity().to_string(),
        fetched: s.as_of.is_some() || s.local_as_of.is_some(),
        projects: vec![],
    }
}

/// Whether you follow a series: one of `authors` (your identities, say)
/// sent or wrote it, or one of `subs` covers it: a followed author sent or
/// wrote it, it is on a followed list, it is a followed thread, or a
/// list's or a query's sync found it.
pub fn follows(s: &Series, authors: &[String], subs: &[Subscription], facts: &Facts) -> bool {
    let has = |id: &str| s.root == id || s.patches.values().any(|p| p == id) || s.replies.iter().any(|r| r.msgid == id);
    authors.iter().any(|a| s.is_by(a))
        || subs.iter().any(|sub| match sub.kind {
            Follow::Author => s.is_by(&sub.value),
            Follow::List => {
                let on = facts.archives.get(&s.root).is_some_and(|a| a.contains(&sub.archive));
                (on && s.lists.contains(&sub.list)) || facts.found_by(&s.root, sub)
            }
            Follow::Query => facts.found_by(&s.root, sub),
            Follow::Thread => facts.found_by(&s.root, sub) || has(&sub.value),
        })
}

/// Someone who replied, and how, across the series given.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Person {
    /// "Name <addr>", as they last signed a reply.
    pub who: String,
    pub bot: bool,
    /// The families they replied in, by key.
    pub series: BTreeSet<String>,
    pub replies: u32,
    /// Replies that gave an approval: Reviewed-by, Acked-by, a profile's.
    pub tags: u32,
    /// Inline reviews without a tag.
    pub comments: u32,
    pub applied: u32,
    pub last: String,
}

/// Everyone who replied to these series (but their senders), most recent first.
pub fn people<'a>(series: impl IntoIterator<Item = &'a Series>) -> Vec<Person> {
    let mut out: BTreeMap<String, Person> = BTreeMap::new();
    for s in series {
        for r in s.replies.iter().filter(|r| !r.mine) {
            let p = out.entry(r.who.to_lowercase()).or_insert_with(|| Person {
                who: r.who.clone(),
                bot: r.bot,
                series: BTreeSet::new(),
                replies: 0,
                tags: 0,
                comments: 0,
                applied: 0,
                last: String::new(),
            });
            p.series.insert(s.family.clone());
            p.replies += 1;
            p.tags += u32::from(r.trailers.iter().any(|(k, _)| APPROVALS.contains(&k.as_str())));
            p.comments += u32::from(r.is_review_comment());
            p.applied += u32::from(r.applied.is_some());
            if r.date > p.last {
                p.last = r.date.clone();
            }
        }
    }
    let mut people: Vec<Person> = out.into_values().collect();
    people.sort_by(|a, b| b.last.cmp(&a.last));
    people
}

/// The series a query names (part of a root message-id, or of a title in
/// any case): the latest version of each matching family, oldest first;
/// with `all`, every matching version, by family and version.
pub fn pick<'a>(families: &'a [Family], query: &str, all: bool) -> Vec<&'a Series> {
    let q = query.to_lowercase();
    let hit = |s: &Series| s.root.contains(query) || s.title.to_lowercase().contains(&q);
    let mut out: Vec<&Series> = vec![];
    for f in families {
        let hits = f.versions.iter().filter(|s| hit(s));
        if all {
            out.extend(hits);
        } else if let Some(latest) = hits.max_by(|a, b| (a.version, &a.date).cmp(&(b.version, &b.date))) {
            out.push(latest);
        }
    }
    if all {
        out.sort_by(|a, b| (&a.family, a.version, &a.date).cmp(&(&b.family, b.version, &b.date)));
    } else {
        out.sort_by(|a, b| a.date.cmp(&b.date));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_cited_messages() {
        assert_eq!(message_url("https://lore.kernel.org/", "a/b@x"), "https://lore.kernel.org/all/a%2Fb@x/");
        let ev = "Reviewed-by: R <r@x> <rev1@x>; applied by M <m@x> <app@x>; 2 replies";
        assert_eq!(evidence_msgids(ev), ["rev1@x", "app@x"]);
        assert!(evidence_msgids("no reply").is_empty());
    }

    #[test]
    fn what_you_follow() {
        let raw = |id: &str, from: &str, subject: &str, irt: &str| {
            let irt = if irt.is_empty() { String::new() } else { format!("In-Reply-To: <{irt}>\n") };
            format!(
                "Message-ID: <{id}>\nFrom: {from}\nSubject: {subject}\n{irt}Date: Mon, 05 Oct 2026 10:00:00 +0000\n\nx\n"
            )
        };
        let msgs: Vec<crate::Message> = [
            raw("a@x", "A <a@x.org>", "[PATCH] a: one", ""),
            raw("b@x", "B <b@x.org>", "[PATCH] b: two", ""),
            raw("br@x", "R <r@x.org>", "Re: [PATCH] b: two", "b@x"),
            raw("c@x", "C <c@x.org>", "[PATCH] c: three", ""),
        ]
        .iter()
        .map(|r| crate::Message::parse(r.as_bytes()).unwrap())
        .collect();
        let sub = |k, v: &str| Subscription::new(k, v, "").unwrap();
        let mut facts = Facts::default();
        facts.lists.insert("c@x".into(), ["linux-usb".to_string()].into());
        facts.archives.insert("c@x".into(), [String::new()].into());
        let q = sub(Follow::Query, "dfn:x");
        facts.found.insert("a@x".into(), [q.clone()].into());
        let series = crate::series::build(&msgs, &facts, &Default::default());
        let who = |authors: &[&str], subs: &[Subscription]| {
            let authors: Vec<String> = authors.iter().map(|a| a.to_string()).collect();
            series.values().filter(|s| follows(s, &authors, subs, &facts)).map(|s| s.root.as_str()).collect::<Vec<_>>()
        };
        assert_eq!(who(&["b@x.org"], &[]), ["b@x"]);
        assert_eq!(who(&[], &[sub(Follow::Author, "a@x.org")]), ["a@x"]);
        assert_eq!(who(&[], &[sub(Follow::List, "linux-usb")]), ["c@x"]);
        // The same list name on another archive is another list.
        let elsewhere = sub(Follow::List, "linux-usb").on("https://example.org").unwrap();
        assert!(who(&[], &[elsewhere]).is_empty());
        assert_eq!(who(&[], &[q]), ["a@x"]);
        // A thread, by any message in it.
        assert_eq!(who(&[], &[sub(Follow::Thread, "br@x")]), ["b@x"]);
        assert!(who(&[], &[]).is_empty());
    }

    #[test]
    fn a_summary_names_who_wrote_what_someone_else_sent() {
        let raw = "Message-ID: <p@x>\nFrom: Relay <relay@x>\nSubject: [PATCH] foo: bar\n\
                   Date: Mon, 05 Oct 2026 10:00:00 +0000\n\nFrom: Jane Dev <Jane@X.org>\n\nBody.\n---\n a | 1 +\n";
        let msgs = [crate::Message::parse(raw.as_bytes()).unwrap()];
        let facts = crate::series::Facts::default();
        let fams = crate::series::families(crate::series::build(&msgs, &facts, &Default::default()), &facts);
        let s = summary(&fams[0]);
        assert_eq!(
            (s.sender.as_str(), s.written_by.iter().map(String::as_str).collect::<Vec<_>>()),
            ("relay@x", vec!["jane@x.org"])
        );
    }
}
