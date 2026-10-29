//! Which commits in a git tree are which posted patches.
//!
//! The host runs git (whole branches, never shallow) and hands over each
//! commit in scope and the patch-ids it computed; this decides. Matchers,
//! strongest first:
//!
//! - message-id: the posting's message-id in a `Link:` or `Message-ID:`
//!   trailer. A message-id cited elsewhere in a commit message names some
//!   other patch, and does not count;
//! - patch-id: `git patch-id --stable` of the commit equals the posting's;
//! - subject: the same subject and author address; the diff was changed.
//!
//! A found commit that a later commit in the same tree reverts ("This
//! reverts commit <sha>", as `git revert` writes it) is reported with the
//! revert, unless that revert was itself reverted.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;

use crate::mail::utc;
use crate::series::{Role, parse_utc};
use crate::subject::normalize_title;

static DIFF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^diff --git a/(\S+) b/(\S+)").expect("valid"));

/// A tree `check` looks in: a ref in a local repository, optionally fetched
/// whole from a URL into `refs/patchtrack/NAME`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub name: String,
    pub repo: String,
    pub git_ref: String,
    pub url: Option<String>,
    pub branch: Option<String>,
    pub role: Role,
}

/// A posted patch, as `check` looks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub msgid: String,
    /// The subject's title, without the [PATCH] prefix.
    pub title: String,
    /// The patch's author, lower case.
    pub author: String,
    pub date: String,
    pub body: String,
}

/// A commit in scope, as `git log --format=%H%x00%ae%x00%s%x00%B` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub subject: String,
    pub message: String,
}

/// Where a posted patch was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub commit: String,
    /// message-id, patch-id or subject.
    pub how: &'static str,
    /// The revert that undid it, if it still stands undone.
    pub reverted_by: Option<String>,
}

/// The files the patches touch: only commits touching them are read.
pub fn paths(posted: &[Posted]) -> Vec<String> {
    let all: BTreeSet<String> = posted
        .iter()
        .flat_map(|p| DIFF.captures_iter(&p.body).flat_map(|c| [c[1].to_string(), c[2].to_string()]))
        .filter(|f| f != "/dev/null")
        .collect();
    all.into_iter().collect()
}

/// Read commits from two days before the earliest posting on; None if no
/// posting has a date.
pub fn since(posted: &[Posted]) -> Option<String> {
    posted.iter().filter_map(|p| parse_utc(&p.date)).min().map(|t| utc(t - 2 * 86_400))
}

/// Parse `git log --format=%H%x00%ae%x00%s%x00%B%x1e` output.
pub fn commits(log: &str) -> Vec<Commit> {
    log.split('\x1e')
        .filter_map(|rec| {
            let mut f = rec.trim_matches('\n').splitn(4, '\0');
            let (sha, author, subject, message) = (f.next()?, f.next()?, f.next()?, f.next()?);
            Some(Commit { sha: sha.into(), author: author.into(), subject: subject.into(), message: message.into() })
        })
        .collect()
}

/// Does a `Link:` or `Message-ID:` trailer of this commit message name
/// the message-id, as a whole word of a URL or between angle brackets?
fn names(message: &str, msgid: &str) -> bool {
    let id = msgid.to_ascii_lowercase();
    let before = |c: Option<char>| c.is_none_or(|c| c.is_whitespace() || c == '/' || c == '<');
    let after = |c: Option<char>| c.is_none_or(|c| c.is_whitespace() || c == '/' || c == '>');
    message.lines().any(|line| {
        let l = line.to_ascii_lowercase();
        let Some(rest) = l.strip_prefix("link:").or_else(|| l.strip_prefix("message-id:")) else { return false };
        let start = l.len() - rest.len();
        l.match_indices(&id)
            .filter(|(at, _)| *at >= start)
            .any(|(at, _)| before(l[..at].chars().next_back()) && after(l[at + id.len()..].chars().next()))
    })
}

/// The first match by trailer for each posting, by position.
pub fn by_message_id(posted: &[Posted], commits: &[Commit]) -> Vec<Option<String>> {
    posted.iter().map(|p| commits.iter().find(|c| names(&c.message, &p.msgid)).map(|c| c.sha.clone())).collect()
}

