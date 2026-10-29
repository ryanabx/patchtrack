//! States and their evidence, through the public API, as a host uses it.
//!
//! Ported from the Python version's tests (tests/test_patchtrack.py,
//! test_trees.py, test_config.py on `main`), with the same expectations.

use std::collections::{BTreeMap, BTreeSet};

use patchtrack_core::Message;
use patchtrack_core::rules::Rules;
use patchtrack_core::series::{self, Facts, Role, Series, State, TreeRow};
use patchtrack_core::settings::Settings;

const ME: &str = "dev@example.org";
const D: &str = "Mon, 05 Oct 2026 10:00:00 +0000";

struct Mail<'a> {
    id: &'a str,
    subject: &'a str,
    from: &'a str,
    irt: Option<&'a str>,
    refs: Option<&'a [&'a str]>,
    body: &'a str,
    date: &'a str,
    to: Option<&'a str>,
    cc: Option<&'a str>,
}

impl Default for Mail<'_> {
    fn default() -> Self {
        Mail { id: "", subject: "", from: ME, irt: None, refs: None, body: "", date: D, to: None, cc: None }
    }
}

impl Mail<'_> {
    fn parse(&self) -> Message {
        let mut h =
            format!("Message-ID: <{}>\nSubject: {}\nFrom: {}\nDate: {}\n", self.id, self.subject, self.from, self.date);
        if let Some(irt) = self.irt {
            let refs = self.refs.map(|r| r.to_vec()).unwrap_or_else(|| vec![irt]);
            h += &format!(
                "In-Reply-To: <{irt}>\nReferences: {}\n",
                refs.iter().map(|r| format!("<{r}>")).collect::<Vec<_>>().join(" ")
            );
        }
        if let Some(to) = self.to {
            h += &format!("To: {to}\n");
        }
        if let Some(cc) = self.cc {
            h += &format!("Cc: {cc}\n");
        }
        Message::parse(format!("{h}\n{}", self.body).as_bytes()).expect("a message")
    }
}

fn reply(id: &str, subject: &str, from: &str, irt: &str, body: &str, date: &str) -> Message {
    Mail { id, subject, from, irt: Some(irt), body, date, ..Mail::default() }.parse()
}

/// A b4-style posting: a cover and n patches; the family is `tag`.
fn posting(tag: &str, v: u32, n: u32, date: &str) -> (String, Vec<Message>) {
    let pre = if v > 1 { format!(" v{v}") } else { String::new() };
    let id = |i: u32| format!("20261005-{tag}-v{v}-{i}-{}{v}@example.org", "a".repeat(11));
    let root = id(0);
    let mut out = vec![
        Mail { id: &root, subject: &format!("[PATCH{pre} 0/{n}] foo: do the thing"), date, ..Mail::default() }.parse(),
    ];
    for i in 1..=n {
        out.push(
            Mail {
                id: &id(i),
                subject: &format!("[PATCH{pre} {i}/{n}] foo: part {i}"),
                irt: Some(&root),
                date,
                ..Mail::default()
            }
            .parse(),
        );
    }
    (root, out)
}

fn build(msgs: &[Message]) -> BTreeMap<String, Series> {
    series::build(msgs, &Facts::default(), &Rules::default())
}

fn fetched(root: &str) -> Facts {
    Facts { fetched: [(root.to_string(), "2026-10-26T00:00:00Z".to_string())].into(), ..Facts::default() }
}

fn row(tree: &str, role: Role, commit: Option<&str>) -> TreeRow {
    TreeRow {
        tree: tree.into(),
        role,
        ref_sha: "f".repeat(40),
        commit_sha: commit.map(str::to_string),
        how: Some("patch-id".into()),
        reverted_by: None,
        checked_at: "2026-10-26T00:00:00Z".into(),
    }
}

// ---- series ----

