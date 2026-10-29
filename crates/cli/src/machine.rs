//! What `bridge` and `serve` do with b4 for the web app: list the prep
//! branches of the repositories they were given, and run b4 on one. Each
//! is the command line's own (`branches`' reading), behind the checks a
//! request from a page needs: only the repositories and branches named,
//! only three b4 actions.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use patchtrack_core::settings::Settings;
use serde_json::{Value, json};

/// The repository a request names, if it is one of ours.
fn repo<'a>(repos: &'a [PathBuf], named: &str) -> Result<&'a Path, String> {
    repos
        .iter()
        .find(|r| r.to_string_lossy() == named)
        .map(PathBuf::as_path)
        .ok_or_else(|| format!("{named:?} is not a repository this server was given"))
}

/// The b4 prep branches of these repositories, as JSON.
pub fn branches(repos: &[PathBuf], settings: &Settings) -> Result<Value, String> {
    let mut out = vec![];
    for repo in repos {
        let repo = repo.to_string_lossy();
        for b in crate::git::branches(&repo, &settings.b4_prefix).map_err(|e| e.to_string())? {
            let sent: Vec<Value> =
                b.tracking.sent().iter().map(|(v, cover)| json!({"version": v, "cover": cover})).collect();
            out.push(json!({
                "repo": repo, "name": b.name, "subject": b.subject, "change_id": b.tracking.change_id,
                "revision": b.tracking.revision, "sent": sent, "patches": b.patch_ids.len(),
            }));
        }
    }
    Ok(Value::Array(out))
}

/// Run b4 on a prep branch ("send", "resend", "trailers"); whether it
/// succeeded, and what it printed.
pub fn b4(
    repos: &[PathBuf],
    settings: &Settings,
    repo_name: &str,
    branch: &str,
    action: &str,
    dry_run: bool,
) -> Result<(bool, String), String> {
    let path = repo(repos, repo_name)?;
    let repo_s = path.to_string_lossy().into_owned();
    let known = crate::git::branches(&repo_s, &settings.b4_prefix).map_err(|e| e.to_string())?;
    if !known.iter().any(|b| b.name == branch) {
        return Err(format!("{branch:?} is not a b4 prep branch of {repo_name}"));
    }
    // b4 works on the branch checked out: in a worktree of its own, so
    // your checkout is never switched; in it, when it is the branch.
    let current = crate::git::git(&repo_s, &["rev-parse", "--abbrev-ref", "HEAD"], None).map_err(|e| e.to_string())?;
    let here = String::from_utf8_lossy(&current).trim() == branch;
    let tree = std::env::temp_dir().join(format!("patchtrack-b4-{}-{}", std::process::id(), crate::now_ms()));
    if !here {
        let t = tree.to_string_lossy();
        crate::git::git(&repo_s, &["worktree", "add", "-q", &t, branch], None).map_err(|e| e.to_string())?;
    }
    let dir = if here { path.to_path_buf() } else { tree.clone() };
    let (args, input): (Vec<&str>, &[u8]) = match (action, dry_run) {
        ("send", true) => (vec!["send", "--dry-run"], b""),
        // One Enter, for b4's last question. Should its pre-flight checks
        // fail, it asks first: the Enter goes there, and nothing is sent.
        ("send", false) => (vec!["send"], b"\n"),
        ("resend", true) => (vec!["send", "--resend", "--dry-run"], b""),
        ("resend", false) => (vec!["send", "--resend"], b"\n"),
        // b4 shows what it found, then asks; "n" leaves the branch alone.
        ("trailers", true) => (vec!["trailers", "-u"], b"n\n"),
        ("trailers", false) => (vec!["-n", "trailers", "-u"], b""),
        _ => {
            remove_tree(&repo_s, &tree, here);
            return Err(format!("{action:?}: the actions are send, resend and trailers"));
        }
    };
    let run = run_b4(&dir, &args, input);
    remove_tree(&repo_s, &tree, here);
    let (ok, output) = run?;
    eprintln!("b4 {} on {branch} in {repo_name}: {}", args.join(" "), if ok { "done" } else { "failed" });
    Ok((ok, output))
}

fn remove_tree(repo: &str, tree: &Path, here: bool) {
    if !here {
        let _ = crate::git::git(repo, &["worktree", "remove", "--force", &tree.to_string_lossy()], None);
    }
}

/// b4, in `dir`, given `input`: whether it succeeded, and all it printed.
fn run_b4(dir: &Path, args: &[&str], input: &[u8]) -> Result<(bool, String), String> {
    use std::io::Write;
    let mut child = Command::new("b4")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("b4: {e} (is b4 installed?)"))?;
    let mut stdin = child.stdin.take().expect("piped");
    let _ = stdin.write_all(input);
    drop(stdin);
    let out = child.wait_with_output().map_err(|e| format!("b4: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text += &String::from_utf8_lossy(&out.stderr);
    Ok((out.status.success(), text))
}
