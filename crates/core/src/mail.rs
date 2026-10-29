//! One email, as the facts the rest of patchtrack reasons about.
//!
//! The store keeps each message's raw bytes; these fields are parsed from
//! them, and parsed again when [`PARSER_VERSION`] changes, so a better
//! parser applies to old mail without fetching it again.

use mail_parser::{HeaderForm, MessageParser, PartKind};

use crate::subject;

/// Bumped whenever [`Message::parse`] would give a different result for
/// the same bytes; the store re-parses rows parsed with an older one.
pub const PARSER_VERSION: u32 = 2;

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Addr {
    pub name: String,
    /// Lower case.
    pub addr: String,
}

impl Addr {
    fn from_mailbox(name: Option<&str>, addr: Option<&str>) -> Addr {
        Addr { name: name.unwrap_or_default().trim().to_string(), addr: addr.unwrap_or_default().trim().to_lowercase() }
    }

    fn parse(value: &[u8]) -> Option<Addr> {
        let parsed = HeaderForm::Addresses.parse(value);
        let first = parsed.value().as_address()?.first()?;
        Some(Addr::from_mailbox(first.name(), first.address()))
    }

    /// "Name <addr>", or the address alone.
    pub fn display(&self) -> String {
        if self.name.is_empty() { self.addr.clone() } else { format!("{} <{}>", self.name, self.addr) }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub msgid: String,
    /// In-Reply-To, or else the last of References.
    pub in_reply_to: Option<String>,
    /// References, oldest first.
    pub refs: Vec<String>,
    /// Who posted it: X-Original-From when a relay rewrote From
    /// ("X via B4 Relay <devnull+...>"), else From.
    pub from: Addr,
    /// Who wrote the patch: git's in-body "From:", else the sender.
    pub author: Addr,
    /// UTC, "2026-10-05T10:00:00Z"; empty if missing or unreadable.
    pub date: String,
    /// Decoded and unfolded.
    pub subject: String,
    /// Lower-cased addresses.
    pub to: Vec<String>,
    pub cc: Vec<String>,
    /// The text/plain parts, with "\n" line ends, without an in-body "From:".
    pub body: String,
}

/// What deciding whether a message matters needs, read from its headers
/// alone (the same way [`Message::parse`] reads them), so a big mailbox is
/// cheap to filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub msgid: String,
    pub in_reply_to: Option<String>,
    pub refs: Vec<String>,
    pub from: Addr,
    pub subject: String,
}

impl Head {
    /// None if the bytes are not a message with a Message-ID.
    pub fn parse(raw: &[u8]) -> Option<Head> {
        Head::of(&MessageParser::new().parse_headers(raw)?)
    }

    fn of(mail: &mail_parser::Message<'_>) -> Option<Head> {
        let headers = mail.headers();
        // The first of a repeated header, as Python's email does: an archive
        // that adds one (public-inbox's second Message-ID for a duplicate,
        // "...@z") puts it after the sender's.
        let first = |name: &str| headers.all(name).next().map(|h| h.value());
        let ids = |name: &str| -> Vec<String> {
            first(name).and_then(|v| v.as_text_list()).map(|l| l.iter().map(strip_angles).collect()).unwrap_or_default()
        };
        let msgid =
            ids("Message-ID").into_iter().next().or_else(|| first("Message-ID")?.as_text().map(strip_angles))?;
        if msgid.is_empty() {
            return None;
        }
        let refs = ids("References");
        let in_reply_to = ids("In-Reply-To").into_iter().next().or_else(|| refs.last().cloned());
        let mailbox =
            |name: &str| first(name)?.as_address()?.first().map(|m| Addr::from_mailbox(m.name(), m.address()));
        let from = headers
            .all("X-Original-From")
            .next()
            .and_then(|h| Addr::parse(h.raw_value()))
            .filter(|a| a.addr.contains('@'))
            .or_else(|| mailbox("From"))
            .unwrap_or_default();
        let subject = first("Subject")
            .and_then(|v| v.as_text())
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        Some(Head { msgid, in_reply_to, refs, from, subject })
    }
}

impl Message {
    /// None if the bytes are not a message with a Message-ID.
    pub fn parse(raw: &[u8]) -> Option<Message> {
        let mail = MessageParser::new().parse(raw)?;
        let Head { msgid, in_reply_to, refs, from, subject } = Head::of(&mail)?;
        let headers = mail.headers();
        let first = |name: &str| headers.all(name).next().map(|h| h.value());
        let addrs = |list: Option<mail_parser::AddressList<'_>>| -> Vec<String> {
            list.map(|l| l.mailboxes().filter_map(|m| m.address().map(|a| a.trim().to_lowercase())).collect())
                .unwrap_or_default()
        };
        let mut body = String::new();
        for part in mail.parts() {
            let plain = part.content_type().is_none_or(|t| {
                t.ctype().eq_ignore_ascii_case("text") && t.subtype().is_none_or(|s| s.eq_ignore_ascii_case("plain"))
            });
            if matches!(part.kind(), PartKind::Text)
                && plain
                && let Some(text) = part.text()
            {
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(&text);
            }
        }
        let body = body.replace("\r\n", "\n");
        let (author, body) = inbody_author(&subject, &from, body);
        Some(Message {
            msgid,
            in_reply_to,
            refs,
            from,
            author,
            date: first("Date").and_then(|v| v.as_datetime()).map(|d| utc(d.to_timestamp())).unwrap_or_default(),
            subject,
            to: mail.all_to().flat_map(|l| addrs(Some(l))).collect(),
            cc: mail.all_cc().flat_map(|l| addrs(Some(l))).collect(),
            body,
        })
    }
}