#[test]
fn states_and_versions() {
    let (r1, v1) = posting("foo", 1, 3, D);
    let (r2, v2) = posting("foo", 2, 3, "Mon, 12 Oct 2026 10:00:00 +0000");
    let p1 = v2[1].msgid.clone();
    let mut msgs = [v1, v2].concat();
    msgs.push(
        Mail {
            id: "r1@x",
            subject: "Re: [PATCH v2 1/3] foo: part 1",
            from: "Rev <rev@x>",
            irt: Some(&p1),
            refs: Some(&[&r2, &p1]),
            body: "> code\nReviewed-by: Rev <rev@x>\n",
            date: "Tue, 13 Oct 2026 10:00:00 +0000",
            ..Mail::default()
        }
        .parse(),
    );
    msgs.push(reply(
        "r2@x",
        "Re: (subset) [PATCH v2 0/3] foo: do the thing",
        "Max <max@x>",
        &r2,
        "Applied, thanks!\n\n[3/3] foo: part 3\n      commit: deadbeef\n",
        "Wed, 14 Oct 2026 10:00:00 +0000",
    ));
    let found = build(&msgs);
    assert_eq!(found.keys().cloned().collect::<BTreeSet<_>>(), BTreeSet::from([r1.clone(), r2.clone()]));
    assert_eq!(found[&r1].state, State::Superseded);
    let s = &found[&r2];
    assert_eq!((s.state, s.applied.clone(), s.family.as_str()), (State::PartiallyApplied, BTreeSet::from([3]), "foo"));
    assert!(s.unanswered);
    assert_eq!(s.tags().map(|(i, k, _, _)| (i, k.to_string())).collect::<Vec<_>>(), vec![(1, "Reviewed-By".into())]);
}

#[test]
fn single_patch_and_author_reply() {
    let root = "solo@example.org";
    let found = build(&[
        Mail { id: root, subject: "[PATCH] bar: fix it", ..Mail::default() }.parse(),
        reply("q@x", "Re: [PATCH] bar: fix it", "Maint <m@x>", root, "Why?\n", "Tue, 06 Oct 2026 10:00:00 +0000"),
        Mail {
            id: "a@x",
            subject: "Re: [PATCH] bar: fix it",
            irt: Some("q@x"),
            refs: Some(&[root, "q@x"]),
            body: "Because.\n",
            date: "Wed, 07 Oct 2026 10:00:00 +0000",
            ..Mail::default()
        }
        .parse(),
        reply("b@x", "Re: [PATCH] bar: fix it", "ci-bot <ci-bot@x>", root, "ok\n", "Wed, 07 Oct 2026 11:00:00 +0000"),
    ]);
    let s = &found[root];
    assert_eq!((s.state, s.total, s.patches.keys().copied().collect::<Vec<_>>()), (State::Discussed, 1, vec![1]));
    assert!(!s.unanswered);
    assert_eq!(s.replies.iter().map(|r| r.bot).collect::<Vec<_>>(), vec![false, false, true]);
}

#[test]
fn recipients_include_the_authors_replies() {
    let root = "solo@example.org";
    let found = build(&[
        Mail { id: root, subject: "[PATCH] bar: fix it", to: Some("Maint <maint@x>"), ..Mail::default() }.parse(),
        Mail {
            id: "a@x",
            subject: "Re: [PATCH] bar: fix it",
            irt: Some(root),
            body: "Adding Max.\n",
            date: "Wed, 07 Oct 2026 10:00:00 +0000",
            cc: Some("Max <max@x>, list@x"),
            ..Mail::default()
        }
        .parse(),
    ]);
    assert_eq!(found[root].to, BTreeSet::from(["maint@x".into(), "max@x".into(), "list@x".into()]));
}

#[test]
fn applied_reply_to_single_patch() {
    let root = "solo@example.org";
    let found = build(&[
        Mail { id: root, subject: "[PATCH] bar: fix it", ..Mail::default() }.parse(),
        reply("y@x", "Re: [PATCH] bar: fix it", "Maint <m@x>", root, "Applied.\n", "Tue, 06 Oct 2026 10:00:00 +0000"),
    ]);
    assert_eq!(found[root].state, State::Applied);
}

#[test]
fn partial_announcement_on_the_cover() {
    let (root, mut msgs) = posting("pa", 1, 3, D);
    msgs.push(reply(
        "y@x",
        "Re: [PATCH 0/3] foo: do the thing",
        "M <m@x>",
        &root,
        "Applied 1/3 to for-next, the rest will follow later.\n",
        "Tue, 06 Oct 2026 10:00:00 +0000",
    ));
    let s = &build(&msgs)[&root];
    assert_eq!((s.state, s.applied.is_empty()), (State::PartiallyApplied, true));
    assert!(s.evidence.contains("which patches is not said"), "{}", s.evidence);
    // An announcement asks nothing of the author, even when the series is not done.
    assert!(!s.unanswered);
}

