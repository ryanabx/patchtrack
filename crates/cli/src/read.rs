//! Reading in a terminal: a series' conversation (`read`) and what changed
//! between two of its versions (`diff`).
//!
//! Messages come in reply order, indented by depth. Long quotes collapse to
//! the few lines a comment answers, signatures are cut, and a patch shows
//! its message with its diff summarised unless diffs are asked for.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};

use patchtrack_core::evidence;
use patchtrack_core::series::Series;
use patchtrack_core::{Message, thread, vdiff};

use crate::pretty::{BOLD, DIM, paint};

/// Quoted lines kept above each comment.
const CONTEXT: usize = 3;
const INDENT: &str = "  ";
const MAX_DEPTH: u32 = 6;

fn is_quoted(line: &str) -> bool {
    line.trim_start().starts_with('>')
}

/// A line that answers something: not a sign-off, and a sentence or a
/// question ("Why?"), not a bare name.
fn is_prose(line: &str) -> bool {
    let text = line.trim();
    if text.is_empty() || evidence::is_closing(text) {
        return false;
    }
    text.split_whitespace().count() >= 2 || text.ends_with(['?', '.', '!', ':'])
}

/// (line, quoted) pairs of a reply, its long quotes cut to the lines a
/// comment answers, and its signature cut.
pub fn collapse(body: &str) -> Vec<(String, bool)> {
    let mut lines: Vec<&str> = body.lines().take_while(|l| l.trim_end() != "--").collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let mut out = vec![];
    let mut i = 0;
    while i < lines.len() {
        if !is_quoted(lines[i]) {
            out.push((lines[i].to_string(), false));
            i += 1;
            continue;
        }
        let mut j = i;
        while j < lines.len() && (is_quoted(lines[j]) || lines[j].trim().is_empty()) {
            j += 1;
        }
        let mut block = &lines[i..j];
        while block.last().is_some_and(|l| l.trim().is_empty()) {
            block = &block[..block.len() - 1];
        }
        // Prose follows: keep what it answers. A sign-off ("Thanks,", a bare
        // name) is not prose; "Why?" is.
        let answered = lines[j..].iter().any(|l| !is_quoted(l) && is_prose(l));
        let keep = if answered { &block[block.len().saturating_sub(CONTEXT)..] } else { &[][..] };
        if block.len() > keep.len() {
            out.push((format!("[… {} quoted lines]", block.len() - keep.len()), true));
        }
        out.extend(keep.iter().map(|q| (q.to_string(), true)));
        if answered && lines[j - 1].is_empty() {
            out.push((String::new(), false));
        }
        i = j;
    }
    out
}

/// A cover letter up to b4's footer (base-commit:, change-id:) or the signature.
pub fn cover_part(body: &str) -> String {
    let footer = |l: &&str| {
        l.starts_with("base-commit:")
            || l.starts_with("change-id:")
            || l.starts_with("prerequisite-")
            || l.trim_end() == "--"
    };
    body.lines().take_while(|l| !footer(l)).collect::<Vec<_>>().join("\n").trim_end().to_string()
}

/// A patch's message, and its diff or (without `diffs`) a one-line summary of it.
pub fn patch_part(body: &str, diffs: bool) -> (String, Option<String>) {
    let (message, rest) = vdiff::split_raw(body);
    if diffs || rest.is_empty() {
        return (body.lines().collect::<Vec<_>>().join("\n").trim_end().to_string(), None);
    }
    let files = rest.lines().filter(|l| l.starts_with("diff --git ")).count();
    let changed = rest
        .lines()
        .filter(|l| (l.starts_with('+') || l.starts_with('-')) && !l.starts_with("+++") && !l.starts_with("---"))
        .count();
    let s = if files == 1 { "" } else { "s" };
    (message, Some(format!("[diff: {files} file{s}, {changed} lines changed; --diffs to show]")))
}

