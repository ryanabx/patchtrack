//! Mail you write: `reply`, `thanks`, `drafts` and `send`.
//!
//! The core composes (patchtrack_core::compose); this keeps what it writes
//! as drafts in the store, opens your editor, and sends a draft through
//! `git send-email` with your sendemail.* configuration, after showing it
//! whole and only when you type "yes". Recipients come from the draft
//! alone: nothing from the configuration or the body is added.

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::Command;

use patchtrack_core::compose::{self, Me};
use patchtrack_core::series::Series;
use patchtrack_core::settings::Settings;
use patchtrack_core::{Message, mail};
use patchtrack_store::{Draft, Store};

use crate::views::{self, Scope};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Who you are when you write: the name setting (or git's user.name) and
/// your first identity (or git's user.email).
pub(crate) fn me(settings: &Settings) -> Result<Me> {
    let git = |key: &str| {
        crate::git::git(".", &["config", key], None).ok().map(|o| String::from_utf8_lossy(&o).trim().to_string())
    };
    let addr = match settings.identities.first() {
        Some(a) => a.clone(),
        None => git("user.email")
            .filter(|a| a.contains('@'))
            .map(|a| a.to_lowercase())
            .ok_or("who are you? set your addresses: patchtrack settings set identities '[\"you@example.org\"]'")?,
    };
    let name = if settings.name.is_empty() { git("user.name").unwrap_or_default() } else { settings.name.clone() };
    Ok(Me { name, addr })
}

/// A fresh Message-ID on your own domain, without <>.
pub(crate) fn new_msgid(me: &Me) -> String {
    let domain = me.addr.rsplit_once('@').map_or("localhost", |(_, d)| d);
    format!("{}.{}.patchtrack@{domain}", crate::now_ms(), std::process::id())
}

fn now() -> Result<(i64, String)> {
    let secs = i64::try_from(crate::now_ms() / 1000)?;
    Ok((secs, mail::utc(secs)))
}

/// The one series a query names, as `close` picks it; or the stored
/// message a message-id names, and the series it is in.
fn target<'a>(
    families: &'a [patchtrack_core::series::Family],
    by_id: &HashMap<&str, &'a Message>,
    query: &str,
) -> Result<(&'a Series, Option<&'a Message>)> {
    let id = query.trim().trim_start_matches('<').trim_end_matches('>');
    if let Some(m) = by_id.get(id) {
        let has = |s: &&Series| {
            s.root == id || s.patches.values().any(|p| p == id) || s.replies.iter().any(|r| r.msgid == id)
        };
        if let Some(s) = families.iter().flat_map(|f| &f.versions).find(has) {
            return Ok((s, Some(m)));
        }
    }
    let hits = views::pick(families, query, false)?;
    match hits.as_slice() {
        [one] => Ok((one, None)),
        many => {
            for s in many {
                eprintln!("  {}  (family {}, from {})", s.title, s.family, s.sender);
            }
            Err(format!("{} series match {query:?}: narrow it, or name a message-id", many.len()).into())
        }
    }
}

/// Run your editor (git's: `git var GIT_EDITOR`) on a file.
fn edit(path: &std::path::Path) -> Result<()> {
    let editor = crate::git::git(".", &["var", "GIT_EDITOR"], None)?;
    let editor = String::from_utf8_lossy(&editor).trim().to_string();
    let status = if cfg!(windows) {
        let mut words = editor.split_whitespace();
        Command::new(words.next().ok_or("no editor: set core.editor or EDITOR")?).args(words).arg(path).status()?
    } else {
        // As git runs it: through the shell, so an editor with options works.
        Command::new("sh").args(["-c", &format!("{editor} \"$@\""), &editor]).arg(path).status()?
    };
    if !status.success() {
        return Err(format!("{editor} failed; no draft kept").into());
    }
    Ok(())
}

pub struct ReplyArgs<'a> {
    pub query: &'a str,
    pub patch: Option<u32>,
    pub tag: Option<&'a str>,
    pub text: Option<&'a str>,
}

