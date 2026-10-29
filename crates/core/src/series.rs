//! Messages into series and families, and where each stands.
//!
//! A *series* is one posting: a cover letter (or a lone first patch) and
//! the patches threaded under it. A *family* is every version of one
//! series, the nearest thing email has to a pull request. Anyone's
//! postings are grouped; whose they are is a filter, not the model.
//!
//! Every state is derived from the mail and the store's facts on each
//! read, and carries its evidence: the message or commit that shows it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::evidence::{self, APPROVALS, Applied};
use crate::mail::{self, Message};
use crate::rules::{Profile, Rules};
use crate::subject::{self, Subject};
use crate::vdiff;

/// Patches further apart than this are not one posting.
const SAME_POSTING_SECS: i64 = 24 * 3600;

/// Where a series stands, most decided first: it shows the first that applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize), serde(rename_all = "kebab-case"))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum State {
    Merged,
    InNext,
    Queued,
    Applied,
    PartiallyApplied,
    Superseded,
    Commented,
    Reviewed,
    Discussed,
    BotOnly,
    #[default]
    NoReply,
    /// No copy of the thread was ever read: "no reply" would be a guess.
    Unfetched,
    /// Closed by you.
    Closed,
}

impl State {
    pub const ALL: [State; 13] = [
        State::Merged,
        State::InNext,
        State::Queued,
        State::Applied,
        State::PartiallyApplied,
        State::Superseded,
        State::Commented,
        State::Reviewed,
        State::Discussed,
        State::BotOnly,
        State::NoReply,
        State::Unfetched,
        State::Closed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            State::Merged => "merged",
            State::InNext => "in-next",
            State::Queued => "queued",
            State::Applied => "applied",
            State::PartiallyApplied => "partially-applied",
            State::Superseded => "superseded",
            State::Commented => "commented",
            State::Reviewed => "reviewed",
            State::Discussed => "discussed",
            State::BotOnly => "bot-only",
            State::NoReply => "no-reply",
            State::Unfetched => "unfetched",
            State::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Option<State> {
        State::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// In, or replaced: waiting on nobody.
    pub fn done(self) -> bool {
        matches!(self, State::Applied | State::Queued | State::InNext | State::Merged | State::Superseded)
    }

    fn from_stage(stage: u8) -> State {
        match stage {
            4 => State::Merged,
            3 => State::InNext,
            2 => State::Queued,
            _ => State::Applied,
        }
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How far one patch got: 0 not applied, 1 a mail says applied, then by tree.
pub fn stage_name(stage: u8) -> &'static str {
    ["not applied", "applied", "queued", "in-next", "merged"][usize::from(stage.min(4))]
}

/// What kind of tree a commit was found in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize), serde(rename_all = "lowercase"))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Role {
    /// A maintainer's tree: queued.
    Maintainer,
    /// linux-next and the like.
    Next,
    Mainline,
}

impl Role {
    pub fn stage(self) -> u8 {
        match self {
            Role::Maintainer => 2,
            Role::Next => 3,
            Role::Mainline => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Maintainer => "maintainer",
            Role::Next => "next",
            Role::Mainline => "mainline",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "maintainer" => Some(Role::Maintainer),
            "next" => Some(Role::Next),
            "mainline" => Some(Role::Mainline),
            _ => None,
        }
    }
}

/// The latest check of one posted patch against one tree; no commit is "not
/// found", which is as much a fact as finding it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TreeRow {
    pub tree: String,
    pub role: Role,
    pub ref_sha: String,
    pub commit_sha: Option<String>,
    /// How it was matched: message-id, patch-id, subject.
    pub how: Option<String>,
    /// A later commit that reverted it.
    pub reverted_by: Option<String>,
    pub checked_at: String,
}

/// What the store knows besides the mail.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// When each thread (by root) was last fetched whole.
    pub fetched: HashMap<String, String>,
    /// Threads a sync wanted fresher and has not got yet.
    pub pending: HashSet<String>,
    /// When local mail was last read for each thread.
    pub local: HashMap<String, String>,
    /// The lists a sync found each root on.
    pub lists: HashMap<String, BTreeSet<String>>,
    /// The archives a sync found each root on ("" for the archive setting's).
    pub archives: HashMap<String, BTreeSet<String>>,
    /// The archive each thread was last wanted or fetched from ("" likewise).
    pub thread_archive: HashMap<String, String>,
    /// The list, query and thread subscriptions whose syncs found each root.
    pub found: HashMap<String, BTreeSet<crate::settings::Subscription>>,
    /// Tree check results, by patch message-id.
    pub trees: HashMap<String, Vec<TreeRow>>,
    /// Families you closed, by [`Family::id`], with your reason.
    pub closed: HashMap<String, String>,
}

