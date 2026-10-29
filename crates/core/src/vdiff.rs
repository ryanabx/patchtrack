//! Patches across versions of a series: which patch became which, and
//! what changed.
//!
//! `git range-diff` (and `b4 diff`) need both versions applied to a base
//! tree; this needs only the mail. A patch is followed by its title, then
//! by an identical diff (a retitled patch), then by a similar title. Each
//! pair is compared twice: its commit message, and its diff with hunk line
//! numbers and index lines removed, so a rebase alone is not a change.
//! Markers follow range-diff: = unchanged, ! changed, < dropped, > added.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::difflib::{SequenceMatcher, unified_diff};
use crate::series::{Family, Series};
use crate::subject::normalize_title;

/// Titles at least this similar (difflib's ratio) are the same patch reworked.
pub const SIMILAR: f64 = 0.6;

static DIFF_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(---\s*$|diff --git )").expect("valid"));
static HUNK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^@@ -\d+(?:,\d+)? \+\d+(?:,\d+)? @@").expect("valid"));

/// (message, normalised diff lines) of a posted patch. Hunk line numbers,
/// git's function context and `index` lines are dropped, so a rebase alone
/// is not a change.
pub fn split(body: &str) -> (Vec<&str>, Vec<&str>) {
    let lines: Vec<&str> = body.lines().collect();
    let cut = lines.iter().position(|l| DIFF_START.is_match(l)).unwrap_or(lines.len());
    let mut message = lines[..cut].to_vec();
    while message.last().is_some_and(|l| l.trim().is_empty()) {
        message.pop();
    }
    let start = lines.iter().position(|l| l.starts_with("diff --git ")).unwrap_or(lines.len());
    let diff = lines[start..]
        .iter()
        .take_while(|l| l.trim_end() != "--")
        .filter(|l| !l.starts_with("index "))
        .map(|l| if HUNK.is_match(l) { "@@" } else { l })
        .collect();
    (message, diff)
}

/// A patch as written: its commit message, and the rest from the "---" line
/// or the first diff on.
pub fn split_raw(body: &str) -> (String, String) {
    let lines: Vec<&str> = body.lines().collect();
    let cut = lines.iter().position(|l| DIFF_START.is_match(l)).unwrap_or(lines.len());
    (lines[..cut].join("\n").trim_end().to_string(), lines[cut..].join("\n"))
}

/// Python's difflib.SequenceMatcher(None, a, b).ratio(), on characters.
pub fn ratio(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    SequenceMatcher::new(&a, &b).ratio()
}

/// {old patch number: new patch number} for the patches two versions share.
pub fn links(old: &Series, new: &Series) -> BTreeMap<u32, u32> {
    let olds: BTreeMap<u32, &str> =
        old.patches.keys().filter(|&&i| i > 0).map(|&i| (i, old.titles.get(&i).map_or("", String::as_str))).collect();
    let news: BTreeMap<u32, &str> =
        new.patches.keys().filter(|&&i| i > 0).map(|&i| (i, new.titles.get(&i).map_or("", String::as_str))).collect();
    let mut out: BTreeMap<u32, u32> = BTreeMap::new();
    let taken = |out: &BTreeMap<u32, u32>, o: u32| out.contains_key(&o);
    let linked = |out: &BTreeMap<u32, u32>, n: u32| out.values().any(|&v| v == n);

    for (&n, title) in &news {
        let t = normalize_title(title);
        if let Some(&o) = olds.iter().find(|&(&o, ot)| !taken(&out, o) && normalize_title(ot) == t).map(|(o, _)| o) {
            out.insert(o, n);
        }
    }
    let body = |s: &Series, i: u32| s.bodies.get(&i).map_or("", String::as_str).to_string();
    let old_bodies: BTreeMap<u32, String> = olds.keys().map(|&o| (o, body(old, o))).collect();
    for &n in news.keys() {
        if linked(&out, n) {
            continue;
        }
        let new_body = body(new, n);
        let d = split(&new_body).1;
        if d.is_empty() {
            continue;
        }
        if let Some(&o) = olds.keys().find(|&&o| !taken(&out, o) && split(&old_bodies[&o]).1 == d) {
            out.insert(o, n);
        }
    }
    for (&n, title) in &news {
        if linked(&out, n) {
            continue;
        }
        let left: Vec<u32> = olds.keys().copied().filter(|&o| !taken(&out, o)).collect();
        if left.is_empty() {
            break;
        }
        // The first of the best, as Python's max() picks.
        let mut best = left[0];
        let mut best_ratio = ratio(olds[&best], title);
        for &o in &left[1..] {
            let r = ratio(olds[&o], title);
            if r > best_ratio {
                (best, best_ratio) = (o, r);
            }
        }
        if best_ratio >= SIMILAR {
            out.insert(best, n);
        }
    }
    out
}

