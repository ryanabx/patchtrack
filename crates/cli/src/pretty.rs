//! Terminal output for `list`: a coloured circle per state, aligned columns.
//!
//! The circle's shape carries the meaning without colour too: filled for a
//! state, half for partially applied, hollow for nothing known yet or
//! superseded, crossed for closed.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use patchtrack_core::news;
use patchtrack_core::series::{Series, State};
use regex::Regex;

pub const RESET: &str = "\x1b[0m";
pub const DIM: &str = "\x1b[2m";
pub const BOLD: &str = "\x1b[1m";

/// Roughly GitHub's: purple merged, green in, grey waiting. In the order
/// the summary line counts them.
const STYLE: [(State, &str, &str); 13] = [
    (State::Merged, "●", "\x1b[35m"),
    (State::InNext, "●", "\x1b[32m"),
    (State::Queued, "●", "\x1b[32m"),
    (State::Applied, "●", "\x1b[32m"),
    (State::PartiallyApplied, "◐", "\x1b[32m"),
    (State::Commented, "●", "\x1b[33m"),
    (State::Reviewed, "●", "\x1b[36m"),
    (State::Discussed, "●", "\x1b[34m"),
    (State::BotOnly, "●", "\x1b[90m"),
    (State::NoReply, "○", ""),
    (State::Unfetched, "○", "\x1b[90m"),
    (State::Superseded, "○", "\x1b[90m"),
    (State::Closed, "⊘", "\x1b[90m"),
];

/// In the default view addresses and message-ids are noise; -v keeps them.
static ADDR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r" <[^<>\s]+@[^<>\s]+>").expect("valid"));
const FLAG: &str = "unanswered";

/// Whether to colour: `always`, `never`, or `auto` (a terminal, unless NO_COLOR is set).
pub fn use_color(choice: &str, is_terminal: bool) -> bool {
    let wanted = match choice {
        "always" => true,
        "never" => false,
        _ => std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()) && is_terminal,
    };
    wanted && ansi()
}

/// A Windows console shows escape codes only once asked to; this asks.
#[cfg(windows)]
fn ansi() -> bool {
    ratatui::crossterm::ansi_support::supports_ansi()
}

#[cfg(not(windows))]
fn ansi() -> bool {
    true
}

pub fn paint(text: &str, code: &str, color: bool) -> String {
    if color && !code.is_empty() { format!("{code}{text}{RESET}") } else { text.to_string() }
}

