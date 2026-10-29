//! Settings: the user's preferences, kept in the store as JSON values.
//!
//! There is no config file. Each key holds one JSON value, and this module
//! decides what is valid, so the CLI and the web app accept and refuse the
//! same things. Command-line options override a setting for one run, and a
//! setting overrides the built-in default ([`KEYS`] lists both).
//!
//! Setting an unknown key, or a value of the wrong shape, is an error, so a
//! typo cannot silently do nothing. A key a store carries that this version
//! does not know (written by a newer patchtrack) is skipped with a warning.
//!
//! What you follow is not a setting but a [`Subscription`]: one row each.

use regex::Regex;
use serde_json::{Map, Value};

use crate::projects::Project;
use crate::rules::{self, BotRules, Profile, Rules};
use crate::sync::{self, Plan};

/// One setting: its name, the shape of its value, its default, and what it does.
#[derive(Debug, Clone, Copy)]
pub struct Key {
    pub name: &'static str,
    pub shape: &'static str,
    /// As JSON.
    pub default: &'static str,
    pub doc: &'static str,
    /// The control a form shows for it: addresses, text, url, time, days,
    /// minutes, bool, projects, profiles or bots.
    pub input: &'static str,
}

/// Every setting, in the order `patchtrack settings` shows them.
pub const KEYS: &[Key] = &[
    Key {
        name: "identities",
        shape: "list of addresses",
        default: "[]",
        doc: "Your addresses: what is yours when listing, and who you are when composing mail.",
        input: "addresses",
    },
    Key {
        name: "name",
        shape: "string",
        default: "\"\"",
        doc: "Your name, as mail you compose is signed and tags are given: `Name <address>`. Natively, git's \
              user.name when this is not set.",
        input: "text",
    },
    Key {
        name: "archive",
        shape: "URL",
        default: "\"https://lore.kernel.org\"",
        doc: "The public-inbox archive to sync with.",
        input: "url",
    },
    Key {
        name: "projects",
        shape: "list of {name, archive, lists, recipients}",
        default: "[]",
        doc: "The projects you work with, as GitHub has repositories: a name, a public-inbox archive, its \
              inboxes there (none: every one) and globs on To/Cc that put mail from your own mailbox in it. \
              Built in, by name: Linux, U-Boot, QEMU, Git, Xen, buildroot, GCC, glibc (`patchtrack project add`).",
        input: "projects",
    },
    Key {
        name: "since",
        shape: "time",
        default: "\"3.months.ago\"",
        doc: "How far back a first sync goes: N.days.ago (hours, weeks, months, years) or an ISO date.",
        input: "time",
    },
    Key {
        name: "list_since",
        shape: "time",
        default: "\"2.weeks.ago\"",
        doc: "How far back a first sync of a followed list or query goes: a list carries far more mail than \
              one author, and its search returns all of it.",
        input: "time",
    },
    Key {
        name: "window_days",
        shape: "days",
        default: "30",
        doc: "Search this many days at a time; a window with too many results is split further.",
        input: "days",
    },
    Key {
        name: "refresh_days",
        shape: "days",
        default: "7",
        doc: "An incremental sync also refetches open series not fetched for this long.",
        input: "days",
    },
    Key {
        name: "sync_every",
        shape: "minutes",
        default: "30",
        doc: "How often `bridge` and `serve` sync what you follow, in minutes (0: only when asked). Each sync is \
              one search per subscription, and a request per thread that changed.",
        input: "minutes",
    },
    Key {
        name: "fetch_all_threads",
        shape: "true or false",
        default: "false",
        doc: "Whether a sync fetches every thread a followed list's or search's search finds, one request each. \
              Otherwise it keeps what the search returned, and a thread is fetched when you open its series. An \
              author's threads, and threads you follow, are always fetched.",
        input: "bool",
    },
    Key {
        name: "stale_minutes",
        shape: "minutes",
        default: "30",
        doc: "Opening a series in the web app fetches its thread when it never was, or was longer ago than this \
              (0: only when you press Refresh).",
        input: "minutes",
    },
    Key {
        name: "ping_days",
        shape: "days",
        default: "7",
        doc: "`nudge` suggests a ping when a series has been quiet this long.",
        input: "days",
    },
    Key {
        name: "resend_days",
        shape: "days",
        default: "14",
        doc: "`nudge` suggests a RESEND when no human has answered for this long.",
        input: "days",
    },
    Key {
        name: "merge_window",
        shape: "true or false",
        default: "true",
        doc: "With a mainline tree, `nudge` holds pings while the merge window is open.",
        input: "bool",
    },
    Key {
        name: "b4_prefix",
        shape: "string",
        default: "\"b4/\"",
        doc: "Where `branches` finds your b4 prep branches: refs/heads/ and this.",
        input: "text",
    },
    Key {
        name: "profiles",
        shape: "list of {name, recipients, approvals, landed}",
        default: "[]",
        doc: "How a project says \"approved\" and \"landed\". recipients are globs on a posting's To:/Cc:; \
              approvals and landed are regexes (case-insensitive) on lines that start a paragraph, and landed \
              counts from the author too. A profile named like a built-in one (\"sourceware\") replaces it.",
        input: "profiles",
    },
    Key {
        name: "bots",
        shape: "{builtin, addresses, patterns, humans, bodies}",
        default: "{\"builtin\": true}",
        doc: "Who is a bot, so their replies are not review. builtin keeps the built-in rule (\"bot\", \"robot\", \
              \"noreply\" as a word of the address, and lkp@intel.com); addresses are bots; patterns are globs \
              on the address; humans are never bots; bodies are regexes on the unquoted body, for people who \
              also send bot mail.",
        input: "bots",
    },
];

