//! The commands that show series: `list`, `show`, `people`, `open`, and
//! `close`/`reopen`, which act on what they show.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};

use patchtrack_core::projects;
use patchtrack_core::series::{self, Family, Role, Series, State};
use patchtrack_core::settings::{Settings, Subscription};
use patchtrack_core::views::{self, Person};
use patchtrack_core::{Message, compose, mail, news, nudge, sync, vdiff};
use patchtrack_store::Store;
use serde_json::{Map, Value, json};

use crate::{pretty, read};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Whose series, and from where.
#[derive(clap::Args)]
pub struct Scope {
    /// Only series this address sent or wrote (repeatable; default: what you
    /// follow and your identities' series, or everyone's if there are none)
    #[arg(short, long = "author")]
    authors: Vec<String>,
    /// Everyone's series, not only those of the authors you follow
    #[arg(long, conflicts_with = "authors")]
    everyone: bool,
    /// Only series a sync found on this inbox
    #[arg(short, long)]
    list: Option<String>,
    /// Only this project's series (see `project`)
    #[arg(short, long)]
    project: Option<String>,
}

/// Every family in scope, latest first; closed ones included.
pub fn load(store: &Store, settings: &Settings, scope: &Scope) -> Result<Vec<Family>> {
    let facts = store.facts()?;
    let mut families = series::families(series::build(&store.messages()?, &facts, &settings.rules), &facts);
    let (authors, subs): (Vec<String>, Vec<Subscription>) = if scope.everyone {
        (vec![], vec![])
    } else if !scope.authors.is_empty() {
        (scope.authors.iter().map(|a| a.to_lowercase()).collect(), vec![])
    } else {
        (settings.identities.clone(), store.subscriptions()?)
    };
    if !authors.is_empty() || !subs.is_empty() {
        families.retain(|f| f.versions.iter().any(|s| views::follows(s, &authors, &subs, &facts)));
    }
    if let Some(list) = &scope.list {
        families.retain(|f| f.versions.iter().any(|s| s.lists.contains(list)));
    }
    if let Some(name) = &scope.project {
        // Among all your projects: one with a better claim on a series takes it.
        let p = crate::project(settings, name)?;
        let all = &settings.projects;
        families.retain(|f| {
            f.versions.iter().any(|s| projects::of(s, &facts, all, &settings.archive).contains(&p.name.as_str()))
        });
    }
    Ok(families)
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .or_else(|| terminal_size::terminal_size().map(|(w, _)| usize::from(w.0)))
        .unwrap_or(100)
}

#[derive(clap::Args)]
pub struct ListArgs {
    #[command(flatten)]
    scope: Scope,
    /// Only series in this state (repeatable)
    #[arg(long, value_parser = State::ALL.map(|s| s.as_str()))]
    state: Vec<String>,
    /// Every version, each with its own state, not only the latest
    #[arg(long)]
    all_versions: bool,
    /// Include families you closed
    #[arg(long)]
    closed: bool,
    /// Only where someone wrote after the author last did
    #[arg(long)]
    unanswered: bool,
    /// Only series not applied, merged or superseded
    #[arg(long)]
    open: bool,
    /// Only series sent (To: or Cc:) to an address containing ADDR
    #[arg(long, value_name = "ADDR")]
    to: Option<String>,
    /// Only series where someone matching WHO replied
    #[arg(long, value_name = "WHO")]
    replied: Option<String>,
    /// Everything, as JSON
    #[arg(long)]
    json: bool,
    /// Addresses, message-ids and fetch times too
    #[arg(short, long)]
    verbose: bool,
    /// Colour the circles (auto: on a terminal, unless NO_COLOR is set)
    #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"])]
    color: String,
}