#[test]
fn cover_applied_without_list_means_all() {
    let (root, mut msgs) = posting("baz", 1, 2, D);
    msgs.push(reply(
        "y@x",
        "Re: [PATCH 0/2] foo: do the thing",
        "M <m@x>",
        &root,
        "Applied to for-next, thanks.\n",
        "Tue, 06 Oct 2026 10:00:00 +0000",
    ));
    assert_eq!(build(&msgs)[&root].applied, BTreeSet::from([1, 2]));
}

#[test]
fn no_reply_needs_a_fetch() {
    let (root, msgs) = posting("q", 1, 2, D);
    assert_eq!(build(&msgs)[&root].state, State::Unfetched);
    let s = &series::build(&msgs, &fetched(&root), &Rules::default())[&root];
    assert_eq!((s.state, s.unanswered), (State::NoReply, false));
    assert_eq!(s.as_of_text(), "as of 2026-10-26 00:00 UTC");
}

#[test]
fn someone_elses_patch_in_the_thread_is_a_reply_and_a_series() {
    let (root, mut msgs) = posting("alt", 1, 1, D);
    let p1 = msgs[1].msgid.clone();
    msgs.push(reply(
        "o@x",
        "[PATCH] foo: another way",
        "Other <o@x>",
        &p1,
        "How about this?\n",
        "Tue, 06 Oct 2026 10:00:00 +0000",
    ));
    let found = build(&msgs);
    assert_eq!(found[&root].state, State::Discussed);
    assert!(found[&root].unanswered);
    assert_eq!(found["o@x"].sender, "o@x");
}

// ---- comments ----

fn thread(replies: &[(&str, Option<&str>, &str)]) -> Series {
    let root = "solo@example.org";
    let mut msgs = vec![Mail { id: root, subject: "[PATCH] bar: fix it", ..Mail::default() }.parse()];
    for (n, (from, irt, body)) in replies.iter().enumerate() {
        let (id, date) = (format!("r{n}@x"), format!("Tue, 06 Oct 2026 1{n}:00:00 +0000"));
        let refs: Vec<&str> = match irt {
            Some(p) => vec![root, p],
            None => vec![root],
        };
        msgs.push(
            Mail {
                id: &id,
                subject: "Re: [PATCH] bar: fix it",
                from,
                irt: Some(irt.unwrap_or(root)),
                refs: Some(&refs),
                body,
                date: &date,
                ..Mail::default()
            }
            .parse(),
        );
    }
    build(&msgs).remove(root).expect("the series")
}

#[test]
fn direct_comment() {
    let s = thread(&[("R <r@x>", None, "> +\tx = 1;\nWhy is this not checked?\n")]);
    assert_eq!(s.state, State::Commented);
    assert!(s.evidence.contains("\"Why is this not checked?\""));
}

#[test]
fn comment_on_a_reply_is_conversation() {
    let me = format!("Dev <{ME}>");
    let s = thread(&[
        (&me, None, "Ping, any thoughts on this?\n"),
        ("R <r@x>", Some("r0@x"), "> Ping, any thoughts on this?\nIt is in my queue now.\n"),
    ]);
    assert_eq!(s.state, State::Discussed);
}

#[test]
fn conversation_after_a_review_does_not_settle_it() {
    // Only direct replies judge a patch: a later reply to the review is talk about it.
    let s = thread(&[
        ("R <r@x>", None, "> +\tx = 1;\nWhy is this not checked?\n"),
        ("Other <o@x>", Some("r0@x"), "> Why is this not checked?\nI wondered that too, it seems odd.\n"),
    ]);
    assert_eq!(s.state, State::Commented);
}

#[test]
fn later_tag_settles_it() {
    let s = thread(&[
        ("R <r@x>", None, "> +\tx = 1;\nWhy is this not checked?\n"),
        ("R <r@x>", None, "> +\tx = 1;\nFine after all.\n\nReviewed-by: R <r@x>\n"),
    ]);
    assert_eq!(s.state, State::Reviewed);
}

// ---- tree stages ----