pub fn key(name: &str) -> Option<&'static Key> {
    KEYS.iter().find(|k| k.name == name)
}

/// Everything the settings say, with the defaults filled in.
#[derive(Debug, Clone)]
pub struct Settings {
    pub identities: Vec<String>,
    pub name: String,
    pub archive: String,
    pub projects: Vec<Project>,
    pub since: String,
    pub list_since: String,
    pub window_days: i64,
    pub refresh_days: i64,
    pub sync_every: i64,
    pub fetch_all_threads: bool,
    pub stale_minutes: i64,
    pub ping_days: i64,
    pub resend_days: i64,
    pub merge_window: bool,
    pub b4_prefix: String,
    /// Yours, then the built-in ones you did not replace.
    pub rules: Rules,
}

impl Default for Settings {
    fn default() -> Self {
        let mut s = Settings {
            identities: vec![],
            name: String::new(),
            archive: String::new(),
            projects: vec![],
            since: String::new(),
            list_since: String::new(),
            window_days: 0,
            refresh_days: 0,
            sync_every: 0,
            fetch_all_threads: false,
            stale_minutes: 0,
            ping_days: 0,
            resend_days: 0,
            merge_window: false,
            b4_prefix: String::new(),
            rules: Rules::default(),
        };
        for k in KEYS {
            s.set(k.name, &serde_json::from_str(k.default).expect("valid default")).expect("valid default");
        }
        s
    }
}

impl Settings {
    /// Settings from the store's (key, JSON) rows. A key this version does
    /// not know is skipped, and said so in the warnings; a known key with
    /// a bad value is an error.
    pub fn load<K: AsRef<str>, V: AsRef<str>>(
        rows: impl IntoIterator<Item = (K, V)>,
    ) -> Result<(Settings, Vec<String>), String> {
        let mut s = Settings::default();
        let mut warnings = vec![];
        for (k, v) in rows {
            let (k, v) = (k.as_ref(), v.as_ref());
            if key(k).is_none() {
                warnings.push(format!("setting {k} is not known to this patchtrack; ignored"));
                continue;
            }
            let value: Value = serde_json::from_str(v).map_err(|e| format!("setting {k}: {e}"))?;
            s.set(k, &value).map_err(|e| format!("setting {k}: {e} (`patchtrack settings unset {k}` removes it)"))?;
        }
        Ok((s, warnings))
    }

