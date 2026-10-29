//! What a subject line says: reply or not, patch or not, version, n/m, RFC, RESEND.

/// A parsed subject: `[RFC PATCH v3 2/4] drm/foo: unplug the device`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subject {
    pub is_reply: bool,
    pub is_patch: bool,
    pub version: u32,
    pub index: Option<u32>,
    pub total: Option<u32>,
    pub rfc: bool,
    pub resend: bool,
    /// "(subset)", which b4 ty puts in front of partial applies.
    pub subset: bool,
    pub title: String,
}

/// Re:, Aw:, Sv:, Fw:, Fwd:, any number of them, any case.
fn strip_reply(s: &str) -> Option<&str> {
    let mut rest = s.trim_start();
    let mut found = false;
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(word) = ["re", "aw", "sv", "fwd", "fw"].iter().find(|w| lower.starts_with(*w)) else { break };
        let after = rest[word.len()..].trim_start();
        let Some(after) = after.strip_prefix(':') else { break };
        rest = after.trim_start();
        found = true;
    }
    found.then_some(rest)
}

fn count(tok: &str) -> Option<(u32, u32)> {
    let (n, m) = tok.split_once('/')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(n) && digits(m)).then(|| Some((n.parse().ok()?, m.parse().ok()?))).flatten()
}

fn version(tok: &str) -> Option<u32> {
    let rest = tok.strip_prefix(['v', 'V'])?;
    (!rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())).then(|| rest.parse().ok()).flatten()
}

pub fn parse(subject: &str) -> Subject {
    let unfolded = subject.split_whitespace().collect::<Vec<_>>().join(" ");
    let (is_reply, mut s) = match strip_reply(&unfolded) {
        Some(rest) => (true, rest),
        None => (false, unfolded.as_str()),
    };
    let mut out = Subject {
        is_reply,
        is_patch: false,
        version: 1,
        index: None,
        total: None,
        rfc: false,
        resend: false,
        subset: false,
        title: String::new(),
    };
    loop {
        s = s.trim_start();
        if let Some(inner) = s.strip_prefix('[').and_then(|r| r.split_once(']')) {
            let (tags, rest) = inner;
            s = rest;
            for tok in tags.split([',', ' ']).filter(|t| !t.is_empty()) {
                match tok.to_ascii_uppercase().as_str() {
                    "PATCH" => out.is_patch = true,
                    "RFC" => out.rfc = true,
                    "RESEND" => out.resend = true,
                    _ => {
                        if let Some(v) = version(tok) {
                            out.version = v;
                        } else if let Some((n, m)) = count(tok) {
                            (out.index, out.total) = (Some(n), Some(m));
                        }
                    }
                }
            }
        } else if s.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("(subset)")) {
            out.subset = true;
            s = &s[8..];
        } else {
            break;
        }
    }
    out.title = s.trim().to_string();
    out
}

/// A title as compared across versions: whitespace collapsed, lower case.
pub fn normalize_title(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patch() {
        let s = parse("[PATCH v3 2/4] drm/foo: unplug the device");
        assert_eq!((s.is_patch, s.is_reply, s.version, s.index, s.total), (true, false, 3, Some(2), Some(4)));
        assert_eq!(s.title, "drm/foo: unplug the device");
    }

    #[test]
    fn reply_rfc_resend_subset() {
        let s = parse("Re: (subset) [RFC PATCH RESEND,v2,1/2] x: y");
        assert!(s.is_reply && s.rfc && s.resend && s.subset && s.is_patch);
        assert_eq!((s.version, s.index, s.title.as_str()), (2, Some(1), "x: y"));
    }

    #[test]
    fn several_prefixes_and_brackets() {
        let s = parse("RE: Aw:  [PATCH] [net-next v2] tcp:\n\tfold   this");
        assert!(s.is_reply && s.is_patch);
        assert_eq!((s.version, s.title.as_str()), (2, "tcp: fold this"));
    }

    #[test]
    fn not_a_patch() {
        assert!(!parse("[GIT PULL] clk changes").is_patch);
        // "Re" must be followed by a colon to be a reply.
        let s = parse("Reviewing [PATCH] isn't a patch subject");
        assert!(!s.is_reply && !s.is_patch);
        // Count and version tokens must be exact.
        let s = parse("[PATCH 1/2/3 v2x] odd");
        assert_eq!((s.version, s.index), (1, None));
    }

    #[test]
    fn multibyte_titles() {
        assert_eq!(parse("[PATCH] é: ünïcödé").title, "é: ünïcödé");
        assert_eq!(parse("ééééé").title, "ééééé");
    }

    #[test]
    fn normalized() {
        assert_eq!(normalize_title("  Foo:  Do\tThe thing "), "foo: do the thing");
    }
}