/// Compose a reply, keep it as a draft, and print it.
pub fn cmd_reply(store: &mut Store, settings: &Settings, scope: &Scope, args: ReplyArgs<'_>) -> Result<()> {
    let tag = match args.tag {
        Some(t) => Some(
            *compose::TAGS
                .iter()
                .find(|k| k.eq_ignore_ascii_case(t))
                .ok_or_else(|| format!("{t}: not a tag; the tags are {}", compose::TAGS.join(", ")))?,
        ),
        None => None,
    };
    let me = me(settings)?;
    let families = views::load(store, settings, scope)?;
    let messages = store.messages()?;
    let by_id: HashMap<&str, &Message> = messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
    let (s, named) = target(&families, &by_id, args.query)?;
    let to = match (named, args.patch) {
        (Some(m), None) => m,
        (_, Some(n)) => {
            let id = s.patches.get(&n).ok_or_else(|| format!("{} has no patch {n}", s.title))?;
            by_id.get(id.as_str()).copied().ok_or("that patch is not in the store")?
        }
        (None, None) => by_id.get(s.root.as_str()).copied().ok_or("the series' first message is not in the store")?,
    };
    let mut body = compose::reply_body(to, args.text.unwrap_or(""), tag, &me);
    if args.text.is_none() && tag.is_none() {
        // Nothing said yet: write it in your editor, below the quote.
        let path = std::env::temp_dir().join(format!("patchtrack-reply-{}.txt", std::process::id()));
        std::fs::write(&path, &body)?;
        edit(&path)?;
        let written = std::fs::read_to_string(&path)?.replace("\r\n", "\n");
        let _ = std::fs::remove_file(&path);
        if written.trim() == body.trim() || written.lines().all(|l| l.trim().is_empty() || l.starts_with('>')) {
            return Err("nothing written; no draft kept".into());
        }
        body = written;
    }
    let msgid = new_msgid(&me);
    let (secs, at) = now()?;
    let raw = compose::reply(to, &me, &body, &compose::rfc2822(secs), &format!("<{msgid}>"));
    let kind = if tag.is_some() { "tag" } else { "reply" };
    let family = format!("{}\0{}", s.sender, s.family);
    store.save_draft(&Draft {
        msgid: msgid.clone(),
        family,
        kind: kind.into(),
        raw: raw.clone(),
        created_at: at,
        sent_at: None,
    })?;
    let mut out = std::io::stdout().lock();
    out.write_all(&raw)?;
    writeln!(out, "\nDraft {msgid} kept. Send it with: patchtrack send {msgid}")?;
    Ok(())
}

/// Thank the author for the applied patches, in b4 ty's form.
pub fn cmd_thanks(store: &mut Store, settings: &Settings, scope: &Scope, query: &str) -> Result<()> {
    let me = me(settings)?;
    let families = views::load(store, settings, scope)?;
    let messages = store.messages()?;
    let by_id: HashMap<&str, &Message> = messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
    let (s, _) = target(&families, &by_id, query)?;
    let cover = by_id.get(s.root.as_str()).copied().ok_or("the series' first message is not in the store")?;
    if !s.tree_rows.values().flatten().any(|r| r.commit_sha.is_some()) {
        eprintln!(
            "patchtrack: no tree check found a commit for these patches (see `check`): every one says \"no commit info\""
        );
    }
    let msgid = new_msgid(&me);
    let (secs, at) = now()?;
    let raw = compose::thanks(s, cover, &me, &compose::rfc2822(secs), &format!("<{msgid}>"));
    let family = format!("{}\0{}", s.sender, s.family);
    store.save_draft(&Draft {
        msgid: msgid.clone(),
        family,
        kind: "thanks".into(),
        raw: raw.clone(),
        created_at: at,
        sent_at: None,
    })?;
    let mut out = std::io::stdout().lock();
    out.write_all(&raw)?;
    writeln!(out, "\nDraft {msgid} kept. Send it with: patchtrack send {msgid}")?;
    Ok(())
}

/// The draft an id names: whole, or a unique part of it.
fn find(store: &Store, id: &str) -> Result<Draft> {
    let id = id.trim().trim_start_matches('<').trim_end_matches('>');
    let all = store.drafts()?;
    if let Some(d) = all.iter().find(|d| d.msgid == id) {
        return Ok(d.clone());
    }
    let hits: Vec<&Draft> = all.iter().filter(|d| d.msgid.contains(id)).collect();
    match hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("no draft {id}; `patchtrack drafts` lists them").into()),
        _ => Err(format!("{} drafts match {id}: give more of it", hits.len()).into()),
    }
}

/// A header's value as written (encoded words and all), unfolded.
fn header(raw: &str, name: &str) -> String {
    let head = raw.split("\n\n").next().unwrap_or("").replace("\n ", " ").replace("\n\t", " ");
    head.lines()
        .find_map(|l| {
            l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim().to_string())
        })
        .unwrap_or_default()
}

pub enum DraftsAction {
    List,
    Show(String),
    Remove(String),
}

pub fn cmd_drafts(store: &mut Store, action: DraftsAction) -> Result<()> {
    let mut out = std::io::stdout().lock();
    match action {
        DraftsAction::List => {
            for d in store.drafts()? {
                let raw = String::from_utf8_lossy(&d.raw);
                let when = d.sent_at.as_ref().map_or(String::new(), |t| format!(" sent {}", &t[..16.min(t.len())]));
                let subject = Message::parse(&d.raw).map(|m| m.subject).unwrap_or_default();
                writeln!(
                    out,
                    "{}  {:<6} {}{when}\n    {subject}\n    to {}",
                    d.msgid,
                    d.kind,
                    &d.created_at[..10.min(d.created_at.len())],
                    header(&raw, "To")
                )?;
            }
        }
        DraftsAction::Show(id) => out.write_all(&find(store, &id)?.raw)?,
        DraftsAction::Remove(id) => {
            let d = find(store, &id)?;
            store.remove_draft(&d.msgid)?;
            eprintln!("removed {}", d.msgid);
        }
    }
    Ok(())
}

