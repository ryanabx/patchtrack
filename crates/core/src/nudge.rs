//! Which open series need a nudge: a reply, a ping or a RESEND.
//!
//! The defaults follow the kernel's Documentation/process/submitting-patches.rst:
//! "Wait for a minimum of one week before resubmitting or pinging reviewers",
//! and "it's also ok to resend the patch or the patch series after a couple of
//! weeks". Nothing here sends mail; `compose` drafts it for a person to send.

use crate::evidence::APPROVALS;
use crate::series::{Family, State, parse_utc};
use crate::sync;

/// "Wait ... possibly longer during busy times like merge windows". A window
/// lasts two weeks; a final tag older than this with no -rc1 after it means
/// the repository's tags are stale, not that the window is open.
pub const WINDOW_MAX_DAYS: i64 = 21;

/// Whether mainline's merge window is open.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Window {
    /// None: cannot tell, and `why` says why.
    pub open: Option<bool>,
    pub tag: String,
    /// The tag's date, YYYY-MM-DD.
    pub since: String,
    pub why: String,
}

/// "v7.3" or "v7.3-rc1", and whether it is an -rc.
fn kernel_tag(tag: &str) -> Option<bool> {
    let rest = tag.strip_prefix('v')?;
    let (version, rc) = match rest.split_once("-rc") {
        Some((v, n)) => (v, Some(n)),
        None => (rest, None),
    };
    let (major, minor) = version.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(major) && digits(minor) && rc.is_none_or(digits)).then_some(rc.is_some())
}

/// The merge window, from a mainline repository's tags, newest first:
/// (name, ISO 8601 date). A final vX.Y with no -rc1 after it opens it.
pub fn merge_window(tags: &[(String, String)], now: i64, repo: &str) -> Window {
    for (tag, when) in tags {
        let Some(rc) = kernel_tag(tag) else { continue };
        let since = when.get(..10).unwrap_or(when).to_string();
        if rc {
            return Window { open: Some(false), tag: tag.clone(), since, why: String::new() };
        }
        let age = sync::since(when, now).map_or(0, |t| (now - t) / 86_400);
        if age > WINDOW_MAX_DAYS {
            let why = format!("{tag} is {age} days old with no -rc1 after it; the tags in {repo} look stale");
            return Window { open: None, tag: tag.clone(), since, why };
        }
        return Window { open: Some(true), tag: tag.clone(), since, why: String::new() };
    }
    Window { open: None, why: format!("no vX.Y tags in {repo}"), ..Window::default() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Someone asked or said something after your last message.
    ReplyOwed,
    Resend,
    Ping,
    /// The same, held for an open merge window.
    ResendAfterRc1,
    PingAfterRc1,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::ReplyOwed => "reply owed",
            Kind::Resend => "resend",
            Kind::Ping => "ping",
            Kind::ResendAfterRc1 => "resend after -rc1",
            Kind::PingAfterRc1 => "ping after -rc1",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Nudge<'a> {
    pub family: &'a Family,
    pub kind: Kind,
    /// Since the last human word, or since the reply that is owed.
    pub days: i64,
    pub why: String,
}

fn days(since: &str, now: i64) -> i64 {
    parse_utc(since).map_or(0, |t| (now - t).div_euclid(86_400))
}

/// The nudge a family needs, or None. During an open merge window, pings
/// and RESENDs wait for -rc1.
pub fn classify<'a>(
    f: &'a Family,
    now: i64,
    ping_days: i64,
    resend_days: i64,
    window: Option<&Window>,
) -> Option<Nudge<'a>> {
    let mut n = classify_now(f, now, ping_days, resend_days)?;
    if window.is_some_and(|w| w.open == Some(true)) && matches!(n.kind, Kind::Ping | Kind::Resend) {
        n.kind = if n.kind == Kind::Ping { Kind::PingAfterRc1 } else { Kind::ResendAfterRc1 };
        n.why += "; merge window open";
    }
    Some(n)
}