    fn set(&mut self, name: &str, v: &Value) -> Result<(), String> {
        match name {
            "identities" => self.identities = addresses(v)?,
            "name" => {
                let name = string(v)?;
                if name.contains(['<', '>', '\n', '\r']) {
                    return Err("a name only, without an address or line breaks".into());
                }
                self.name = name.trim().to_string();
            }
            "archive" => self.archive = archive_url(&string(v)?)?,
            "projects" => {
                let mut out: Vec<Project> = vec![];
                for p in list(v)? {
                    let o = fields(p, "a project", &["name", "archive", "lists", "recipients"])?;
                    let name = o.get("name").map(string).transpose()?.unwrap_or_default().trim().to_string();
                    if name.is_empty() || out.iter().any(|q| q.name.eq_ignore_ascii_case(&name)) {
                        return Err(format!("a project needs a name of its own (got {name:?})"));
                    }
                    let archive = o.get("archive").map(string).transpose()?.ok_or("a project needs an archive")?;
                    let archive = archive_url(&archive)?;
                    let lists = o.get("lists").map(strings).transpose()?.unwrap_or_default();
                    let lists = lists.iter().map(|l| inbox(l)).collect::<Result<Vec<_>, _>>()?;
                    let recipients = o.get("recipients").map(strings).transpose()?.unwrap_or_default();
                    let recipients =
                        recipients.iter().map(|r| r.trim().to_lowercase()).filter(|r| !r.is_empty()).collect();
                    out.push(Project { name, archive, lists, recipients });
                }
                self.projects = out;
            }
            "since" | "list_since" => {
                let text = string(v)?;
                sync::since(&text, 0)?;
                if name == "since" {
                    self.since = text;
                } else {
                    self.list_since = text;
                }
            }
            "window_days" => self.window_days = days(v)?,
            "refresh_days" => self.refresh_days = days(v)?,
            "fetch_all_threads" => self.fetch_all_threads = v.as_bool().ok_or("must be true or false")?,
            "stale_minutes" => {
                self.stale_minutes =
                    v.as_i64().filter(|&n| n >= 0).ok_or("must be a whole number of minutes, 0 or more")?
            }
            "sync_every" => {
                self.sync_every =
                    v.as_i64().filter(|&n| n >= 0).ok_or("must be a whole number of minutes, 0 or more")?
            }
            "ping_days" => self.ping_days = days(v)?,
            "resend_days" => self.resend_days = days(v)?,
            "merge_window" => self.merge_window = v.as_bool().ok_or("must be true or false")?,
            "b4_prefix" => self.b4_prefix = string(v)?,
            "profiles" => {
                let mut mine = list(v)?.iter().map(profile).collect::<Result<Vec<_>, _>>()?;
                let builtin: Vec<_> =
                    rules::builtin_profiles().into_iter().filter(|b| mine.iter().all(|p| p.name != b.name)).collect();
                mine.extend(builtin);
                self.rules.profiles = mine;
            }
            "bots" => self.rules.bots = bots(v)?,
            _ => return Err(format!("no setting called {name}")),
        }
        Ok(())
    }

    /// A subscription as the store keeps it: on the archive setting's own
    /// archive, its archive is "" (so it follows that setting).
    pub fn own(&self, mut sub: Subscription) -> Subscription {
        if sub.archive == self.archive {
            sub.archive.clear();
        }
        sub
    }

    /// The URL of a subscription's archive ("" is the archive setting's).
    pub fn archive_of<'a>(&'a self, archive_key: &'a str) -> &'a str {
        if archive_key.is_empty() { &self.archive } else { archive_key }
    }

    /// Every archive a subscription or a project names, the setting's first.
    pub fn archives<'a>(&'a self, subs: &'a [Subscription]) -> Vec<&'a str> {
        let mut out = vec![self.archive.as_str()];
        for a in
            subs.iter().map(|s| self.archive_of(&s.archive)).chain(self.projects.iter().map(|p| p.archive.as_str()))
        {
            if !out.contains(&a) {
                out.push(a);
            }
        }
        out
    }