fn staged(rows: &[(u32, Vec<TreeRow>)], applied_mail: bool) -> Series {
    let mut msgs = vec![
        Mail { id: "c@x", subject: "[PATCH 0/2] t: thing", date: "Thu, 01 Oct 2026 00:00:00 +0000", ..Mail::default() }
            .parse(),
    ];
    for i in 1..=2 {
        msgs.push(
            Mail {
                id: &format!("p{i}@x"),
                subject: &format!("[PATCH {i}/2] t: part {i}"),
                irt: Some("c@x"),
                date: "Thu, 01 Oct 2026 00:00:00 +0000",
                ..Mail::default()
            }
            .parse(),
        );
    }
    if applied_mail {
        msgs.push(reply(
            "y@x",
            "Re: [PATCH 0/2] t: thing",
            "M <m@x>",
            "c@x",
            "Applied, thanks.\n",
            "Fri, 02 Oct 2026 00:00:00 +0000",
        ));
    }
    let facts = Facts {
        fetched: [("c@x".to_string(), "2026-10-03T00:00:00Z".to_string())].into(),
        trees: rows.iter().map(|(i, r)| (format!("p{i}@x"), r.clone())).collect(),
        ..Facts::default()
    };
    series::build(&msgs, &facts, &Rules::default()).remove("c@x").expect("the series")
}

#[test]
fn lowest_stage_wins() {
    let s = staged(
        &[
            (1, vec![row("mainline", Role::Mainline, Some(&"a".repeat(40)))]),
            (2, vec![row("next", Role::Next, Some(&"b".repeat(40))), row("mainline", Role::Mainline, None)]),
        ],
        false,
    );
    assert_eq!((s.state, s.stages.clone()), (State::InNext, BTreeMap::from([(1, 4), (2, 3)])));
    assert!(s.evidence.contains("2/2 is bbbbbbbbbbbb in next"), "{}", s.evidence);
}

#[test]
fn partially_queued() {
    let s = staged(
        &[
            (1, vec![row("mfd", Role::Maintainer, Some(&"a".repeat(40)))]),
            (2, vec![row("mfd", Role::Maintainer, None)]),
        ],
        false,
    );
    assert_eq!((s.state, s.evidence.as_str()), (State::PartiallyApplied, "1/2 queued, 2/2 not applied"));
}

#[test]
fn reverted_does_not_count() {
    let mut reverted = row("next", Role::Next, Some(&"a".repeat(40)));
    reverted.reverted_by = Some("b".repeat(40));
    let s = staged(&[(1, vec![reverted]), (2, vec![row("next", Role::Next, Some(&"c".repeat(40)))])], false);
    assert_eq!(s.state, State::PartiallyApplied);
    assert!(s.evidence.contains("1/2 was aaaaaaaaaaaa in next, reverted by bbbbbbbbbbbb"), "{}", s.evidence);
}

#[test]
fn silent_apply_is_noted() {
    let rows = [
        (1, vec![row("next", Role::Next, Some(&"a".repeat(40)))]),
        (2, vec![row("next", Role::Next, Some(&"c".repeat(40)))]),
    ];
    let s = staged(&rows, false);
    assert_eq!(s.state, State::InNext);
    assert!(s.evidence.contains("no \"applied\" mail"));
    assert!(!staged(&rows, true).evidence.contains("no \"applied\" mail"));
}

#[test]
fn mail_says_applied_tree_does_not() {
    let s =
        staged(&[(1, vec![row("foo", Role::Maintainer, None)]), (2, vec![row("foo", Role::Maintainer, None)])], true);
    assert_eq!(s.state, State::Applied);
    assert!(s.evidence.contains("not in foo as of 2026-10-26 00:00"), "{}", s.evidence);
}

// ---- families ----

fn family(msgs: &[Message], trees: BTreeMap<String, Vec<TreeRow>>) -> series::Family {
    let facts = Facts { trees: trees.into_iter().collect(), ..Facts::default() };
    let mut fams = series::families(series::build(msgs, &facts, &Rules::default()), &facts);
    assert_eq!(fams.len(), 1);
    fams.remove(0)
}

fn applied_on(root: &str, subject: &str) -> Message {
    reply("ap@x", subject, "M <m@x>", root, "Applied, thanks.\n", "Tue, 13 Oct 2026 10:00:00 +0000")
}

#[test]
fn dropped_because_applied() {
    let (_, v1) = posting("fam", 1, 3, D);
    let (r2, v2) = posting("fam", 2, 2, "Mon, 12 Oct 2026 10:00:00 +0000");
    let p3 = v1[3].msgid.clone();
    let mut msgs = [v1, v2].concat();
    msgs.push(applied_on(&r2, "Re: [PATCH v2 0/2] foo: do the thing"));
    let fam = family(&msgs, [(p3, vec![row("next", Role::Next, Some(&"c".repeat(40)))])].into());
    assert_eq!(fam.state, State::Applied);
    assert_eq!(fam.evidence, "v2 1/2 applied, v2 2/2 applied, v1 3/3 in-next");
}

