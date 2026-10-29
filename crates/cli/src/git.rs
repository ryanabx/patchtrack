//! git, as a subprocess (`git` on PATH; `git.exe` on Windows), for `check`.
//!
//! Fetches take whole branches into refs/patchtrack/NAME: on a shallow
//! history, "not found" would mean nothing. Options that a user's config
//! could turn on and that would change what git prints (colour, signatures,
//! external diff drivers) are turned off on the command line.

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};

use patchtrack_core::trees::{self, Commit, Found, Posted, Tree};

#[derive(Debug)]
pub struct GitError(pub String);

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitError {}

/// Run git in repo, feeding it `input`; its output.
pub fn git(repo: &str, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, GitError> {
    let mut cmd = Command::new("git");
    cmd.args(["-C", repo]);
    for setting in [
        "log.showSignature=false",
        "color.ui=never",
        "diff.noprefix=false",
        "diff.mnemonicPrefix=false",
        "diff.relative=false",
    ] {
        cmd.args(["-c", setting]);
    }
    cmd.args(args);
    cmd.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| GitError(format!("git: {e}")))?;
    if let Some(data) = input {
        let mut stdin = child.stdin.take().expect("piped");
        let data = data.to_vec();
        // Written from a thread, so a large input cannot deadlock against a full stdout.
        let writer = std::thread::spawn(move || stdin.write_all(&data));
        let out = child.wait_with_output().map_err(|e| GitError(format!("git: {e}")))?;
        let _ = writer.join();
        return finish(args, out);
    }
    finish(args, child.wait_with_output().map_err(|e| GitError(format!("git: {e}")))?)
}