    /// What following `value` (of `kind`) in `project` (a name, or "" for
    /// none) means, as `patchtrack follow -p` takes it: on the project's
    /// archive, in `list` or else, for an author or a query, in each of the
    /// project's lists.
    pub fn follow(&self, kind: Follow, value: &str, list: &str, project: &str) -> Result<Vec<Subscription>, String> {
        let p = match project.trim() {
            "" => None,
            name => Some(
                self.projects
                    .iter()
                    .find(|p| p.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| format!("no project {name:?}"))?,
            ),
        };
        let lists: Vec<&str> = match p {
            Some(p)
                if list.trim().is_empty() && matches!(kind, Follow::Author | Follow::Query) && !p.lists.is_empty() =>
            {
                p.lists.iter().map(String::as_str).collect()
            }
            _ => vec![list],
        };
        let archive = p.map_or("", |p| p.archive.as_str());
        lists
            .into_iter()
            .map(|l| {
                let s = Subscription::new(kind, value, l)?;
                let s = if kind == Follow::Thread && archive.is_empty() { s } else { s.on(archive)? };
                Ok(self.own(s))
            })
            .collect()
    }

    /// A sync plan for these subscriptions, from these settings.
    pub fn plan(&self, searches: Vec<Subscription>, now: i64) -> Result<Plan, String> {
        let mut plan = Plan::new(&self.archive, vec![], sync::since(&self.since, now)?);
        plan.searches = searches;
        plan.first_since_wide = sync::since(&self.list_since, now)?;
        (plan.window_days, plan.refresh_days) = (self.window_days, self.refresh_days);
        plan.wide_threads = self.fetch_all_threads;
        Ok(plan)
    }
}

/// Check a value for a key, as typed by a person, and return it as the
/// JSON to store. A value that is not JSON is taken as a string, so
/// `archive https://example.org` needs no quotes.
pub fn validate(name: &str, text: &str) -> Result<String, String> {
    if key(name).is_none() {
        let names: Vec<_> = KEYS.iter().map(|k| k.name).collect();
        return Err(format!("no setting called {name}; the settings are {}", names.join(", ")));
    }
    let value = serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()));
    Settings::default().set(name, &value).map_err(|e| format!("{name}: {e}"))?;
    Ok(value.to_string())
}

/// What you follow: one search per subscription and list.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Subscription {
    pub kind: Follow,
    /// An author's address (lower case), an inbox's name, a public-inbox
    /// query, or a thread's message-id.
    pub value: String,
    /// The inbox to search; "all" searches every list. A list's is itself;
    /// a thread's is "all".
    pub list: String,
    /// The archive to search, "" for the archive setting's.
    pub archive: String,
}

impl Subscription {
    /// A subscription as a person names it, checked: an address, an inbox,
    /// a query, or a message-id or an archive URL of a message.
    pub fn new(kind: Follow, value: &str, list: &str) -> Result<Subscription, String> {
        let list = if list.trim().is_empty() { "all".to_string() } else { inbox(list)? };
        Ok(match kind {
            Follow::Author => Subscription { kind, value: address(value)?, list, archive: String::new() },
            Follow::List => {
                let name = inbox(value)?;
                if name == "all" {
                    return Err(
                        "\"all\" is every list: follow one inbox, or an author or a query in all of them".into()
                    );
                }
                Subscription { kind, list: name.clone(), value: name, archive: String::new() }
            }
            Follow::Query => {
                let q = value.trim();
                if q.is_empty() || q.contains(['\n', '\r']) {
                    return Err(format!("{value:?} is not a query: one line of public-inbox search terms"));
                }
                Subscription { kind, value: q.to_string(), list, archive: String::new() }
            }
            Follow::Thread => {
                // An archive URL says where the thread is: fetch it from there.
                let (inbox, archive) = match value.trim().split_once("://") {
                    Some((scheme, rest)) => {
                        let mut parts = rest.split('/');
                        let host = parts.next().unwrap_or("");
                        let inbox = parts.next().filter(|i| inbox(i).is_ok()).unwrap_or("all");
                        (inbox.to_string(), format!("{scheme}://{host}"))
                    }
                    None => ("all".to_string(), String::new()),
                };
                Subscription { kind, value: message_id(value)?, list: inbox, archive }
            }
        })
    }

