//! Who is a bot, and how a project says "approved" and "landed".

use std::sync::LazyLock;

use regex::Regex;

use crate::evidence;

static BOT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(^|[.+@-])(bot|robot|noreply|no-reply)([.+@-]|$)|^lkp@intel\.com$").expect("valid")
});

/// fnmatch-style: `*`, `?`, `[abc]`, `[!abc]`; everything else literal.
pub fn glob(pattern: &str, text: &str) -> bool {
    fn class(p: &[char], c: char) -> Option<(bool, usize)> {
        // p starts after '['; returns (matched, length of the class including ']')
        let (negate, start) = if p.first() == Some(&'!') { (true, 1) } else { (false, 0) };
        let mut i = start;
        let mut hit = false;
        while i < p.len() && (p[i] != ']' || i == start) {
            if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
                hit |= p[i] <= c && c <= p[i + 2];
                i += 3;
            } else {
                hit |= p[i] == c;
                i += 1;
            }
        }
        (i < p.len()).then_some((hit != negate, i + 1))
    }
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|k| go(&p[1..], &t[k..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some('[') => match (t.first(), class(&p[1..], *t.first().unwrap_or(&'\0'))) {
                (Some(_), Some((true, n))) => go(&p[1 + n..], &t[1..]),
                (Some(_), Some((false, _))) | (None, _) => false,
                // An unclosed '[' is a literal.
                (Some(&c), None) => c == '[' && go(&p[1..], &t[1..]),
            },
            Some(&c) => t.first() == Some(&c) && go(&p[1..], &t[1..]),
        }
    }
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    go(&p, &t)
}

/// Which senders are bots; their replies are not review.
#[derive(Debug, Clone)]
pub struct BotRules {
    /// The built-in rule: "bot", "robot", "noreply" as a word of the address, and lkp@intel.com.
    pub builtin: bool,
    pub addresses: Vec<String>,
    pub patterns: Vec<String>,
    /// Never bots, whatever matched.
    pub humans: Vec<String>,
    /// Regexes on the unquoted body, for people who also send bot mail.
    pub bodies: Vec<Regex>,
}

impl Default for BotRules {
    fn default() -> Self {
        BotRules { builtin: true, addresses: vec![], patterns: vec![], humans: vec![], bodies: vec![] }
    }
}

impl BotRules {
    pub fn is_bot(&self, address: &str, body: &str) -> bool {
        let a = address.to_lowercase();
        if self.humans.contains(&a) {
            return false;
        }
        if self.addresses.contains(&a) || self.patterns.iter().any(|p| glob(p, &a)) {
            return true;
        }
        if !self.bodies.is_empty() {
            let text = evidence::unquoted(body);
            if self.bodies.iter().any(|r| r.is_match(&text)) {
                return true;
            }
        }
        self.builtin && BOT.is_match(&a)
    }
}

/// How a project says "approved" and "landed", for the series sent to it.
#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    /// Globs on the posting's To: and Cc: addresses.
    pub recipients: Vec<String>,
    /// Regexes on a reply's paragraph-starting lines: an approval.
    pub approvals: Vec<Regex>,
    /// The same, for "it is in"; counted from the author too.
    pub landed: Vec<Regex>,
}

impl Profile {
    pub fn matches<'a>(&self, addresses: impl IntoIterator<Item = &'a String>) -> bool {
        addresses.into_iter().any(|a| self.recipients.iter().any(|g| glob(g, a)))
    }
}

/// Measured on a week of gcc-patches (2026-10-26): reviewers approve with a
/// bare "OK."; "OK for mainline?" and "OK, but..." are conversation. Authors
/// commit themselves and say "Pushed as r17-1234".
pub fn sourceware() -> Profile {
    Profile {
        name: "sourceware".into(),
        recipients: vec!["*@gcc.gnu.org".into(), "*@sourceware.org".into()],
        approvals: vec![
            Regex::new(concat!(
                r"(?i)^(?:this (?:patch |version )?is )?(?:ok|lgtm)",
                r"(?: (?:for|to) [\w./+-]+(?: [\w./+-]+)?)?(?:[.!](?:\s|$)|$)"
            ))
            .expect("valid"),
        ],
        landed: vec![Regex::new(r"(?i)^(?:ok and |now )?(?:pushed|committed|installed)\b").expect("valid")],
    }
}

pub fn builtin_profiles() -> Vec<Profile> {
    vec![sourceware()]
}

/// Everything derivation is configured by.
#[derive(Debug, Clone)]
pub struct Rules {
    pub bots: BotRules,
    /// The first that matches a posting's recipients applies.
    pub profiles: Vec<Profile>,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { bots: BotRules::default(), profiles: builtin_profiles() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob("*@gcc.gnu.org", "gcc-patches@gcc.gnu.org"));
        assert!(!glob("*@gcc.gnu.org", "gcc-patches@gcc.gnu.org.evil"));
        assert!(glob("runner?@ci.example.org", "runner7@ci.example.org"));
        assert!(glob("[a-c]x[!0-9]", "bxz"));
        assert!(!glob("[a-c]x[!0-9]", "bx1"));
        assert!(glob("a[b", "a[b"));
        assert!(glob("*", ""));
    }

    #[test]
    fn builtin_bots() {
        let r = BotRules::default();
        assert!(r.is_bot("sashiko-bot@kernel.org", ""));
        assert!(r.is_bot("lkp@intel.com", ""));
        assert!(!r.is_bot("abbott@example.org", "")); // "bot" inside a word
    }

    #[test]
    fn configured_bots() {
        let r = BotRules {
            addresses: vec!["ci@x.org".into()],
            patterns: vec!["*@ci.example.org".into()],
            humans: vec!["review-bot@x.org".into()],
            bodies: vec![Regex::new("(?m)^My bot found errors").unwrap()],
            ..BotRules::default()
        };
        assert!(r.is_bot("CI@x.org", ""));
        assert!(r.is_bot("runner7@ci.example.org", ""));
        assert!(!r.is_bot("review-bot@x.org", "")); // humans beat the built-in rule
        assert!(r.is_bot("reviewer@example.org", "My bot found errors running 'make'\n"));
        assert!(!r.is_bot("reviewer@example.org", "> My bot found errors\nAcked-by: Rev\n"));
        assert!(!BotRules { builtin: false, ..BotRules::default() }.is_bot("x-bot@y.org", ""));
    }

    #[test]
    fn sourceware_approvals() {
        let rx = sourceware().approvals;
        for line in ["OK.", "Ok", "LGTM.", "This is OK.", "OK for trunk.", "Ok for trunk!", "OK. No issues."] {
            assert!(evidence::first_match(line, &rx).is_some(), "{line}");
        }
        for line in ["OK for mainline?", "ok, will do.", "OK, not really wrong, but still odd", "Okay then"] {
            assert!(evidence::first_match(line, &rx).is_none(), "{line}");
        }
    }
}