impl Facts {
    /// Whether this subscription's sync found the thread rooted here.
    pub fn found_by(&self, root: &str, sub: &crate::settings::Subscription) -> bool {
        self.found.get(root).is_some_and(|f| f.contains(sub))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Reply {
    pub msgid: String,
    /// "Name <addr>"; the bare address for the author's own.
    pub who: String,
    pub date: String,
    /// The patch replied to, 0 for the cover.
    pub index: u32,
    pub bot: bool,
    pub trailers: Vec<(String, String)>,
    pub applied: Option<Applied>,
    /// Sent by the series' sender.
    pub mine: bool,
    /// A reply to the posting itself, not to another reply.
    pub direct: bool,
    /// A profile's "Pushed as r17-1234", which counts even from the author.
    pub landed: bool,
    /// Inline prose under quoted text.
    pub comments: Vec<String>,
}

impl Reply {
    /// A direct reply from a person that comments inline and gives no tag.
    pub fn is_review_comment(&self) -> bool {
        self.direct
            && !self.mine
            && !self.bot
            && self.applied.is_none()
            && !self.comments.is_empty()
            && !self.trailers.iter().any(|(k, _)| APPROVALS.contains(&k.as_str()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Series {
    /// The cover letter's message-id, or the first patch's.
    pub root: String,
    /// The sender's address; a family is one sender's.
    pub sender: String,
    pub version: u32,
    pub total: u32,
    pub title: String,
    pub date: String,
    pub rfc: bool,
    pub resend: bool,
    /// b4's change-id slug, or "title:" and the normalised title.
    pub family: String,
    /// Patch number to message-id; 0 is the cover letter.
    pub patches: BTreeMap<u32, String>,
    pub titles: BTreeMap<u32, String>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub bodies: BTreeMap<u32, String>,
    /// Patch authors other than the sender, "Name <addr>".
    pub written_by: BTreeSet<String>,
    pub replies: Vec<Reply>,
    pub lists: BTreeSet<String>,
    /// To: and Cc: of the posting and of the author's replies.
    pub to: BTreeSet<String>,
    /// The profile whose recipients the posting matched.
    pub profile: Option<String>,
    /// When the whole thread was last fetched.
    pub as_of: Option<String>,
    /// A sync wanted a fresher copy and has not got it yet.
    pub pending: bool,
    /// When local mail was last read for this thread.
    pub local_as_of: Option<String>,
    pub superseded_by: Option<String>,
    pub state: State,
    pub evidence: String,
    /// Patch numbers a mail says were applied.
    pub applied: BTreeSet<u32>,
    /// Patch number to stage; see [`stage_name`].
    pub stages: BTreeMap<u32, u8>,
    pub tree_rows: BTreeMap<u32, Vec<TreeRow>>,
    /// Someone wrote after the author last did.
    pub unanswered: bool,
}

impl Series {
    /// (patch, kind, who, message-id) of every tag given.
    pub fn tags(&self) -> impl Iterator<Item = (u32, &str, &str, &str)> {
        self.replies
            .iter()
            .flat_map(|r| r.trailers.iter().map(|(k, w)| (r.index, k.as_str(), w.as_str(), r.msgid.as_str())))
    }

    /// Sent or written by addr: a patch someone resent for its author is
    /// still the author's.
    pub fn is_by(&self, addr: &str) -> bool {
        let addr = addr.to_lowercase();
        self.sender == addr
            || self
                .written_by
                .iter()
                .any(|w| w.rsplit_once('<').map_or(w.as_str(), |(_, a)| a.trim_end_matches('>')) == addr)
    }

    pub fn last_activity(&self) -> &str {
        self.replies.iter().map(|r| r.date.as_str()).fold(self.date.as_str(), |a, b| a.max(b))
    }

    pub fn as_of_text(&self) -> String {
        let when = |t: &str| t.get(..16).unwrap_or(t).replace('T', " ");
        let mut text = match (&self.as_of, &self.local_as_of) {
            (Some(a), l) if l.as_ref().is_none_or(|l| a >= l) => format!("as of {} UTC", when(a)),
            (a, Some(l)) => {
                // Local mail holds what was sent to you; a reply that did not copy you is missed.
                let mut t = format!("as of {} UTC, from your mailbox only", when(l));
                if let Some(a) = a {
                    t += &format!(" (archive {})", when(a));
                }
                t
            }
            _ => "thread never fetched".to_string(),
        };
        if self.pending {
            text += "; a fresher fetch is pending";
        }
        text
    }

    pub fn label(&self) -> String {
        let mut bits = vec![];
        if self.rfc {
            bits.push("RFC".to_string());
        }
        bits.push(if self.version > 1 { format!("PATCH v{}", self.version) } else { "PATCH".into() });
        if self.resend {
            bits.push("RESEND".into());
        }
        let n = self.patches.keys().filter(|&&i| i > 0).count().max(1);
        format!("[{}] {} ({n} patch{})", bits.join(" "), self.title, if n == 1 { "" } else { "es" })
    }
}

/// A patch the family's state is judged on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Part {
    pub root: String,
    pub version: u32,
    pub patch: u32,
    pub total: u32,
    pub stage: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Family {
    pub key: String,
    pub sender: String,
    /// Oldest first.
    pub versions: Vec<Series>,
    pub state: State,
    pub evidence: String,
    /// Each patch still in play, as far as any version of it got.
    pub parts: Vec<Part>,
    /// Why you closed it, if you did.
    pub closed: Option<String>,
}

impl Family {
    pub fn head(&self) -> &Series {
        self.versions.last().expect("a family has a version")
    }

    /// "sender NUL key": how the store names a family (closed, seen).
    pub fn id(&self) -> String {
        format!("{}\0{}", self.sender, self.key)
    }
}

/// Seconds since the epoch of a date as [`mail::utc`] writes it.
pub fn parse_utc(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || b[19] != b'Z'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    // Howard Hinnant's days_from_civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + num(11..13)? * 3600 + num(14..16)? * 60 + num(17..19)?)
}

fn close_in_time(a: &str, b: &str) -> bool {
    match (parse_utc(a), parse_utc(b)) {
        (Some(a), Some(b)) => (a - b).abs() <= SAME_POSTING_SECS,
        _ => true,
    }
}

/// "2026-10-05 10:00": a date to the minute, for evidence.
fn minute(t: &str) -> String {
    t.get(..16).unwrap_or(t).replace('T', " ")
}

fn day(t: &str) -> &str {
    t.get(..10).unwrap_or(t)
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// Every series in messages, by root, with its replies and state.
pub fn build(messages: &[Message], facts: &Facts, rules: &Rules) -> BTreeMap<String, Series> {
    let by_id: HashMap<&str, &Message> = messages.iter().map(|m| (m.msgid.as_str(), m)).collect();
    let subj: HashMap<&str, Subject> =
        messages.iter().map(|m| (m.msgid.as_str(), subject::parse(&m.subject))).collect();
    let is_patch = |id: &str| subj.get(id).is_some_and(|s| s.is_patch && !s.is_reply);
    let index_of = |id: &str| subj[id].index.unwrap_or(1);

    // A patch belongs under its parent when the parent is a patch of the
    // same posting: same sender, version, count and RESEND, sent close by.
    let root_of = |id: &str| -> String {
        let mut cur = id;
        let mut seen = HashSet::new();
        loop {
            let (m, s) = (by_id[cur], &subj[cur]);
            let parent = m.in_reply_to.as_deref().filter(|p| is_patch(p) && seen.insert(*p));
            let Some(parent) = parent else { return cur.to_string() };
            let (pm, ps) = (by_id[parent], &subj[parent]);
            let same = (ps.version, ps.total, ps.resend) == (s.version, s.total, s.resend)
                && pm.from.addr == m.from.addr
                && close_in_time(&m.date, &pm.date);
            if index_of(cur) == 0 || !same {
                return cur.to_string();
            }
            cur = parent;
        }
    };

    let mut patches: Vec<&Message> = messages.iter().filter(|m| is_patch(&m.msgid)).collect();
    patches.sort_by(|a, b| a.date.cmp(&b.date));
    let mut series: BTreeMap<String, Series> = BTreeMap::new();
    let mut profiles: HashMap<String, Option<&Profile>> = HashMap::new();
    let mut owner: HashMap<&str, (String, u32)> = HashMap::new();
    for m in patches {
        let root = root_of(&m.msgid);
        let s = series.entry(root.clone()).or_insert_with(|| {
            let (r, rs) = (by_id[root.as_str()], &subj[root.as_str()]);
            Series {
                root: root.clone(),
                sender: r.from.addr.clone(),
                version: rs.version,
                total: rs.total.unwrap_or(1),
                title: rs.title.clone(),
                date: r.date.clone(),
                rfc: rs.rfc,
                resend: rs.resend,
                family: mail::change_id(&root, &r.body)
                    .unwrap_or_else(|| format!("title:{}", subject::normalize_title(&rs.title))),
                patches: BTreeMap::new(),
                titles: BTreeMap::new(),
                bodies: BTreeMap::new(),
                written_by: BTreeSet::new(),
                replies: vec![],
                lists: facts.lists.get(&root).cloned().unwrap_or_default(),
                to: r.to.iter().chain(&r.cc).cloned().collect(),
                profile: None,
                as_of: facts.fetched.get(&root).cloned(),
                pending: facts.pending.contains(&root),
                local_as_of: facts.local.get(&root).cloned(),
                superseded_by: None,
                state: State::NoReply,
                evidence: String::new(),
                applied: BTreeSet::new(),
                stages: BTreeMap::new(),
                tree_rows: BTreeMap::new(),
                unanswered: false,
            }
        });
        if s.patches.is_empty() {
            let p = rules.profiles.iter().find(|p| p.matches(&s.to));
            s.profile = p.map(|p| p.name.clone());
            profiles.insert(root.clone(), p);
        }
        let i = index_of(&m.msgid);
        s.patches.insert(i, m.msgid.clone());
        s.titles.insert(i, subj[m.msgid.as_str()].title.clone());
        s.bodies.insert(i, m.body.clone());
        if !m.author.addr.is_empty() && m.author.addr != m.from.addr {
            s.written_by.insert(m.author.display());
        }
        owner.insert(&m.msgid, (root, i));
    }

    let mut by_date: Vec<&Message> = messages.iter().collect();
    by_date.sort_by(|a, b| a.date.cmp(&b.date));
    for m in by_date {
        let chain = m.in_reply_to.iter().chain(m.refs.iter().rev());
        let Some((root, index)) = chain.filter_map(|c| owner.get(c.as_str())).next() else { continue };
        let s = series.get_mut(root).expect("an owner's series");
        let mine = m.from.addr == s.sender;
        // The author's own postings are series, not replies; anyone else's
        // patch in the thread is both.
        if mine && is_patch(&m.msgid) {
            continue;
        }
        let profile = profiles[root];
        let landed = profile.and_then(|p| evidence::first_match(&m.body, &p.landed));
        if mine {
            // People the author brought in later were asked too.
            s.to.extend(m.to.iter().chain(&m.cc).cloned());
            s.replies.push(Reply {
                msgid: m.msgid.clone(),
                who: m.from.addr.clone(),
                date: m.date.clone(),
                index: *index,
                bot: false,
                trailers: vec![],
                applied: landed.clone().map(Applied::landed),
                mine: true,
                direct: false,
                landed: landed.is_some(),
                comments: vec![],
            });
            continue;
        }
        let who = format!("{} <{}>", m.from.name, m.from.addr).trim().to_string();
        let mut trailers = evidence::trailers(&m.body);
        if trailers.is_empty() && profile.is_some_and(|p| evidence::first_match(&m.body, &p.approvals).is_some()) {
            trailers = vec![("Approved".into(), who.clone())];
        }
        s.replies.push(Reply {
            msgid: m.msgid.clone(),
            who,
            date: m.date.clone(),
            index: *index,
            bot: rules.bots.is_bot(&m.from.addr, &m.body),
            trailers,
            applied: evidence::applied(&m.body, &m.subject).or_else(|| landed.clone().map(Applied::landed)),
            mine: false,
            direct: m.in_reply_to.as_ref() == s.patches.get(index),
            landed: landed.is_some(),
            comments: evidence::inline_comments(&m.body),
        });
    }

    supersede(&mut series);
    for s in series.values_mut() {
        s.tree_rows = s
            .patches
            .iter()
            .filter(|&(&i, _)| i > 0)
            .map(|(&i, mid)| (i, facts.trees.get(mid).cloned().unwrap_or_default()))
            .collect();
        derive(s);
    }
    series
}

/// (sender, family) to each member's (version, date, root).
type Members = BTreeMap<(String, String), Vec<(u32, String, String)>>;

fn supersede(series: &mut BTreeMap<String, Series>) {
    let mut fams = Members::new();
    for s in series.values() {
        fams.entry((s.sender.clone(), s.family.clone())).or_default().push((s.version, s.date.clone(), s.root.clone()));
    }
    for members in fams.values_mut() {
        members.sort();
        for pair in members.windows(2) {
            series.get_mut(&pair[0].2).expect("a member").superseded_by = Some(pair[1].2.clone());
        }
    }
}

/// The state a set of patch stages adds up to, or None if none got anywhere.
fn state_from_stages(stages: impl IntoIterator<Item = u8>) -> Option<State> {
    let stages: Vec<u8> = stages.into_iter().collect();
    let done = stages.iter().copied().min()?;
    match done {
        2.. => Some(State::from_stage(done)),
        1 => Some(State::Applied),
        _ if stages.iter().any(|&s| s > 0) => Some(State::PartiallyApplied),
        _ => None,
    }
}

/// Per patch (and the cover), the latest direct review if it commented
/// without a tag. Only direct replies to a posting judge it: replies to
/// replies are conversation (routing, thanks, follow-ups) more often than
/// review.
fn open_comments(s: &Series) -> Vec<&Reply> {
    let mut latest: BTreeMap<u32, &Reply> = BTreeMap::new();
    for r in &s.replies {
        if r.direct && !r.mine && !r.bot && r.applied.is_none() {
            latest.insert(r.index, r);
        }
    }
    let mut out: Vec<&Reply> = latest.into_values().filter(|r| r.is_review_comment()).collect();
    out.sort_by(|a, b| a.date.cmp(&b.date));
    out
}

fn derive(s: &mut Series) {
    let mut wanted: BTreeSet<u32> = s.patches.keys().copied().filter(|&i| i > 0).collect();
    if wanted.is_empty() {
        wanted.insert(1);
    }
    let mut unknown_part: Option<usize> = None;
    for (n, r) in s.replies.iter().enumerate() {
        let Some(a) = &r.applied else { continue };
        if r.mine && !r.landed {
            continue;
        }
        let got: BTreeSet<u32> = if !a.indices.is_empty() {
            a.indices.iter().copied().collect()
        } else if r.index == 0 && !a.subset {
            wanted.clone()
        } else {
            BTreeSet::from([r.index])
        };
        let hit: BTreeSet<u32> = got.intersection(&wanted).copied().collect();
        if hit.is_empty() {
            // A subset on the cover that does not say which: some went in.
            unknown_part = Some(n);
        }
        s.applied.extend(hit);
        if s.evidence.is_empty() {
            s.evidence = format!("{}, {}: \"{}\" <{}>", r.who, day(&r.date), a.line, r.msgid);
        }
    }

    let mut misses: BTreeMap<String, String> = BTreeMap::new();
    let mut hits: HashSet<String> = HashSet::new();
    let mut reverts: Vec<(u32, TreeRow)> = vec![];
    for &i in &wanted {
        let mut stage = u8::from(s.applied.contains(&i));
        for row in s.tree_rows.get(&i).into_iter().flatten() {
            if row.reverted_by.as_deref().is_some_and(|r| !r.is_empty()) {
                reverts.push((i, row.clone()));
                continue;
            }
            match row.commit_sha.as_deref().filter(|c| !c.is_empty()) {
                None => {
                    let w = misses.entry(row.tree.clone()).or_default();
                    if row.checked_at > *w {
                        *w = row.checked_at.clone();
                    }
                }
                Some(_) => {
                    hits.insert(row.tree.clone());
                    stage = stage.max(row.role.stage());
                }
            }
        }
        s.stages.insert(i, stage);
    }
    misses.retain(|t, _| !hits.contains(t));

    let others = s.replies.iter().filter(|r| !r.mine).count();
    // Indices, not references: the state is assigned while they are in use.
    let humans: Vec<usize> = (0..s.replies.len()).filter(|&n| !s.replies[n].bot && !s.replies[n].mine).collect();
    let reviewed = s
        .tags()
        .find(|t| APPROVALS.contains(&t.1))
        .map(|(i, kind, who, mid)| format!("{kind}: {who} on {i}/{} <{mid}>", s.total));
    let done = s.stages.values().copied().min().unwrap_or(0);
    if done >= 2 {
        s.state = State::from_stage(done);
        let i = *wanted.iter().find(|i| s.stages[i] == done).expect("a patch at the lowest stage");
        // The latest check at that stage; the first of equals, as Python's max() picks.
        let row = s.tree_rows[&i]
            .iter()
            .filter(|r| r.commit_sha.as_deref().is_some_and(|c| !c.is_empty()) && r.role.stage() == done)
            .fold(None::<&TreeRow>, |best, r| match best {
                Some(b) if b.checked_at >= r.checked_at => Some(b),
                _ => Some(r),
            })
            .expect("a row at that stage");
        s.evidence = format!(
            "{i}/{} is {} in {} (by {}), as of {} UTC",
            s.total,
            short(row.commit_sha.as_deref().unwrap_or_default()),
            row.tree,
            row.how.as_deref().unwrap_or("?"),
            minute(&row.checked_at)
        );
        if s.applied.is_empty() && s.as_of.is_some() {
            s.evidence += "; no \"applied\" mail in the thread";
        }
    } else if done == 1 {
        s.state = State::Applied;
        if !misses.is_empty() {
            // Today's most useful fact: a maintainer said so, the trees do not show it yet.
            let list: Vec<String> = misses.iter().map(|(t, w)| format!("{t} as of {}", minute(w))).collect();
            s.evidence += &format!("; not in {}", list.join(", "));
        }
    } else if s.stages.values().any(|&st| st > 0) {
        s.state = State::PartiallyApplied;
        let parts: Vec<String> =
            wanted.iter().map(|i| format!("{i}/{} {}", s.total, stage_name(s.stages[i]))).collect();
        s.evidence = parts.join(", ");
    } else if let Some(n) = unknown_part {
        let r = &s.replies[n];
        s.state = State::PartiallyApplied;
        s.evidence = format!(
            "{}, {}: \"{}\" (which patches is not said) <{}>",
            r.who,
            day(&r.date),
            r.applied.as_ref().map_or("", |a| a.line.as_str()),
            r.msgid
        );
    } else if let Some(by) = &s.superseded_by {
        s.state = State::Superseded;
        s.evidence = format!("by <{by}>");
    } else if let Some(ev) = open_comments(s)
        .last()
        .map(|r| format!("{} on {}/{}: \"{}\" <{}>", r.who, r.index, s.total, r.comments[0], r.msgid))
    {
        s.state = State::Commented;
        s.evidence = ev;
    } else if let Some(ev) = reviewed {
        s.state = State::Reviewed;
        s.evidence = ev;
    } else if let Some(&last) = humans.last() {
        let (n, last) = (humans.len(), &s.replies[last]);
        s.evidence = format!("{n} repl{}, last {} <{}>", if n == 1 { "y" } else { "ies" }, last.who, last.msgid);
        s.state = State::Discussed;
    } else if others > 0 {
        s.state = State::BotOnly;
        s.evidence = format!("{others} from bots");
    } else if s.as_of.is_none() && s.local_as_of.is_none() {
        // "No reply" is only a fact once the thread has been read somewhere.
        s.state = State::Unfetched;
    } else {
        s.state = State::NoReply;
    }

    for (i, row) in &reverts {
        let sep = if s.evidence.is_empty() { "" } else { "; " };
        s.evidence += &format!(
            "{sep}{i}/{} was {} in {}, reverted by {}",
            s.total,
            short(row.commit_sha.as_deref().unwrap_or_default()),
            row.tree,
            short(row.reverted_by.as_deref().unwrap_or_default())
        );
    }

    // A person wrote after the author last did. A fact, not a judgement: a
    // bare Reviewed-by counts too. An "Applied" notice asks for nothing, and a
    // superseded version waits on nobody.
    let last_mine = s.replies.iter().filter(|r| r.mine).map(|r| r.date.as_str()).fold(s.date.as_str(), |a, b| a.max(b));
    let last_ask =
        humans.iter().map(|&n| &s.replies[n]).filter(|r| r.applied.is_none()).map(|r| r.date.as_str()).next_back();
    s.unanswered = last_ask.is_some_and(|d| d > last_mine) && !s.state.done() && s.superseded_by.is_none();
}

/// Families by latest activity; closed families say so, with your reason.
pub fn families(series: BTreeMap<String, Series>, facts: &Facts) -> Vec<Family> {
    let mut all: Vec<Series> = series.into_values().collect();
    all.sort_by(|a, b| (a.version, &a.date).cmp(&(b.version, &b.date)));
    let mut order: Vec<(String, String)> = vec![];
    let mut groups: HashMap<(String, String), Vec<Series>> = HashMap::new();
    for s in all {
        let key = (s.sender.clone(), s.family.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(s);
    }
    let mut out: Vec<Family> = order
        .into_iter()
        .map(|key| {
            let versions = groups.remove(&key).expect("a group");
            let mut f = Family {
                key: key.1,
                sender: key.0,
                versions,
                state: State::NoReply,
                evidence: String::new(),
                parts: vec![],
                closed: None,
            };
            derive_family(&mut f);
            if let Some(why) = facts.closed.get(&f.id()) {
                f.closed = Some(why.clone());
                f.state = State::Closed;
                f.evidence = if why.is_empty() { "closed by you".into() } else { format!("closed by you: {why}") };
            }
            f
        })
        .collect();
    // Stable, so equal times keep their order.
    out.sort_by(|a, b| b.head().last_activity().cmp(a.head().last_activity()));
    out
}

/// Judge the family on the head's patches, each as far as any version of it
/// got. A patch is the same patch in another version if [`vdiff::links`]
/// pairs them. A patch a later version dropped still counts if it was
/// applied somewhere (dropped because it went in); one dropped and never
/// applied was abandoned.
fn derive_family(f: &mut Family) {
    let head = f.versions.len() - 1;
    // An older version whose commits a newer version also matched (same diff,
    // so same patch-id) is that version's history, not a second merge.
    let mut later: BTreeSet<String> = BTreeSet::new();
    for v in (0..f.versions.len()).rev() {
        let s = &mut f.versions[v];
        let commits: BTreeSet<String> = s
            .tree_rows
            .values()
            .flatten()
            .filter(|r| r.reverted_by.as_deref().is_none_or(str::is_empty))
            .filter_map(|r| r.commit_sha.clone().filter(|c| !c.is_empty()))
            .collect();
        if v != head && !commits.is_empty() && commits.is_subset(&later) {
            s.state = State::Superseded;
            s.evidence = format!(
                "by <{}>; a later version has the same commits",
                s.superseded_by.as_deref().unwrap_or_default()
            );
        }
        later.extend(commits);
    }

    // Follow each patch forward version by version to the head, or to the
    // last version that had it.
    #[derive(Clone, PartialEq, Eq, Hash)]
    enum End {
        Head(u32),
        Gone(usize, u32),
    }
    let mut end: HashMap<(usize, u32), End> =
        f.versions[head].stages.keys().map(|&i| ((head, i), End::Head(i))).collect();
    for v in (0..head).rev() {
        let link = vdiff::links(&f.versions[v], &f.versions[v + 1]);
        for &i in f.versions[v].stages.keys() {
            let to = link.get(&i).and_then(|n| end.get(&(v + 1, *n))).cloned().unwrap_or(End::Gone(v, i));
            end.insert((v, i), to);
        }
    }
    // Later versions win ties; keep the order in which ends were first seen.
    let mut best: Vec<(End, u8, usize, u32)> = vec![];
    for (v, s) in f.versions.iter().enumerate() {
        for (&i, &stage) in &s.stages {
            let k = end[&(v, i)].clone();
            match best.iter_mut().find(|b| b.0 == k) {
                Some(b) if stage >= b.1 => *b = (k, stage, v, i),
                Some(_) => {}
                None => best.push((k, stage, v, i)),
            }
        }
    }
    let part = |&(_, stage, v, i): &(End, u8, usize, u32)| {
        let s = &f.versions[v];
        Part { root: s.root.clone(), version: s.version, patch: i, total: s.total, stage }
    };
    let mut parts: Vec<(usize, Part)> = vec![];
    for &i in f.versions[head].stages.keys() {
        let b = best.iter().find(|b| b.0 == End::Head(i)).expect("each head patch ends at itself");
        parts.push((b.2, part(b)));
    }
    for b in best.iter().filter(|b| matches!(b.0, End::Gone(..)) && b.1 >= 1) {
        parts.push((b.2, part(b)));
    }

    let head_series = &f.versions[head];
    if parts.iter().all(|(v, _)| *v == head) {
        (f.state, f.evidence) = (head_series.state, head_series.evidence.clone());
    } else {
        f.state = state_from_stages(parts.iter().map(|(_, p)| p.stage)).unwrap_or(head_series.state);
        let text: Vec<String> = parts
            .iter()
            .map(|(_, p)| format!("v{} {}/{} {}", p.version, p.patch, p.total, stage_name(p.stage)))
            .collect();
        f.evidence = text.join(", ");
    }
    f.parts = parts.into_iter().map(|(_, p)| p).collect();
}

/// Every series in messages, grouped into families, with no facts beyond
/// the mail and the built-in rules.
pub fn group(messages: &[Message]) -> Vec<Family> {
    let facts = Facts::default();
    families(build(messages, &facts, &Rules::default()), &facts)
}
#[cfg(test)]
mod tests {
    use super::*;

    const ME: &str = "Dev <dev@example.org>";

    fn mail(id: &str, subject: &str, from: &str, irt: Option<&str>, date: &str, body: &str) -> Message {
        let irt = irt.map(|p| format!("In-Reply-To: <{p}>\nReferences: <{p}>\n")).unwrap_or_default();
        let raw = format!("Message-ID: <{id}>\nSubject: {subject}\nFrom: {from}\n{irt}Date: {date}\n\n{body}");
        Message::parse(raw.as_bytes()).expect("a message")
    }

    /// A cover letter and n patches threaded under it.
    fn posting(tag: &str, v: u32, n: u32, date: &str) -> Vec<Message> {
        let vs = if v > 1 { format!(" v{v}") } else { String::new() };
        let root = format!("{tag}-v{v}-0@x");
        let mut out = vec![mail(&root, &format!("[PATCH{vs} 0/{n}] foo: do the thing"), ME, None, date, "")];
        for i in 1..=n {
            out.push(mail(
                &format!("{tag}-v{v}-{i}@x"),
                &format!("[PATCH{vs} {i}/{n}] foo: part {i}"),
                ME,
                Some(&root),
                date,
                "",
            ));
        }
        out
    }

    #[test]
    fn versions_make_a_family() {
        let mut msgs = posting("a", 1, 2, "Mon, 05 Oct 2026 10:00:00 +0000");
        msgs.extend(posting("a", 2, 3, "Mon, 12 Oct 2026 10:00:00 +0000"));
        msgs.push(mail(
            "r@x",
            "Re: [PATCH 1/2] foo: part 1",
            "Rev <r@x>",
            Some("a-v1-1@x"),
            "Tue, 06 Oct 2026 10:00:00 +0000",
            "",
        ));
        let fams = group(&msgs);
        assert_eq!(fams.len(), 1);
        let f = &fams[0];
        assert_eq!((f.key.as_str(), f.sender.as_str()), ("title:foo: do the thing", "dev@example.org"));
        assert_eq!(f.versions.iter().map(|s| (s.version, s.patches.len())).collect::<Vec<_>>(), vec![(1, 3), (2, 4)]);
        assert_eq!(f.head().root, "a-v2-0@x");
        assert_eq!(f.head().titles[&2], "foo: part 2");
    }

    #[test]
    fn without_a_cover_patches_thread_under_the_first() {
        let d = "Mon, 05 Oct 2026 10:00:00 +0000";
        let msgs = vec![
            mail("p1@x", "[PATCH 1/2] bar: one", ME, None, d, ""),
            mail("p2@x", "[PATCH 2/2] bar: two", ME, Some("p1@x"), d, ""),
        ];
        let fams = group(&msgs);
        assert_eq!(fams.len(), 1);
        assert_eq!((fams[0].head().root.as_str(), fams[0].head().title.as_str()), ("p1@x", "bar: one"));
        assert_eq!(fams[0].head().patches.len(), 2);
    }

    #[test]
    fn a_new_version_sent_in_reply_is_its_own_series() {
        // git send-email --in-reply-to the previous version's cover.
        let mut msgs = posting("b", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000");
        msgs.push(mail(
            "b2@x",
            "[PATCH v2] foo: do the thing",
            ME,
            Some("b-v1-0@x"),
            "Tue, 06 Oct 2026 10:00:00 +0000",
            "",
        ));
        let fams = group(&msgs);
        assert_eq!(fams[0].versions.iter().map(|s| s.root.as_str()).collect::<Vec<_>>(), vec!["b-v1-0@x", "b2@x"]);
    }

    #[test]
    fn someone_elses_patch_in_reply_is_theirs() {
        let d = "Mon, 05 Oct 2026 10:00:00 +0000";
        let mut msgs = posting("c", 1, 1, d);
        // Same version, count and time as the patch it answers: only the sender differs.
        msgs.push(mail("o@x", "[PATCH 1/1] foo: an alternative", "Other <o@x>", Some("c-v1-1@x"), d, ""));
        let fams = group(&msgs);
        assert_eq!(fams.len(), 2);
        assert!(fams.iter().any(|f| f.sender == "o@x" && f.head().root == "o@x"));
    }

    #[test]
    fn patches_days_apart_are_not_one_posting() {
        let msgs = vec![
            mail("q1@x", "[PATCH 1/2] q: one", ME, None, "Mon, 05 Oct 2026 10:00:00 +0000", ""),
            mail("q2@x", "[PATCH 2/2] q: two", ME, Some("q1@x"), "Wed, 07 Oct 2026 10:00:00 +0000", ""),
        ];
        let roots: BTreeSet<_> = group(&msgs).iter().flat_map(|f| f.versions.iter().map(|s| s.root.clone())).collect();
        assert_eq!(roots, BTreeSet::from(["q1@x".to_string(), "q2@x".into()]));
    }

    #[test]
    fn change_id_joins_retitled_versions() {
        let d1 = "Mon, 05 Oct 2026 10:00:00 +0000";
        let d2 = "Mon, 12 Oct 2026 10:00:00 +0000";
        let msgs = vec![
            mail("20261005-thing-v1-0-0123456789ab@x", "[PATCH] foo: old title", ME, None, d1, ""),
            mail("20261012-thing-v2-0-0123456789ab@x", "[PATCH v2] foo: new title", ME, None, d2, ""),
        ];
        let fams = group(&msgs);
        assert_eq!(fams.len(), 1);
        assert_eq!((fams[0].key.as_str(), fams[0].versions.len()), ("thing", 2));
    }

    #[test]
    fn written_by_and_reply_cycles() {
        let d = "Mon, 05 Oct 2026 10:00:00 +0000";
        // Two patches that claim to reply to each other must not loop.
        let msgs = vec![
            mail("x1@x", "[PATCH 1/2] x: one", ME, Some("x2@x"), d, "From: Au Thor <au@x>\n\nFix.\n"),
            mail("x2@x", "[PATCH 2/2] x: two", ME, Some("x1@x"), d, ""),
        ];
        let fams = group(&msgs);
        let written: BTreeSet<_> =
            fams.iter().flat_map(|f| f.versions.iter().flat_map(|s| s.written_by.clone())).collect();
        assert_eq!(written, BTreeSet::from(["Au Thor <au@x>".to_string()]));
    }

    #[test]
    fn dates_round_trip() {
        for ts in [0, 1_637_446_921, 951_782_400, 4_102_444_799] {
            assert_eq!(parse_utc(&mail::utc(ts)), Some(ts));
        }
        assert_eq!(parse_utc(""), None);
        assert_eq!(parse_utc("2026-10-05 10:00:00"), None);
    }
}