    /// The same, on another archive than the archive setting's ("" for that one).
    pub fn on(self, archive: &str) -> Result<Subscription, String> {
        let archive = if archive.trim().is_empty() { String::new() } else { archive_url(archive)? };
        Ok(Subscription { archive, ..self })
    }

    /// What it is, for a person: "me@x.org", "every series on linux-usb",
    /// and the archive's host when it is not the archive setting's.
    pub fn label(&self) -> String {
        let r#in = if self.list == "all" { String::new() } else { format!(" in {}", self.list) };
        let on = match self.archive.split_once("://") {
            Some((_, host)) => format!(" on {host}"),
            None => String::new(),
        };
        match self.kind {
            Follow::Author => format!("{}{in}{on}", self.value),
            Follow::List => format!("every series on {}{on}", self.value),
            Follow::Query => format!("query {:?}{in}{on}", self.value),
            Follow::Thread => format!("thread <{}>{on}", self.value),
        }
    }
}

/// Kinds of subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Follow {
    /// Their series, and the replies to them.
    Author,
    /// Every series posted to an inbox: a maintainer's queue.
    List,
    /// What a public-inbox search finds, and the threads it is in.
    Query,
    /// One thread.
    Thread,
}

impl Follow {
    pub fn as_str(self) -> &'static str {
        match self {
            Follow::Author => "author",
            Follow::List => "list",
            Follow::Query => "query",
            Follow::Thread => "thread",
        }
    }

    pub fn parse(s: &str) -> Option<Follow> {
        match s {
            "author" => Some(Follow::Author),
            "list" => Some(Follow::List),
            "query" => Some(Follow::Query),
            "thread" => Some(Follow::Thread),
            _ => None,
        }
    }
}

/// An archive's base URL: http(s), without a trailing "/".
pub fn archive_url(text: &str) -> Result<String, String> {
    let url = text.trim();
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    if !host.is_some_and(|h| !h.trim_matches('/').is_empty() && !h.contains(char::is_whitespace)) {
        return Err(format!("{url:?} is not an http(s) URL"));
    }
    Ok(url.trim_end_matches('/').to_string())
}

/// An inbox's name as the archive's URLs use it: "linux-usb", "all".
pub fn inbox(text: &str) -> Result<String, String> {
    let t = text.trim();
    let ok = t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && t.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok { Ok(t.to_string()) } else { Err(format!("{text:?} is not an inbox name, such as linux-usb")) }
}

/// A message-id from what a person pastes: "<id>", "id", or the message's
/// URL on an archive ("https://lore.kernel.org/linux-usb/id/", "…/id/T/#u").
pub fn message_id(text: &str) -> Result<String, String> {
    let t = text.trim();
    let id = if let Some(rest) = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://")) {
        // host / inbox / id [/ T / #u ...]
        let mut parts = rest.split('/');
        let (_host, _inbox, id) = (parts.next(), parts.next(), parts.next().unwrap_or(""));
        percent_decode(id)
    } else {
        t.strip_prefix('<').and_then(|i| i.strip_suffix('>')).unwrap_or(t).to_string()
    };
    if id.contains('@') && !id.contains(['<', '>', ' ', '\t', '/']) && !id.starts_with('@') {
        Ok(id)
    } else {
        Err(format!("{text:?} is not a message-id or a message's archive URL"))
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, ..) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn string(v: &Value) -> Result<String, String> {
    v.as_str().map(str::to_string).ok_or_else(|| "must be a string".into())
}

fn list(v: &Value) -> Result<&Vec<Value>, String> {
    v.as_array().ok_or_else(|| "must be a list".into())
}

fn strings(v: &Value) -> Result<Vec<String>, String> {
    list(v)?.iter().map(|s| s.as_str().map(str::to_string).ok_or_else(|| "must be a list of strings".into())).collect()
}

