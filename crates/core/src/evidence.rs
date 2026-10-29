//! What a reply says: its tags, an "applied" announcement, inline comments.
//!
//! Calibrated on real mail (see AGENTS.md on `main`): the patterns are the
//! Python version's, unchanged.

use std::sync::LazyLock;

use regex::Regex;

use crate::subject;

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid built-in pattern")
}

static TRAILER: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?im)^\s*(Reviewed-by|Acked-by|Tested-by|Reported-by|Suggested-by|Nacked-by):\s*(.+?)\s*$"));
static APPLIED_LINE: LazyLock<Regex> = LazyLock::new(|| {
    re(concat!(
        r"(?i)^\s*(?:",
        r"(?:thanks,?\s+)?(?:applied|queued|picked up)\b",
        r"|(?:i'?ve|i have|i)\s+(?:now\s+)?(?:applied|queued|picked up)\b.*\b(?:to|into|thanks)\b",
        r"|this (?:patch|series) was applied to",
        // QEMU: "This was merged into master (<sha>)."
        r"|(?:this|the) (?:patch |series |change )?(?:was|has been|is now|got) (?:merged|applied|pushed|committed)",
        r" (?:into|to|in)\b",
        r")",
    ))
});
/// b4 ty: "[1/2] subject\n      commit: <sha>"
static B4_TY: LazyLock<Regex> = LazyLock::new(|| re(r"(?m)^\s*\[(\d+)/(\d+)\]\s.*\n\s+commit:\s*([0-9a-f]{7,40})"));
/// A line that is nothing but an announcement: "Applied.", "Queued, thanks."
static BARE_APPLIED: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^\s*(?:thanks,?\s+)?(?:applied|queued)[.!]?(?:,?\s*thanks[.!]?)?\s*$"));
/// "Committed to trunk as r17-1234, others to follow": only part of a series.
static PARTIAL: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b(?:others?|the rest|rest|remaining(?: ones| patches)?)\b[^.;]*?\b(?:follow|later|to come)\b")
});
static CLOSING: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)^(thanks|thank you|regards|cheers|best|br|kind regards|looks good|lgtm)\b|wrote:$"));

/// A sign-off or a quote's attribution line ("Thanks,", "LGTM", "On Mon, X
/// wrote:"), not prose that answers anything.
pub fn is_closing(text: &str) -> bool {
    CLOSING.is_match(text)
}

/// The tags that approve a patch; "Approved" is a profile's "OK.".
pub const APPROVALS: [&str; 3] = ["Reviewed-By", "Acked-By", "Approved"];

fn is_signature(line: &str) -> bool {
    line.trim_end() == "--"
}

fn is_quoted(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

/// The body without quoted lines, and without anything after a signature.
pub fn unquoted(body: &str) -> String {
    body.lines().take_while(|l| !is_signature(l)).filter(|l| !is_quoted(l)).collect::<Vec<_>>().join("\n")
}

/// Python's str.title(): "reviewed-by" -> "Reviewed-By".
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut after_letter = false;
    for c in s.chars() {
        if after_letter {
            out.extend(c.to_lowercase())
        } else {
            out.extend(c.to_uppercase())
        }
        after_letter = c.is_alphabetic();
    }
    out
}

/// Trailers given in a reply, (kind, who), ignoring quoted text.
pub fn trailers(body: &str) -> Vec<(String, String)> {
    TRAILER.captures_iter(&unquoted(body)).map(|c| (title_case(&c[1]), c[2].to_string())).collect()
}

/// Lines of prose written below quoted text: the reviewer's comments.
///
/// Skips the "On ..., X wrote:" attribution, trailers, sign-offs, and lines
/// under three words; anything after a signature is ignored.
pub fn inline_comments(body: &str) -> Vec<String> {
    let mut quoted = false;
    let mut out = Vec::new();
    for line in body.lines().take_while(|l| !is_signature(l)) {
        if is_quoted(line) {
            quoted = true;
            continue;
        }
        let text = line.trim();
        if quoted
            && text.split_whitespace().count() >= 3
            && !TRAILER.find(text).is_some_and(|m| m.start() == 0)
            && !CLOSING.is_match(text)
        {
            out.push(text.to_string());
        }
    }
    out
}

/// Unquoted lines that start a paragraph (after a blank or quoted line),
/// or are a bare announcement. A wrapped sentence's continuation ("...as it
/// was\napplied first.") is not an announcement, however it begins.
fn paragraph_starts(body: &str) -> impl Iterator<Item = &str> {
    let mut prev = "";
    body.lines().take_while(|l| !is_signature(l)).filter(move |&line| {
        let starts = prev.trim().is_empty() || is_quoted(prev);
        prev = line;
        !is_quoted(line) && !line.trim().is_empty() && (starts || BARE_APPLIED.is_match(line))
    })
}