/// A series as `list --json` gives it: the model, the row's state and
/// evidence, its family's, and what a reader of the JSON would otherwise
/// have to derive.
fn row_json(f: &Family, s: &Series, state: State, evidence: &str, archive: &str) -> Result<Value> {
    let mut v = serde_json::to_value(s)?;
    let obj = v.as_object_mut().expect("a series is an object");
    for (k, x) in [
        ("state", json!(state)),
        ("evidence", json!(evidence)),
        ("family_state", json!(f.state)),
        ("family_evidence", json!(f.evidence)),
        ("closed", json!(f.closed)),
        ("label", json!(s.label())),
        ("url", json!(views::message_url(archive, &s.root))),
        ("as_of_text", json!(s.as_of_text())),
        ("last_activity", json!(s.last_activity())),
        ("evidence_msgids", json!(views::evidence_msgids(evidence))),
        // The Python version's names, kept for scripts written against it.
        ("author", json!(s.sender)),
        ("trees", json!(s.tree_rows)),
        (
            "stages",
            json!(
                s.stages.iter().map(|(i, &st)| (i.to_string(), json!(series::stage_name(st)))).collect::<Map<_, _>>()
            ),
        ),
        (
            "tags",
            json!(
                s.tags().map(|(i, k, w, m)| json!({"patch": i, "kind": k, "who": w, "msgid": m})).collect::<Vec<_>>()
            ),
        ),
    ] {
        obj.insert(k.into(), x);
    }
    Ok(v)
}

pub fn cmd_list(store: &Store, settings: &Settings, args: &ListArgs) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let families = load(store, settings, &args.scope)?;
    let wants = |st: State| args.state.is_empty() || args.state.iter().any(|w| w == st.as_str());
    let mut rows: Vec<(&Family, &Series, State, &str)> = vec![];
    for f in &families {
        if f.closed.is_some() && !(args.closed || args.state.iter().any(|w| w == "closed")) {
            continue;
        }
        let shown: Vec<(&Series, State, &str)> = if args.all_versions {
            f.versions.iter().rev().map(|s| (s, s.state, s.evidence.as_str())).collect()
        } else {
            vec![(f.head(), f.state, f.evidence.as_str())]
        };
        for (s, state, evidence) in shown {
            let to = args.to.as_ref().map(|t| t.to_lowercase());
            let replied = args.replied.as_ref().map(|w| w.to_lowercase());
            if !wants(state)
                || (args.unanswered && !s.unanswered)
                || (args.open && (state.done() || state == State::PartiallyApplied))
                || to.is_some_and(|t| !s.to.iter().any(|a| a.contains(&t)))
                || replied.is_some_and(|w| !s.replies.iter().any(|r| !r.mine && r.who.to_lowercase().contains(&w)))
            {
                continue;
            }
            rows.push((f, s, state, evidence));
        }
    }
    if args.json {
        let json: Vec<Value> = rows
            .iter()
            .map(|(f, s, state, ev)| row_json(f, s, *state, ev, &settings.archive))
            .collect::<Result<_>>()?;
        writeln!(out, "{}", serde_json::to_string_pretty(&json)?)?;
        return Ok(());
    }
    let color = pretty::use_color(&args.color, std::io::stdout().is_terminal());
    let width = terminal_width();
    let plain: Vec<(&Series, State, &str)> = rows.iter().map(|&(_, s, st, ev)| (s, st, ev)).collect();
    for line in pretty::render(&plain, color, width, args.verbose) {
        writeln!(out, "{line}")?;
    }
    if !rows.is_empty() {
        let states: Vec<State> = rows.iter().map(|r| r.2).collect();
        writeln!(out, "\n{}", pretty::summary(&states, color, Some(width)))?;
    }
    Ok(())
}

/// The series a query names, or an error saying none does.
pub(crate) fn pick<'a>(families: &'a [Family], query: &str, all: bool) -> Result<Vec<&'a Series>> {
    let hits = views::pick(families, query, all);
    if hits.is_empty() {
        return Err(format!("no series matches {query:?}").into());
    }
    Ok(hits)
}