/// Lines for the conversation of series s.
pub fn conversation(s: &Series, messages: &HashMap<&str, &Message>, color: bool, diffs: bool) -> Vec<String> {
    let index: HashMap<&str, u32> = s.patches.iter().map(|(&i, m)| (m.as_str(), i)).collect();
    let replies: HashMap<&str, _> = s.replies.iter().map(|r| (r.msgid.as_str(), r)).collect();
    let mut out = vec![];
    for (m, depth) in thread::order(s, messages) {
        let pad = INDENT.repeat(depth.min(MAX_DEPTH) as usize);
        let at = index.get(m.msgid.as_str()).copied();
        let head = match at {
            Some(i) => {
                let what = if s.total > 1 { format!("{i}/{}", s.total) } else { "patch".into() };
                format!("{what}  {}", m.subject.split_whitespace().collect::<Vec<_>>().join(" "))
            }
            None => {
                let r = replies.get(m.msgid.as_str());
                let what = match r {
                    Some(r) if r.mine => "(you)",
                    Some(r) if r.bot => "[bot]",
                    _ => "",
                };
                let who = if m.from.name.is_empty() { &m.from.addr } else { &m.from.name };
                format!("{who} {what}").trim().to_string()
            }
        };
        let date = m.date.get(..16).unwrap_or(&m.date).replace('T', " ");
        out.push(format!("{pad}{}{}", paint(&format!("── {head} "), BOLD, color), paint(&date, DIM, color)));
        let body: Vec<(String, bool)> = match at {
            Some(i) => {
                let (text, summary) = if i > 0 { patch_part(&m.body, diffs) } else { (cover_part(&m.body), None) };
                text.lines().map(|l| (l.to_string(), false)).chain(summary.map(|s| (s, true))).collect()
            }
            None => collapse(&m.body),
        };
        for (line, quoted) in body {
            out.push(format!("{pad}   {}", if quoted { paint(&line, DIM, color) } else { line }));
        }
        out.push(String::new());
    }
    out
}

/// Lines describing the change from one version to another.
pub fn version_diff(old: &Series, new: &Series, color: bool) -> Vec<String> {
    let changes = vdiff::changes(old, new);
    let mut lines =
        vec![paint(&format!("{}: v{} → v{}", new.title, old.version, new.version), BOLD, color), String::new()];
    if !changes.changelog.is_empty() {
        lines.push(format!("  {}", paint("The author's changelog:", DIM, color)));
        lines.extend(changes.changelog.iter().map(|l| format!("    {l}")));
        lines.push(String::new());
    }
    for p in changes.pairs {
        let o = p.old.as_ref().map_or("-".to_string(), |r| format!("{}/{}", r.patch, old.total));
        let n = p.new.as_ref().map_or("-".to_string(), |r| format!("{}/{}", r.patch, new.total));
        let title = p.new.as_ref().or(p.old.as_ref()).map_or("", |r| r.title.as_str());
        let code = match p.marker.as_str() {
            "=" => DIM,
            "!" => "\x1b[33m",
            "<" => "\x1b[31m",
            _ => "\x1b[32m",
        };
        lines.push(paint(&format!("{o:>5} {} {n:<5} {title}", p.marker), code, color));
        for d in p.message.iter().chain(&p.diff) {
            let code = if d.starts_with('+') && !d.starts_with("+++") {
                "\x1b[32m"
            } else if d.starts_with('-') && !d.starts_with("---") {
                "\x1b[31m"
            } else if d.starts_with("@@") {
                "\x1b[36m"
            } else {
                DIM
            };
            lines.push(format!("      {}", paint(d, code, color)));
        }
    }
    lines
}