/// Where git keeps send-email, if it is installed (Fedora and Debian
/// package it apart, as git-email).
pub(crate) fn send_email_installed() -> bool {
    crate::git::git(".", &["--exec-path"], None).is_ok_and(|p| {
        let dir = PathBuf::from(String::from_utf8_lossy(&p).trim());
        dir.join("git-send-email").exists() || dir.join("git-send-email.exe").exists()
    })
}

const NO_SEND_EMAIL: &str = "git send-email is not installed (it is the git-email package on Fedora and Debian)";

/// Send a message as it is with git send-email: to its To and Cc and no
/// one else (none from the configuration, none from the body's tags),
/// threaded by its own headers. `quiet` keeps send-email's output from
/// the terminal (a server's log has its own line).
pub fn deliver(raw: &[u8], quiet: bool) -> std::result::Result<(), String> {
    if !send_email_installed() {
        return Err(NO_SEND_EMAIL.into());
    }
    let m = Message::parse(raw).ok_or("not a message")?;
    if m.to.is_empty() {
        return Err("the message has no To".into());
    }
    let path = std::env::temp_dir().join(format!("patchtrack-send-{}-{}.eml", std::process::id(), crate::now_ms()));
    std::fs::write(&path, raw).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("git");
    cmd.args(["send-email", "--confirm=never", "--no-to", "--no-cc", "--no-bcc", "--no-signed-off-by-cc"])
        .args(["--suppress-cc=self", "--suppress-cc=author", "--suppress-cc=sob"])
        .args(["--suppress-cc=misc-by", "--suppress-cc=bodycc", "--no-annotate", "--no-format-patch"])
        // From exactly as the message has it: any other spelling, and
        // send-email takes it for someone else's patch and adds an in-body
        // From: line.
        .arg(format!("--from={}", header(&String::from_utf8_lossy(raw), "From")));
    for a in &m.to {
        cmd.arg(format!("--to={a}"));
    }
    for a in &m.cc {
        cmd.arg(format!("--cc={a}"));
    }
    cmd.arg(&path).stdin(std::process::Stdio::null());
    let out = cmd.output();
    let _ = std::fs::remove_file(&path);
    let out = out.map_err(|e| format!("git send-email: {e}"))?;
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !quiet {
        eprint!("{stderr}{stdout}");
    }
    // Its exit status is not enough: given an address it cannot read, it
    // says so, sends nothing, and exits 0. It sent only if it says it did.
    let sent =
        stdout.lines().any(|l| l.strip_prefix("Result: ").is_some_and(|r| r.starts_with("OK") || r.starts_with('2')));
    if out.status.success() && sent {
        return Ok(());
    }
    let why = stderr.lines().chain(stdout.lines()).rfind(|l| l.contains("error") || l.starts_with("Result:"));
    Err(format!("git send-email did not send it{}", why.map_or(String::new(), |w| format!(": {}", w.trim()))))
}

/// Show a draft whole, ask, and send it with git send-email; false if it was not sent.
pub fn cmd_send(store: &mut Store, settings: &Settings, id: &str) -> Result<bool> {
    let d = find(store, id)?;
    if let Some(at) = &d.sent_at {
        return Err(format!("{} was sent already, at {at}", d.msgid).into());
    }
    if !send_email_installed() {
        return Err(format!(
            "{NO_SEND_EMAIL}; or save the draft with `patchtrack drafts show ID > reply.eml` and send it \
                            from your mail client"
        )
        .into());
    }
    let m = Message::parse(&d.raw).ok_or("the draft is not a message")?;
    let mut out = std::io::stdout().lock();
    out.write_all(&d.raw)?;
    out.flush()?;
    let n = m.to.len() + m.cc.len();
    eprint!("\nSend this to {n} address{} with git send-email? Type yes to send: ", if n == 1 { "" } else { "es" });
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if answer.trim() != "yes" {
        eprintln!("Not sent. The draft is kept.");
        return Ok(false);
    }
    deliver(&d.raw, false).map_err(|e| format!("{e}; the draft is kept"))?;
    let (_, at) = now()?;
    store.mark_sent(&d.msgid, &at)?;
    // What you sent is mail of yours: in the store now, as local mail is.
    crate::store_local(store, settings, std::slice::from_ref(&d.raw))?;
    eprintln!("Sent {}.", d.msgid);
    Ok(true)
}