pub fn cmd_show(store: &Store, settings: &Settings, scope: &Scope, query: &str) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let families = load(store, settings, scope)?;
    let hits = pick(&families, query, true)?;
    for f in families.iter().filter(|f| f.versions.len() > 1 && f.versions.iter().any(|s| hits.contains(&s))) {
        writeln!(out, "family {}: {}, {} versions", f.key, f.state, f.versions.len())?;
        writeln!(out, "{}", if f.evidence.is_empty() { String::new() } else { format!("  {}\n", f.evidence) })?;
    }
    let when = |t: &str| t.get(..16).unwrap_or(t).replace('T', " ");
    for s in hits {
        writeln!(out, "{}", s.label())?;
        writeln!(out, "  {}{}, {}", s.state, if s.unanswered { " (unanswered)" } else { "" }, s.as_of_text())?;
        if !s.evidence.is_empty() {
            writeln!(out, "  {}", s.evidence)?;
        }
        writeln!(out, "  root <{}>, family {}", s.root, s.family)?;
        if !s.written_by.is_empty() {
            let by: Vec<&str> = s.written_by.iter().map(String::as_str).collect();
            writeln!(out, "  written by {}; posted by {}", by.join(", "), s.sender)?;
        }
        for (&i, msgid) in &s.patches {
            let stage = if i > 0 {
                format!(" [{}]", series::stage_name(s.stages.get(&i).copied().unwrap_or(0)))
            } else {
                String::new()
            };
            writeln!(out, "  {i}/{} <{msgid}>{stage}", s.total)?;
            for (_, kind, who, _) in s.tags().filter(|t| t.0 == i) {
                writeln!(out, "      {kind}: {who}")?;
            }
            for row in s.tree_rows.get(&i).into_iter().flatten() {
                let short = |sha: &str| sha.get(..12).unwrap_or(sha).to_string();
                let mut at = match &row.commit_sha {
                    Some(c) => format!("{} (by {})", short(c), row.how.as_deref().unwrap_or("?")),
                    None => "not found".into(),
                };
                if let Some(r) = &row.reverted_by {
                    at += &format!(", reverted by {}", short(r));
                }
                writeln!(out, "      {} @ {}: {at}, {} UTC", row.tree, short(&row.ref_sha), when(&row.checked_at))?;
            }
        }
        for r in &s.replies {
            let who =
                if r.mine { "(you)".to_string() } else { format!("{}{}", r.who, if r.bot { " [bot]" } else { "" }) };
            writeln!(out, "  {} on {}/{}: {who}", when(&r.date), r.index, s.total)?;
        }
        writeln!(out)?;
    }
    Ok(())
}

pub fn cmd_people(store: &Store, settings: &Settings, scope: &Scope, bots: bool, as_json: bool) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let families = load(store, settings, scope)?;
    let mut people: Vec<Person> = views::people(families.iter().flat_map(|f| &f.versions));
    if !bots {
        people.retain(|p| !p.bot);
    }
    if as_json {
        writeln!(out, "{}", serde_json::to_string_pretty(&people)?)?;
        return Ok(());
    }
    writeln!(
        out,
        "{:<10}  {:>6} {:>7} {:>4} {:>8} {:>7}  who",
        "last", "series", "replies", "tags", "comments", "applied"
    )?;
    for p in &people {
        writeln!(
            out,
            "{:<10}  {:>6} {:>7} {:>4} {:>8} {:>7}  {}",
            p.last.get(..10).unwrap_or(&p.last),
            p.series.len(),
            p.replies,
            p.tags,
            p.comments,
            p.applied,
            p.who
        )?;
    }
    Ok(())
}

/// Close (or reopen) the families a query names. More than one is refused
/// unless `all`, so a vague query cannot close what you did not mean.
pub fn cmd_close(
    store: &mut Store,
    settings: &Settings,
    scope: &Scope,
    query: &str,
    reason: Option<&str>,
    all: bool,
    close: bool,
) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let families = load(store, settings, scope)?;
    let hits = pick(&families, query, all)?;
    let mut chosen: Vec<(String, &Series)> = vec![];
    for s in hits {
        let id = format!("{}\0{}", s.sender, s.family);
        if !chosen.iter().any(|(i, _)| *i == id) {
            chosen.push((id, s));
        }
    }
    if chosen.len() > 1 && !all {
        for (_, s) in &chosen {
            eprintln!("  {}  (family {}, from {})", s.title, s.family, s.sender);
        }
        return Err(format!("{} families match {query:?}: narrow it, or pass --all", chosen.len()).into());
    }
    let now = mail::utc(i64::try_from(crate::now_ms() / 1000)?);
    for (id, s) in chosen {
        if close {
            store.close_family(&id, reason.unwrap_or(""), &now)?;
            writeln!(out, "closed: {}", s.title)?;
        } else if store.reopen_family(&id)? {
            writeln!(out, "reopened: {}", s.title)?;
        } else {
            writeln!(out, "not closed: {}", s.title)?;
        }
    }
    Ok(())
}

pub fn cmd_open(store: &Store, settings: &Settings, scope: &Scope, query: &str, all: bool, print: bool) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let families = load(store, settings, scope)?;
    for s in pick(&families, query, all)? {
        let url = format!("{}/all/{}/T/#u", settings.archive, sync::path_quote(&s.root));
        if print || open::that(&url).is_err() {
            writeln!(out, "{url}")?;
        } else {
            eprintln!("opened {url}");
        }
    }
    Ok(())
}

