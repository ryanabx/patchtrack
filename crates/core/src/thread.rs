//! A series' conversation: its mail in reply order, each with what the
//! derivation knows about it. How to show it (quotes folded, colours) is
//! the frontend's.

use std::collections::{HashMap, HashSet};

use crate::mail::Message;
use crate::series::Series;
use crate::vdiff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize), serde(rename_all = "lowercase"))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Kind {
    Cover,
    Patch,
    Reply,
}

/// One message of a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Entry {
    pub msgid: String,
    pub in_reply_to: Option<String>,
    /// 0 for a message with no parent in the conversation.
    pub depth: u32,
    pub kind: Kind,
    /// Its patch number, or the number of the patch it replies to; 0 is the cover.
    pub patch: u32,
    pub from_name: String,
    pub from_addr: String,
    pub date: String,
    pub subject: String,
    /// Sent by the series' sender.
    pub mine: bool,
    pub bot: bool,
    /// A reply to the posting itself, not to another reply.
    pub direct: bool,
    pub body: String,
    /// A patch's commit message, and its diff (from the "---" or the first diff on).
    pub message: Option<String>,
    pub diff: Option<String>,
    /// (kind, who) of each tag a reply gives.
    pub tags: Vec<(String, String)>,
    /// The line that says it was applied.
    pub applied: Option<String>,
    /// Inline prose under quoted text.
    pub comments: Vec<String>,
}

/// (message, depth) of every stored message in series s, in reply order:
/// each message after its parent; siblings by date, then patch number (a
/// series is often sent within one second).
pub fn order<'a>(s: &Series, messages: &HashMap<&str, &'a Message>) -> Vec<(&'a Message, u32)> {
    let ids: HashSet<&str> = std::iter::once(s.root.as_str())
        .chain(s.patches.values().map(String::as_str))
        .chain(s.replies.iter().map(|r| r.msgid.as_str()))
        .collect();
    let index: HashMap<&str, u32> = s.patches.iter().map(|(&i, m)| (m.as_str(), i)).collect();
    let mut mail: Vec<&Message> = ids.iter().filter_map(|id| messages.get(id).copied()).collect();
    mail.sort_by(|a, b| {
        let key = |m: &Message| index.get(m.msgid.as_str()).copied().unwrap_or(u32::MAX);
        (&a.date, key(a), &a.msgid).cmp(&(&b.date, key(b), &b.msgid))
    });
    let parent_of = |m: &'a Message| -> Option<&'a str> {
        m.in_reply_to.as_deref().filter(|p| *p != m.msgid && ids.contains(p) && messages.contains_key(p))
    };
    let mut children: HashMap<Option<&str>, Vec<&Message>> = HashMap::new();
    for &m in &mail {
        children.entry(parent_of(m)).or_default().push(m);
    }
    let mut out = Vec::with_capacity(mail.len());
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack: Vec<(&Message, u32)> = children.get(&None).into_iter().flatten().rev().map(|m| (*m, 0)).collect();
    while let Some((m, depth)) = stack.pop() {
        if !seen.insert(&m.msgid) {
            continue;
        }
        out.push((m, depth));
        stack.extend(children.get(&Some(m.msgid.as_str())).into_iter().flatten().rev().map(|c| (*c, depth + 1)));
    }
    // A cycle of replies has no root to start from: show it anyway, at the top.
    for m in mail {
        if seen.insert(&m.msgid) {
            out.push((m, 0));
        }
    }
    out
}