/// Show lines through $PAGER (or `less -RFX`) on a terminal; straight to
/// stdout otherwise, or where there is no pager (Windows has no `less`).
pub fn page(lines: &[String]) -> std::io::Result<()> {
    let mut text = lines.join("\n");
    text.push('\n');
    let stdout = std::io::stdout();
    if stdout.is_terminal() {
        let pager = std::env::var("PAGER").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "less -RFX".into());
        let mut words = pager.split_whitespace();
        if let Some(program) = words.next()
            && let Ok(mut child) = Command::new(program).args(words).stdin(Stdio::piped()).spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                // The reader may quit before the end: not an error.
                let _ = stdin.write_all(text.as_bytes());
            }
            child.wait()?;
            return Ok(());
        }
    }
    stdout.lock().write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(out: &[(String, bool)]) -> Vec<&str> {
        out.iter().map(|(l, _)| l.as_str()).collect()
    }

    #[test]
    fn keeps_what_a_comment_answers() {
        let body = String::from("On Mon, X wrote:\n")
            + &(0..10).map(|i| format!("> line {i}\n")).collect::<String>()
            + "\nWhy?\n\nThanks\n";
        let out = collapse(&body);
        let t = texts(&out);
        assert_eq!(t[..5], ["On Mon, X wrote:", "[… 7 quoted lines]", "> line 7", "> line 8", "> line 9"]);
        assert!(t.contains(&"Why?"));
        assert!(out[1].1 && out[2].1 && !out[0].1);
    }

    #[test]
    fn trailing_quote_and_sign_off() {
        let body = String::from("Looks good to me overall.\n\n")
            + &(0..20).map(|i| format!("> l{i}\n")).collect::<String>()
            + "\nSam\n-- \nsig\n";
        let out = collapse(&body);
        let t = texts(&out);
        assert!(t.contains(&"[… 20 quoted lines]"), "{t:?}");
        assert!(!t.contains(&"> l19"));
        assert_eq!(t.last(), Some(&"Sam"));
        assert!(!t.contains(&"sig"));
    }

    #[test]
    fn a_sign_off_answers_nothing() {
        let out = collapse("> a\n> b\n> c\n> d\n\nThank you,\nSam\n");
        assert_eq!(texts(&out), ["[… 4 quoted lines]", "Thank you,", "Sam"]);
    }

    #[test]
    fn patch_and_cover_parts() {
        let body =
            "Fix it.\n\nSigned-off-by: D\n---\n a.c | 2 +-\n\ndiff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\n-x\n+y\n";
        let (head, summary) = patch_part(body, false);
        assert_eq!(head, "Fix it.\n\nSigned-off-by: D");
        // The blank line before the diff is not a changed line (the Python version counted it).
        assert_eq!(summary.as_deref(), Some("[diff: 1 file, 2 lines changed; --diffs to show]"));
        let (all, none) = patch_part(body, true);
        assert!(all.contains("+y") && none.is_none());
        let cover = "Intro.\n\n---\nD (1):\n  a: fix\n\nbase-commit: abc\nchange-id: x\n\nBest regards,\n-- \nD\n";
        assert_eq!(cover_part(cover), "Intro.\n\n---\nD (1):\n  a: fix");
    }

    #[test]
    fn reply_order_and_depth() {
        use patchtrack_core::rules::Rules;
        use patchtrack_core::series::{self, Facts};
        let mail = |id: &str, subject: &str, from: &str, refs: &[&str], date: &str, body: &str| {
            let irt = refs.last().map_or(String::new(), |p| {
                let all: Vec<String> = refs.iter().map(|r| format!("<{r}>")).collect();
                format!("In-Reply-To: <{p}>\nReferences: {}\n", all.join(" "))
            });
            let raw = format!("Message-ID: <{id}>\nSubject: {subject}\nFrom: {from}\n{irt}Date: {date}\n\n{body}");
            Message::parse(raw.as_bytes()).unwrap()
        };
        let me = "Dev <dev@example.org>";
        let d = "Mon, 05 Oct 2026 10:00:00 +0000";
        let msgs = vec![
            mail("c@x", "[PATCH 0/2] foo: do the thing", me, &[], d, "Cover.\n"),
            mail("p1@x", "[PATCH 1/2] foo: part 1", me, &["c@x"], d, "One.\n"),
            mail("p2@x", "[PATCH 2/2] foo: part 2", me, &["c@x"], d, "Two.\n"),
            mail(
                "a@x",
                "Re: [PATCH 2/2] foo: part 2",
                "Rev <r@x>",
                &["c@x", "p2@x"],
                "Tue, 06 Oct 2026 10:00:00 +0000",
                "> +x\nWhy is this here?\n",
            ),
            mail(
                "b@x",
                "Re: [PATCH 2/2] foo: part 2",
                "dev@example.org",
                &["c@x", "p2@x", "a@x"],
                "Tue, 06 Oct 2026 11:00:00 +0000",
                "> Why is this here?\nBecause of the reset.\n",
            ),
            mail(
                "l@x",
                "Re: [PATCH 0/2] foo: do the thing",
                "Late <l@x>",
                &["c@x"],
                "Wed, 07 Oct 2026 10:00:00 +0000",
                "Applied, thanks!\n",
            ),
        ];
        let found = series::build(&msgs, &Facts::default(), &Rules::default());
        let by_id: HashMap<&str, &Message> = msgs.iter().map(|m| (m.msgid.as_str(), m)).collect();
        let lines = conversation(&found["c@x"], &by_id, false, false);
        let heads: Vec<&String> = lines.iter().filter(|l| l.contains("── ")).collect();
        let pads: Vec<&str> = heads.iter().map(|h| h.split("──").next().unwrap()).collect();
        assert_eq!(pads, ["", "  ", "  ", "    ", "      ", "  "]);
        assert!(
            heads[1].contains("1/2")
                && heads[3].contains("Rev")
                && heads[4].contains("(you)")
                && heads[5].contains("Late")
        );
    }
}
