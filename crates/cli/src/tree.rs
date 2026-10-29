//! `tree` and `check`: the git trees posted patches are looked for in.

use std::io::Write;
use std::path::PathBuf;

use patchtrack_core::series::{Role, TreeRow};
use patchtrack_core::settings::Settings;
use patchtrack_core::subject;
use patchtrack_core::trees::{Posted, Tree};
use patchtrack_core::{Message, mail};
use patchtrack_store::Store;

use crate::{git, pretty, views};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(clap::Subcommand)]
pub enum TreeAction {
    /// Add a tree, or replace the one of that name.
    Add {
        /// What to call it, in results and in `show`
        name: String,
        /// The local repository
        #[arg(long)]
        repo: PathBuf,
        /// A ref already in the repository, e.g. origin/master
        #[arg(long = "ref", value_name = "REF", conflicts_with_all = ["url", "branch"])]
        git_ref: Option<String>,
        /// Fetch the branch from here, whole, into refs/patchtrack/NAME (with --branch)
        #[arg(long, requires = "branch")]
        url: Option<String>,
        /// The branch to fetch from --url
        #[arg(long, requires = "url")]
        branch: Option<String>,
        /// maintainer (found = queued), next (in-next) or mainline (merged)
        #[arg(long, default_value = "maintainer", value_parser = ["maintainer", "next", "mainline"])]
        role: String,
    },
    /// The trees, with their roles and where they come from.
    List,
    /// Forget a tree. Results already found stay with the patches.
    Rm {
        /// A name `tree list` shows
        name: String,
    },
}

pub fn cmd_tree(store: &mut Store, action: TreeAction) -> Result<bool> {
    let mut out = std::io::stdout().lock();
    match action {
        TreeAction::List => {
            for t in store.trees()? {
                let from = match (&t.url, &t.branch) {
                    (Some(u), Some(b)) => format!("{u} {b}"),
                    _ => "local".into(),
                };
                writeln!(out, "{:<16} {:<10} {} {}  ({from})", t.name, t.role.as_str(), t.repo, t.git_ref)?;
            }
        }
        TreeAction::Rm { name } => {
            if !store.remove_tree(&name)? {
                eprintln!("patchtrack: no tree called {name}");
                return Ok(false);
            }
        }
        TreeAction::Add { name, repo, git_ref, url, branch, role } => {
            if git_ref.is_none() && url.is_none() {
                return Err("give either --ref, or --url and --branch".into());
            }
            let repo = std::path::absolute(&repo)?;
            let git_ref = git_ref.unwrap_or_else(|| format!("refs/patchtrack/{name}"));
            let role = Role::parse(&role).expect("clap checked it");
            store.add_tree(&Tree { name, repo: repo.to_string_lossy().into_owned(), git_ref, url, branch, role })?;
        }
    }
    Ok(true)
}

/// Every posted patch of the series in scope.
fn posted(store: &Store, settings: &Settings, scope: &views::Scope) -> Result<Vec<Posted>> {
    let families = views::load(store, settings, scope)?;
    let messages = store.messages()?;
    let by_id: std::collections::HashMap<&str, &Message> = messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
    let mut out = vec![];
    for s in families.iter().flat_map(|f| &f.versions) {
        for (_, msgid) in s.patches.iter().filter(|(i, _)| **i > 0) {
            let Some(m) = by_id.get(msgid.as_str()) else { continue };
            let author = if m.author.addr.is_empty() { &m.from.addr } else { &m.author.addr };
            out.push(Posted {
                msgid: msgid.clone(),
                title: subject::parse(&m.subject).title,
                author: author.clone(),
                date: m.date.clone(),
                body: m.body.clone(),
            });
        }
    }
    Ok(out)
}