/// The conversation of series s, from its stored messages by id.
pub fn conversation(s: &Series, messages: &HashMap<&str, &Message>) -> Vec<Entry> {
    let index: HashMap<&str, u32> = s.patches.iter().map(|(&i, m)| (m.as_str(), i)).collect();
    let replies: HashMap<&str, &crate::series::Reply> = s.replies.iter().map(|r| (r.msgid.as_str(), r)).collect();
    order(s, messages)
        .into_iter()
        .map(|(m, depth)| {
            let mut e = Entry {
                msgid: m.msgid.clone(),
                in_reply_to: m.in_reply_to.clone(),
                depth,
                kind: Kind::Reply,
                patch: 0,
                from_name: m.from.name.clone(),
                from_addr: m.from.addr.clone(),
                date: m.date.clone(),
                subject: m.subject.clone(),
                mine: m.from.addr == s.sender,
                bot: false,
                direct: false,
                body: m.body.clone(),
                message: None,
                diff: None,
                tags: vec![],
                applied: None,
                comments: vec![],
            };
            if let Some(&i) = index.get(m.msgid.as_str()) {
                e.patch = i;
                if i == 0 {
                    e.kind = Kind::Cover;
                } else {
                    e.kind = Kind::Patch;
                    let (message, diff) = vdiff::split_raw(&m.body);
                    (e.message, e.diff) = (Some(message), Some(diff));
                }
            } else if let Some(r) = replies.get(m.msgid.as_str()) {
                (e.patch, e.mine, e.bot, e.direct) = (r.index, r.mine, r.bot, r.direct);
                e.tags = r.trailers.clone();
                e.applied = r.applied.as_ref().map(|a| a.line.clone());
                e.comments = r.comments.clone();
            }
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rules;
    use crate::series::{self, Facts};

    /// irt: the reference chain, oldest first; the last is In-Reply-To.
    fn mail(id: &str, subject: &str, from: &str, irt: Option<&str>, date: &str, body: &str) -> Message {
        let irt = irt
            .map(|chain| {
                let refs: Vec<String> = chain.split(' ').map(|r| format!("<{r}>")).collect();
                format!("In-Reply-To: {}\nReferences: {}\n", refs.last().unwrap(), refs.join(" "))
            })
            .unwrap_or_default();
        Message::parse(
            format!("Message-ID: <{id}>\nSubject: {subject}\nFrom: {from}\n{irt}Date: {date}\n\n{body}").as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn reply_order_and_depth() {
        let d = |h: u32| format!("Mon, 05 Oct 2026 1{h}:00:00 +0000");
        let msgs = vec![
            mail("c@x", "[PATCH 0/2] t: thing", "Dev <dev@x>", None, &d(0), "Cover.\n"),
            // Sent in the same second as patch 1: patch number decides.
            mail("p2@x", "[PATCH 2/2] t: two", "Dev <dev@x>", Some("c@x"), &d(0), "Two.\n---\ndiff --git a/b b/b\n"),
            mail("p1@x", "[PATCH 1/2] t: one", "Dev <dev@x>", Some("c@x"), &d(0), "One.\n---\ndiff --git a/a b/a\n"),
            mail(
                "r1@x",
                "Re: [PATCH 1/2] t: one",
                "Rev <r@x>",
                Some("c@x p1@x"),
                &d(2),
                "> One.\nWhy is this needed here?\n",
            ),
            mail("r2@x", "Re: [PATCH 1/2] t: one", "Dev <dev@x>", Some("c@x p1@x r1@x"), &d(3), "Because.\n"),
            mail("r3@x", "Re: [PATCH 0/2] t: thing", "Rev <r@x>", Some("c@x"), &d(1), "Reviewed-by: Rev <r@x>\n"),
        ];
        let found = series::build(&msgs, &Facts::default(), &Rules::default());
        let by_id: HashMap<&str, &Message> = msgs.iter().map(|m| (m.msgid.as_str(), m)).collect();
        let conv = conversation(&found["c@x"], &by_id);
        let got: Vec<(&str, u32, Kind)> = conv.iter().map(|e| (e.msgid.as_str(), e.depth, e.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("c@x", 0, Kind::Cover),
                ("p1@x", 1, Kind::Patch),
                ("r1@x", 2, Kind::Reply),
                ("r2@x", 3, Kind::Reply),
                ("p2@x", 1, Kind::Patch),
                ("r3@x", 1, Kind::Reply),
            ]
        );
        assert_eq!(
            (conv[1].message.as_deref(), conv[1].diff.as_deref()),
            (Some("One."), Some("---\ndiff --git a/a b/a"))
        );
        assert!(conv[3].mine && !conv[2].mine && conv[2].direct && !conv[3].direct);
        assert_eq!(conv[5].tags, vec![("Reviewed-By".to_string(), "Rev <r@x>".to_string())]);
        assert_eq!(conv[2].comments, vec!["Why is this needed here?"]);
    }
}