/// The revert that last undid sha, or None if it stands: each revert of
/// the previous revert flips it (a revert of a revert puts it back).
pub fn undone_by(commits: &[Commit], sha: &str) -> Option<String> {
    let (mut cur, mut by) = (sha.to_string(), None::<String>);
    let mut seen = BTreeSet::new();
    loop {
        let needle = format!("This reverts commit {cur}");
        let Some(r) = commits.iter().find(|c| c.message.contains(&needle)) else { return by };
        if !seen.insert(r.sha.clone()) {
            return by;
        }
        by = if by.is_some() { None } else { Some(r.sha.clone()) };
        cur = r.sha.clone();
    }
}

/// Where each posting is, by position. `commit_ids` is patch-id to the
/// first commit (in log order) with it; `posted_ids` the postings'
/// patch-ids, by position, None where not computed or there is no diff.
pub fn decide(
    posted: &[Posted],
    commits: &[Commit],
    commit_ids: &HashMap<String, String>,
    posted_ids: &[Option<String>],
) -> Vec<Option<Found>> {
    let mut out: Vec<Option<(String, &'static str)>> =
        by_message_id(posted, commits).into_iter().map(|c| c.map(|sha| (sha, "message-id"))).collect();
    for (i, slot) in out.iter_mut().enumerate() {
        if slot.is_none()
            && let Some(sha) = posted_ids.get(i).and_then(Option::as_ref).and_then(|pid| commit_ids.get(pid))
        {
            *slot = Some((sha.clone(), "patch-id"));
        }
    }
    for (p, slot) in posted.iter().zip(out.iter_mut()) {
        if slot.is_none() {
            let want = (normalize_title(&p.title), p.author.to_lowercase());
            if let Some(c) = commits.iter().find(|c| (normalize_title(&c.subject), c.author.to_lowercase()) == want) {
                *slot = Some((c.sha.clone(), "subject"));
            }
        }
    }
    // A revert touches the same files after the commit, so it is in scope.
    out.into_iter()
        .map(|hit| hit.map(|(commit, how)| Found { reverted_by: undone_by(commits, &commit), commit, how }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_trailer_names_the_posting() {
        assert!(names("a: x\n\nLink: https://patch.msgid.link/link-1@x\n", "link-1@x"));
        assert!(names("a: x\n\nMessage-ID: <Link-1@X>\n", "link-1@x"));
        assert!(names("a: x\n\nlink: https://lore.kernel.org/r/link-1@x/\n", "link-1@x"));
        // Cited in the body, not a trailer.
        assert!(!names("a: x\n\n[1] https://lore.kernel.org/all/link-1@x/\n", "link-1@x"));
        // Part of a longer message-id.
        assert!(!names("Link: https://lore.kernel.org/r/2-link-1@x\n", "link-1@x"));
        assert!(!names("Link: https://lore.kernel.org/r/link-1@x.y\n", "link-1@x"));
        // Indented: not a trailer.
        assert!(!names("  Link: https://lore.kernel.org/r/link-1@x\n", "link-1@x"));
    }

    #[test]
    fn reverts_and_reverts_of_reverts() {
        let c = |sha: &str, message: &str| Commit {
            sha: sha.into(),
            author: String::new(),
            subject: String::new(),
            message: message.into(),
        };
        // Newest first, as git log gives them.
        let log =
            [c("r2", "Revert \"Revert\"\n\nThis reverts commit r1."), c("r1", "This reverts commit a."), c("a", "a")];
        assert_eq!(undone_by(&log[1..], "a").as_deref(), Some("r1"));
        assert_eq!(undone_by(&log, "a"), None);
        assert_eq!(undone_by(&log, "nothing"), None);
    }

    #[test]
    fn paths_and_since() {
        let p = Posted {
            msgid: "m@x".into(),
            title: "t".into(),
            author: "a@x".into(),
            date: "2026-10-05T10:00:00Z".into(),
            body: "x\n---\ndiff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\ndiff --git a/new.c b/new.c\nnew file mode 100644\n--- /dev/null\n".into(),
        };
        assert_eq!(paths(std::slice::from_ref(&p)), ["a.c", "new.c"]);
        assert_eq!(since(&[p]).as_deref(), Some("2026-10-03T10:00:00Z"));
    }

    #[test]
    fn parses_the_log_format() {
        let log = "aaa\0dev@x\0a: one\0a: one\n\nLink: x\n\x1e\nbbb\0dev@x\0b: two\0b: two\n\x1e\n";
        let got = commits(log);
        assert_eq!(got.len(), 2);
        assert_eq!((got[1].sha.as_str(), got[1].subject.as_str()), ("bbb", "b: two"));
        assert!(got[0].message.contains("Link: x"));
    }
}