pub fn cmd_check(
    store: &mut Store,
    settings: &Settings,
    scope: &views::Scope,
    names: &[String],
    fetch: bool,
) -> Result<bool> {
    let chosen: Vec<Tree> =
        store.trees()?.into_iter().filter(|t| names.is_empty() || names.contains(&t.name)).collect();
    if chosen.is_empty() {
        return Err("no trees: add one with `patchtrack tree add`".into());
    }
    let posted = posted(store, settings, scope)?;
    if posted.is_empty() {
        return Ok(true);
    }
    let mut all_ok = true;
    for t in chosen {
        let result = (|| {
            if fetch && t.url.is_some() {
                eprintln!(
                    "{}: fetching {} {}",
                    t.name,
                    t.url.as_deref().unwrap_or(""),
                    t.branch.as_deref().unwrap_or("")
                );
                git::fetch(&t)?;
            }
            if git::is_shallow(&t.repo)? {
                eprintln!("{}: {} is shallow; 'not found' may be wrong", t.name, t.repo);
            }
            git::check(&t, &posted)
        })();
        let (ref_sha, found) = match result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("{}: {e}", t.name);
                all_ok = false;
                continue;
            }
        };
        let now = mail::utc(i64::try_from(crate::now_ms() / 1000)?);
        for (p, hit) in posted.iter().zip(&found) {
            let row = TreeRow {
                tree: t.name.clone(),
                role: t.role,
                ref_sha: ref_sha.clone(),
                commit_sha: hit.as_ref().map(|h| h.commit.clone()),
                how: hit.as_ref().map(|h| h.how.to_string()),
                reverted_by: hit.as_ref().and_then(|h| h.reverted_by.clone()),
                checked_at: now.clone(),
            };
            store.record_tree_result(&p.msgid, &row)?;
        }
        let n = found.iter().filter(|h| h.is_some()).count();
        eprintln!("{} @ {}: {n} of {} posted patches found", t.name, &ref_sha[..ref_sha.len().min(12)], posted.len());
    }
    Ok(all_ok)
}

/// `branches`: each b4 prep branch against what was posted from it.
pub fn cmd_branches(
    store: &Store,
    settings: &Settings,
    scope: &views::Scope,
    repos: &[PathBuf],
    color: &str,
) -> Result<bool> {
    use std::io::IsTerminal;
    let mut out = std::io::stdout().lock();
    let repos: Vec<PathBuf> = if repos.is_empty() { vec![std::env::current_dir()?] } else { repos.to_vec() };
    let families = views::load(store, settings, scope)?;
    let by_root: std::collections::HashMap<&str, (&patchtrack_core::series::Family, &patchtrack_core::series::Series)> =
        families.iter().flat_map(|f| f.versions.iter().map(move |s| (s.root.as_str(), (f, s)))).collect();
    let color = pretty::use_color(color, std::io::stdout().is_terminal());
    let mut all_ok = true;
    for repo in repos {
        let repo = std::path::absolute(&repo)?.to_string_lossy().into_owned();
        let branches = match git::branches(&repo, &settings.b4_prefix) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{repo}: {e}");
                all_ok = false;
                continue;
            }
        };
        for b in branches {
            let sent = b.tracking.sent();
            let known = sent.iter().rev().find_map(|(v, root)| by_root.get(root.as_str()).map(|fs| (*v, *fs)));
            let (circle, state, note) = match (sent.last(), known) {
                (None, _) => {
                    let n = b.patch_ids.len();
                    let note =
                        format!("v{} never sent; {n} patch{}", b.tracking.revision, if n == 1 { "" } else { "es" });
                    ("◌".to_string(), "draft".to_string(), note)
                }
                (Some((v, _)), None) => (
                    "●".to_string(),
                    "unknown".to_string(),
                    format!("sent up to v{v}, but not in the store (sync further back?)"),
                ),
                (Some(_), Some((v, (f, s)))) => {
                    let bodies: Vec<&str> = s.bodies.iter().filter(|(i, _)| **i > 0).map(|(_, b)| b.as_str()).collect();
                    let posted = git::posted_patch_ids(&repo, &bodies)?;
                    let (changed, added, dropped) = patchtrack_core::b4::compare(&b.patch_ids, &posted);
                    let what: Vec<String> = [(changed, "changed"), (added, "added"), (dropped, "dropped")]
                        .into_iter()
                        .filter(|(n, _)| *n > 0)
                        .map(|(n, w)| format!("{n} {w}"))
                        .collect();
                    let mut note = if what.is_empty() {
                        format!("as sent in v{v}")
                    } else {
                        format!("v{} in preparation: {} since v{v}", b.tracking.revision, what.join(", "))
                    };
                    // Tags given since: b4 trailers -u writes them into the branch.
                    let given: Vec<(String, String)> =
                        s.tags().map(|(_, k, w, _)| (k.to_string(), w.to_string())).collect();
                    let new = patchtrack_core::b4::new_tags(&given, &b.messages);
                    if !new.is_empty() {
                        let n = new.len();
                        note += &format!("; {n} new tag{} to take in: b4 trailers -u", if n == 1 { "" } else { "s" });
                    }
                    (pretty::circle(f.state, color), f.state.as_str().to_string(), note)
                }
            };
            writeln!(out, "{circle} {state:<17} {}", b.name)?;
            writeln!(out, "{:20}{}", "", pretty::paint(&note, pretty::DIM, color))?;
        }
    }
    Ok(all_ok)
}