/// An email address as settings and subscriptions keep it: lower case.
pub fn address(text: &str) -> Result<String, String> {
    let a = text.trim();
    match a.split_once('@') {
        Some((user, host)) if !user.is_empty() && !host.is_empty() && !a.contains(char::is_whitespace) => {
            Ok(a.to_lowercase())
        }
        _ => Err(format!("{text:?} is not an address")),
    }
}

fn addresses(v: &Value) -> Result<Vec<String>, String> {
    strings(v)?.iter().map(|a| address(a)).collect()
}

fn days(v: &Value) -> Result<i64, String> {
    v.as_i64().filter(|&n| n > 0).ok_or_else(|| "must be a whole number of days, more than 0".into())
}

/// The object's fields, refusing any not in `allowed`.
fn fields<'a>(v: &'a Value, what: &str, allowed: &[&str]) -> Result<&'a Map<String, Value>, String> {
    let obj = v.as_object().ok_or_else(|| format!("{what} must be an object"))?;
    let mut unknown: Vec<_> = obj.keys().filter(|k| !allowed.contains(&k.as_str())).map(String::as_str).collect();
    if !unknown.is_empty() {
        unknown.sort();
        return Err(format!("unknown key(s) in {what}: {} (known: {})", unknown.join(", "), allowed.join(", ")));
    }
    Ok(obj)
}

fn regexes(v: Option<&Value>, flags: &str, what: &str) -> Result<Vec<Regex>, String> {
    let Some(v) = v else { return Ok(vec![]) };
    strings(v)
        .map_err(|e| format!("{what} {e}"))?
        .iter()
        .map(|r| Regex::new(&format!("(?{flags}){r}")).map_err(|e| format!("{what}: {e}")))
        .collect()
}

fn profile(v: &Value) -> Result<Profile, String> {
    let obj = fields(v, "a profile", &["name", "recipients", "approvals", "landed"])?;
    let name = obj.get("name").and_then(Value::as_str).filter(|n| !n.is_empty());
    let recipients = obj.get("recipients").map(strings).transpose()?.filter(|r| !r.is_empty());
    let (Some(name), Some(recipients)) = (name, recipients) else {
        return Err("every profile needs a name and recipients".into());
    };
    Ok(Profile {
        name: name.to_string(),
        recipients: recipients.iter().map(|r| r.to_lowercase()).collect(),
        approvals: regexes(obj.get("approvals"), "i", &format!("profile {name} approvals"))?,
        landed: regexes(obj.get("landed"), "i", &format!("profile {name} landed"))?,
    })
}