/// Evidence in a reply that a maintainer applied something.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Applied {
    /// The line that says so.
    pub line: String,
    /// The patches it names (b4 ty's list); empty means the message replied to.
    pub indices: Vec<u32>,
    /// Only part of what it replies to: "(subset)", or "the rest will follow".
    pub subset: bool,
}

impl Applied {
    /// A profile's "landed" line, which names no patches.
    pub fn landed(line: String) -> Applied {
        let subset = partial(&line);
        Applied { line, indices: Vec::new(), subset }
    }
}

pub fn partial(line: &str) -> bool {
    PARTIAL.is_match(line)
}

/// Evidence in a reply that a maintainer applied something, or None.
pub fn applied(body: &str, subject: &str) -> Option<Applied> {
    let line = paragraph_starts(body).find(|l| APPLIED_LINE.is_match(l))?.trim().to_string();
    let text = unquoted(body);
    let mut indices: Vec<u32> = B4_TY.captures_iter(&text).filter_map(|c| c[1].parse().ok()).collect();
    indices.sort_unstable();
    indices.dedup();
    let subset = subject::parse(subject).subset || partial(&line);
    Some(Applied { line, indices, subset })
}

/// The first paragraph-starting line of body that a pattern matches.
pub fn first_match(body: &str, patterns: &[Regex]) -> Option<String> {
    if patterns.is_empty() {
        return None;
    }
    paragraph_starts(body).map(str::trim).find(|l| patterns.iter().any(|p| p.is_match(l))).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailers_skip_quotes() {
        let body = "> Reviewed-by: Old <old@x>\nLooks fine.\n\nAcked-by: Rev <rev@x>\n";
        assert_eq!(trailers(body), vec![("Acked-By".to_string(), "Rev <rev@x>".to_string())]);
        assert_eq!(trailers("reviewed-BY:   R <r@x>  \n")[0].0, "Reviewed-By");
    }

    #[test]
    fn b4_ty() {
        let body = "On Mon, X wrote:\n> blah\n\nApplied, thanks!\n\n[2/3] mfd: add the thing\n      commit: 0123456789ab\n\nBest regards,\n";
        let a = applied(body, "Re: (subset) [PATCH 0/3] foo").unwrap();
        assert_eq!((a.indices, a.subset), (vec![2], true));
    }

    #[test]
    fn plain_applied() {
        let a = applied("Applied to the for-next branch of foo.git.\n", "").unwrap();
        assert!(a.indices.is_empty());
    }

    #[test]
    fn merged_into() {
        assert!(applied("This was merged into master (0123456789abcdef).\n", "").is_some());
        assert!(applied("This should be merged into the other series first.\n", "").is_none());
    }

    #[test]
    fn patchwork_bot() {
        assert!(applied("Hello:\n\nThis series was applied to netdev/net-next.git\n", "").is_some());
    }

    #[test]
    fn bare_line_after_prose() {
        assert!(applied("Fairly simple one and clearly a net gain.\nApplied.\n", "").is_some());
    }

    #[test]
    fn after_a_quote() {
        assert!(applied("> [PATCH] foo\nApplied to master, thanks!\n", "").is_some());
    }

    #[test]
    fn not_applied() {
        // The last three are wrapped sentences seen on gcc-patches, git@ and xen-devel.
        for body in [
            "> Applied, thanks!\nWhy?\n",
            "Can this be applied to stable as well?\n",
            "Thanks.\n-- \nApplied Signal Corp\n",
            "The attribute is now\napplied to struct after its definition\".\n",
            "This one depends on the other being\napplied first.\n",
            "Those tests will be\npicked up and run automatically in Gitlab CI.\n",
        ] {
            assert!(applied(body, "").is_none(), "{body}");
        }
    }

    #[test]
    fn partial_lines() {
        assert!(partial("Committed to trunk as r17-1234, others to follow."));
        assert!(partial("Applied 1/3 to for-next, the rest will follow later."));
        assert!(!partial("Applied to master, thanks. Other tests look fine."));
    }

    #[test]
    fn inline() {
        let body = "On Mon, 5 Oct 2026, Dev wrote:\n> +\tval = readl(x);\n\nThis needs a lock here.\n\n> +\treturn 0;\n\n\
                    Thanks for the patch!\nReviewed-by: R <r@x>\nok\n-- \nsome signature with many words\n";
        assert_eq!(inline_comments(body), vec!["This needs a lock here."]);
        assert!(inline_comments("Looks fine to me overall.\n\n> +\tx = 1;\n").is_empty());
    }

    #[test]
    fn first_match_needs_a_paragraph_start() {
        let p = [Regex::new("(?i)^ok").unwrap()];
        assert_eq!(first_match("> quoted\nOK.\n", &p).as_deref(), Some("OK."));
        assert_eq!(first_match("It seems\nok to me\n", &p), None);
        assert_eq!(first_match("OK.\n", &[]), None);
    }
}