/// One patch of a version and its title.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PatchRef {
    pub patch: u32,
    pub title: String,
}

/// A patch of the older version and what became of it in the newer one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Pair {
    pub old: Option<PatchRef>,
    pub new: Option<PatchRef>,
    /// "=" unchanged, "!" changed, "<" dropped, ">" added.
    pub marker: String,
    /// A unified diff of the commit messages; empty if they are the same.
    pub message: Vec<String>,
    /// A unified diff of the normalised diffs; empty if they are the same.
    pub diff: Vec<String>,
}

/// The patches of two versions, paired: in the newer version's order, then
/// what was dropped.
pub fn pairs(old: &Series, new: &Series) -> Vec<Pair> {
    let body = |s: &Series, i: u32| s.bodies.get(&i).map_or("", String::as_str).to_string();
    let title = |s: &Series, i: u32| s.titles.get(&i).cloned().unwrap_or_default();
    let back: BTreeMap<u32, u32> = links(old, new).into_iter().map(|(o, n)| (n, o)).collect();
    let mut olds: Vec<u32> = old.patches.keys().copied().filter(|&i| i > 0).collect();
    let mut out = vec![];
    for n in new.patches.keys().copied().filter(|&i| i > 0) {
        let new_ref = Some(PatchRef { patch: n, title: title(new, n) });
        let Some(&o) = back.get(&n) else {
            out.push(Pair { old: None, new: new_ref, marker: ">".into(), message: vec![], diff: vec![] });
            continue;
        };
        olds.retain(|&x| x != o);
        let (old_body, new_body) = (body(old, o), body(new, n));
        let ((om, od), (nm, nd)) = (split(&old_body), split(&new_body));
        let message = unified_diff(&om, &nm, "old message", "new message", 1);
        let diff = unified_diff(&od, &nd, "old diff", "new diff", 2);
        let marker = if message.is_empty() && diff.is_empty() { "=" } else { "!" };
        out.push(Pair {
            old: Some(PatchRef { patch: o, title: title(old, o) }),
            new: new_ref,
            marker: marker.into(),
            message,
            diff,
        });
    }
    for o in olds {
        out.push(Pair {
            old: Some(PatchRef { patch: o, title: title(old, o) }),
            new: None,
            marker: "<".into(),
            message: vec![],
            diff: vec![],
        });
    }
    out
}

static CHANGES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^Changes in v\d+").expect("valid"));

/// The "Changes in vN:" section of a cover letter, as the author wrote it.
pub fn changelog(cover: &str) -> Vec<String> {
    let lines: Vec<&str> = cover.lines().collect();
    let Some(start) = lines.iter().position(|l| CHANGES.is_match(l)) else { return vec![] };
    let mut out = vec![lines[start].to_string()];
    for l in &lines[start + 1..] {
        let after_links = l.trim().is_empty() && out.last().is_some_and(|p| p.starts_with("- Link to v"));
        if l.starts_with("base-commit:") || l.starts_with("change-id:") || l.starts_with("---") || after_links {
            break;
        }
        out.push(l.to_string());
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    out
}

/// What changed from one version to the next: the author's changelog
/// from the newer cover letter, and the patches paired.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Changes {
    pub from: u32,
    pub to: u32,
    pub changelog: Vec<String>,
    pub pairs: Vec<Pair>,
}

pub fn changes(old: &Series, new: &Series) -> Changes {
    Changes {
        from: old.version,
        to: new.version,
        changelog: changelog(new.bodies.get(&0).map_or("", String::as_str)),
        pairs: pairs(old, new),
    }
}

/// What became of one file from one version to another: GitHub's "files
/// changed" between two pushes, from the mail alone.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct FileChange {
    pub path: String,
    /// "=" unchanged, "!" changed, "<" no longer touched, ">" newly touched.
    pub marker: String,
    /// The patches of each version that touch it.
    pub old_patches: Vec<u32>,
    pub new_patches: Vec<u32>,
    /// A unified diff of the file's normalised hunks, every patch's in
    /// order; empty if they are the same.
    pub diff: Vec<String>,
}

/// Each file a version touches, in the order its patches first touch it,
/// with every patch's normalised lines for it (see [`split`]) in patch order.
fn by_file(s: &Series) -> Vec<(String, Vec<u32>, Vec<String>)> {
    let mut out: Vec<(String, Vec<u32>, Vec<String>)> = vec![];
    for (&i, body) in s.bodies.iter().filter(|&(&i, _)| i > 0) {
        let mut at: Option<usize> = None;
        for line in split(body).1 {
            if let Some(rest) = line.strip_prefix("diff --git ") {
                let path = rest.rsplit_once(" b/").map_or(rest, |(_, b)| b).to_string();
                let k = match out.iter().position(|(p, ..)| *p == path) {
                    Some(k) => k,
                    None => {
                        out.push((path, vec![], vec![]));
                        out.len() - 1
                    }
                };
                if !out[k].1.contains(&i) {
                    out[k].1.push(i);
                }
                at = Some(k);
            } else if let Some(k) = at {
                out[k].2.push(line.to_string());
            }
        }
    }
    out
}