fn classify_now(f: &Family, now: i64, ping_days: i64, resend_days: i64) -> Option<Nudge<'_>> {
    let s = f.head();
    if f.closed.is_some() || f.state.done() || f.state == State::PartiallyApplied {
        return None;
    }
    let humans: Vec<_> = s.replies.iter().filter(|r| !r.bot && !r.mine).collect();
    let last_mine = s.replies.iter().filter(|r| r.mine).map(|r| r.date.as_str()).fold(s.date.as_str(), |a, b| a.max(b));
    // A reply is owed when the last person to write after you asked or said
    // something; a bare tag or an "Applied" asks nothing.
    let asks: Vec<_> = humans
        .iter()
        .filter(|r| r.applied.is_none() && r.date.as_str() > last_mine)
        .filter(|r| !r.trailers.iter().any(|(k, _)| APPROVALS.contains(&k.as_str())))
        .collect();
    if let Some(last) = asks.last() {
        let who = last.who.split(" <").next().unwrap_or(&last.who);
        return Some(Nudge {
            family: f,
            kind: Kind::ReplyOwed,
            days: days(&last.date, now),
            why: format!("{who} wrote last"),
        });
    }
    let last_word = humans.iter().map(|r| r.date.as_str()).fold(last_mine, |a, b| a.max(b));
    let d = days(last_word, now);
    if humans.is_empty() && d >= resend_days {
        return Some(Nudge { family: f, kind: Kind::Resend, days: d, why: format!("no human reply in {d} days") });
    }
    if d >= ping_days {
        let why = if humans.is_empty() {
            format!("no human reply in {d} days")
        } else if f.state == State::Reviewed {
            format!("reviewed, then nothing for {d} days")
        } else {
            format!("quiet for {d} days")
        };
        return Some(Nudge { family: f, kind: Kind::Ping, days: d, why });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use crate::rules::Rules;
    use crate::series::{self, Facts};

    const ME: &str = "Dev <dev@example.org>";
    /// 2026-10-27T00:00:00Z
    const NOW: i64 = 1_793_059_200;

    fn mail(id: &str, subject: &str, from: &str, irt: Option<&str>, date: &str, body: &str) -> Message {
        let irt = irt.map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n")).unwrap_or_default();
        // The cover is sent to the maintainer, copying the list and the author.
        let to = if irt.is_empty() { "To: Maint <maint@x>\nCc: list@x, dev@example.org\n" } else { "" };
        let raw = format!("Message-ID: <{id}>\nSubject: {subject}\nFrom: {from}\n{to}{irt}Date: {date}\n\n{body}");
        Message::parse(raw.as_bytes()).unwrap()
    }

    /// One posting (a cover and a patch) and these replies to its cover.
    fn messages(replies: &[(&str, &str, &str)], sent: &str) -> Vec<Message> {
        let mut msgs = vec![
            mail("ng-0@x", "[PATCH 0/1] foo: do the thing", ME, None, sent, ""),
            mail("ng-1@x", "[PATCH 1/1] foo: part 1", ME, Some("ng-0@x"), sent, ""),
        ];
        for (n, (from, body, date)) in replies.iter().enumerate() {
            msgs.push(mail(&format!("r{n}@x"), "Re: [PATCH 0/1] foo: do the thing", from, Some("ng-0@x"), date, body));
        }
        msgs
    }

    fn family(replies: &[(&str, &str, &str)], sent: &str) -> Family {
        let mut facts = Facts::default();
        facts.fetched.insert("ng-0@x".into(), "2026-10-26T00:00:00Z".into());
        series::families(series::build(&messages(replies, sent), &facts, &Rules::default()), &facts).remove(0)
    }

    fn kind(replies: &[(&str, &str, &str)], sent: &str) -> Option<Kind> {
        classify(&family(replies, sent), NOW, 7, 14, None).map(|n| n.kind)
    }

    const OCT5: &str = "Mon, 05 Oct 2026 10:00:00 +0000";

    #[test]
    fn silence() {
        assert_eq!(kind(&[], OCT5), Some(Kind::Resend)); // 21 days, no human
        assert_eq!(kind(&[], "Sat, 17 Oct 2026 10:00:00 +0000"), Some(Kind::Ping)); // 9 days
        assert_eq!(kind(&[], "Mon, 26 Oct 2026 10:00:00 +0000"), None);
    }

    #[test]
    fn a_question_owes_a_reply_a_tag_does_not() {
        let q = ("R <r@x>", "> +x\nWhy?\n", "Mon, 26 Oct 2026 10:00:00 +0000");
        let tag = ("R <r@x>", "Reviewed-by: R <r@x>\n", "Tue, 13 Oct 2026 10:00:00 +0000");
        assert_eq!(kind(&[q], OCT5), Some(Kind::ReplyOwed));
        let f = family(&[tag], OCT5);
        let n = classify(&f, NOW, 7, 14, None).unwrap();
        // Reviewed, then two weeks of nothing.
        assert_eq!((n.kind, n.why.as_str()), (Kind::Ping, "reviewed, then nothing for 13 days"));
    }

    #[test]
    fn your_own_ping_restarts_the_clock() {
        assert_eq!(kind(&[(ME, "Ping?\n", "Mon, 26 Oct 2026 10:00:00 +0000")], OCT5), None);
    }

    #[test]
    fn a_draft_is_threaded_plain_text() {
        let f = family(&[], OCT5);
        let n = classify(&f, NOW, 7, 14, None).unwrap();
        let root = &messages(&[], OCT5)[0];
        let raw = crate::compose::ping(
            &n,
            root,
            "dev@example.org",
            "https://lore.kernel.org",
            "Tue, 27 Oct 2026 00:00:00 +0000",
            "<d@example.org>",
        );
        let text = String::from_utf8(raw.clone()).unwrap();
        assert!(text.contains("\nTo: maint@x\nCc: list@x\nSubject: Re: [PATCH 0/1] foo: do the thing\n"), "{text}");
        assert!(text.contains("\nIn-Reply-To: <ng-0@x>\nReferences: <ng-0@x>\n"));
        assert!(text.contains("Content-Transfer-Encoding: 7bit\n") && text.contains("b4 send --resend v1"));
        assert!(text.contains("sent on 2026-10-05, which has had no reply so far.\n"));
        // It reads back as what it says.
        let m = Message::parse(&raw).unwrap();
        assert_eq!((m.msgid.as_str(), m.in_reply_to.as_deref()), ("d@example.org", Some("ng-0@x")));
        assert_eq!(crate::compose::draft_name(&n), "2026-10-05-title_foo_ do the thing.eml");
    }

    fn tags(list: &[(&str, &str)]) -> Vec<(String, String)> {
        // Newest first, as `git for-each-ref --sort=-creatordate` lists them.
        list.iter().rev().map(|(t, d)| (t.to_string(), d.to_string())).collect()
    }

    #[test]
    fn open_closed_stale() {
        let w = merge_window(
            &tags(&[("v7.3-rc7", "2026-10-11T20:00:00+00:00"), ("v7.3", "2026-10-18T20:00:00+00:00")]),
            NOW,
            "r",
        );
        assert_eq!((w.open, w.tag.as_str(), w.since.as_str()), (Some(true), "v7.3", "2026-10-18"));
        let w = merge_window(
            &tags(&[("v7.3", "2026-10-18T20:00:00+00:00"), ("v7.4-rc1", "2026-10-25T20:00:00+00:00")]),
            NOW,
            "r",
        );
        assert_eq!(w.open, Some(false));
        let w = merge_window(&tags(&[("v7.2", "2026-08-01T20:00:00+00:00")]), NOW, "r");
        assert_eq!(w.open, None);
        assert!(w.why.contains("stale"), "{}", w.why);
        assert_eq!(merge_window(&tags(&[("release-1", "2026-10-20T00:00:00+00:00")]), NOW, "r").open, None);
        assert_eq!(merge_window(&tags(&[("v7.3-rc", "2026-10-20T00:00:00+00:00")]), NOW, "r").open, None);
    }

    #[test]
    fn an_open_window_holds_pings_not_replies() {
        let win = Window { open: Some(true), tag: "v7.3".into(), since: "2026-10-18".into(), why: String::new() };
        assert_eq!(classify(&family(&[], OCT5), NOW, 7, 14, Some(&win)).unwrap().kind, Kind::ResendAfterRc1);
        let q = ("R <r@x>", "> +x\nWhy?\n", "Mon, 26 Oct 2026 10:00:00 +0000");
        assert_eq!(classify(&family(&[q], OCT5), NOW, 7, 14, Some(&win)).unwrap().kind, Kind::ReplyOwed);
    }
}
