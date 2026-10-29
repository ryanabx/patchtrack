//! Mail patchtrack drafts for a person to send: plain text, 7bit or 8bit,
//! never quoted-printable (which breaks lines mid-word on the lists and in
//! an editor) and never flowed.
//!
//! The host supplies what the core may not make up: the date and a fresh
//! Message-ID.
//!
//! Replies are addressed and quoted as b4 writes them (`b4 ty`): to the
//! sender and everyone else on To, Cc as it was, never yourself; the
//! subject "Re: " once; a short quote of the message's first paragraph.

use crate::mail::Message;
use crate::nudge::{Kind, Nudge};
use crate::series::{self, Series};
use crate::views::message_url;

/// Headers longer than this are folded, as RFC 5322 recommends.
const FOLD_AT: usize = 78;

/// "Mon, 05 Oct 2026 10:00:00 +0000" for epoch seconds.
pub fn rfc2822(ts: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let iso = crate::mail::utc(ts); // 2026-10-05T10:00:00Z
    let month: usize = iso[5..7].parse().unwrap_or(1);
    let day = DAYS[usize::try_from(ts.div_euclid(86_400).rem_euclid(7)).unwrap_or(0)];
    format!("{day}, {} {} {} {} +0000", &iso[8..10], MONTHS[month - 1], &iso[..4], &iso[11..19])
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]) } else { '=' });
        }
    }
    out
}

/// A header's value, as RFC 2047 encoded words where it is not ASCII.
fn encode(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    // Whole characters per word, each word within the line length.
    let mut words = vec![];
    let mut chunk = String::new();
    for c in value.chars() {
        if (chunk.len() + c.len_utf8()).div_ceil(3) * 4 > 45 {
            words.push(format!("=?utf-8?b?{}?=", base64(chunk.as_bytes())));
            chunk.clear();
        }
        chunk.push(c);
    }
    words.push(format!("=?utf-8?b?{}?=", base64(chunk.as_bytes())));
    words.join(" ")
}

/// An address header's value: each "Name <addr>" with only its name
/// encoded (an encoded word may not hold the address), and quoted where it
/// has characters an address list gives meaning to.
fn encode_addresses(value: &str) -> String {
    let one = |a: &str| match a.rsplit_once(" <") {
        Some((name, addr)) if !name.is_ascii() => format!("{} <{addr}", encode(name)),
        Some((name, addr)) if name.contains([',', ';', ':', '@', '.', '"', '(', ')']) && !name.starts_with('"') => {
            format!("\"{}\" <{addr}", name.replace(['\\', '"'], ""))
        }
        _ => a.to_string(),
    };
    value.split(", ").map(one).collect::<Vec<_>>().join(", ")
}

/// "Name: value", folded at spaces to stay within the line length.
fn header(name: &str, value: &str) -> String {
    let mut out = format!("{name}:");
    let mut line = out.len();
    for word in value.split(' ').filter(|w| !w.is_empty()) {
        if line + 1 + word.len() > FOLD_AT && line > name.len() + 1 {
            out.push_str("\n ");
            line = 1;
        } else {
            out.push(' ');
            line += 1;
        }
        out.push_str(word);
        line += word.len();
    }
    out
}

/// A plain-text message: these headers (unencoded values), then MIME's, then the body.
pub fn message(headers: &[(&str, String)], body: &str) -> Vec<u8> {
    let mut out = String::new();
    for (name, value) in headers.iter().filter(|(_, v)| !v.is_empty()) {
        let value = if ["From", "To", "Cc"].contains(name) { encode_addresses(value) } else { encode(value) };
        out += &header(name, &value);
        out.push('\n');
    }
    let cte = if body.is_ascii() { "7bit" } else { "8bit" };
    out += &format!(
        "Content-Type: text/plain; charset=\"utf-8\"\nContent-Transfer-Encoding: {cte}\nMIME-Version: 1.0\n\n"
    );
    out += body;
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out.into_bytes()
}