pub fn cmd_read(
    store: &Store,
    settings: &Settings,
    scope: &Scope,
    query: &str,
    all: bool,
    diffs: bool,
    color: &str,
) -> Result<()> {
    let families = load(store, settings, scope)?;
    let hits = pick(&families, query, all)?;
    let messages = store.messages()?;
    let by_id: HashMap<&str, &Message> = messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
    let color = pretty::use_color(color, std::io::stdout().is_terminal());
    let mut lines = vec![];
    for s in hits {
        lines.push(pretty::paint(
            &format!("{}  ·  {}  ·  {}", s.label(), s.state, s.as_of_text()),
            pretty::BOLD,
            color,
        ));
        lines.push(String::new());
        lines.extend(read::conversation(s, &by_id, color, diffs));
    }
    Ok(read::page(&lines)?)
}

pub fn cmd_diff(
    store: &Store,
    settings: &Settings,
    scope: &Scope,
    query: &str,
    from: Option<u32>,
    to: Option<u32>,
    color: &str,
) -> Result<()> {
    let families = load(store, settings, scope)?;
    let hits = pick(&families, query, true)?;
    let matched: Vec<&Family> =
        families.iter().filter(|f| f.versions.iter().any(|v| hits.iter().any(|h| std::ptr::eq(*h, v)))).collect();
    if matched.len() > 1 {
        for f in &matched {
            eprintln!("  {}  (family {}, from {})", f.head().title, f.key, f.sender);
        }
        return Err(format!("{} families match {query:?}: narrow it", matched.len()).into());
    }
    let by_v = vdiff::versions(matched[0]);
    if by_v.len() < 2 {
        return Err(format!("only one version of {:?} is known", matched[0].head().title).into());
    }
    let new = match to {
        Some(v) => by_v.get(&v).copied(),
        None => by_v.values().next_back().copied(),
    };
    let old = match (from, new) {
        (Some(v), _) => by_v.get(&v).copied(),
        (None, Some(n)) => by_v.range(..n.version).next_back().map(|(_, s)| *s),
        (None, None) => None,
    };
    let (Some(old), Some(new)) = (old, new) else {
        let known: Vec<String> = by_v.keys().map(|v| format!("v{v}")).collect();
        return Err(format!("versions known: {}", known.join(", ")).into());
    };
    let color = pretty::use_color(color, std::io::stdout().is_terminal());
    Ok(read::page(&read::version_diff(old, new, color))?)
}

/// `news`: what changed since the last look, or (`since`) since a time.
pub fn cmd_news(
    store: &mut Store,
    settings: &Settings,
    scope: &Scope,
    peek: bool,
    since: Option<&str>,
    color: &str,
) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let families = load(store, settings, scope)?;
    let now = i64::try_from(crate::now_ms() / 1000)?;
    let (seen, peek) = match since {
        Some(text) => {
            let when = mail::utc(sync::since(text, now)?);
            writeln!(out, "Changes since {} UTC:", when.get(..16).unwrap_or(&when).replace('T', " "))?;
            (news::baseline(&store.messages()?, &store.facts()?, &settings.rules, &when), true)
        }
        None => (store.seen()?, peek),
    };
    let color = pretty::use_color(color, std::io::stdout().is_terminal());
    if seen.is_empty() && since.is_none() {
        let n = families.len();
        writeln!(out, "First look: {n} series recorded. From now on, `news` shows what changed.")?;
    } else {
        let open: Vec<Family> = families.iter().filter(|f| f.closed.is_none()).cloned().collect();
        let found = news::changes(&open, &seen);
        for c in &found {
            for line in pretty::change(c, color) {
                writeln!(out, "{line}")?;
            }
        }
        if found.is_empty() {
            writeln!(out, "Nothing new.")?;
        }
    }
    if !peek {
        let shown: Vec<(String, String)> = families.iter().map(|f| (f.id(), news::snapshot(f))).collect();
        store.mark_seen(shown.iter().map(|(f, s)| (f.as_str(), s.as_str())), &mail::utc(now))?;
    }
    Ok(())
}

