//! Your local `b4 prep` branches, linked to what you posted.
//!
//! b4 keeps its state in an empty cover commit at the base of each prep
//! branch, after a "--- b4-submit-tracking ---" line: the change-id, the next
//! revision to send, and the cover message-id of every version sent. The
//! patches are the commits on top of it. So a branch can say which posted
//! series it is, and whether it has changed since the last version went out.

use std::collections::{BTreeMap, HashSet};

use serde_json::Value;

pub const MARKER: &str = "--- b4-submit-tracking ---";

/// What b4's tracking commit says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracking {
    pub change_id: String,
    /// The next version b4 will send.
    pub revision: u32,
    /// "v5" to the cover message-ids it was sent as.
    pub history: BTreeMap<String, Vec<String>>,
}

impl Tracking {
    /// From the tracking commit's message; None if it is not one.
    pub fn parse(message: &str) -> Option<Tracking> {
        let (_, rest) = message.split_once(MARKER)?;
        let json: String = rest.lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>().join("\n");
        let v: Value = serde_json::from_str(&json).ok()?;
        let s = &v["series"];
        let revision = match &s["revision"] {
            Value::Number(n) => n.as_u64(),
            Value::String(t) => t.parse().ok(),
            _ => Some(1),
        };
        let history = s["history"]
            .as_object()
            .map(|h| {
                h.iter()
                    .map(|(k, ids)| {
                        let ids = ids.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string);
                        (k.clone(), ids.collect())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Tracking {
            change_id: s["change-id"].as_str().unwrap_or_default().to_string(),
            revision: u32::try_from(revision?).ok()?,
            history,
        })
    }

    /// Versions sent, oldest first, as (version, the last cover message-id it was sent as).
    pub fn sent(&self) -> Vec<(u32, String)> {
        let mut out: Vec<(u32, String)> = self
            .history
            .iter()
            .filter_map(|(v, ids)| Some((v.strip_prefix('v')?.parse().ok()?, ids.last()?.clone())))
            .collect();
        out.sort();
        out
    }
}

/// (changed, added, dropped) between a branch's patch-ids and a posting's.
pub fn compare(branch: &[String], posted: &[String]) -> (usize, usize, usize) {
    let (mine, theirs): (HashSet<&String>, HashSet<&String>) = (branch.iter().collect(), posted.iter().collect());
    let same = mine.intersection(&theirs).count();
    let (only_mine, only_theirs) = (mine.len() - same, theirs.len() - same);
    let changed = only_mine.min(only_theirs);
    (changed, only_mine - changed, only_theirs - changed)
}

/// The tags given to a posting (kind, "Name <addr>") that no commit message
/// of the branch carries yet: what `b4 trailers -u` would add. Each once,
/// in order; matched by kind (in any case) and address.
pub fn new_tags(given: &[(String, String)], messages: &[String]) -> Vec<(String, String)> {
    let addr = |who: &str| {
        let a = who.rsplit_once('<').map_or(who, |(_, a)| a.trim_end_matches('>'));
        a.trim().to_lowercase()
    };
    let carried: HashSet<(String, String)> = messages
        .iter()
        .flat_map(|m| m.lines())
        .filter_map(|l| l.split_once(": "))
        .map(|(k, who)| (k.trim().to_lowercase(), addr(who)))
        .collect();
    let mut out: Vec<(String, String)> = vec![];
    for (kind, who) in given {
        let key = (kind.to_lowercase(), addr(who));
        if !carried.contains(&key) && !out.iter().any(|(k, w)| (k.to_lowercase(), addr(w)) == key) {
            out.push((kind.clone(), who.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_commits() {
        let body = "feat: the cover\n\n--- b4-submit-tracking ---\n# This section is used internally by b4 prep\n\
                    {\"series\": {\"revision\": 3, \"change-id\": \"20261001-feat-abcdef123456\", \"prefixes\": [],\n\
                    \"history\": {\"v1\": [\"a@x\"], \"v2\": [\"b@x\", \"b2@x\"], \"v10\": [\"c@x\"], \"vX\": [\"d@x\"]}}}\n";
        let t = Tracking::parse(body).unwrap();
        assert_eq!((t.change_id.as_str(), t.revision), ("20261001-feat-abcdef123456", 3));
        assert_eq!(t.sent(), [(1, "a@x".to_string()), (2, "b2@x".to_string()), (10, "c@x".to_string())]);
        assert!(Tracking::parse("an ordinary commit").is_none());
    }

    #[test]
    fn comparing_patch_ids() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(compare(&ids(&["a", "b"]), &ids(&["a", "b"])), (0, 0, 0));
        assert_eq!(compare(&ids(&["a", "B"]), &ids(&["a", "b"])), (1, 0, 0));
        assert_eq!(compare(&ids(&["a", "B", "c"]), &ids(&["a", "b"])), (1, 1, 0));
        assert_eq!(compare(&ids(&["a"]), &ids(&["a", "b"])), (0, 0, 1));
    }

    #[test]
    fn tags_the_branch_does_not_carry_yet() {
        let tag = |k: &str, w: &str| (k.to_string(), w.to_string());
        let given = [
            tag("Reviewed-By", "Rev One <REV@x.org>"),
            tag("Acked-by", "Ack <ack@x.org>"),
            tag("Reviewed-by", "Rev One <rev@x.org>"), // the same tag on another patch
        ];
        let messages =
            ["usb: one\n\nBody.\n\nReviewed-by: Rev One <rev@x.org>\nSigned-off-by: Me <me@x.org>\n".to_string()];
        assert_eq!(new_tags(&given, &messages), [tag("Acked-by", "Ack <ack@x.org>")]);
        // A trailer line counts in any case, with or without a name.
        let none = ["usb: one\n\nAcked-by: ack@x.org\nreviewed-by: Rev <rev@x.org>\n".to_string()];
        assert!(new_tags(&given, &none).is_empty());
        assert_eq!(new_tags(&given, &[]).len(), 2);
    }
}