/// A ping (or a RESEND reminder) for a nudged series, threaded to its root
/// (`root`, the stored cover or first patch) and addressed to its recipients.
pub fn ping(n: &Nudge, root: &Message, from: &str, archive: &str, date: &str, msgid: &str) -> Vec<u8> {
    let s = n.family.head();
    let mut body = String::new();
    if matches!(n.kind, Kind::Resend | Kind::ResendAfterRc1) {
        body += &format!(
            "[Draft only. The kernel's process documentation suggests a RESEND after a couple\n\
             of weeks of silence, unchanged, e.g.: b4 send --resend v{}\n\
             Delete this note and use the ping below if you prefer to ping first.]\n\n",
            s.version
        );
    }
    let sent = s.date.get(..10).unwrap_or(&s.date);
    let silent = if n.why.starts_with("no human") { ", which has had no reply so far" } else { "" };
    body += &format!(
        "Hi,\n\nGentle ping on this series, sent on {sent}{silent}.\nIs there anything I should change?\n\n{}\n\nThanks,\n",
        message_url(archive, &s.root)
    );
    let cc: Vec<&str> = root.cc.iter().map(String::as_str).filter(|a| *a != from).collect();
    let headers = [
        ("From", from.to_string()),
        ("To", root.to.join(", ")),
        ("Cc", cc.join(", ")),
        ("Subject", format!("Re: {}", root.subject)),
        ("In-Reply-To", format!("<{}>", s.root)),
        ("References", format!("<{}>", s.root)),
        ("Date", date.to_string()),
        ("Message-ID", msgid.to_string()),
    ];
    message(&headers, &body)
}

/// Who writes: "Name <addr>"; a bare address when the name is not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Me {
    pub name: String,
    /// Lower case.
    pub addr: String,
}

impl Me {
    pub fn display(&self) -> String {
        if self.name.is_empty() { self.addr.clone() } else { format!("{} <{}>", self.name, self.addr) }
    }
}

/// The tags a review gives, as they are spelled in trailers.
pub const TAGS: [&str; 4] = ["Reviewed-by", "Acked-by", "Tested-by", "Reported-by"];

/// Reply-all, as b4 addresses it: To is the sender and the other To
/// addresses, Cc the Cc addresses; never `me`, never twice.
pub fn recipients(to: &Message, me: &str) -> (Vec<String>, Vec<String>) {
    let mut seen = vec![me.to_lowercase()];
    let mut keep = |a: &String| {
        let a = a.to_lowercase();
        if a.is_empty() || seen.contains(&a) {
            return None;
        }
        seen.push(a.clone());
        Some(a)
    };
    let from = std::iter::once(&to.from.addr);
    let tos: Vec<String> = from.chain(&to.to).filter_map(&mut keep).collect();
    let ccs: Vec<String> = to.cc.iter().filter_map(&mut keep).collect();
    (tos, ccs)
}

