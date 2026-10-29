//! What changed since you last looked: GitHub's notifications for email.
//!
//! A snapshot of each family records what `news` last showed: its state,
//! its head version and the replies it had. The next look reports the
//! difference and takes a new snapshot. Snapshots are a record of what you
//! were shown, not of the mail, so the store keeps them apart (`seen`).

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::mail::Message;
use crate::rules::Rules;
use crate::series::{self, Facts, Family, Reply, Series, State};

/// What `news` shows of a family, as the JSON the store keeps.
pub fn snapshot(f: &Family) -> String {
    let mut replies: Vec<&str> =
        f.versions.iter().flat_map(|s| &s.replies).filter(|r| !r.mine).map(|r| r.msgid.as_str()).collect();
    replies.sort_unstable();
    json!({"state": f.state.as_str(), "head": f.head().root, "version": f.head().version, "replies": replies})
        .to_string()
}

/// How one family differs from its snapshot.
#[derive(Debug, Clone)]
pub struct Change<'a> {
    pub family: &'a Family,
    /// No snapshot: a family not seen before.
    pub new_series: bool,
    /// A newer version was posted.
    pub new_version: Option<u32>,
    /// (before, after).
    pub state: Option<(State, State)>,
    /// New replies from others, oldest first, with the version each is on.
    pub replies: Vec<(&'a Series, &'a Reply)>,
}

impl Change<'_> {
    fn any(&self) -> bool {
        self.new_series || self.new_version.is_some() || self.state.is_some() || !self.replies.is_empty()
    }
}

/// A change for every family that differs from its snapshot in `seen`
/// (by [`Family::id`]). A snapshot that cannot be read counts as none.
pub fn changes<'a>(families: impl IntoIterator<Item = &'a Family>, seen: &HashMap<String, String>) -> Vec<Change<'a>> {
    let mut out = vec![];
    for f in families {
        let mut c = Change { family: f, new_series: false, new_version: None, state: None, replies: vec![] };
        let Some(old) = seen.get(&f.id()).and_then(|s| serde_json::from_str::<Value>(s).ok()) else {
            c.new_series = true;
            out.push(c);
            continue;
        };
        let head = f.head();
        if old["version"].as_u64().is_some_and(|v| u64::from(head.version) > v) && old["head"] != json!(head.root) {
            c.new_version = Some(head.version);
        }
        if let Some(before) = old["state"].as_str().and_then(State::parse)
            && before != f.state
        {
            c.state = Some((before, f.state));
        }
        let known: Vec<&str> = old["replies"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        c.replies = f
            .versions
            .iter()
            .flat_map(|s| s.replies.iter().map(move |r| (s, r)))
            .filter(|(_, r)| !r.mine && !known.contains(&r.msgid.as_str()))
            .collect();
        c.replies.sort_by(|a, b| a.1.date.cmp(&b.1.date));
        if c.any() {
            out.push(c);
        }
    }
    out
}

/// One line saying what a reply did: applied, tagged, commented or replied.
pub fn describe(s: &Series, r: &Reply) -> String {
    let name = r.who.split(" <").next().filter(|n| !n.is_empty()).unwrap_or(&r.who);
    let who = format!("{name}{}", if r.bot { " (bot)" } else { "" });
    let on = if s.total > 1 { format!("on {}/{}", r.index, s.total) } else { String::new() };
    let what = if let Some(a) = &r.applied {
        format!("\"{}\"", a.line)
    } else if !r.trailers.is_empty() {
        let mut kinds: Vec<&str> = r.trailers.iter().map(|(k, _)| k.as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        kinds.join(", ")
    } else if r.direct && !r.comments.is_empty() {
        format!("commented: \"{}\"", r.comments[0])
    } else {
        "replied".into()
    };
    let date = r.date.get(..10).unwrap_or(&r.date);
    [date, who.as_str(), on.as_str(), what.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" ")
}

/// Snapshots of every family from the mail dated up to `when` (UTC,
/// "2026-10-05T10:00:00Z"): what `news --since` compares with.
///
/// Only mail carries its own dates. What the store learned later (tree
/// checks, fetches) is used on both sides, or a store created after `when`
/// would report everything it ever checked as news.
pub fn baseline(messages: &[Message], facts: &Facts, rules: &Rules, when: &str) -> HashMap<String, String> {
    let then: Vec<Message> = messages.iter().filter(|m| m.date.as_str() <= when).cloned().collect();
    series::families(series::build(&then, facts, rules), facts).iter().map(|f| (f.id(), snapshot(f))).collect()
}