/// A state's circle and its ANSI colour.
pub fn style(state: State) -> (&'static str, &'static str) {
    STYLE.iter().find(|(s, ..)| *s == state).map_or(("●", ""), |&(_, shape, code)| (shape, code))
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// Cut to `w` characters, the last an ellipsis.
fn fit(text: &str, w: usize) -> String {
    if width(text) <= w { text.to_string() } else { text.chars().take(w.saturating_sub(1)).collect::<String>() + "…" }
}

fn pad(text: &str, w: usize) -> String {
    format!("{text}{}", " ".repeat(w.saturating_sub(width(text))))
}

pub fn circle(state: State, color: bool) -> String {
    let (shape, code) = style(state);
    paint(shape, code, color)
}

fn meta(s: &Series) -> String {
    let n = s.patches.keys().filter(|&&i| i > 0).count().max(1);
    let date = s.last_activity().get(..10).unwrap_or("");
    format!("v{} · {n} patch{} · {date}", s.version, if n == 1 { "" } else { "es" })
}

/// Lines for (series, state, evidence) rows, in aligned columns that fit `cols`.
pub fn render(rows: &[(&Series, State, &str)], color: bool, cols: usize, verbose: bool) -> Vec<String> {
    if rows.is_empty() {
        return vec![];
    }
    let state_w = rows.iter().map(|(_, st, _)| st.as_str().len()).max().unwrap_or(0);
    let metas: Vec<String> = rows.iter().map(|(s, ..)| meta(s)).collect();
    let meta_w = metas.iter().map(|m| width(m)).max().unwrap_or(0);
    let flag_w = if rows.iter().any(|(s, ..)| s.unanswered) { FLAG.len() + 2 } else { 0 };
    let title_w = cols.saturating_sub(2 + state_w + 2 + 2 + meta_w + flag_w).max(20);
    let indent = " ".repeat(2 + state_w + 2);
    let mut out = vec![];
    for ((s, state, evidence), m) in rows.iter().zip(&metas) {
        let code = style(*state).1;
        let title = pad(&fit(&format!("{}{}", s.title, if s.rfc { " [RFC]" } else { "" }), title_w), title_w);
        let flag =
            if s.unanswered { format!("  {}", paint(FLAG, &format!("\x1b[33m{BOLD}"), color)) } else { String::new() };
        out.push(format!(
            "{} {}  {}  {}{flag}",
            circle(*state, color),
            paint(&pad(state.as_str(), state_w), code, color),
            paint(&title, BOLD, color),
            paint(&format!("{m:>meta_w$}"), DIM, color),
        ));
        let mut detail = if verbose { evidence.to_string() } else { ADDR.replace_all(evidence, "").into_owned() };
        if s.pending && !verbose {
            detail = if detail.is_empty() { String::new() } else { detail + "; " } + "a fresher fetch is pending";
        }
        if !detail.is_empty() {
            out.push(indent.clone() + &paint(&fit(&detail, cols.saturating_sub(indent.len())), DIM, color));
        }
        if verbose {
            out.push(indent.clone() + &paint(&format!("<{}>  {}", s.root, s.as_of_text()), DIM, color));
        }
    }
    out
}

/// The count of each state, wrapped at `cols` between items.
pub fn summary(states: &[State], color: bool, cols: Option<usize>) -> String {
    let mut counts: BTreeMap<State, usize> = BTreeMap::new();
    for &st in states {
        *counts.entry(st).or_default() += 1;
    }
    let (mut lines, mut line, mut used): (Vec<String>, Vec<String>, usize) = (vec![], vec![], 0);
    for (st, ..) in STYLE.iter().filter(|(st, ..)| counts.contains_key(st)) {
        let n = counts[st];
        let plain = format!("{} {n} {st}", style(*st).0);
        if !line.is_empty() && cols.is_some_and(|c| used + 3 + width(&plain) > c) {
            lines.push(line.join("   "));
            (line, used) = (vec![], 0);
        }
        line.push(format!("{} {n} {st}", circle(*st, color)));
        used += if used > 0 { 3 } else { 0 } + width(&plain);
    }
    lines.push(line.join("   "));
    lines.join("\n")
}

/// Lines for one change `news` found: a headline, then what happened.
pub fn change(c: &news::Change, color: bool) -> Vec<String> {
    let s = c.family.head();
    let mut lines = vec![format!(
        "{} {}  {}",
        circle(c.family.state, color),
        paint(&s.title, BOLD, color),
        paint(&format!("v{}", s.version), DIM, color)
    )];
    if c.new_series {
        lines.push(format!("    new: {}", c.family.state));
    }
    if let Some(v) = c.new_version {
        lines.push(format!("    v{v} posted"));
    }
    if let Some((before, after)) = c.state {
        lines.push(format!("    {} {before} → {} {after}", circle(before, color), circle(after, color)));
    }
    for (series, reply) in &c.replies {
        lines.push(format!("    {}", news::describe(series, reply)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use patchtrack_core::Message;
    use patchtrack_core::rules::Rules;
    use patchtrack_core::series::{self, Facts, Family};

    fn mail(id: &str, subject: &str, from: &str, irt: Option<&str>, date: &str, body: &str) -> Message {
        let irt = irt.map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n")).unwrap_or_default();
        let raw = format!("Message-ID: <{id}>\nSubject: {subject}\nFrom: {from}\n{irt}Date: {date}\n\n{body}");
        Message::parse(raw.as_bytes()).unwrap()
    }

    fn posting(tag: &str, v: u32, n: u32, title: &str, date: &str) -> Vec<Message> {
        let pre = if v > 1 { format!(" v{v}") } else { String::new() };
        let root = format!("{tag}-0@x");
        let mut out = vec![mail(&root, &format!("[PATCH{pre} 0/{n}] {title}"), "Dev <dev@x>", None, date, "")];
        for i in 1..=n {
            let subject = format!("[PATCH{pre} {i}/{n}] foo: part {i}");
            out.push(mail(&format!("{tag}-{i}@x"), &subject, "Dev <dev@x>", Some(&root), date, ""));
        }
        out
    }

    /// Two families: one plain, one with a very long title and a question on it.
    fn families() -> Vec<Family> {
        let mut msgs = posting("pa", 1, 2, "foo: do the thing", "Mon, 05 Oct 2026 10:00:00 +0000");
        msgs.extend(posting("pb", 3, 1, &"a very long title ".repeat(8), "Mon, 05 Oct 2026 11:00:00 +0000"));
        msgs.push(mail(
            "q@x",
            "Re: [PATCH v3 0/1] x",
            "R <r@x>",
            Some("pb-0@x"),
            "Tue, 06 Oct 2026 10:00:00 +0000",
            "> +x\nWhy is this needed?\n",
        ));
        let mut facts = Facts::default();
        for root in ["pa-0@x", "pb-0@x"] {
            facts.fetched.insert(root.into(), "2026-10-07T00:00:00Z".into());
        }
        series::families(series::build(&msgs, &facts, &Rules::default()), &facts)
    }

    fn rows(fams: &[Family]) -> Vec<(&Series, State, &str)> {
        fams.iter().map(|f| (f.head(), f.state, f.evidence.as_str())).collect()
    }

    #[test]
    fn plain_is_aligned_and_fits() {
        let fams = families();
        let lines: Vec<String> =
            render(&rows(&fams), false, 100, false).into_iter().filter(|l| l.starts_with(['●', '○', '◐'])).collect();
        assert_eq!(lines.len(), 2);
        assert!(!lines.concat().contains('\x1b'));
        let date_at: Vec<usize> = lines.iter().map(|l| l[..l.find("2026-10-").unwrap()].chars().count()).collect();
        assert_eq!(date_at[0], date_at[1], "{lines:?}"); // the date column lines up
        assert!(lines.iter().all(|l| width(l) <= 100), "{lines:?}");
        assert!(lines.concat().contains('…'));
    }

    #[test]
    fn evidence_drops_addresses_unless_verbose() {
        let fams = families();
        let plain = render(&rows(&fams), false, 120, false).join("\n");
        assert!(plain.contains("R on 0/1: \"Why is this needed?\""), "{plain}");
        assert!(!plain.contains("<r@x>"));
        assert!(render(&rows(&fams), false, 120, true).join("\n").contains("<r@x>"));
    }

    #[test]
    fn color() {
        let fams = families();
        assert!(render(&rows(&fams), true, 100, false).join("\n").contains("\x1b[33m●")); // commented is yellow
        let s = summary(&[State::NoReply, State::Commented, State::NoReply], false, None);
        assert!(s.starts_with("● 1 commented   ○ 2 no-reply"), "{s}");
    }

    #[test]
    fn summary_wraps() {
        use State::*;
        let states = [Merged, InNext, Applied, Commented, Reviewed, BotOnly, NoReply];
        let lines: Vec<String> = summary(&states, false, Some(40)).lines().map(str::to_string).collect();
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| width(l) <= 40), "{lines:?}");
        assert_eq!(summary(&states, false, None).lines().count(), 1);
    }

    #[test]
    fn when_to_colour() {
        // NO_COLOR is read from the environment, which tests share: only the cases it cannot change.
        assert!(use_color("always", false));
        assert!(!use_color("never", true));
        assert!(!use_color("auto", false));
    }
}
