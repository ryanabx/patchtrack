//! Your own mail: which of it to keep, and which threads it shows.
//!
//! A mailbox holds much that is not about the series you follow, so only
//! what matters is kept: patches the authors sent or wrote, and replies
//! whose References or In-Reply-To reach a kept or already stored message
//! (followed through replies to replies). Headers are read first; a
//! message is parsed in full only when its subject says it is a patch and
//! its sender is not one of the authors, because the in-body From: may
//! still name one. Reading the files is the host's.
//!
//! Local mail holds only what was sent to you: a reply that dropped you
//! from Cc is not in it. Threads it showed are marked with the time, and
//! their "as of" text says "from your mailbox only".

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::mail::{Head, Message};
use crate::series::Series;
use crate::subject;

/// The indices of the messages worth keeping, in order. `known` is every
/// message-id the store holds; `authors` are lower case.
pub fn relevant(raws: &[impl AsRef<[u8]>], known: &HashSet<String>, authors: &[String]) -> Vec<usize> {
    let by_author = |a: &str| authors.iter().any(|x| x == a);
    let mut threads: HashSet<String> = HashSet::new();
    let mut keep = vec![false; raws.len()];
    // Not (yet) kept: (index, the message-ids it refers to).
    let mut others: Vec<(usize, Vec<String>)> = vec![];
    for (i, raw) in raws.iter().enumerate() {
        let Some(h) = Head::parse(raw.as_ref()) else { continue };
        if known.contains(&h.msgid) {
            continue;
        }
        let s = subject::parse(&h.subject);
        let mine = s.is_patch
            && !s.is_reply
            && (by_author(&h.from.addr) || Message::parse(raw.as_ref()).is_some_and(|m| by_author(&m.author.addr)));
        if mine {
            keep[i] = true;
            threads.insert(h.msgid);
        } else {
            let mut refs = h.refs;
            refs.extend(h.in_reply_to);
            others.push((i, refs));
        }
    }
    // Replies to replies: a pass per level of the thread found in this batch.
    loop {
        let before = others.len();
        others.retain(|(i, refs)| {
            if refs.iter().any(|r| known.contains(r) || threads.contains(r)) {
                keep[*i] = true;
                threads.insert(Head::parse(raws[*i].as_ref()).expect("parsed before").msgid);
                false
            } else {
                true
            }
        });
        if others.len() == before {
            break;
        }
    }
    keep.iter().enumerate().filter_map(|(i, &k)| k.then_some(i)).collect()
}

/// The roots of the series local mail shows now: those with a message new
/// from it, and those it showed before (`local`, root to when), whose
/// mailbox was just read again.
pub fn shown_roots(
    series: &BTreeMap<String, Series>,
    new: &HashSet<String>,
    local: &HashMap<String, String>,
) -> Vec<String> {
    series
        .values()
        .filter(|s| {
            local.contains_key(&s.root)
                || new.contains(&s.root)
                || s.patches.values().any(|p| new.contains(p))
                || s.replies.iter().any(|r| new.contains(&r.msgid))
        })
        .map(|s| s.root.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(id: &str, from: &str, subject: &str, irt: Option<&str>, body: &str) -> Vec<u8> {
        let irt = irt.map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n")).unwrap_or_default();
        format!(
            "Message-ID: <{id}>\nFrom: {from}\nSubject: {subject}\n{irt}Date: Mon, 05 Oct 2026 10:00:00 +0000\n\n{body}"
        )
        .into_bytes()
    }

    fn kept(raws: &[Vec<u8>], known: &[&str]) -> Vec<usize> {
        let known = known.iter().map(|k| k.to_string()).collect();
        relevant(raws, &known, &["me@x.org".to_string()])
    }

    #[test]
    fn keeps_your_patches_and_their_threads() {
        let raws = [
            mail("cover@x", "Me <me@x.org>", "[PATCH 0/1] foo: thing", None, ""),
            mail("p1@x", "Me <Me@X.org>", "[PATCH 1/1] foo: part", Some("cover@x"), ""),
            mail("r1@x", "R <r@x>", "Re: [PATCH 1/1] foo: part", Some("p1@x"), "> +x\nWhy?\n"),
            // A reply to a reply, earlier in the mailbox than what it answers.
            mail("r3@x", "Me <me@x.org>", "Re: [PATCH 1/1] foo: part", Some("r2@x"), "Because.\n"),
            mail("r2@x", "R <r@x>", "Re: [PATCH 1/1] foo: part", Some("r1@x"), "Hm?\n"),
            mail("noise@x", "S <s@x>", "[PATCH] somebody else's patch", None, ""),
            mail("chat@x", "Me <me@x.org>", "lunch?", None, ""),
            // Your reply in someone else's thread is not about your series.
            mail("mine-elsewhere@x", "Me <me@x.org>", "Re: [PATCH] somebody else's patch", Some("noise@x"), ""),
        ];
        assert_eq!(kept(&raws, &[]), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_patch_you_wrote_that_someone_else_sent() {
        let raws = [mail("p@x", "Colleague <c@x>", "[PATCH] foo: yours", None, "From: Me <me@x.org>\n\nFix.\n")];
        assert_eq!(kept(&raws, &[]), [0]);
        let theirs = [mail("p@x", "Colleague <c@x>", "[PATCH] foo: theirs", None, "From: Other <o@x>\n\nFix.\n")];
        assert!(kept(&theirs, &[]).is_empty());
    }

    #[test]
    fn replies_to_stored_threads_and_nothing_already_stored() {
        let raws = [
            mail("r@x", "R <r@x>", "Re: [PATCH] foo: old", Some("old@x"), "ok\n"),
            mail("old@x", "Me <me@x.org>", "[PATCH] foo: old", None, ""),
        ];
        assert_eq!(kept(&raws, &["old@x"]), [0]);
    }
}