fn finish(args: &[&str], out: std::process::Output) -> Result<Vec<u8>, GitError> {
    if !out.status.success() {
        let what = args.iter().take(3).copied().collect::<Vec<_>>().join(" ");
        return Err(GitError(format!("git {what}: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(out.stdout)
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Fetch a tree's whole branch into its ref; nothing for a tree without a URL.
pub fn fetch(tree: &Tree) -> Result<(), GitError> {
    if let (Some(url), Some(branch)) = (&tree.url, &tree.branch) {
        git(&tree.repo, &["fetch", "--quiet", "--no-tags", url, &format!("+{branch}:{}", tree.git_ref)], None)?;
    }
    Ok(())
}

pub fn is_shallow(repo: &str) -> Result<bool, GitError> {
    Ok(text(git(repo, &["rev-parse", "--is-shallow-repository"], None)?).trim() == "true")
}

/// The repository's v* tags, newest first: (name, ISO 8601 date).
pub fn tags(repo: &str) -> Result<Vec<(String, String)>, GitError> {
    let format = "--format=%(refname:short) %(creatordate:iso-strict)";
    let out = text(git(repo, &["for-each-ref", "--sort=-creatordate", format, "refs/tags/v*"], None)?);
    Ok(out.lines().filter_map(|l| l.split_once(' ')).map(|(t, d)| (t.to_string(), d.to_string())).collect())
}

/// `git patch-id --stable` of each text, by position; None where there is no diff.
fn patch_ids(repo: &str, blobs: &[&str]) -> Result<Vec<Option<String>>, GitError> {
    let feed: String =
        blobs.iter().enumerate().map(|(i, b)| format!("From {i:040x} Mon Sep 17 00:00:00 2001\n{b}\n")).collect();
    let mut by_tag: HashMap<usize, String> = HashMap::new();
    for line in text(git(repo, &["patch-id", "--stable"], Some(feed.as_bytes()))?).lines() {
        if let Some((pid, tag)) = line.split_once(' ')
            && let Ok(i) = usize::from_str_radix(tag.trim(), 16)
        {
            by_tag.insert(i, pid.to_string());
        }
    }
    Ok((0..blobs.len()).map(|i| by_tag.remove(&i)).collect())
}

/// A b4 prep branch: its name, what its tracking commit says, and the
/// patch-ids of the commits on top of it.
pub struct Branch {
    pub name: String,
    pub tracking: patchtrack_core::b4::Tracking,
    pub patch_ids: Vec<String>,
    /// The commit messages, newest first.
    pub messages: Vec<String>,
    /// The cover letter's subject: the first line of b4's tracking commit.
    pub subject: String,
}

/// Every b4 prep branch in repo under refs/heads/PREFIX.
pub fn branches(repo: &str, prefix: &str) -> Result<Vec<Branch>, GitError> {
    use patchtrack_core::b4::{MARKER, Tracking};
    let refs = text(git(
        repo,
        &["for-each-ref", "--format=%(refname:short) %(objectname)", &format!("refs/heads/{prefix}")],
        None,
    )?);
    let mut out = vec![];
    for (name, tip) in refs.lines().filter_map(|l| l.split_once(' ')) {
        let grep = format!("--grep=^{MARKER}$");
        let found = text(git(repo, &["log", "-n1", "--format=%H%x00%B", &grep, tip], None)?);
        let Some((cover, body)) = found.trim_matches('\n').split_once('\0') else { continue };
        let Some(tracking) = Tracking::parse(body) else { continue };
        let range = format!("{cover}..{tip}");
        let log = git(
            repo,
            &["log", "-p", "--no-merges", "--no-color", "--no-ext-diff", "--format=commit %H", &range],
            None,
        )?;
        let ids = text(git(repo, &["patch-id", "--stable"], Some(&log))?);
        let patch_ids = ids.lines().filter_map(|l| l.split(' ').next()).map(str::to_string).collect();
        let bodies = text(git(repo, &["log", "--no-merges", "--format=%B%x00", &range], None)?);
        let messages = bodies.split('\0').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect();
        let subject = body.lines().next().unwrap_or("").trim().to_string();
        out.push(Branch { name: name.to_string(), tracking, patch_ids, messages, subject });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// The patch-ids of posted patch bodies that have a diff.
pub fn posted_patch_ids(repo: &str, bodies: &[&str]) -> Result<Vec<String>, GitError> {
    Ok(patch_ids(repo, bodies)?.into_iter().flatten().collect())
}

/// The tree's commit, and where each posted patch is in it, by position.
pub fn check(tree: &Tree, posted: &[Posted]) -> Result<(String, Vec<Option<Found>>), GitError> {
    let target = format!("{}^{{commit}}", tree.git_ref);
    let ref_sha = text(git(&tree.repo, &["rev-parse", "--verify", &target], None)?).trim().to_string();
    let paths = trees::paths(posted);
    if paths.is_empty() {
        return Ok((ref_sha, vec![None; posted.len()]));
    }
    let since = trees::since(posted).map(|s| format!("--since={s}"));
    let mut scope: Vec<&str> = vec!["--no-merges"];
    scope.extend(since.as_deref());
    scope.push(&ref_sha);
    scope.push("--");
    scope.extend(paths.iter().map(String::as_str));

    let log = text(git(
        &tree.repo,
        &[&["log", "--no-color", "--format=%H%x00%ae%x00%s%x00%B%x1e"], &scope[..]].concat(),
        None,
    )?);
    let commits: Vec<Commit> = trees::commits(&log);
    let todo: Vec<usize> = trees::by_message_id(posted, &commits)
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_none())
        .map(|(i, _)| i)
        .collect();
    let mut commit_ids: HashMap<String, String> = HashMap::new();
    let mut posted_ids: Vec<Option<String>> = vec![None; posted.len()];
    if !todo.is_empty() {
        // --full-diff: the path filter picks commits but must not trim their
        // diffs, or a commit that also touches other files would match falsely.
        let args =
            [&["log", "-p", "--full-diff", "--no-color", "--no-ext-diff", "--format=commit %H"], &scope[..]].concat();
        let patches = git(&tree.repo, &args, None)?;
        for line in text(git(&tree.repo, &["patch-id", "--stable"], Some(&patches))?).lines() {
            if let Some((pid, sha)) = line.split_once(' ') {
                commit_ids.entry(pid.to_string()).or_insert_with(|| sha.trim().to_string());
            }
        }
        let bodies: Vec<&str> = todo.iter().map(|&i| posted[i].body.as_str()).collect();
        for (&i, pid) in todo.iter().zip(patch_ids(&tree.repo, &bodies)?) {
            posted_ids[i] = pid;
        }
    }
    Ok((ref_sha, trees::decide(posted, &commits, &commit_ids, &posted_ids)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use patchtrack_core::series::Role;
    use std::path::PathBuf;

    const AUTHOR: &str = "dev@example.org";

    /// A throwaway repository with five files, committed.
    struct Repo(PathBuf);

    impl Repo {
        fn new(name: &str) -> Repo {
            let dir = std::env::temp_dir().join(format!("patchtrack-git-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(".gitconfig-empty"), "").unwrap();
            let r = Repo(dir);
            r.git(&["init", "-q", "-b", "main"]);
            std::fs::write(r.0.join(".git/info/exclude"), ".gitconfig-empty\n").unwrap();
            for f in ["a.c", "b.c", "c.c", "d.c", "e.c"] {
                r.write(f, "one\ntwo\nthree\n");
            }
            r.commit("base");
            r
        }

        fn path(&self) -> &str {
            self.0.to_str().unwrap()
        }

        /// git as a person here would run it: an author, and no global config.
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Dev")
                .env("GIT_AUTHOR_EMAIL", AUTHOR)
                .env("GIT_COMMITTER_NAME", "Maint")
                .env("GIT_COMMITTER_EMAIL", "maint@example.org")
                .env("GIT_CONFIG_GLOBAL", self.0.join(".gitconfig-empty"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap()
        }

        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.0.join(name), text).unwrap();
        }

        fn commit(&self, message: &str) -> String {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
            self.git(&["rev-parse", "HEAD"]).trim().to_string()
        }

        /// The working tree's diff, posted as a patch; the tree is reset.
        fn post(&self, msgid: &str, title: &str, days: i64) -> Posted {
            let diff = self.git(&["diff"]);
            self.git(&["checkout", "--", "."]);
            posting(msgid, title, days, &diff)
        }

        fn tree(&self) -> Tree {
            Tree {
                name: "t".into(),
                repo: self.path().into(),
                git_ref: "main".into(),
                url: None,
                branch: None,
                role: Role::Next,
            }
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn posting(msgid: &str, title: &str, days: i64, diff: &str) -> Posted {
        let now = i64::try_from(crate::now_ms() / 1000).unwrap();
        Posted {
            msgid: msgid.into(),
            title: title.into(),
            author: AUTHOR.into(),
            date: patchtrack_core::mail::utc(now + days * 86_400),
            body: format!("{title}\n\nSigned-off-by: Dev\n---\n{diff}"),
        }
    }

    fn found(commit: &str, how: &'static str, reverted_by: Option<&str>) -> Option<Found> {
        Some(Found { commit: commit.into(), how, reverted_by: reverted_by.map(str::to_string) })
    }

    #[test]
    fn matchers() {
        let r = Repo::new("matchers");
        // Posted patches, as their diffs were when sent.
        r.write("a.c", "one\nTWO\nthree\n");
        let p_link = r.post("link-1@x", "a: shout two", -1);
        r.write("b.c", "one\ntwo\nTHREE\n");
        let p_pid = r.post("pid-1@x", "b: shout three", -1);
        r.write("c.c", "ONE\ntwo\nthree\n");
        let p_subj = r.post("subj-1@x", "c: shout one", -1);
        r.write("d.c", "one\ntwo\nthree\nfour\n");
        let p_cited = r.post("cited-1@x", "d: add four", -1);
        r.write("e.c", "one\ntwo\nthree\nfive\n");
        let p_trim = r.post("trim-1@x", "e: add five", -1);

        // What the maintainer committed.
        r.write("a.c", "one\nTWO\nthree\n");
        let c_link = r.commit("a: shout two, reworded\n\nLink: https://patch.msgid.link/link-1@x\n");
        r.write("b.c", "one\ntwo\nTHREE\n");
        let c_pid = r.commit("b: a different subject");
        // Contains trim-1's diff plus another file: a different patch. Only
        // --full-diff stops the path filter trimming it into a false match.
        r.write("e.c", "one\ntwo\nthree\nfive\n");
        r.write("README", "unrelated file in the same commit\n");
        r.commit("e: something bigger");
        r.write("c.c", "ONE\ntwo\nthree\nextra\n");
        let c_subj = r.commit("c: shout one");
        r.write("a.c", "zero\none\nTWO\nthree\n");
        r.commit("a: follow-up\n\nAs discussed in [1].\n\n[1] https://lore.kernel.org/all/cited-1@x/\n");

        let (sha, got) = check(&r.tree(), &[p_link, p_pid, p_subj, p_cited, p_trim]).unwrap();
        assert_eq!(sha, r.git(&["rev-parse", "main"]).trim());
        assert_eq!(got[0], found(&c_link, "message-id", None));
        assert_eq!(got[1], found(&c_pid, "patch-id", None));
        assert_eq!(got[2], found(&c_subj, "subject", None));
        assert_eq!(got[3], None); // cited in a body, not a trailer
        assert_eq!(got[4], None);
    }

    #[test]
    fn fetch_whole_branch() {
        let up = Repo::new("upstream");
        up.write("a.c", "one\nTWO\nthree\n");
        let diff = up.git(&["diff"]);
        let sha = up.commit("a: shout two\n\nLink: https://lore.kernel.org/r/f-1@x\n");
        let local = Repo::new("local");
        let tree = Tree {
            name: "up".into(),
            repo: local.path().into(),
            git_ref: "refs/patchtrack/up".into(),
            url: Some(up.path().into()),
            branch: Some("main".into()),
            role: Role::Mainline,
        };
        fetch(&tree).unwrap();
        assert!(!is_shallow(local.path()).unwrap());
        let got = check(&tree, &[posting("f-1@x", "a: shout two", -1, &diff)]).unwrap().1;
        assert_eq!(got[0], found(&sha, "message-id", None));
    }

    #[test]
    fn reverts_and_reverts_of_reverts() {
        let r = Repo::new("revert");
        r.write("a.c", "one\nTWO\nthree\n");
        let diff = r.git(&["diff"]);
        let sha = r.commit("a: shout two\n\nLink: https://lore.kernel.org/r/rv-1@x\n");
        let posted = [posting("rv-1@x", "a: shout two", -1, &diff)];
        r.git(&["revert", "--no-edit", &sha]);
        let first = r.git(&["rev-parse", "HEAD"]).trim().to_string();
        assert_eq!(check(&r.tree(), &posted).unwrap().1[0], found(&sha, "message-id", Some(&first)));
        r.git(&["revert", "--no-edit", &first]);
        assert_eq!(check(&r.tree(), &posted).unwrap().1[0], found(&sha, "message-id", None)); // back in
        r.git(&["revert", "--no-edit", "HEAD"]);
        let third = r.git(&["rev-parse", "HEAD"]).trim().to_string();
        assert_eq!(check(&r.tree(), &posted).unwrap().1[0], found(&sha, "message-id", Some(&third)));
    }

    #[test]
    fn nothing_after_posting() {
        let r = Repo::new("late");
        r.write("a.c", "one\nTWO\nthree\n");
        let diff = r.git(&["diff"]);
        r.commit("a: shout two");
        assert_eq!(check(&r.tree(), &[posting("late@x", "a: shout two", 30, &diff)]).unwrap().1[0], None);
    }

    #[test]
    fn a_missing_ref_is_an_error() {
        let r = Repo::new("noref");
        let tree = Tree { git_ref: "refs/patchtrack/none".into(), ..r.tree() };
        assert!(check(&tree, &[]).unwrap_err().to_string().contains("rev-parse"));
    }
}