/// Every file either version touches, as the newer orders them, then those
/// only the older touched.
pub fn files(old: &Series, new: &Series) -> Vec<FileChange> {
    let (olds, news) = (by_file(old), by_file(new));
    let mut out = vec![];
    for (path, new_patches, new_lines) in &news {
        let before = olds.iter().find(|(p, ..)| p == path);
        let (old_patches, old_lines) = before.map_or((vec![], vec![]), |(_, n, l)| (n.clone(), l.clone()));
        let (a, b): (Vec<&str>, Vec<&str>) =
            (old_lines.iter().map(String::as_str).collect(), new_lines.iter().map(String::as_str).collect());
        let diff = unified_diff(&a, &b, "old", "new", 2);
        let marker = if before.is_none() {
            ">"
        } else if diff.is_empty() {
            "="
        } else {
            "!"
        };
        let diff = if before.is_none() { vec![] } else { diff };
        out.push(FileChange {
            path: path.clone(),
            marker: marker.into(),
            old_patches,
            new_patches: new_patches.clone(),
            diff,
        });
    }
    for (path, old_patches, _) in olds.iter().filter(|(p, ..)| !news.iter().any(|(q, ..)| q == p)) {
        out.push(FileChange {
            path: path.clone(),
            marker: "<".into(),
            old_patches: old_patches.clone(),
            new_patches: vec![],
            diff: vec![],
        });
    }
    out
}