/// The message part of a body: before the diff ("---"), the signature
/// ("-- ") or a diff, without the trailers that end it.
fn message_part(body: &str) -> Vec<&str> {
    let mut lines: Vec<&str> =
        body.lines().take_while(|l| *l != "---" && *l != "-- " && !l.starts_with("diff --git ")).collect();
    let trailer = |l: &str| {
        l.split_once(": ").is_some_and(|(k, v)| {
            !k.is_empty() && !v.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    };
    while lines.last().is_some_and(|l| l.trim().is_empty() || trailer(l)) {
        lines.pop();
    }
    lines
}

/// b4's short quote: the message's first lines, past a greeting, up to the
/// first blank line after five, then "[...]".
pub fn quote_short(body: &str) -> String {
    let mut lines = message_part(body);
    let greeting = lines.first().is_some_and(|l| {
        let w = l.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_lowercase();
        ["hi", "hello", "greetings", "dear"].contains(&w.as_str()) && l.len() > w.len()
    });
    if greeting {
        lines.remove(0);
        while lines.first().is_some_and(|l| l.trim().is_empty()) {
            lines.remove(0);
        }
    }
    if lines.is_empty() {
        return ">".into();
    }
    let mut out = vec![];
    for (n, l) in lines.iter().enumerate() {
        if n > 5 && l.trim().is_empty() {
            out.extend([">".to_string(), "> [...]".to_string()]);
            break;
        }
        out.push(quoted(l));
    }
    out.join("\n")
}

/// Every line of a body quoted, for answering inline.
pub fn quote_all(body: &str) -> String {
    body.trim_end().lines().map(quoted).collect::<Vec<_>>().join("\n")
}

fn quoted(line: &str) -> String {
    let l = line.trim_end();
    if l.is_empty() {
        ">".into()
    } else if l.starts_with('>') {
        format!(">{l}")
    } else {
        format!("> {l}")
    }
}

/// "On Mon, 05 Oct 2026 10:00:00 +0000, Name wrote:"
pub fn attribution(to: &Message) -> String {
    let when = series::parse_utc(&to.date).map(rfc2822).unwrap_or_else(|| to.date.clone());
    let who = if to.from.name.is_empty() { to.from.addr.as_str() } else { to.from.name.as_str() };
    format!("On {when}, {who} wrote:")
}

/// A reply to `to` from `me`, with this body, threaded under it.
pub fn reply(to: &Message, me: &Me, body: &str, date: &str, msgid: &str) -> Vec<u8> {
    reply_as(to, me, &format!("Re: {}", strip_re(&to.subject)), body, date, msgid)
}

fn reply_as(to: &Message, me: &Me, subject: &str, body: &str, date: &str, msgid: &str) -> Vec<u8> {
    let (tos, ccs) = recipients(to, &me.addr);
    let refs: Vec<String> = to.refs.iter().chain([&to.msgid]).map(|r| format!("<{r}>")).collect();
    let headers = [
        ("From", me.display()),
        ("To", tos.join(", ")),
        ("Cc", ccs.join(", ")),
        ("Subject", subject.to_string()),
        ("In-Reply-To", format!("<{}>", to.msgid)),
        ("References", refs.join(" ")),
        ("Date", date.to_string()),
        ("Message-ID", msgid.to_string()),
    ];
    message(&headers, body)
}

fn strip_re(subject: &str) -> &str {
    let t = subject.trim_start();
    match t.get(..3) {
        Some(re) if re.eq_ignore_ascii_case("re:") => t[3..].trim_start(),
        _ => t,
    }
}

/// The body of a reply: who wrote what it answers, then b4's short quote
/// and `text`, or with no text the whole message quoted to answer inline;
/// then the tag it gives, if any ("Reviewed-by", say), signed by `me`.
pub fn reply_body(to: &Message, text: &str, tag: Option<&str>, me: &Me) -> String {
    let text = text.trim();
    let mut body = attribution(to) + "\n";
    if text.is_empty() && tag.is_none() {
        return body + &quote_all(&to.body) + "\n\n";
    }
    body += &quote_short(&to.body);
    if !text.is_empty() {
        body += &format!("\n\n{text}");
    }
    if let Some(tag) = tag {
        body += &format!("\n\n{tag}: {}", me.display());
    }
    body + "\n"
}

/// Lines of a hunk quoted above a comment, at most: the rest is "[...]".
pub const REVIEW_CONTEXT: usize = 6;

/// An inline review of a patch, as reviewers on the lists write one: who
/// wrote it, then for each comment the file's header and the hunk it is
/// on, quoted up to the line it is about (at most [`REVIEW_CONTEXT`] lines
/// of the hunk, "[...]" for what is left out), and the comment below;
/// then `summary`, and the tag it gives, signed by `me`. `comments` are
/// (line of the patch's diff, counted from 0 in the part
/// [`crate::vdiff::split_raw`] gives, and what to say about it); a line
/// that is not in a hunk is an error.
pub fn review_body(
    to: &Message,
    comments: &[(usize, String)],
    summary: &str,
    tag: Option<&str>,
    me: &Me,
) -> Result<String, String> {
    let diff = crate::vdiff::split_raw(&to.body).1;
    let lines: Vec<&str> = diff.lines().collect();
    let mut sorted: Vec<&(usize, String)> = comments.iter().filter(|(_, t)| !t.trim().is_empty()).collect();
    sorted.sort_by_key(|(at, _)| *at);
    let mut body = attribution(to) + "\n";
    // The file and the hunk last quoted, and the last line quoted.
    let (mut file, mut hunk, mut last): (Option<usize>, Option<usize>, Option<usize>) = (None, None, None);
    for (at, text) in sorted {
        let at = *at;
        let line = lines.get(at).ok_or_else(|| format!("line {at} is not in the patch"))?;
        let f = lines[..=at].iter().rposition(|l| l.starts_with("diff --git "));
        let h = lines[..=at].iter().rposition(|l| l.starts_with("@@ "));
        let (Some(f), Some(h)) = (f, h) else { return Err(format!("line {at} is not in a hunk")) };
        if h < f || at == h || line.starts_with("diff --git ") {
            return Err(format!("line {at} is not a line of a hunk"));
        }
        if file != Some(f) {
            body += "\n";
            for l in &lines[f..h] {
                body += &(quoted(l) + "\n");
            }
        }
        // Where this quote starts: after what is quoted already, past the
        // hunk's header, and no more than REVIEW_CONTEXT lines above.
        let from = (h + 1).max(at + 1 - REVIEW_CONTEXT.min(at + 1));
        let from = if hunk == Some(h) { from.max(last.map_or(0, |l| l + 1)) } else { from };
        if hunk != Some(h) {
            body += &(quoted(lines[h]) + "\n");
        }
        if from > last.map_or(h + 1, |l| l + 1).max(h + 1) {
            body += "> [...]\n";
        }
        for l in &lines[from..=at] {
            body += &(quoted(l) + "\n");
        }
        body += &format!("\n{}\n", text.trim());
        (file, hunk, last) = (Some(f), Some(h), Some(at));
    }
    if last.is_none() {
        body += &quote_short(&to.body);
        body += "\n";
    }
    let summary = summary.trim();
    if !summary.is_empty() {
        body += &format!("\n{summary}\n");
    }
    if let Some(tag) = tag {
        body += &format!("\n{tag}: {}\n", me.display());
    }
    Ok(body)
}

/// Thanks for applied patches, in `b4 ty`'s form, to the series' cover (or
/// its only patch): each patch with the commit a tree check found for it,
/// "(no commit info)" where none did. With only some patches applied, the
/// subject says "(subset)", as b4's does.
pub fn thanks(s: &Series, cover: &Message, me: &Me, date: &str, msgid: &str) -> Vec<u8> {
    let numbers: Vec<u32> = s.titles.keys().copied().filter(|&n| n > 0 || s.total == 0).collect();
    let pad = numbers.len().max(1).to_string().len();
    let mut summary = vec![];
    let mut applied = 0;
    for &n in &numbers {
        let prefix = format!("[{:0pad$}/{}] ", n.max(1), numbers.len().max(1));
        summary.push(format!("{prefix}{}", s.titles[&n]));
        let commit = s.tree_rows.get(&n).and_then(|rows| rows.iter().find_map(|r| r.commit_sha.clone()));
        let indent = " ".repeat(prefix.chars().count());
        match commit {
            Some(c) => {
                applied += 1;
                summary.push(format!("{indent}commit: {c}"));
            }
            None => summary.push(format!("{indent}(no commit info)")),
        }
    }
    let body = format!(
        "{}\n{}\n\nApplied, thanks!\n\n{}\n\nBest regards,\n-- \n{}\n",
        attribution(cover),
        quote_short(&cover.body),
        summary.join("\n"),
        me.display()
    );
    let subset = if applied > 0 && applied < numbers.len() { "(subset) " } else { "" };
    let subject = format!("Re: {subset}{}", strip_re(&cover.subject));
    reply_as(cover, me, &subject, &body, date, msgid)
}

/// A file name for a draft: the posting's date and the family, with
/// nothing a file system (Windows' included) would refuse.
pub fn draft_name(n: &Nudge) -> String {
    let key: String = n
        .family
        .key
        .chars()
        .take(60)
        .map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c })
        .collect();
    format!("{}-{}.eml", n.family.head().date.get(..10).unwrap_or("undated"), key.trim_end_matches(['.', ' ']))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_encodings() {
        assert_eq!(rfc2822(1_791_194_400), "Mon, 05 Oct 2026 10:00:00 +0000");
        assert_eq!(rfc2822(0), "Thu, 01 Jan 1970 00:00:00 +0000");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(encode("plain"), "plain");
        assert_eq!(encode("Re: héllo"), "=?utf-8?b?UmU6IGjDqWxsbw==?=");
        assert_eq!(encode_addresses("Mé <m@x>, a@x"), "=?utf-8?b?TcOp?= <m@x>, a@x");
        assert_eq!(encode_addresses("Dev J. One <d@x>"), "\"Dev J. One\" <d@x>");
        let long = header("To", &(0..8).map(|i| format!("person{i}@example.org")).collect::<Vec<_>>().join(", "));
        assert!(long.lines().all(|l| l.len() <= FOLD_AT), "{long}");
        assert!(long.lines().skip(1).all(|l| l.starts_with(' ')));
    }

    fn parse(raw: &str) -> Message {
        Message::parse(raw.as_bytes()).unwrap()
    }

    const PATCH: &str = "Message-ID: <p1@x>\nFrom: Dev One <dev@x.org>\nTo: list@vger.org\n\
        Subject: [PATCH 1/1] usb: fix\nDate: Mon, 05 Oct 2026 10:00:00 +0000\n\n\
        Fix it.\n\nSigned-off-by: Dev One <dev@x.org>\n---\n a.c | 3 ++-\n\n\
        diff --git a/a.c b/a.c\nindex 1..2 100644\n--- a/a.c\n+++ b/a.c\n\
        @@ -1,10 +1,11 @@\n one\n two\n three\n four\n five\n six\n seven\n-old\n+new\n+more\n eight\n\
        diff --git a/b.c b/b.c\n--- a/b.c\n+++ b/b.c\n@@ -1 +1 @@\n-x\n+y\n-- \n2.51.0\n";

    #[test]
    fn a_review_quotes_each_comments_hunk_up_to_its_line() {
        let to = parse(PATCH);
        let me = Me { name: "R".into(), addr: "r@x.org".into() };
        let diff: Vec<String> = crate::vdiff::split_raw(&to.body).1.lines().map(str::to_string).collect();
        let at = |l: &str| diff.iter().position(|d| d == l).unwrap();
        let comments = [
            (at("+more"), "Why more?".to_string()),
            (at("+new"), "  Good.  ".to_string()),
            (at("+y"), "And y.".to_string()),
            (at(" two"), "".to_string()), // nothing to say: no quote
        ];
        let body = review_body(&to, &comments, "Otherwise fine.", Some("Reviewed-by"), &me).unwrap();
        assert_eq!(
            body,
            "On Mon, 05 Oct 2026 10:00:00 +0000, Dev One wrote:\n\n\
             > diff --git a/a.c b/a.c\n> index 1..2 100644\n> --- a/a.c\n> +++ b/a.c\n> @@ -1,10 +1,11 @@\n> [...]\n\
             >  four\n>  five\n>  six\n>  seven\n> -old\n> +new\n\nGood.\n> +more\n\nWhy more?\n\n\
             > diff --git a/b.c b/b.c\n> --- a/b.c\n> +++ b/b.c\n> @@ -1 +1 @@\n> -x\n> +y\n\nAnd y.\n\n\
             Otherwise fine.\n\nReviewed-by: R <r@x.org>\n"
        );
        // Not a line of a hunk: the commit message's, a header, past the end.
        for bad in [0, at("diff --git a/b.c b/b.c"), at("@@ -1 +1 @@"), diff.len() + 3] {
            assert!(review_body(&to, &[(bad, "x".into())], "", None, &me).is_err(), "{bad}");
        }
        // No comment: a short quote, then what you say.
        let body = review_body(&to, &[], "Thanks.", Some("Acked-by"), &me).unwrap();
        assert!(body.ends_with("> Fix it.\n\nThanks.\n\nAcked-by: R <r@x.org>\n"), "{body}");
    }

    const COVER: &str = "Message-ID: <c@x>\nFrom: Dev One <dev@x.org>\nTo: Maint <m@x.org>, list@vger.org\n\
        Cc: me@x.org, other@x.org\nSubject: [PATCH v2 0/2] usb: fix things\nDate: Mon, 05 Oct 2026 10:00:00 +0000\n\n\
        Hi all,\n\nThis fixes two things.\nThe first is old.\n\nSigned-off-by: Dev One <dev@x.org>\n---\nbase-commit: abc\n";

    #[test]
    fn a_reply_goes_to_everyone_but_you_threaded_under_what_it_answers() {
        let to = parse(COVER);
        assert_eq!(
            recipients(&to, "ME@x.org"),
            (vec!["dev@x.org".into(), "m@x.org".into(), "list@vger.org".into()], vec!["other@x.org".into()])
        );
        let me = Me { name: "Mé Reviewer".into(), addr: "me@x.org".into() };
        let raw = reply(&to, &me, "Looks good.\n", "Tue, 06 Oct 2026 10:00:00 +0000", "<r1@x.org>");
        assert_eq!(
            String::from_utf8(raw).unwrap(),
            "From: =?utf-8?b?TcOpIFJldmlld2Vy?= <me@x.org>\nTo: dev@x.org, m@x.org, list@vger.org\nCc: other@x.org\n\
             Subject: Re: [PATCH v2 0/2] usb: fix things\nIn-Reply-To: <c@x>\nReferences: <c@x>\n\
             Date: Tue, 06 Oct 2026 10:00:00 +0000\nMessage-ID: <r1@x.org>\n\
             Content-Type: text/plain; charset=\"utf-8\"\nContent-Transfer-Encoding: 7bit\nMIME-Version: 1.0\n\nLooks good.\n"
        );
        // A reply to a reply: Re: once, and the whole chain of References.
        let re = parse(
            "Message-ID: <r@x>\nFrom: m@x.org\nTo: dev@x.org\nSubject: Re: [PATCH] a\n\
                        In-Reply-To: <c@x>\nReferences: <c@x>\n\nWhy?\n",
        );
        let raw = String::from_utf8(reply(&re, &me, "x\n", "d", "<r2@x>")).unwrap();
        assert!(raw.contains("Subject: Re: [PATCH] a\n") && raw.contains("References: <c@x> <r@x>\n"), "{raw}");
    }

    #[test]
    fn quotes_as_b4_does() {
        let to = parse(COVER);
        // Past the greeting; the message only: no trailers, no diff or footer.
        assert_eq!(quote_short(&to.body), "> This fixes two things.\n> The first is old.");
        let long: String = (1..=9).map(|i| format!("line {i}\n")).collect::<String>() + "\nmore\n";
        assert_eq!(quote_short(&long).lines().last(), Some("> [...]"));
        assert_eq!(quote_short(&long).lines().count(), 11);
        assert_eq!(quote_short("Reviewed-by: R <r@x>\n"), ">");
        assert_eq!(quote_all("a\n\n> b\n"), "> a\n>\n>> b");
        let me = Me { name: "R".into(), addr: "r@x.org".into() };
        let on = "On Mon, 05 Oct 2026 10:00:00 +0000, Dev One wrote:\n";
        assert_eq!(
            reply_body(&to, "", Some("Reviewed-by"), &me),
            format!("{on}> This fixes two things.\n> The first is old.\n\nReviewed-by: R <r@x.org>\n")
        );
        assert_eq!(
            reply_body(&to, " Nice. ", Some("Acked-by"), &me),
            format!("{on}> This fixes two things.\n> The first is old.\n\nNice.\n\nAcked-by: R <r@x.org>\n")
        );
        // Nothing to say yet: everything quoted, to answer inline.
        assert!(reply_body(&to, "", None, &me).starts_with(&format!("{on}> Hi all,\n>\n> This fixes")));
    }

    #[test]
    fn thanks_in_b4_ty_form_from_the_tree_check() {
        let p = |n: u32, title: &str| {
            parse(&format!(
                "Message-ID: <p{n}@x>\nFrom: Dev One <dev@x.org>\nSubject: [PATCH v2 {n}/2] {title}\n\
                 In-Reply-To: <c@x>\nReferences: <c@x>\nDate: Mon, 05 Oct 2026 10:00:00 +0000\n\n---\n a | 1 +\n"
            ))
        };
        let msgs = [parse(COVER), p(1, "usb: one"), p(2, "usb: two")];
        let mut facts = crate::series::Facts::default();
        let row = crate::series::TreeRow {
            tree: "usb".into(),
            role: crate::series::Role::Maintainer,
            ref_sha: "f".repeat(40),
            commit_sha: Some("0123456789ab".into()),
            how: Some("patch-id".into()),
            reverted_by: None,
            checked_at: "2026-10-07T00:00:00Z".into(),
        };
        facts.trees.insert("p1@x".into(), vec![row]);
        let s = crate::series::build(&msgs, &facts, &Default::default()).remove("c@x").unwrap();
        let me = Me { name: "Maint".into(), addr: "m@x.org".into() };
        let raw = String::from_utf8(thanks(&s, &msgs[0], &me, "d", "<t@x>")).unwrap();
        assert!(raw.contains("Subject: Re: (subset) [PATCH v2 0/2] usb: fix things\n"), "{raw}");
        assert!(raw.contains("To: dev@x.org, list@vger.org\nCc: me@x.org, other@x.org\n"), "{raw}");
        let body = raw.split_once("\n\n").unwrap().1;
        assert_eq!(
            body,
            "On Mon, 05 Oct 2026 10:00:00 +0000, Dev One wrote:\n> This fixes two things.\n> The first is old.\n\n\
             Applied, thanks!\n\n[1/2] usb: one\n      commit: 0123456789ab\n[2/2] usb: two\n      (no commit info)\n\n\
             Best regards,\n-- \nMaint <m@x.org>\n"
        );
    }
}