#[test]
fn abandoned_patch_does_not_count() {
    let (_, v1) = posting("ab", 1, 3, D);
    let (r2, v2) = posting("ab", 2, 2, "Mon, 12 Oct 2026 10:00:00 +0000");
    let mut msgs = [v1, v2].concat();
    msgs.push(applied_on(&r2, "Re: [PATCH v2 0/2] foo: do the thing"));
    let fam = family(&msgs, BTreeMap::new());
    assert_eq!((fam.state, fam.parts.len(), fam.head().root.as_str()), (State::Applied, 2, r2.as_str()));
}

#[test]
fn older_version_with_the_same_commits_is_superseded() {
    let (_, v1) = posting("dup", 1, 1, D);
    let (_, v2) = posting("dup", 2, 1, "Mon, 12 Oct 2026 10:00:00 +0000");
    let hit = vec![row("mainline", Role::Mainline, Some(&"c".repeat(40)))];
    let trees = [(v1[1].msgid.clone(), hit.clone()), (v2[1].msgid.clone(), hit)].into();
    let fam = family(&[v1, v2].concat(), trees);
    assert_eq!(fam.versions.iter().map(|s| s.state).collect::<Vec<_>>(), vec![State::Superseded, State::Merged]);
    assert_eq!(fam.state, State::Merged);
}

/// Per version, a list of (title, diff text); tree: ((version, patch), role).
fn versions(specs: &[&[(&str, &str)]], tree: &[((u32, u32), Role)]) -> series::Family {
    let mut msgs = vec![];
    for (v, patches) in (1..).zip(specs) {
        let pre = if v > 1 { format!("v{v} ") } else { String::new() };
        let root = format!("fam-v{v}-0@x");
        let date = format!("Mon, {:02} Oct 2026 10:00:00 +0000", 5 + 7 * v);
        let n = patches.len();
        msgs.push(
            Mail {
                id: &root,
                subject: &format!("[PATCH {pre}0/{n}] foo: series"),
                date: &date,
                body: "---\nchange-id: 20261001-famtest-abcdef123456\n",
                ..Mail::default()
            }
            .parse(),
        );
        for (i, (title, diff)) in (1..).zip(patches.iter()) {
            let body = format!("Text.\n---\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+{diff}\n");
            msgs.push(
                Mail {
                    id: &format!("fam-v{v}-{i}@x"),
                    subject: &format!("[PATCH {pre}{i}/{n}] {title}"),
                    irt: Some(&root),
                    date: &date,
                    body: &body,
                    ..Mail::default()
                }
                .parse(),
            );
        }
    }
    let trees = tree
        .iter()
        .map(|&((v, i), role)| {
            let mut r = row("t", role, Some(&format!("{v}{i}").repeat(20)));
            r.checked_at = "2026-12-01T00:00:00Z".into();
            (format!("fam-v{v}-{i}@x"), vec![r])
        })
        .collect();
    family(&msgs, trees)
}

#[test]
fn retitled_patch_keeps_its_history() {
    let fam = versions(
        &[&[("a: one", "a1"), ("b: old title", "b1")], &[("a: one", "a1"), ("b: quite another title", "b1")]],
        &[((1, 2), Role::Next), ((2, 1), Role::Next)],
    );
    // v2 2/2 is v1 2/2 under a new title (same diff): it is in next, not a dropped patch.
    assert_eq!((fam.state, fam.parts.len()), (State::InNext, 2));
}

#[test]
fn patch_dropped_after_several_versions_counts_once() {
    let fam = versions(
        &[
            &[("a: one", "a1"), ("m: cell", "m1")],
            &[("a: one", "a2"), ("m: cell", "m1")],
            &[("a: one", "a3"), ("m: cell", "m1")],
            &[("a: one", "a4")],
        ],
        &[((1, 2), Role::Next), ((2, 2), Role::Next), ((3, 2), Role::Next)],
    );
    assert_eq!(fam.parts.iter().map(|p| (p.version, p.patch)).collect::<Vec<_>>(), vec![(4, 1), (3, 2)]);
}