/// A family's versions by number, oldest first; a RESEND replaces the
/// posting of its version.
pub fn versions(family: &Family) -> BTreeMap<u32, &Series> {
    let mut sorted: Vec<&Series> = family.versions.iter().collect();
    sorted.sort_by(|a, b| (a.version, &a.date).cmp(&(b.version, &b.date)));
    sorted.into_iter().map(|s| (s.version, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "{msg}

Signed-off-by: D <d@x>
---
 a.c | 2 +-
 1 file changed

diff --git a/a.c b/a.c
index {idx}..1111111 100644
--- a/a.c
+++ b/a.c
@@ -{line},3 +{line},3 @@ {ctx}
 keep
-{old}
+{new}
--
2.51.0
";

    struct P<'a> {
        msg: &'a str,
        idx: &'a str,
        line: u32,
        ctx: &'a str,
        old: &'a str,
        new: &'a str,
    }

    impl Default for P<'_> {
        fn default() -> Self {
            P { msg: "Fix a.", idx: "abc1234", line: 10, ctx: "int main(void)", old: "x", new: "y" }
        }
    }

    impl P<'_> {
        fn body(&self) -> String {
            PATCH
                .replace("{msg}", self.msg)
                .replace("{idx}", self.idx)
                .replace("{line}", &self.line.to_string())
                .replace("{ctx}", self.ctx)
                .replace("{old}", self.old)
                .replace("{new}", self.new)
        }
    }

    fn series(patches: &[(&str, P)]) -> Series {
        let mut s = empty();
        for (i, (title, p)) in patches.iter().enumerate() {
            let i = i as u32 + 1;
            s.patches.insert(i, format!("{i}@x"));
            s.titles.insert(i, title.to_string());
            s.bodies.insert(i, p.body());
        }
        s.total = patches.len() as u32;
        s
    }

    /// A series with no patches, to fill in.
    fn empty() -> Series {
        let raw = b"Message-ID: <e@x>\nSubject: [PATCH] x\nFrom: D <d@x>\n\n";
        let msgs = [crate::Message::parse(raw).unwrap()];
        let mut s = crate::series::group(&msgs).remove(0).head().clone();
        (s.patches, s.titles, s.bodies) = Default::default();
        s
    }

    #[test]
    fn rebase_alone_is_unchanged() {
        let old = series(&[("a: fix", P::default())]);
        let new = series(&[("a: fix", P { idx: "fff9999", line: 42, ctx: "static int other(void)", ..P::default() })]);
        let p = pairs(&old, &new);
        assert_eq!((p.len(), p[0].marker.as_str()), (1, "="));
    }

    #[test]
    fn changed_message_and_diff() {
        let old = series(&[("a: fix", P::default())]);
        let new = series(&[("a: fix", P { msg: "Fix a properly.", new: "z", ..P::default() })]);
        let p = &pairs(&old, &new)[0];
        assert_eq!(p.marker, "!");
        assert!(p.message.contains(&"+Fix a properly.".to_string()), "{:?}", p.message);
        assert!(p.diff.contains(&"++z".to_string()), "{:?}", p.diff);
    }

    #[test]
    fn added_dropped_and_retitled() {
        let old = series(&[("a: allow longer tables", P::default()), ("b: drop me", P { old: "q", ..P::default() })]);
        let new =
            series(&[("a: allow 101 table points", P::default()), ("c: brand new", P { old: "r", ..P::default() })]);
        let got: Vec<(Option<String>, Option<String>, String)> =
            pairs(&old, &new).into_iter().map(|p| (p.old.map(|r| r.title), p.new.map(|r| r.title), p.marker)).collect();
        let t = |s: &str| Some(s.to_string());
        assert_eq!(
            got,
            vec![
                (t("a: allow longer tables"), t("a: allow 101 table points"), "=".into()),
                (None, t("c: brand new"), ">".into()),
                (t("b: drop me"), None, "<".into()),
            ]
        );
    }

    #[test]
    fn files_across_versions() {
        let two = |body: &str, path: &str| body.replace("a/a.c b/a.c", &format!("a/{path} b/{path}"));
        let old = series(&[
            ("a: one", P::default()),
            ("b: two", P { old: "p", ..P::default() }),
            ("c: three", P { old: "gone", ..P::default() }),
        ]);
        let mut old = old;
        old.bodies.insert(3, two(&old.bodies[&3], "c.c"));
        // v2: a.c's first hunk changed, its second rebased only; c.c no longer
        // touched; d.c new.
        let mut new = series(&[
            ("a: one", P { new: "z", ..P::default() }),
            ("b: two", P { old: "p", line: 99, ..P::default() }),
            ("d: four", P::default()),
        ]);
        new.bodies.insert(3, two(&new.bodies[&3], "d.c"));
        let got = files(&old, &new);
        let summary: Vec<(&str, &str, &[u32], &[u32])> = got
            .iter()
            .map(|f| (f.path.as_str(), f.marker.as_str(), f.old_patches.as_slice(), f.new_patches.as_slice()))
            .collect();
        assert_eq!(summary, [("a.c", "!", &[1, 2][..], &[1, 2][..]), ("d.c", ">", &[], &[3]), ("c.c", "<", &[3], &[])]);
        assert!(
            got[0].diff.contains(&"-+y".to_string()) && got[0].diff.contains(&"++z".to_string()),
            "{:?}",
            got[0].diff
        );
        // A rebase alone changes no file.
        let rebased = series(&[
            ("a: one", P { idx: "9999999", line: 7, ..P::default() }),
            ("b: two", P { old: "p", ..P::default() }),
        ]);
        let same = series(&[("a: one", P::default()), ("b: two", P { old: "p", ..P::default() })]);
        assert!(files(&same, &rebased).iter().all(|f| f.marker == "=" && f.diff.is_empty()));
    }

    #[test]
    fn changelogs() {
        let cover =
            "Intro.\n\nChanges in v5:\n- Rework the probe.\n- Link to v4: https://x\n\n---\nD (2):\nbase-commit: abc\n";
        assert_eq!(changelog(cover), ["Changes in v5:", "- Rework the probe.", "- Link to v4: https://x"]);
        assert!(changelog("No changelog here.").is_empty());
    }

    #[test]
    fn ratio_matches_difflib() {
        // Values from Python's difflib.SequenceMatcher(None, a, b).ratio().
        let cases = [
            ("abcd", "bcde", 0.75),
            ("foo: fix the thing", "foo: fix that thing", 0.918_918_918_918_919),
            ("b: old title", "b: quite another title", 0.588_235_294_117_647),
            ("", "", 1.0),
            ("abc", "", 0.0),
            ("über naïve ünïcödé", "uber naive unicode", 0.666_666_666_666_666_6),
        ];
        // Long and repetitive: autojunk drops every character from the matching.
        let (a, b) = ("a".repeat(150) + &"b".repeat(100), "b".repeat(120) + &"a".repeat(130));
        assert_eq!(ratio(&a, &b), 0.0);
        for (a, b, want) in cases {
            assert!((ratio(a, b) - want).abs() < 1e-12, "{a:?} {b:?}: {} != {want}", ratio(a, b));
        }
    }

    #[test]
    fn rebase_alone_is_no_change() {
        let a = "Fix.\n---\ndiff --git a/a.c b/a.c\nindex abc..def 100644\n--- a/a.c\n+++ b/a.c\n@@ -10,3 +10,3 @@ int f\n-x\n+y\n--\n2.51\n";
        let b = "Fix.\n---\ndiff --git a/a.c b/a.c\nindex 123..456 100644\n--- a/a.c\n+++ b/a.c\n@@ -42,3 +42,3 @@ int g\n-x\n+y\n";
        assert_eq!(split(a), split(b));
        assert_eq!(split(a).0, vec!["Fix."]);
    }
}