fn strip_angles(id: &str) -> String {
    id.trim().trim_start_matches('<').trim_end_matches('>').trim().to_string()
}

/// The patch's author and its body, after git's in-body From: rule.
///
/// A patch whose body starts with "From: X <x@y>" was written by X, whoever
/// sent it: `git am` records X, and the line (with one blank line after it)
/// is not part of the message. Otherwise the sender is the author.
fn inbody_author(subject: &str, sender: &Addr, body: String) -> (Addr, String) {
    let s = subject::parse(subject);
    if !s.is_patch || s.is_reply || !body.starts_with("From:") {
        return (sender.clone(), body);
    }
    let Some(eol) = body.find('\n') else { return (sender.clone(), body) };
    let value = body["From:".len()..eol].trim_matches([' ', '\t']);
    match Addr::parse(value.as_bytes()) {
        Some(a) if !value.is_empty() && a.addr.contains('@') => {
            let mut rest = &body[eol + 1..];
            if let Some(blank) = rest.find('\n').filter(|&n| rest[..n].trim_matches([' ', '\t']).is_empty()) {
                rest = &rest[blank + 1..];
            }
            (a, rest.to_string())
        }
        _ => (sender.clone(), body),
    }
}

/// Seconds since the epoch as "YYYY-MM-DDTHH:MM:SSZ".
pub fn utc(ts: i64) -> String {
    let (days, secs) = (ts.div_euclid(86_400), ts.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

/// b4's series slug: from the change-id trailer, or else the message-id.
pub fn change_id(msgid: &str, body: &str) -> Option<String> {
    if let Some(id) = body.lines().find_map(|l| l.strip_prefix("change-id:")).map(str::trim)
        && !id.is_empty()
        && !id.contains(char::is_whitespace)
    {
        // 20260915-<slug>-d35d2ac3d6f0
        let slug = strip_date(id).map(|s| strip_suffix_hash(s).unwrap_or(s));
        return Some(slug.filter(|s| !s.is_empty()).unwrap_or(id).to_string());
    }
    // 20261025-<slug>-v5-0-0123456789ab@example.org
    let local = msgid.split('@').next()?;
    if !msgid.contains('@') {
        return None;
    }
    let rest = strip_suffix_hash(strip_date(local)?)?;
    let (rest, n) = rest.rsplit_once('-')?;
    let (slug, v) = rest.rsplit_once('-')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(n) && v.strip_prefix('v').is_some_and(digits) && !slug.is_empty()).then(|| slug.to_string())
}

/// "20261025-rest" -> "rest"
fn strip_date(s: &str) -> Option<&str> {
    let (date, rest) = s.split_at_checked(8)?;
    (date.bytes().all(|b| b.is_ascii_digit())).then_some(rest.strip_prefix('-')?)
}

/// "rest-0123456789ab" -> "rest"
fn strip_suffix_hash(s: &str) -> Option<&str> {
    let (rest, hash) = s.rsplit_once('-')?;
    (hash.len() == 12 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(headers: &str, body: &str) -> Vec<u8> {
        format!("{headers}\n\n{body}").into_bytes()
    }

    #[test]
    fn fields() {
        let raw = msg(
            "Message-ID: <a@x>\nIn-Reply-To: <p@x>\nReferences: <r@x>\n <p@x>\n\
             From: =?utf-8?q?J=C3=B6rg?= <Joerg@Example.ORG>\nTo: a@x, B <b@x>\nCc: c@x\nTo: d@x\n\
             Date: Mon, 05 Oct 2026 12:00:00 +0200\nSubject: Re: [PATCH v2 1/3]\n foo: bar",
            "Hello.\r\nBye.\r\n",
        );
        let m = Message::parse(&raw).unwrap();
        assert_eq!(m.msgid, "a@x");
        assert_eq!((m.in_reply_to.as_deref(), m.refs.clone()), (Some("p@x"), vec!["r@x".to_string(), "p@x".into()]));
        assert_eq!(m.from, Addr { name: "Jörg".into(), addr: "joerg@example.org".into() });
        assert_eq!(m.author, m.from);
        assert_eq!(m.date, "2026-10-05T10:00:00Z");
        assert_eq!(m.subject, "Re: [PATCH v2 1/3] foo: bar");
        assert_eq!(
            (m.to.clone(), m.cc.clone()),
            (vec!["a@x".into(), "b@x".into(), "d@x".into()], vec!["c@x".to_string()])
        );
        assert_eq!(m.body, "Hello.\nBye.\n");
    }

    #[test]
    fn the_first_of_a_repeated_header() {
        // public-inbox's second copy of a message-id gets a Message-ID of its own, after the original.
        let raw = msg(
            "Message-ID: <orig@x>\nMessage-ID: <20261024130719.abc@z>\nFrom: A <a@x>\nFrom: B <b@x>\n\
             Subject: first\nSubject: second\nDate: Mon, 05 Oct 2026 10:00:00 +0000\nDate: Tue, 06 Oct 2026 10:00:00 +0000",
            "",
        );
        let m = Message::parse(&raw).unwrap();
        assert_eq!((m.msgid.as_str(), m.from.addr.as_str(), m.subject.as_str()), ("orig@x", "a@x", "first"));
        assert_eq!(m.date, "2026-10-05T10:00:00Z");
    }

    #[test]
    fn no_message_id_is_not_a_message() {
        assert!(Message::parse(b"From: a@x\nSubject: hi\n\nbody\n").is_none());
    }

    #[test]
    fn in_reply_to_falls_back_to_the_last_reference() {
        let m = Message::parse(&msg("Message-ID: <a@x>\nReferences: <r@x> <p@x>", "")).unwrap();
        assert_eq!(m.in_reply_to.as_deref(), Some("p@x"));
    }

    #[test]
    fn b4_relay_copy_is_the_original_sender() {
        let raw = msg(
            "Message-ID: <a@x>\nFrom: Dev via B4 Relay <devnull+dev.example.org@kernel.org>\n\
             X-Original-From: Dev <dev@example.org>\nSubject: [PATCH] foo: bar",
            "From: Dev <dev@example.org>\n\nThe message.\n",
        );
        let m = Message::parse(&raw).unwrap();
        assert_eq!((m.from.addr.as_str(), m.author.addr.as_str()), ("dev@example.org", "dev@example.org"));
        assert_eq!(m.body, "The message.\n");
    }

    #[test]
    fn inbody_from_is_the_author_of_a_patch_only() {
        let patch = msg("Message-ID: <a@x>\nFrom: S <s@x>\nSubject: [PATCH 1/2] foo", "From: Au Thor <au@x>\n\nFix.\n");
        let m = Message::parse(&patch).unwrap();
        assert_eq!(
            (m.from.addr.as_str(), m.author.display(), m.body.as_str()),
            ("s@x", "Au Thor <au@x>".into(), "Fix.\n")
        );
        // A reply that starts with "From:" is just a reply.
        let reply = msg("Message-ID: <b@x>\nFrom: S <s@x>\nSubject: Re: [PATCH 1/2] foo", "From: Au <au@x>\n\nok\n");
        assert_eq!(Message::parse(&reply).unwrap().author.addr, "s@x");
        // Without an address it is not an author line.
        let odd = msg("Message-ID: <c@x>\nFrom: S <s@x>\nSubject: [PATCH] foo", "From: the start\n\nFix.\n");
        let m = Message::parse(&odd).unwrap();
        assert_eq!((m.author.addr.as_str(), m.body.as_str()), ("s@x", "From: the start\n\nFix.\n"));
    }

    #[test]
    fn multipart_keeps_text_plain_only() {
        let raw = msg(
            "Message-ID: <a@x>\nMIME-Version: 1.0\nContent-Type: multipart/mixed; boundary=b",
            "--b\nContent-Type: text/plain\n\none\n--b\nContent-Type: text/html\n\n<p>no</p>\n\
             --b\nContent-Type: text/x-patch; charset=utf-8\n\nnot plain\n\
             --b\nContent-Type: text/plain; charset=iso-8859-1\nContent-Transfer-Encoding: quoted-printable\n\nt=E9l=E9\n--b--\n",
        );
        // The line break before a boundary belongs to the boundary (RFC 2046).
        assert_eq!(Message::parse(&raw).unwrap().body, "one\ntélé");
    }

    #[test]
    fn utc_dates() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(1_637_446_921), "2021-11-20T22:22:01Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn change_ids() {
        assert_eq!(change_id("x@y", "c\n\nchange-id: 20260915-foo-thing-d35d2ac3d6f0\n").as_deref(), Some("foo-thing"));
        assert_eq!(change_id("x@y", "change-id: 20260915-foo\n").as_deref(), Some("foo"));
        assert_eq!(change_id("x@y", "change-id: my-own-id\n").as_deref(), Some("my-own-id"));
        assert_eq!(change_id("20261025-foo-bar-v5-0-0123456789ab@example.org", "").as_deref(), Some("foo-bar"));
        assert_eq!(change_id("20261025-foo-v12-3-0123456789ab@x", "").as_deref(), Some("foo"));
        assert_eq!(change_id("20261025-foo-v5-0-0123456789AB@x", ""), None);
        assert_eq!(change_id("cover.1234.git.dev@example.org", ""), None);
        assert_eq!(change_id("20261025-foo-v5-0-0123456789ab", ""), None);
        // A quoted trailer is not this series' change-id.
        assert_eq!(change_id("x@y", "> change-id: 20260915-other-d35d2ac3d6f0\n"), None);
    }
}