#[test]
fn older_version_applied_head_not_yet() {
    let (r1, v1) = posting("old", 1, 1, D);
    let (_, v2) = posting("old", 2, 1, "Mon, 12 Oct 2026 10:00:00 +0000");
    let mut msgs = [v1, v2].concat();
    msgs.push(applied_on(&r1, "Re: [PATCH 0/1] foo: do the thing"));
    let fam = family(&msgs, BTreeMap::new());
    assert_eq!((fam.state, fam.evidence.as_str()), (State::Applied, "v1 1/1 applied"));
}

#[test]
fn closed_family() {
    let (root, msgs) = posting("cl", 1, 1, D);
    let facts =
        Facts { closed: [(format!("{ME}\0cl"), "folded into another series".to_string())].into(), ..fetched(&root) };
    let fams = series::families(series::build(&msgs, &facts, &Rules::default()), &facts);
    assert_eq!(
        (fams[0].state, fams[0].evidence.as_str()),
        (State::Closed, "closed by you: folded into another series")
    );
}

// ---- profiles and bots ----

fn series_to(to: &str, replies: &[(&str, &str)], rules: &Rules) -> Series {
    let (root, mut msgs) = posting("pf", 1, 1, D);
    msgs[0] = Mail { id: &root, subject: "[PATCH 0/1] foo: do the thing", to: Some(to), ..Mail::default() }.parse();
    for (n, (from, body)) in replies.iter().enumerate() {
        msgs.push(reply(
            &format!("r{n}@x"),
            "Re: [PATCH 0/1] foo: do the thing",
            from,
            &root,
            body,
            &format!("Tue, 06 Oct 2026 1{n}:00:00 +0000"),
        ));
    }
    series::build(&msgs, &fetched(&root), rules).remove(&root).expect("the series")
}

#[test]
fn gcc_series() {
    let me = format!("Dev <{ME}>");
    let rules = Rules::default();
    let s = series_to(
        "gcc-patches@gcc.gnu.org",
        &[("Rev <r@x>", "> +x\nOK.\n"), (&me, "Pushed as r17-1234, thanks.\n")],
        &rules,
    );
    assert_eq!((s.profile.as_deref(), s.state), (Some("sourceware"), State::Applied));
    let s = series_to("gcc-patches@gcc.gnu.org", &[("Rev <r@x>", "> +x\nOK.\n")], &rules);
    assert_eq!(s.state, State::Reviewed);
    assert!(s.evidence.contains("Approved"));
}

#[test]
fn kernel_series_have_no_profile() {
    let me = format!("Dev <{ME}>");
    let s =
        series_to("linux-usb@vger.kernel.org", &[("Rev <r@x>", "OK.\n"), (&me, "Pushed as abc.\n")], &Rules::default());
    assert_eq!((s.profile, s.state), (None, State::Discussed));
}

/// Rules as the settings in the store make them.
fn rules_from(key: &str, json: &str) -> Rules {
    Settings::load([(key, json)]).expect("valid settings").0.rules
}

#[test]
fn a_configured_profile() {
    let rules = rules_from(
        "profiles",
        r#"[{"name": "sourceware", "recipients": ["*@gcc.gnu.org"], "approvals": ["^ship it"]}]"#,
    );
    let to = "gcc-patches@gcc.gnu.org";
    assert_eq!(series_to(to, &[("Rev <r@x>", "Ship it!\n")], &rules).state, State::Reviewed);
    // It replaced the built-in profile of that name: a bare "OK." no longer approves.
    assert_eq!(series_to(to, &[("Rev <r@x>", "> +x\nOK.\n")], &rules).state, State::Discussed);
}

#[test]
fn state_follows_bot_rules() {
    let root = "solo@example.org";
    let msgs = [
        Mail { id: root, subject: "[PATCH] bar: fix it", ..Mail::default() }.parse(),
        reply(
            "r@x",
            "Re: [PATCH] bar: fix it",
            "CI <ci@x.org>",
            root,
            "Build OK.\n",
            "Tue, 06 Oct 2026 10:00:00 +0000",
        ),
    ];
    assert_eq!(build(&msgs)[root].state, State::Discussed);
    let rules = rules_from("bots", r#"{"addresses": ["CI@x.org"]}"#);
    assert_eq!(series::build(&msgs, &fetched(root), &rules)[root].state, State::BotOnly);
}