fn bots(v: &Value) -> Result<BotRules, String> {
    let obj = fields(v, "bots", &["builtin", "addresses", "patterns", "humans", "bodies"])?;
    let lower = |k: &str| -> Result<Vec<String>, String> {
        let all = obj.get(k).map(strings).transpose().map_err(|e| format!("bots {k} {e}"))?;
        Ok(all.unwrap_or_default().iter().map(|a| a.to_lowercase()).collect())
    };
    Ok(BotRules {
        builtin: match obj.get("builtin") {
            None => true,
            Some(b) => b.as_bool().ok_or("bots builtin must be true or false")?,
        },
        addresses: lower("addresses")?,
        patterns: lower("patterns")?,
        humans: lower("humans")?,
        bodies: regexes(obj.get("bodies"), "m", "bots bodies")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(rows: &[(&str, &str)]) -> Result<Settings, String> {
        Settings::load(rows.iter().copied()).map(|(s, _)| s)
    }

    #[test]
    fn defaults() {
        let s = Settings::default();
        assert_eq!((s.archive.as_str(), s.since.as_str()), ("https://lore.kernel.org", "3.months.ago"));
        assert_eq!((s.window_days, s.refresh_days, s.ping_days, s.resend_days), (30, 7, 7, 14));
        assert!(s.merge_window && s.rules.bots.builtin && s.identities.is_empty());
        assert_eq!(s.rules.profiles.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["sourceware"]);
    }

    #[test]
    fn values_as_typed() {
        assert_eq!(validate("archive", "https://example.org/").unwrap(), "\"https://example.org/\"");
        assert_eq!(validate("identities", r#"["Me@X.org"]"#).unwrap(), r#"["Me@X.org"]"#);
        assert_eq!(validate("window_days", "14").unwrap(), "14");
        assert_eq!(validate("since", "6.months.ago").unwrap(), "\"6.months.ago\"");
        let s = load(&[("identities", r#"["Me@X.org"]"#), ("archive", "\"https://example.org/\"")]).unwrap();
        assert_eq!((s.identities, s.archive.as_str()), (vec!["me@x.org".to_string()], "https://example.org"));
    }

    #[test]
    fn bad_values_are_errors() {
        for (k, v) in [
            ("identitys", r#"["me@x"]"#), // no such key
            ("identities", "me@x"),       // a string, not a list
            ("identities", r#"["not an address"]"#),
            ("archive", "lore.kernel.org"), // no scheme
            ("window_days", "true"),
            ("window_days", "0"),
            ("since", "20261023x"),
            ("merge_window", "yes"),
            ("bots", r#"{"address": []}"#), // a typo
            ("bots", r#"{"bodies": ["("]}"#),
            ("bots", r#"{"builtin": "no"}"#),
            ("profiles", r#"[{"name": "y"}]"#), // no recipients
            ("profiles", r#"[{"name": "y", "recipients": ["a"], "landed": ["("]}]"#),
            ("profiles", r#"[{"name": "y", "recipients": ["a"], "approval": []}]"#),
        ] {
            assert!(validate(k, v).is_err(), "{k} = {v}");
        }
    }

    #[test]
    fn a_stored_bad_value_names_the_way_out() {
        let e = load(&[("window_days", "\"x\"")]).unwrap_err();
        assert!(e.contains("settings unset window_days"), "{e}");
    }

    #[test]
    fn unknown_stored_keys_are_skipped_with_a_warning() {
        let (s, warnings) = Settings::load([("from_the_future", "1"), ("ping_days", "3")]).unwrap();
        assert_eq!(s.ping_days, 3);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_profile_replaces_the_builtin_of_its_name() {
        let s = load(&[(
            "profiles",
            r#"[{"name": "sourceware", "recipients": ["*@gcc.gnu.org"], "approvals": ["^ship it"]},
                {"name": "x", "recipients": ["Dev@X.org"]}]"#,
        )])
        .unwrap();
        let p = &s.rules.profiles;
        assert_eq!(p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["sourceware", "x"]);
        assert!(p[0].approvals[0].is_match("Ship it!"));
        assert_eq!(p[1].recipients, ["dev@x.org"]);
    }

    #[test]
    fn bots() {
        let s = load(&[(
            "bots",
            r#"{"addresses": ["CI@x.org"], "patterns": ["*@CI.example.org"], "humans": ["review-bot@x.org"],
                "bodies": ["^My bot found errors"]}"#,
        )])
        .unwrap();
        let b = &s.rules.bots;
        assert!(b.builtin);
        assert!(b.is_bot("ci@x.org", "") && b.is_bot("runner7@ci.example.org", ""));
        assert!(!b.is_bot("review-bot@x.org", ""));
        // Multi-line: a body regex anchored at a line, not only the first.
        assert!(b.is_bot("reviewer@example.org", "Hi,\nMy bot found errors running 'make'\n"));
        let off = load(&[("bots", r#"{"builtin": false}"#)]).unwrap();
        assert!(!off.rules.bots.is_bot("x-bot@y.org", ""));
    }

    #[test]
    fn projects_are_checked() {
        let p = load(&[(
            "projects",
            r#"[{"name": "GCC", "archive": "https://inbox.sourceware.org/", "lists": ["gcc-patches"], "recipients": ["GCC-Patches@gcc.gnu.org"]}]"#,
        )])
        .unwrap();
        assert_eq!(p.projects[0].archive, "https://inbox.sourceware.org");
        assert_eq!(p.projects[0].recipients, ["gcc-patches@gcc.gnu.org"]);
        for bad in [
            r#"[{"archive": "https://x.org"}]"#,
            r#"[{"name": "a", "archive": "ftp://x"}]"#,
            r#"[{"name": "a", "archive": "https://x.org", "lists": ["a b"]}]"#,
            r#"[{"name": "a", "archive": "https://x.org"}, {"name": "A", "archive": "https://y.org"}]"#,
            r#"[{"name": "a", "archive": "https://x.org", "colour": "red"}]"#,
        ] {
            assert!(validate("projects", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn plan_takes_the_sync_settings() {
        let s = load(&[("archive", "\"https://x.org\""), ("since", "\"2026-10-01\""), ("window_days", "14")]).unwrap();
        let me = Subscription::new(Follow::Author, "Me@X.org", "linux-usb").unwrap();
        let plan = s.plan(vec![me.clone()], 1_800_000_000).unwrap();
        assert_eq!((plan.archive.as_str(), plan.first_since, plan.window_days), ("https://x.org", 1_790_812_800, 14));
        assert_eq!((plan.refresh_days, plan.first_since_wide), (7, 1_800_000_000 - 14 * 86_400));
        assert_eq!(plan.searches, [me]);
    }

    #[test]
    fn subscriptions_are_checked_as_named() {
        let sub = |k, v: &str, l: &str| Subscription::new(k, v, l);
        assert_eq!(sub(Follow::Author, "Me@X.org", "").unwrap().list, "all");
        let list = sub(Follow::List, "linux-usb", "ignored").unwrap();
        assert_eq!((list.value.as_str(), list.list.as_str()), ("linux-usb", "linux-usb"));
        assert!(sub(Follow::List, "all", "").is_err());
        assert!(sub(Follow::List, "a$(rm -rf ~)", "").is_err());
        assert!(sub(Follow::Author, "me@x.org", "linux usb").is_err());
        assert_eq!(sub(Follow::Query, " dfn:drivers/usb/ ", "linux-usb").unwrap().value, "dfn:drivers/usb/");
        assert!(sub(Follow::Query, "a\nb", "").is_err());
        let lore = "https://lore.kernel.org";
        for (pasted, list, archive) in [
            ("<20261005.1234-1-me@x.org>", "all", ""),
            ("20261005.1234-1-me@x.org", "all", ""),
            ("https://lore.kernel.org/linux-usb/20261005.1234-1-me@x.org/", "linux-usb", lore),
            ("https://lore.kernel.org/all/20261005.1234-1-me%40x.org/T/#u", "all", lore),
            (
                "https://inbox.sourceware.org/gcc-patches/20261005.1234-1-me@x.org/",
                "gcc-patches",
                "https://inbox.sourceware.org",
            ),
        ] {
            let t = sub(Follow::Thread, pasted, "").unwrap();
            assert_eq!(
                (t.value.as_str(), t.list.as_str(), t.archive.as_str()),
                ("20261005.1234-1-me@x.org", list, archive),
                "{pasted}"
            );
        }
        // On the archive setting's archive, a subscription follows that setting.
        let s = load(&[]).unwrap();
        let t = sub(Follow::Thread, "https://lore.kernel.org/linux-usb/a@x/", "").unwrap();
        assert_eq!(s.own(t.clone()).archive, "");
        let gcc = sub(Follow::Author, "a@x", "gcc-patches").unwrap().on("https://inbox.sourceware.org/").unwrap();
        assert_eq!(
            (s.own(gcc.clone()).archive.as_str(), gcc.label().as_str()),
            ("https://inbox.sourceware.org", "a@x in gcc-patches on inbox.sourceware.org")
        );
        assert_eq!(s.archives(&[gcc, t]), [lore, "https://inbox.sourceware.org"]);
        assert!(sub(Follow::Thread, "https://lore.kernel.org/linux-usb/", "").is_err());
        assert!(sub(Follow::Thread, "not an id", "").is_err());
        assert_eq!(sub(Follow::List, "linux-usb", "").unwrap().label(), "every series on linux-usb");
    }
}