#[derive(clap::Args)]
pub struct NudgeArgs {
    #[command(flatten)]
    scope: Scope,
    /// Ping after this many quiet days (default: the ping_days setting)
    #[arg(long)]
    ping_days: Option<i64>,
    /// RESEND after this many days with no human reply (default: the resend_days setting)
    #[arg(long)]
    resend_days: Option<i64>,
    /// Write a ping for each as an .eml file in DIR (never sent)
    #[arg(long, value_name = "DIR")]
    draft: Option<std::path::PathBuf>,
    /// Do not hold pings for an open merge window (read from a mainline tree's tags)
    #[arg(long)]
    ignore_merge_window: bool,
    /// Colour (auto: on a terminal, unless NO_COLOR is set)
    #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"])]
    color: String,
}

/// Whether the merge window is open, from a mainline tree's tags; None
/// with no mainline tree, or when the merge_window setting is off.
pub(crate) fn merge_window(store: &Store, settings: &Settings, now: i64) -> Result<Option<nudge::Window>> {
    let mainline = store.trees()?.into_iter().find(|t| t.role == Role::Mainline);
    Ok(match mainline {
        Some(t) if settings.merge_window => Some(match crate::git::tags(&t.repo) {
            Ok(tags) => nudge::merge_window(&tags, now, &t.repo),
            Err(e) => nudge::Window { why: e.to_string(), ..nudge::Window::default() },
        }),
        _ => None,
    })
}

pub fn cmd_nudge(store: &mut Store, settings: &Settings, args: &NudgeArgs) -> Result<()> {
    let mut out = std::io::stdout().lock();
    let ping = args.ping_days.unwrap_or(settings.ping_days);
    let resend = args.resend_days.unwrap_or(settings.resend_days);
    let now = i64::try_from(crate::now_ms() / 1000)?;
    let window = if args.ignore_merge_window { None } else { merge_window(store, settings, now)? };
    let color = pretty::use_color(&args.color, std::io::stdout().is_terminal());
    match &window {
        Some(w) if w.open == Some(true) => {
            let text = format!(
                "The merge window has been open since {} ({}): pings and RESENDs wait for -rc1.",
                w.tag, w.since
            );
            writeln!(out, "{}", pretty::paint(&text, pretty::DIM, color))?;
        }
        Some(w) if w.open.is_none() => {
            writeln!(out, "{}", pretty::paint(&format!("Merge window unknown: {}.", w.why), pretty::DIM, color))?
        }
        _ => {}
    }
    let families = load(store, settings, &args.scope)?;
    let mut found: Vec<nudge::Nudge> =
        families.iter().filter_map(|f| nudge::classify(f, now, ping, resend, window.as_ref())).collect();
    found.sort_by_key(|n| (n.kind, -n.days));
    let messages = if args.draft.is_some() { store.messages()? } else { vec![] };
    let by_id: HashMap<&str, &Message> = messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
    for (k, n) in found.iter().enumerate() {
        let s = n.family.head();
        writeln!(
            out,
            "{} {}  {}  {}",
            pretty::circle(n.family.state, color),
            pretty::paint(&format!("{:<17}", n.kind.as_str()), pretty::BOLD, color),
            s.title,
            pretty::paint(&format!("v{} · {}", s.version, n.why), pretty::DIM, color)
        )?;
        if let Some(dir) = &args.draft
            && n.kind != nudge::Kind::ReplyOwed
            && let Some(root) = by_id.get(s.root.as_str())
        {
            std::fs::create_dir_all(dir)?;
            let from = &n.family.sender;
            let domain = from.rsplit_once('@').map_or("localhost", |(_, d)| d);
            let msgid = format!("<{}.{}.{k}@{domain}>", crate::now_ms(), std::process::id());
            let raw = compose::ping(n, root, from, &settings.archive, &compose::rfc2822(now), &msgid);
            let path = dir.join(compose::draft_name(n));
            std::fs::write(&path, &raw)?;
            store.save_draft(&patchtrack_store::Draft {
                msgid: msgid.trim_matches(['<', '>']).to_string(),
                family: n.family.id(),
                kind: "ping".into(),
                raw,
                created_at: mail::utc(now),
                sent_at: None,
            })?;
            writeln!(out, "{:21}draft: {}", "", path.display())?;
        }
    }
    if found.is_empty() {
        writeln!(out, "Nothing to nudge (ping after {ping} days, RESEND after {resend}).")?;
    } else if args.draft.is_some() {
        writeln!(out, "\nDrafts only: nothing was sent. Edit and send them yourself.")?;
    }
    Ok(())
}
