//! patchtrack: track patch series on mailing lists, like pull requests.

mod api;
mod book;
mod bridge;
mod compose;
mod git;
mod local;
mod machine;
mod pretty;
mod read;
mod tree;
mod tui;
mod views;

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use patchtrack_core::mail::Head;
use patchtrack_core::settings::{self, Follow, Settings, Subscription};
use patchtrack_core::sync::{self, Effect, Known, Method, Outcome, Request};
use patchtrack_core::{mail, series};
use patchtrack_store::{Export, Store};

/// Honest: what we are, and where to find out more or complain.
const USER_AGENT: &str = concat!("patchtrack/", env!("CARGO_PKG_VERSION"), " (+", env!("CARGO_PKG_REPOSITORY"), ")");

#[derive(Parser)]
#[command(name = "patchtrack", version, about)]
struct Cli {
    /// The store (default: patchtrack/store.sqlite3 in your data directory:
    /// $XDG_DATA_HOME or ~/.local/share on Linux, %LOCALAPPDATA% on Windows)
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

/// What `sync` was asked for.
/// What the bridge or the server may do on this machine, besides fetching.
#[derive(clap::Args)]
struct Allow {
    /// Send drafts from the app through git send-email (only from your identities)
    #[arg(long)]
    allow_send: bool,
    /// A repository whose b4 prep branches the app may list and run b4 on (repeatable)
    #[arg(long = "repo", value_name = "PATH")]
    repos: Vec<PathBuf>,
}

/// What `follow` and `unfollow` name.
#[derive(clap::Args)]
struct Followed {
    /// Email addresses of authors
    addresses: Vec<String>,
    /// Inbox to search, e.g. linux-usb (repeatable; default: all)
    #[arg(short, long = "list")]
    lists: Vec<String>,
    /// Every series posted to this inbox, e.g. linux-usb (repeatable)
    #[arg(long = "inbox", value_name = "INBOX")]
    inboxes: Vec<String>,
    /// What this public-inbox search finds, e.g. 'dfn:drivers/usb/' (repeatable)
    #[arg(long = "query", value_name = "QUERY")]
    queries: Vec<String>,
    /// One thread: a message-id, or a message's archive URL (repeatable)
    #[arg(long = "thread", value_name = "MESSAGE")]
    threads: Vec<String>,
    /// In this project (see `project`): its archive, and its lists unless -l
    #[arg(short, long, conflicts_with = "archive")]
    project: Option<String>,
    /// On this public-inbox archive, not the archive setting's
    #[arg(long, value_name = "URL")]
    archive: Option<String>,
}

impl Followed {
    fn is_empty(&self) -> bool {
        self.addresses.is_empty() && self.inboxes.is_empty() && self.queries.is_empty() && self.threads.is_empty()
    }

    /// Every subscription named, checked; an author and a query once per -l,
    /// or per list of the project's.
    fn subscriptions(&self, settings: &Settings) -> Result<Vec<Subscription>, String> {
        let project = match &self.project {
            Some(name) => Some(project(settings, name)?),
            None => None,
        };
        let archive = project.map(|p| p.archive.clone()).or(self.archive.clone()).unwrap_or_default();
        let lists = match (self.lists.is_empty(), project) {
            (false, _) => self.lists.clone(),
            (true, Some(p)) if !p.lists.is_empty() => p.lists.clone(),
            _ => vec![String::new()],
        };
        let subs = self.named(&lists)?;
        // A thread's URL names its own archive; anything else is on the one chosen here.
        let on = |s: Subscription| if s.kind == Follow::Thread && archive.is_empty() { Ok(s) } else { s.on(&archive) };
        subs.into_iter().map(|s| on(s).map(|s| settings.own(s))).collect()
    }

    fn named(&self, lists: &[String]) -> Result<Vec<Subscription>, String> {
        let mut out = vec![];
        for l in lists {
            for a in &self.addresses {
                out.push(Subscription::new(Follow::Author, a, l)?);
            }
            for q in &self.queries {
                out.push(Subscription::new(Follow::Query, q, l)?);
            }
        }
        for i in &self.inboxes {
            out.push(Subscription::new(Follow::List, i, "")?);
        }
        for t in &self.threads {
            out.push(Subscription::new(Follow::Thread, t, "")?);
        }
        Ok(out)
    }
}

/// A project of the settings', by name in any case.
pub(crate) fn project<'a>(
    settings: &'a Settings,
    name: &str,
) -> Result<&'a patchtrack_core::projects::Project, String> {
    settings.projects.iter().find(|p| p.name.eq_ignore_ascii_case(name)).ok_or_else(|| {
        let names: Vec<&str> = settings.projects.iter().map(|p| p.name.as_str()).collect();
        format!("no project {name:?} (yours: {}); `patchtrack project add` adds one", names.join(", "))
    })
}

#[derive(clap::Subcommand)]
enum DraftsCommand {
    /// Every draft, sent or not, oldest first (the default)
    List,
    /// One draft, as the message it is
    Show {
        /// Its Message-ID, or a unique part of it
        draft: String,
    },
    /// Forget a draft
    Rm {
        /// Its Message-ID, or a unique part of it
        draft: String,
    },
}

#[derive(clap::Args)]
pub(crate) struct SyncArgs {
    /// Email address, instead of the authors you follow (repeatable)
    #[arg(short, long = "author")]
    authors: Vec<String>,
    /// Inbox to search, e.g. linux-usb, instead of the lists you follow them in (repeatable)
    #[arg(short, long = "list")]
    lists: Vec<String>,
    /// N.days.ago (hours, weeks, months, years) or an ISO date; default: since
    /// the last sync, or the `since` setting for the first
    #[arg(long)]
    since: Option<String>,
    /// Refetch every thread with mail in the window, not only what changed
    #[arg(long)]
    full: bool,
    /// Incremental: also refetch open series not fetched for this long (default: the setting)
    #[arg(long)]
    refresh_days: Option<i64>,
    /// Search this many days at a time; big windows are split further (default: the setting)
    #[arg(long)]
    window_days: Option<i64>,
    /// Seconds between requests
    #[arg(long, default_value_t = 1.0)]
    delay: f64,
    /// Search only; the threads it would fetch are left for the next sync
    #[arg(long)]
    no_threads: bool,
    /// Read your local mail (see `source`) only, not the archive
    #[arg(long, conflicts_with = "no_local")]
    offline: bool,
    /// Do not read your local mail first
    #[arg(long)]
    no_local: bool,
    /// The public-inbox archive (default: the setting)
    #[arg(long)]
    archive: Option<String>,
}

impl Default for SyncArgs {
    /// What a plain `patchtrack sync` does.
    fn default() -> Self {
        SyncArgs {
            authors: vec![],
            lists: vec![],
            since: None,
            full: false,
            refresh_days: None,
            window_days: None,
            delay: 1.0,
            no_threads: false,
            offline: false,
            no_local: false,
            archive: None,
        }
    }
}

#[derive(Subcommand)]
enum SettingsAction {
    /// Set KEY to VALUE: JSON, or a bare string.
    Set {
        /// A setting's name (see `patchtrack settings --help`)
        key: String,
        /// JSON, or a bare string
        value: String,
    },
    /// Put KEY back to its default.
    Unset {
        /// A setting's name
        key: String,
    },
}

#[derive(Subcommand)]
enum SourceAction {
    /// Read PATH on every sync: a Maildir, a directory of .eml files, or an mbox file.
    Add {
        /// A Maildir, a directory of .eml files, or an mbox file
        path: PathBuf,
    },
    /// The local mail sync reads, and when it last did.
    List,
    /// Stop reading PATH. Mail already read stays in the store.
    Rm {
        /// A path `source list` shows
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum ProjectAction {
    /// Add a project: a built-in one by name, or your own with --archive.
    Add {
        /// Linux, U-Boot, QEMU, Git, Xen, buildroot, GCC, glibc; or a name of your own
        name: String,
        /// Its public-inbox archive (your own project; default: the built-in one's)
        #[arg(long, value_name = "URL")]
        archive: Option<String>,
        /// One of its inboxes on the archive (repeatable; none: every one)
        #[arg(short, long = "list")]
        lists: Vec<String>,
        /// A glob on To/Cc that puts your own mail in it (repeatable)
        #[arg(long = "to", value_name = "GLOB")]
        recipients: Vec<String>,
    },
    /// Your projects, and the built-in ones you could add.
    List,
    /// Forget a project. What you follow in it, and its mail, stay.
    Rm { name: String },
}

#[derive(Subcommand)]
enum Command {
    /// Fetch the authors' mail from the archive, and the threads of their series.
    ///
    /// One search per author and list, (f:ADDR OR tc:ADDR) AND rt:SINCE..:
    /// tc: finds other people's replies, which nearly always copy the author.
    /// Then the whole thread of every series the search touched is fetched,
    /// because a reply that does not copy the author is only in the thread.
    ///
    /// With no -a, it syncs the authors you follow, in the lists you follow
    /// them in (see `follow`); -a and -l replace them for one run.
    ///
    /// The first sync of an author covers --since (the `since` setting,
    /// three months by default).
    /// Later syncs search from the last one, with a day's overlap, and refetch
    /// only the threads new mail landed in, plus open series not fetched for
    /// --refresh-days. Searches run in windows of --window-days; a window
    /// with 1000 or more results is searched again in halves. Timeouts and
    /// HTTP 429/5xx are retried with backoff, honouring Retry-After. A failed
    /// search records nothing, so the next sync covers the same window; a
    /// thread whose fetch failed stays owed until a sync fetches it. The exit
    /// status is non-zero when anything failed.
    ///
    /// Your local mail (see `source`) is read first, keeping only what
    /// matters: patches the authors (and your identities) sent or wrote, and
    /// replies in their threads. New local mail makes the archive fetch its
    /// thread. --offline reads only local mail; --no-local skips it.
    Sync(SyncArgs),
    /// Follow an author, a list, a search or a thread: `sync` fetches what
    /// you follow. With nothing to follow, list what you follow.
    ///
    /// An author's series and the replies to them; with --inbox, every
    /// series posted to an inbox (a maintainer's queue); with --query, what
    /// a public-inbox search finds, and the threads it is in; with --thread,
    /// one thread, by a message-id or its archive URL. -l limits an
    /// author's or a query's searches to one inbox (repeatable); the default
    /// searches every list ("all"). A list or a query finds far more than
    /// an author: its first sync goes back only as far as the list_since
    /// setting, and each thread it finds is a request. Following is kept in
    /// the store, so an exported store follows the same.
    Follow {
        #[command(flatten)]
        what: Followed,
    },
    /// Stop following an author, a list, a search or a thread.
    ///
    /// An author or a query in every list, or (-l) in one. Their mail stays
    /// in the store.
    Unfollow {
        #[command(flatten)]
        what: Followed,
    },
    /// Show the settings, or change one.
    ///
    /// Settings are kept in the store, so an exported store carries them
    /// (a public export does not). With no action, every setting is shown,
    /// with its default where it is not set: the settings in effect.
    /// Values are JSON, or a bare string:
    ///
    ///     patchtrack settings set identities '["me@example.org"]'
    ///     patchtrack settings set archive https://lore.kernel.org
    ///     patchtrack settings set bots '{"addresses": ["ci@example.org"]}'
    ///
    /// An unknown key, or a value of the wrong shape, is an error.
    /// Command-line options override a setting for one run.
    #[command(verbatim_doc_comment)]
    Settings {
        #[command(subcommand)]
        action: Option<SettingsAction>,
    },
    /// The projects you work with, as GitHub has repositories: the Linux
    /// kernel on lore, GCC on sourceware, U-Boot.
    ///
    /// A project is a public-inbox archive, its lists there (none: every
    /// one), and globs on To/Cc that place mail from your own mailbox in
    /// it. `follow -p NAME` follows in a project (on its archive, in its
    /// lists), `list -p NAME` shows a project's series, and `sync` syncs
    /// each archive in turn. With no action, list them. Projects are the
    /// `projects` setting, so an exported store carries them.
    Project {
        #[command(subcommand)]
        action: Option<ProjectAction>,
    },
    /// Your own mail, which sync reads before the archive: Maildirs,
    /// directories of .eml files, and mbox files.
    ///
    /// Paths are this machine's: an exported store leaves them out.
    Source {
        #[command(subcommand)]
        action: SourceAction,
    },
    /// Name the git trees that `check` looks in.
    ///
    /// A tree is a ref in a local repository, with a role: maintainer (a
    /// patch found there is queued), next (in-next) or mainline (merged).
    /// Give either --ref (a ref already in the repository), or --url and
    /// --branch: `check --fetch` then fetches the whole branch into
    /// refs/patchtrack/NAME. It never makes a shallow fetch, because a
    /// shallow history would make "not found" meaningless. Trees are this
    /// machine's: an exported store leaves them out, but keeps the results.
    Tree {
        #[command(subcommand)]
        action: tree::TreeAction,
    },
    /// Look for every posted patch in the trees.
    ///
    /// Only commits after the posting that touch the posted files are read,
    /// which takes seconds even on linux-next. Matchers, strongest first:
    /// message-id (the posting's message-id in a Link: or Message-ID:
    /// trailer, which b4 adds; a message-id cited elsewhere in a commit
    /// message names some other patch and does not count); patch-id (equal
    /// `git patch-id --stable`); subject (the same subject and author, with a
    /// changed diff).
    ///
    /// Not finding a patch is stored as a result too, with the tree's commit
    /// and the time. A found commit that a later commit in the same tree
    /// reverts does not count (unless that revert was itself reverted).
    Check {
        /// Only this tree (repeatable; default: every tree)
        #[arg(short, long = "tree")]
        trees: Vec<String>,
        /// Fetch the trees that have a --url first
        #[arg(long)]
        fetch: bool,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Your b4 prep branches, against what you posted.
    ///
    /// b4 keeps its state in an empty cover commit at the base of each prep
    /// branch: the change-id, the next revision to send, and the cover
    /// message-id of every version sent. So each branch can show its
    /// family's state, and one of: "as sent in vN"; "vN+1 in preparation: K
    /// changed since vN" (compared by `git patch-id`); "draft: never sent".
    /// When reviewers gave tags the branch's commits do not carry yet, it
    /// says how many, and that `b4 trailers -u` takes them in.
    ///
    /// It looks in the repository you are in, or the ones --repo names, for
    /// branches under refs/heads/ and the b4_prefix setting ("b4/").
    Branches {
        /// A repository to look in (repeatable; default: the current directory)
        #[arg(long)]
        repo: Vec<PathBuf>,
        /// Colour (auto: on a terminal, unless NO_COLOR is set)
        #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"])]
        color: String,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Read mail into the store: mbox files (lei, b4, Thunderbird, lore's
    /// exports), single messages (.eml), Maildirs and directories of .eml
    /// files, or another patchtrack store.
    ///
    /// A file that starts with a "From " line is read as an mbox (mboxrd), an
    /// SQLite file as a store, anything else as one message. Everything in
    /// what you name is kept: a file you name is something you want to see,
    /// such as someone else's thread from `b4 mbox`. --only-followed keeps
    /// only what `source` keeps (the series of the authors you follow and your
    /// identities, and the replies in their threads), for a whole mailbox;
    /// to read local mail on every sync, see `source`. Mail already
    /// stored is left alone. The threads it shows are marked as seen in
    /// local mail. A store is merged: its mail, fetch times and
    /// tree results are added, while your settings, what you follow and
    /// your closed families stay as they are (theirs fill only what you
    /// have not set, and what they follow is taken only into a store that
    /// follows nothing).
    Import {
        /// mbox or .eml files, Maildirs, .eml directories, or store files
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Keep only the series of the authors you follow and your identities, and the replies in their threads
        #[arg(long)]
        only_followed: bool,
    },
    /// Write a copy of the store to FILE, for another machine, the web app,
    /// or (--public) for publishing.
    ///
    /// The copy leaves out this machine's own paths (local mail sources and
    /// git trees). --public also leaves out your settings, what `news` last
    /// showed you, why you closed families, and which threads came from
    /// your mailbox: what remains is mail from the archive and what
    /// patchtrack learned about it. FILE must not exist.
    Export {
        /// Where to write the copy; it must not exist
        file: PathBuf,
        /// Leave out everything that is yours rather than the archive's
        #[arg(long)]
        public: bool,
    },
    /// One line per series family: its state, title, version and last activity.
    ///
    /// A family is every version of one series, the nearest email has to a
    /// pull request: one sender's postings with the same b4 change-id, or
    /// else the same title. Its state spans its versions. A coloured circle
    /// shows the state: purple merged, green applied/queued/in-next, yellow
    /// commented, cyan reviewed, blue discussed, grey waiting. The shape
    /// says the same without colour: ● a state, ◐ partially applied, ○
    /// nothing known yet or superseded, ⊘ closed. Colour is on for a
    /// terminal unless NO_COLOR is set.
    ///
    /// Below each line is the evidence for its state. -v adds addresses,
    /// message-ids and the time the thread was last fetched; --json gives
    /// everything as JSON. --all-versions shows each posting's own state.
    ///
    /// Filters combine. "What have I put in front of this maintainer that is
    /// still open?" is `patchtrack list --open --to maintainer@example.org`.
    List(views::ListArgs),
    /// What changed since you last looked: GitHub's notifications for email.
    ///
    /// Every family is compared with a snapshot of what `news` showed last
    /// time. It reports new series and new versions of existing ones; state
    /// changes, e.g. "reviewed → applied" or "in-next → merged"; and each new
    /// reply, with what it did: its tag, its "Applied …" line, or the first
    /// line of an inline comment.
    ///
    /// Then it takes a new snapshot, so the same change is shown once; --peek
    /// leaves the snapshot alone. The first run only records a baseline.
    ///
    /// --since WHEN compares with the mail as it stood at that time instead
    /// of with your last look, and implies --peek. Only mail carries its own
    /// dates, so tree movements (queued, in-next, merged) appear through the
    /// ordinary `news`.
    News {
        /// Show the changes without marking them seen
        #[arg(long)]
        peek: bool,
        /// Compare with the mail as of this time (N.days.ago or an ISO date) instead of your last look
        #[arg(long)]
        since: Option<String>,
        /// Colour (auto: on a terminal, unless NO_COLOR is set)
        #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"])]
        color: String,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Open series that need you: a reply, a ping, or a RESEND.
    ///
    /// A reply is owed when someone asked or said something after your last
    /// message (a bare tag or an "Applied" asks nothing). A ping is due a week
    /// or more after anyone last wrote; a RESEND, two weeks or more with no
    /// human reply at all. The defaults follow the kernel's
    /// submitting-patches.rst: "wait for a minimum of one week before
    /// resubmitting or pinging reviewers", and it is "ok to resend … after a
    /// couple of weeks". Your own pings restart the clock. The ping_days and
    /// resend_days settings change the thresholds.
    ///
    /// With a mainline tree (see `tree`), its repository's tags show whether
    /// the merge window is open (a final vX.Y with no -rc1 after it). Pings
    /// and RESENDs then say "after -rc1": the kernel asks for patience
    /// "during busy times like merge windows". A final tag over three weeks
    /// old reads as stale tags, not an open window. --ignore-merge-window,
    /// or the merge_window setting, turns this off.
    ///
    /// --draft DIR writes each ping as a plain-text .eml, threaded to the
    /// series and addressed to its recipients, for you to edit and send, and
    /// keeps it as a draft too (see `drafts`, `send`). A RESEND draft names
    /// the `b4 send --resend vN` command. Nothing is sent from here.
    Nudge(views::NudgeArgs),
    /// Write a reply: to a series' cover letter, one of its patches, or any
    /// message in it, addressed and quoted as b4 would.
    ///
    /// The reply goes to the sender and everyone else on To, Cc as it was,
    /// never to you, threaded under what it answers. --tag gives a review
    /// tag (Reviewed-by, Acked-by, Tested-by, Reported-by) signed with your
    /// name and first identity (the name and identities settings, or git's
    /// user.name and user.email). With --message or --tag, the quote is a
    /// short one, as b4 ty writes; with neither, your editor (git's) opens
    /// on the whole message quoted, to answer inline. The reply is kept as
    /// a draft and printed: nothing is sent until `send`.
    Reply {
        /// Part of a title or root message-id; or a message-id in the store
        series: String,
        /// Answer this patch of the series (its number) instead of the cover
        #[arg(short = 'n', long)]
        patch: Option<u32>,
        /// Give this tag, e.g. Reviewed-by
        #[arg(short, long)]
        tag: Option<String>,
        /// What you say, below a short quote
        #[arg(short, long)]
        message: Option<String>,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Thank an author for applied patches, as `b4 ty` writes it.
    ///
    /// To the series' cover letter (or its only patch): "Applied, thanks!"
    /// and each patch with the commit a tree check found for it (see
    /// `check`), or "(no commit info)"; "(subset)" in the subject when only
    /// some were found. b4 ty knows only what `b4 shazam` applied; this
    /// knows every commit `check` found, however it landed. Kept as a draft
    /// and printed: nothing is sent until `send`.
    Thanks {
        /// Part of a title or root message-id
        series: String,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// List the mail you composed, show one, or remove one.
    ///
    /// Drafts are kept in the store until you remove them, sent or not,
    /// so a full export carries them and a public one does not. A draft is
    /// named by its Message-ID, or any part of it that is unique.
    Drafts {
        #[command(subcommand)]
        action: Option<DraftsCommand>,
    },
    /// Send a draft with git send-email, after showing it whole.
    ///
    /// patchtrack prints the message and sends it only when you type "yes".
    /// It goes through `git send-email`, so your sendemail.* configuration
    /// (SMTP server, credentials, or a sendmail command) applies, and
    /// patchtrack never holds a password. Recipients are the draft's To and
    /// Cc and no one else: none from your configuration, none from the
    /// body's tags. git send-email is the git-email package on Fedora and
    /// Debian, and comes with Git for Windows. A sent message is kept in
    /// the store as your mail.
    Send {
        /// The draft's Message-ID, or a unique part of it
        draft: String,
    },
    /// Everything about one series: patches, tags, tree results and replies.
    ///
    /// The series is named by part of its title or root message-id; every
    /// matching version is shown, after a line for its family.
    Show {
        /// Part of a root message-id or title
        series: String,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Everyone who replied to the series, and how.
    ///
    /// One line per person: the last time they wrote, how many series they
    /// touched, replies, tags given (Reviewed-by, Acked-by, a profile's
    /// approval), inline reviews, and applies. Bots are left out unless you
    /// pass --bots.
    People {
        #[command(flatten)]
        scope: views::Scope,
        /// Include bots
        #[arg(long)]
        bots: bool,
        /// Everything, as JSON
        #[arg(long)]
        json: bool,
    },
    /// Close a series family you abandoned, or one that went in some other way.
    ///
    /// For example, a series folded into someone else's. A closed family
    /// leaves `list` (`list --closed` shows it with your reason), and
    /// `reopen` undoes it. The series is named by part of its title or root
    /// message-id; a query that matches several families is refused unless
    /// you pass --all.
    Close {
        /// Part of a root message-id or title
        series: String,
        /// Why, shown with the family
        #[arg(long)]
        reason: Option<String>,
        /// Every family that matches
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Undo `close`: the family comes back to `list`.
    ///
    /// The series is named as for `close`; a query that matches several
    /// families is refused unless you pass --all.
    Reopen {
        /// Part of a root message-id or title
        series: String,
        /// Every family that matches
        #[arg(long)]
        all: bool,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Read a series' conversation in the terminal.
    ///
    /// Messages come in reply order, indented by depth, through $PAGER (or
    /// `less -RFX`; with neither, straight to the terminal). The cover letter
    /// is shown up to b4's footer. Each patch shows its message, with its
    /// diff summarised unless --diffs is given. In replies, long quotes
    /// collapse to the few lines each comment answers, and signatures are
    /// cut.
    ///
    /// The series is named by part of its title or root message-id. Without
    /// --all, the latest version of each matching family is shown.
    Read {
        /// Part of a root message-id or title
        series: String,
        /// Every matching version, not only the latest
        #[arg(long)]
        all: bool,
        /// Show the patches' diffs
        #[arg(long)]
        diffs: bool,
        /// Colour (auto: on a terminal, unless NO_COLOR is set)
        #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"])]
        color: String,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// What changed between two versions of a series.
    ///
    /// By default, the latest version against the one before; --from and
    /// --to choose others. The output starts with the author's own "Changes
    /// in vN" from the cover letter. Patches are then paired by title, then
    /// by an identical diff, then by a similar title, and marked as `git
    /// range-diff` does: = unchanged, ! changed, < dropped, > added. A
    /// changed pair shows the diff of its commit message and the diff of
    /// its diffs.
    ///
    /// Hunk line numbers, git's function context and index lines are
    /// ignored, so a rebase alone is not a change. It works from the mail
    /// alone; for a true range-diff against a base tree, `b4 diff` does
    /// that.
    Diff {
        /// Part of a root message-id or title
        series: String,
        /// The older version (default: the one before --to)
        #[arg(long = "from", value_name = "N")]
        from: Option<u32>,
        /// The newer version (default: the latest)
        #[arg(long, value_name = "M")]
        to: Option<u32>,
        /// Colour (auto: on a terminal, unless NO_COLOR is set)
        #[arg(long, default_value = "auto", value_parser = ["auto", "always", "never"])]
        color: String,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Browse your series interactively.
    ///
    /// In the list: j/k move, / filters by title or state, f shows open
    /// series only, Enter reads a series, d shows its version diff, o opens
    /// it on the archive, ? lists the keys, and q goes back or quits.
    Tui {
        /// Include families you closed
        #[arg(long)]
        closed: bool,
        #[command(flatten)]
        scope: views::Scope,
    },
    /// Serve this machine's store to the web app: it reads, syncs and acts here.
    ///
    /// The web app keeps nothing of its own: it shows the store of the
    /// machine the bridge runs on, and every change it makes (what you
    /// follow, settings, drafts, closing a series) is made there. The bridge
    /// syncs that store by itself: when it starts, every `sync_every`
    /// minutes (a setting, 30 by default), when you follow something, and
    /// when the app asks. Mail dropped on the app is imported here too.
    ///
    /// The bridge listens on 127.0.0.1 only, answers only the app's origin
    /// (and those --origin adds), and requires the pairing token the link
    /// below carries. It prints a link that opens the app and pairs it. Your
    /// browser will ask once whether the site may reach this computer's local
    /// network: that is the bridge.
    ///
    /// From anywhere, your phone say: make the bridge reachable over HTTPS
    /// with a tunnel or a proxy that forwards to it on this machine
    /// (`tailscale serve`, Cloudflare Tunnel, Caddy), and give that address
    /// as --public-url. The bridge still listens on 127.0.0.1 only; it
    /// answers requests naming that host too, and prints a second link,
    /// which pairs the app with that address after you confirm it there.
    /// Anyone who has the token can use the bridge: keep the link to
    /// yourself, or make a new token.
    ///
    /// With --allow-send, the app can send a draft through this machine's
    /// `git send-email` (only mail from your identities); with --repo, it
    /// lists your b4 prep branches there and runs `b4 send`, `b4 send
    /// --resend` and `b4 trailers -u` on them (each first as a dry run, in
    /// a worktree of its own, so your checkout is never switched).
    ///
    /// The token is kept in the store (never exported), so a paired app
    /// stays paired; --new-token makes a new one.
    Bridge {
        /// Listen on this port, on 127.0.0.1
        #[arg(long, default_value_t = 47130)]
        port: u16,
        /// The web app's address: its origin may use the bridge, and the pairing link opens it
        #[arg(long, default_value = "https://ryanabx.github.io/patchtrack/")]
        app: String,
        /// Another origin that may use the bridge, e.g. a development server (repeatable)
        #[arg(long)]
        origin: Vec<String>,
        /// Make a new pairing token; the app must be paired again
        #[arg(long)]
        new_token: bool,
        /// Where a tunnel or proxy makes this bridge reachable over HTTPS, e.g. https://desktop.example.ts.net
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        #[command(flatten)]
        allow: Allow,
    },
    /// Serve the web app and this machine's store from your own server:
    /// patchtrack on your own domain.
    ///
    /// To use the published app with your desktop's bridge from anywhere,
    /// your phone say, `serve` is not needed: see `bridge --public-url`.
    /// `serve` is for hosting the app itself.
    ///
    /// It serves the built web app (--app-dir: the release's web archive,
    /// unpacked) and, at the same address, what the bridge serves: the
    /// store, synced on its own as the bridge syncs it, and with
    /// --allow-send and --repo, sending drafts and running b4. The app and
    /// its API share an origin, so no browser asks about the local network
    /// and no CORS is needed.
    ///
    /// It speaks plain HTTP: put it behind a proxy that speaks HTTPS (Caddy,
    /// nginx). --public-url is the address people open; requests naming
    /// another host are refused. Every API request needs the token the
    /// printed link pairs a browser with; the token is kept in the store,
    /// --new-token makes a new one.
    Serve {
        /// Listen here (behind your HTTPS proxy)
        #[arg(long, default_value = "127.0.0.1:8730", value_name = "ADDR:PORT")]
        listen: String,
        /// The address people open, e.g. https://patchtrack.example.org
        #[arg(long, value_name = "URL")]
        public_url: String,
        /// The built web app to serve at its root
        #[arg(long, value_name = "DIR")]
        app_dir: PathBuf,
        /// Make a new pairing token; browsers must be paired again
        #[arg(long)]
        new_token: bool,
        #[command(flatten)]
        allow: Allow,
    },
    /// Write the book's generated pages (commands, settings) under BOOK.
    ///
    /// For building the documentation: see book/README.md. Each command's
    /// page is its help text, so the two cannot drift apart.
    #[command(hide = true)]
    BookPages { book: PathBuf },
    /// Open a series' thread on the archive in your browser.
    ///
    /// The series is named by part of its title or root message-id; the
    /// latest version of each matching family is opened, or every version
    /// with --all. --print prints the URL instead.
    Open {
        /// Part of a root message-id or title
        series: String,
        /// Every matching version, not only the latest
        #[arg(long)]
        all: bool,
        /// Print the URL instead of opening it
        #[arg(long)]
        print: bool,
        #[command(flatten)]
        scope: views::Scope,
    },
}

/// In the platform's local data directory: it can be large, so on Windows
/// it is not in the roaming profile.
fn default_db() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("patchtrack").join("store.sqlite3")
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// An HTTP client that reports every status as an answer, with our User-Agent.
fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// The host's half of a request: HTTP, nothing else.
fn fetch(agent: &ureq::Agent, req: &Request) -> Outcome {
    let sent = match req.method {
        Method::Get => agent.get(&req.url).call(),
        Method::Post => agent.post(&req.url).send_empty(),
    };
    match sent {
        Ok(mut resp) => {
            let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            let (content_type, retry_after) = (header("content-type").unwrap_or_default(), header("retry-after"));
            let status = resp.status().as_u16();
            match resp.body_mut().with_config().limit(1 << 31).read_to_vec() {
                Ok(body) => Outcome::Response { status, content_type, retry_after, body },
                Err(e) => Outcome::Failed(e.to_string()),
            }
        }
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

/// The authors you follow, in order, each once.
pub(crate) fn followed(store: &Store) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut authors: Vec<String> =
        store.subscriptions()?.into_iter().filter(|s| s.kind == Follow::Author).map(|s| s.value).collect();
    authors.dedup();
    Ok(authors)
}

/// "https://host" of a URL.
fn origin_of(url: &str) -> String {
    url.split_once("://")
        .map_or(url.to_string(), |(scheme, rest)| format!("{scheme}://{}", rest.split('/').next().unwrap_or(rest)))
}

/// What `bridge` and `serve` share: the token, who may call, what is allowed.
struct Server {
    /// Where the token is kept in the store's machine table.
    token_key: &'static str,
    new_token: bool,
    port: u16,
    origins: Vec<String>,
    hosts: Vec<String>,
    app_dir: Option<PathBuf>,
    allow: Allow,
}

impl Server {
    fn bridge(&self, store: &mut Store, settings: &Settings) -> Result<bridge::Bridge, Box<dyn std::error::Error>> {
        let token = match store.machine(self.token_key)? {
            Some(t) if !self.new_token => t,
            _ => {
                let t = bridge::new_token();
                store.set_machine(self.token_key, &t)?;
                t
            }
        };
        if self.allow.allow_send && settings.identities.is_empty() {
            return Err("--allow-send sends only mail from your identities: set them first \
                        (patchtrack settings set identities '[\"you@example.org\"]')"
                .into());
        }
        Ok(bridge::Bridge {
            port: self.port,
            token,
            origins: self.origins.clone(),
            hosts: self.hosts.clone(),
            app_dir: self.app_dir.clone(),
        })
    }

    /// Serve until the process ends, syncing the store on its own meanwhile.
    fn run(&self, b: bridge::Bridge, listener: std::net::TcpListener, db: &std::path::Path) -> Result<(), String> {
        let repos: Vec<PathBuf> =
            self.allow.repos.iter().map(|r| std::path::absolute(r).unwrap_or_else(|_| r.clone())).collect();
        let api = api::Api::new(db, repos, self.allow.allow_send)?;
        std::thread::scope(|scope| {
            scope.spawn(|| api.live.run(api.db()));
            b.serve(listener, &api);
        });
        Ok(())
    }
}

/// What to sync: -a and -l replace what is followed with authors in
/// inboxes; with neither, every subscription, or else every author synced
/// before.
fn searches(
    store: &Store,
    authors: &[String],
    lists: &[String],
) -> Result<Vec<Subscription>, Box<dyn std::error::Error>> {
    let lists = if lists.is_empty() { vec!["all".to_string()] } else { lists.to_vec() };
    let cross = |authors: &[String]| -> Result<Vec<Subscription>, String> {
        lists.iter().flat_map(|l| authors.iter().map(|a| Subscription::new(Follow::Author, a, l))).collect()
    };
    if !authors.is_empty() {
        return Ok(cross(authors)?);
    }
    let subs = store.subscriptions()?;
    if lists.len() == 1 && lists[0] == "all" && !subs.is_empty() {
        return Ok(subs);
    }
    let followed = followed(store)?;
    let authors = if followed.is_empty() { store.authors()? } else { followed };
    if authors.is_empty() {
        return Err("nothing to sync: follow an author (`patchtrack follow EMAIL`), a list (--inbox), a search \
                    (--query) or a thread (--thread), or pass -a EMAIL"
            .into());
    }
    Ok(cross(&authors)?)
}

/// Store local mail and mark the threads it shows; returns the message-ids
/// that were new.
pub(crate) fn store_local(
    store: &mut Store,
    settings: &Settings,
    raws: &[Vec<u8>],
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    let known = store.msgids()?;
    let new: HashSet<String> =
        raws.iter().filter_map(|r| Head::parse(r)).map(|h| h.msgid).filter(|id| !known.contains(id)).collect();
    store.add_all(raws.iter().map(Vec::as_slice))?;
    let facts = store.facts()?;
    let all = series::build(&store.messages()?, &facts, &settings.rules);
    let now = mail::utc(i64::try_from(now_ms() / 1000)?);
    store.mark_local(patchtrack_core::local::shown_roots(&all, &new, &facts.local), &now)?;
    Ok(new)
}

/// Read the local mail sources, keeping what matters to these authors;
/// returns the message-ids that were new.
fn read_sources(
    store: &mut Store,
    settings: &Settings,
    authors: &[String],
    log: &mut dyn FnMut(String),
) -> Result<HashSet<String>, Box<dyn std::error::Error>> {
    let sources = store.sources()?;
    if sources.is_empty() {
        return Ok(HashSet::new());
    }
    let now = mail::utc(i64::try_from(now_ms() / 1000)?);
    let (mut raws, mut read) = (vec![], vec![]);
    for (path, read_at) in sources {
        let since = read_at.as_deref().and_then(series::parse_utc);
        match local::read(std::path::Path::new(&path), since) {
            Ok(msgs) => {
                raws.extend(msgs);
                read.push(path);
            }
            Err(e) => log(format!("patchtrack: {path}: {e}")),
        }
    }
    let mut authors: Vec<String> = authors.iter().chain(&settings.identities).map(|a| a.to_lowercase()).collect();
    authors.sort();
    authors.dedup();
    let keep = patchtrack_core::local::relevant(&raws, &store.msgids()?, &authors);
    let kept: Vec<Vec<u8>> = keep.into_iter().map(|i| std::mem::take(&mut raws[i])).collect();
    let new = store_local(store, settings, &kept)?;
    for path in read {
        store.mark_read(&path, &now)?;
    }
    log(format!("local mail: {} messages read, {} new", raws.len(), new.len()));
    Ok(new)
}

/// Runs a sync; false if anything failed.
fn cmd_sync(store: &mut Store, settings: &Settings, args: SyncArgs) -> Result<bool, Box<dyn std::error::Error>> {
    let searches = searches(store, &args.authors, &args.lists)?;
    sync_store(store, settings, &args, searches, &mut |line| eprintln!("{line}"))
}

/// Sync these subscriptions, reading local mail first unless told not to,
/// each line of what it does to `log`; false if anything failed.
pub(crate) fn sync_store(
    store: &mut Store,
    settings: &Settings,
    args: &SyncArgs,
    searches: Vec<Subscription>,
    log: &mut dyn FnMut(String),
) -> Result<bool, Box<dyn std::error::Error>> {
    let local_new = if args.no_local {
        HashSet::new()
    } else {
        let authors: Vec<String> =
            searches.iter().filter(|s| s.kind == Follow::Author).map(|s| s.value.clone()).collect();
        read_sources(store, settings, &authors, log)?
    };
    if args.offline {
        return Ok(true);
    }
    let now = i64::try_from(now_ms() / 1000)?;
    let agent = http_agent();
    let mut ok = true;
    // One sync per archive, one after the other: each is polite to its own.
    for (key, searches) in by_archive(searches) {
        let mut plan = settings.plan(searches, now)?;
        plan.archive = settings.archive_of(&key).to_string();
        plan.archive_key = key.clone();
        if let Some(archive) = args.archive.as_ref().filter(|_| key.is_empty()) {
            plan.archive = archive.trim_end_matches('/').to_string();
        }
        if !key.is_empty() {
            log(format!("{}:", plan.archive));
        }
        plan.since = args.since.as_ref().map(|s| sync::since(s, now)).transpose()?;
        plan.full = args.full;
        plan.refresh_days = args.refresh_days.unwrap_or(plan.refresh_days);
        plan.window_days = args.window_days.unwrap_or(plan.window_days);
        plan.delay_ms = (args.delay.max(0.0) * 1000.0) as u64;
        plan.threads = !args.no_threads;
        let known = Known {
            messages: store.messages()?,
            last_sync: store.last_syncs()?,
            facts: store.facts()?,
            rules: settings.rules.clone(),
            local_new: if key.is_empty() { local_new.clone() } else { HashSet::new() },
        };
        let mut sync = sync::Sync::new(plan, known);
        while let Some(effect) = sync.next(now_ms()) {
            match effect {
                Effect::Fetch(req) => {
                    std::thread::sleep(Duration::from_millis(req.not_before.saturating_sub(now_ms())));
                    let outcome = fetch(&agent, &req);
                    sync.receive(req.id, outcome, now_ms());
                }
                Effect::Log(line) => log(line),
                other => store.apply(&other)?,
            }
        }
        ok &= !sync.failed();
    }
    Ok(ok)
}

/// Subscriptions grouped by archive, the archive setting's ("") first; it
/// comes even with none, as threads may be owed there.
fn by_archive(subs: Vec<Subscription>) -> Vec<(String, Vec<Subscription>)> {
    let mut out: Vec<(String, Vec<Subscription>)> = vec![(String::new(), vec![])];
    for s in subs {
        match out.iter_mut().find(|(k, _)| *k == s.archive) {
            Some((_, v)) => v.push(s),
            None => out.push((s.archive.clone(), vec![s])),
        }
    }
    out
}

fn cmd_project(
    store: &mut Store,
    settings: &Settings,
    action: Option<ProjectAction>,
) -> Result<(), Box<dyn std::error::Error>> {
    use patchtrack_core::projects::{self, Project};
    let mut out = std::io::stdout().lock();
    let save = |store: &mut Store, list: &[Project]| -> Result<(), Box<dyn std::error::Error>> {
        let json = serde_json::to_string(list)?;
        Ok(store.set_setting("projects", &settings::validate("projects", &json)?)?)
    };
    match action.unwrap_or(ProjectAction::List) {
        ProjectAction::List => {
            for p in &settings.projects {
                let lists = if p.lists.is_empty() { "every list".to_string() } else { p.lists.join(", ") };
                writeln!(out, "{}  {}  ({lists})", p.name, p.archive)?;
                if !p.recipients.is_empty() {
                    writeln!(out, "    mail to {}", p.recipients.join(", "))?;
                }
            }
            let more: Vec<&str> = projects::PRESETS
                .iter()
                .map(|p| p.0)
                .filter(|n| !settings.projects.iter().any(|p| p.name.eq_ignore_ascii_case(n)))
                .collect();
            if settings.projects.is_empty() {
                writeln!(out, "No projects: everything is on {}.", settings.archive)?;
            }
            writeln!(out, "Built in, to add by name: {}", more.join(", "))?;
        }
        ProjectAction::Add { name, archive, lists, recipients } => {
            let mut p = match (projects::preset(&name), &archive) {
                (Some(p), None) => p,
                (_, Some(a)) => Project { name: name.clone(), archive: a.clone(), lists: vec![], recipients: vec![] },
                (None, None) => {
                    return Err(format!("{name:?} is not built in: give its --archive (and its -l lists)").into());
                }
            };
            if !lists.is_empty() {
                p.lists = lists;
            }
            if !recipients.is_empty() {
                p.recipients = recipients;
            }
            let mut all = settings.projects.clone();
            all.retain(|q| !q.name.eq_ignore_ascii_case(&p.name));
            all.push(p);
            save(store, &all)?;
            eprintln!("added {name}; follow in it with `patchtrack follow -p {name} ...`");
        }
        ProjectAction::Rm { name } => {
            let mut all = settings.projects.clone();
            let before = all.len();
            all.retain(|p| !p.name.eq_ignore_ascii_case(&name));
            if all.len() == before {
                return Err(project(settings, &name).err().unwrap_or_default().into());
            }
            save(store, &all)?;
        }
    }
    Ok(())
}

fn cmd_follow(store: &mut Store, settings: &Settings, what: &Followed) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    if what.is_empty() {
        for s in store.subscriptions()? {
            let value = if s.kind == Follow::Query { format!("{:?}", s.value) } else { s.value.clone() };
            let r#in =
                if s.list == "all" || s.kind == Follow::List { String::new() } else { format!(" in {}", s.list) };
            let on = if s.archive.is_empty() { String::new() } else { format!(" on {}", s.archive) };
            writeln!(out, "{} {value}{in}{on}", s.kind.as_str())?;
        }
        return Ok(());
    }
    let at = mail::utc(i64::try_from(now_ms() / 1000)?);
    for s in what.subscriptions(settings)? {
        if store.subscribe(&s, &at)? {
            eprintln!("following {}", s.label());
            if matches!(s.kind, Follow::List | Follow::Query) {
                eprintln!("  its first sync searches back to the list_since setting, and fetches each thread it finds");
            }
        } else {
            eprintln!("already following {}", s.label());
        }
    }
    Ok(())
}

fn cmd_unfollow(store: &mut Store, settings: &Settings, what: &Followed) -> Result<bool, Box<dyn std::error::Error>> {
    if what.is_empty() {
        return Err("name what to stop following: addresses, --inbox, --query or --thread".into());
    }
    let mut all = true;
    for s in what.subscriptions(settings)? {
        // An author or a query with no -l (nor project): in every list.
        let any_list =
            what.lists.is_empty() && what.project.is_none() && matches!(s.kind, Follow::Author | Follow::Query);
        let removed = store.unsubscribe(s.kind, &s.value, (!any_list).then_some(s.list.as_str()), Some(&s.archive))?;
        if removed == 0 {
            let label = if any_list { s.label().trim_end_matches(" in all").to_string() } else { s.label() };
            eprintln!("patchtrack: not following {label}");
            all = false;
        }
    }
    Ok(all)
}

fn cmd_settings(store: &mut Store, action: Option<SettingsAction>) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    match action {
        Some(SettingsAction::Set { key, value }) => store.set_setting(&key, &settings::validate(&key, &value)?)?,
        Some(SettingsAction::Unset { key }) => {
            if !store.unset_setting(&key)? {
                eprintln!("{key} was not set");
            }
        }
        None => {
            let set: std::collections::BTreeMap<String, String> = store.settings()?.into_iter().collect();
            for k in settings::KEYS {
                match set.get(k.name) {
                    Some(v) => writeln!(out, "{} = {v}", k.name)?,
                    None => writeln!(out, "{} = {}  # default", k.name, k.default)?,
                }
            }
            for (k, v) in set.iter().filter(|(k, _)| settings::key(k).is_none()) {
                writeln!(out, "{k} = {v}  # not known to this patchtrack; ignored")?;
            }
        }
    }
    Ok(())
}

fn cmd_source(store: &mut Store, action: SourceAction) -> Result<bool, Box<dyn std::error::Error>> {
    let mut out = std::io::stdout().lock();
    match action {
        SourceAction::Add { path } => {
            let path = std::path::absolute(&path)?;
            if !path.exists() {
                return Err(format!("{}: no such file or directory", path.display()).into());
            }
            let at = mail::utc(i64::try_from(now_ms() / 1000)?);
            if !store.add_source(&path.to_string_lossy(), &at)? {
                eprintln!("{} is a source already", path.display());
            }
        }
        SourceAction::List => {
            for (path, read_at) in store.sources()? {
                writeln!(out, "{path}  {}", read_at.map_or("never read".to_string(), |t| format!("read {t}")))?;
            }
        }
        SourceAction::Rm { path } => {
            let path = std::path::absolute(&path)?;
            if !store.remove_source(&path.to_string_lossy())? {
                eprintln!("patchtrack: {} is not a source", path.display());
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn run(cli: Cli) -> Result<bool, Box<dyn std::error::Error>> {
    if let Command::BookPages { book } = &cli.command {
        book::write(command(), book)?;
        return Ok(true);
    }
    let path = cli.db.unwrap_or_else(default_db);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut store = Store::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    store.reparse()?;
    // `settings` itself must work with a bad value stored: it is how to fix one.
    if let Command::Settings { action } = cli.command {
        cmd_settings(&mut store, action)?;
        return Ok(true);
    }
    let (settings, warnings) = Settings::load(store.settings()?)?;
    for w in warnings {
        eprintln!("patchtrack: {w}");
    }
    match cli.command {
        Command::Sync(args) => return cmd_sync(&mut store, &settings, args),
        Command::Project { action } => cmd_project(&mut store, &settings, action)?,
        Command::Follow { what } => cmd_follow(&mut store, &settings, &what)?,
        Command::Unfollow { what } => return cmd_unfollow(&mut store, &settings, &what),
        Command::Settings { .. } | Command::BookPages { .. } => unreachable!("handled above"),
        Command::Source { action } => return cmd_source(&mut store, action),
        Command::Tree { action } => return tree::cmd_tree(&mut store, action),
        Command::Branches { repo, color, scope } => {
            return tree::cmd_branches(&store, &settings, &scope, &repo, &color);
        }
        Command::Check { trees, fetch, scope } => return tree::cmd_check(&mut store, &settings, &scope, &trees, fetch),
        Command::Import { files, only_followed } => {
            let mut authors: Vec<String> =
                followed(&store)?.into_iter().chain(settings.identities.iter().cloned()).collect();
            authors.sort();
            authors.dedup();
            if only_followed && authors.is_empty() {
                return Err(
                    "--only-followed: you follow no one and have no identities (see `follow`, `settings`)".into()
                );
            }
            for file in files {
                let raw = if file.is_dir() {
                    vec![]
                } else {
                    std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?
                };
                if raw.starts_with(b"SQLite format 3\0") {
                    let new = store.import(&file.to_string_lossy()).map_err(|e| format!("{}: {e}", file.display()))?;
                    eprintln!("{}: a store, {new} new message{}", file.display(), if new == 1 { "" } else { "s" });
                    continue;
                }
                let mut msgs = if file.is_dir() { local::read(&file, None)? } else { local::messages(raw) };
                let read = msgs.len();
                if only_followed {
                    let keep = patchtrack_core::local::relevant(&msgs, &store.msgids()?, &authors);
                    msgs = keep.into_iter().map(|i| std::mem::take(&mut msgs[i])).collect();
                }
                let new = store_local(&mut store, &settings, &msgs)?.len();
                let s = if read == 1 { "" } else { "s" };
                let kept = if only_followed { format!(", {} kept", msgs.len()) } else { String::new() };
                eprintln!("{}: {read} message{s}{kept}, {new} new", file.display());
            }
        }
        Command::Export { file, public } => {
            if file.exists() {
                return Err(format!("{}: exists already", file.display()).into());
            }
            store.export(&file.to_string_lossy(), if public { Export::Public } else { Export::Full })?;
        }
        Command::List(args) => views::cmd_list(&store, &settings, &args)?,
        Command::Show { series, scope } => views::cmd_show(&store, &settings, &scope, &series)?,
        Command::People { scope, bots, json } => views::cmd_people(&store, &settings, &scope, bots, json)?,
        Command::Close { series, reason, all, scope } => {
            views::cmd_close(&mut store, &settings, &scope, &series, reason.as_deref(), all, true)?
        }
        Command::Reopen { series, all, scope } => {
            views::cmd_close(&mut store, &settings, &scope, &series, None, all, false)?
        }
        Command::Nudge(args) => views::cmd_nudge(&mut store, &settings, &args)?,
        Command::Reply { series, patch, tag, message, scope } => {
            let args = compose::ReplyArgs { query: &series, patch, tag: tag.as_deref(), text: message.as_deref() };
            compose::cmd_reply(&mut store, &settings, &scope, args)?
        }
        Command::Thanks { series, scope } => compose::cmd_thanks(&mut store, &settings, &scope, &series)?,
        Command::Drafts { action } => {
            let action = match action {
                None | Some(DraftsCommand::List) => compose::DraftsAction::List,
                Some(DraftsCommand::Show { draft }) => compose::DraftsAction::Show(draft),
                Some(DraftsCommand::Rm { draft }) => compose::DraftsAction::Remove(draft),
            };
            compose::cmd_drafts(&mut store, action)?
        }
        Command::Send { draft } => return compose::cmd_send(&mut store, &settings, &draft),
        Command::Bridge { port, app, origin, new_token, public_url, allow } => {
            // A page served over https may reach plain http only on its own computer.
            let public = match public_url {
                Some(u) if !settings::archive_url(&u).is_ok_and(|u| u.starts_with("https://")) => {
                    return Err(format!("--public-url {u:?}: give the https address your tunnel or proxy makes").into());
                }
                Some(u) => Some(u.trim_end_matches('/').to_string()),
                None => None,
            };
            let app_origin = origin_of(&app);
            let mut origins = vec![app_origin];
            origins.extend(origin.iter().map(|o| o.trim_end_matches('/').to_string()));
            let listener = std::net::TcpListener::bind(("127.0.0.1", port))
                .map_err(|e| format!("127.0.0.1:{port}: {e} (is a bridge already running? --port picks another)"))?;
            let server = Server {
                token_key: "bridge-token",
                new_token,
                port,
                origins,
                hosts: public.iter().map(|u| origin_of(u).trim_start_matches("https://").to_string()).collect(),
                app_dir: None,
                allow,
            };
            let b = server.bridge(&mut store, &settings)?;
            eprintln!("patchtrack bridge on http://127.0.0.1:{port}, for {}", b.origins.join(", "));
            eprintln!(
                "Pair the web app by opening:\n\n  {app}#/bridge?url=http://127.0.0.1:{port}&token={}\n",
                b.token
            );
            if let Some(public) = &public {
                eprintln!(
                    "From anywhere, through {public} (keep this link to yourself):\n\n  {app}#/bridge?url={public}&token={}\n",
                    b.token
                );
            }
            server.run(b, listener, &std::path::absolute(&path)?)?;
        }
        Command::Serve { listen, public_url, app_dir, new_token, allow } => {
            let public =
                settings::archive_url(&public_url).map_err(|_| format!("{public_url:?} is not an http(s) URL"))?;
            if !app_dir.join("index.html").exists() {
                return Err(format!(
                    "{}: no index.html; give the built web app (the release's web archive, unpacked)",
                    app_dir.display()
                )
                .into());
            }
            let listener = std::net::TcpListener::bind(&listen).map_err(|e| format!("{listen}: {e}"))?;
            let port = listener.local_addr()?.port();
            let host = public.split_once("://").map_or(public.as_str(), |(_, h)| h).to_string();
            let server = Server {
                token_key: "serve-token",
                new_token,
                port,
                origins: vec![origin_of(&public)],
                hosts: vec![host],
                app_dir: Some(app_dir),
                allow,
            };
            let b = server.bridge(&mut store, &settings)?;
            if !public.starts_with("https://") {
                eprintln!("patchtrack: {public} is not https: put it behind a proxy that speaks HTTPS");
            }
            eprintln!("patchtrack serve on {listen}, as {public}");
            eprintln!("Pair a browser by opening:\n\n  {public}/#/bridge?url={public}&token={}\n", b.token);
            server.run(b, listener, &std::path::absolute(&path)?)?;
        }
        Command::Tui { closed, scope } => {
            use std::io::IsTerminal;
            if !std::io::stdout().is_terminal() {
                return Err("tui needs a terminal".into());
            }
            let mut families = views::load(&store, &settings, &scope)?;
            families.retain(|f| closed || f.closed.is_none());
            let model = tui::Model::new(families, store.messages()?, &settings.archive);
            tui::run(model, |url| open::that(url).is_ok())?;
        }
        Command::News { peek, since, color, scope } => {
            views::cmd_news(&mut store, &settings, &scope, peek, since.as_deref(), &color)?
        }
        Command::Read { series, all, diffs, color, scope } => {
            views::cmd_read(&store, &settings, &scope, &series, all, diffs, &color)?
        }
        Command::Diff { series, from, to, color, scope } => {
            views::cmd_diff(&store, &settings, &scope, &series, from, to, &color)?
        }
        Command::Open { series, all, print, scope } => views::cmd_open(&store, &settings, &scope, &series, all, print)?,
    }
    Ok(true)
}

/// Every setting with its default and what it does, for `settings --help`.
fn settings_help() -> String {
    let mut out = String::from("Settings:\n");
    for k in settings::KEYS {
        out += &format!("  {} ({}; default {})\n          {}\n", k.name, k.shape, k.default, k.doc);
    }
    out
}

/// The command line, with help that is generated rather than written.
fn command() -> clap::Command {
    Cli::command().mut_subcommand("settings", |c| c.after_long_help(settings_help()))
}

fn main() -> ExitCode {
    let cli = match Cli::from_arg_matches(&command().get_matches()) {
        Ok(cli) => cli,
        Err(e) => e.exit(),
    };
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        // Whoever read the output stopped (`patchtrack list | head`): nothing is wrong.
        Err(e) if e.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) => {
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("patchtrack: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn follow(store: &mut Store, who: &str, list: &str) {
        store.subscribe(&Subscription::new(Follow::Author, who, list).unwrap(), "t").unwrap();
    }

    /// Author subscriptions, (address, inbox) each.
    fn pairs(v: &[(&str, &str)]) -> Vec<Subscription> {
        v.iter().map(|(a, l)| Subscription::new(Follow::Author, a, l).unwrap()).collect()
    }

    #[test]
    fn every_command_documents_itself() {
        // The help text is the documentation: `--help`, and the book's command pages.
        for c in command().get_subcommands().filter(|c| !c.is_hide_set()) {
            let about = c.get_about().map(|a| a.to_string()).unwrap_or_default();
            assert!(!about.is_empty(), "{} has no one-line help", c.get_name());
            let long = c.get_long_about().map(|a| a.to_string()).unwrap_or_default();
            assert!(long.split_whitespace().count() > 10, "{} has no description", c.get_name());
        }
    }

    #[test]
    fn what_sync_searches() {
        let mut s = Store::open_in_memory().unwrap();
        let none: [String; 0] = [];
        assert!(searches(&s, &none, &none).unwrap_err().to_string().contains("patchtrack follow"));
        // Authors synced before, when nothing is followed.
        s.apply(&Effect::RecordSync {
            sub: pairs(&[("old@x", "all")])[0].clone(),
            query: "q".into(),
            ran_at: "t".into(),
        })
        .unwrap();
        assert_eq!(searches(&s, &none, &none).unwrap(), pairs(&[("old@x", "all")]));
        // What is followed, in the lists it is followed in.
        follow(&mut s, "a@x", "linux-usb");
        follow(&mut s, "a@x", "u-boot");
        follow(&mut s, "b@x", "all");
        assert_eq!(
            searches(&s, &none, &none).unwrap(),
            pairs(&[("a@x", "linux-usb"), ("a@x", "u-boot"), ("b@x", "all")])
        );
        // -l replaces the lists, -a the authors.
        let l = ["git".to_string()];
        assert_eq!(searches(&s, &none, &l).unwrap(), pairs(&[("a@x", "git"), ("b@x", "git")]));
        let a = ["C@X".to_string()];
        assert_eq!(searches(&s, &a, &none).unwrap(), pairs(&[("c@x", "all")]));
        assert_eq!(searches(&s, &a, &l).unwrap(), pairs(&[("c@x", "git")]));
        // A followed list is synced with the authors; -l still means authors in an inbox.
        let usb = Subscription::new(Follow::List, "linux-usb", "").unwrap();
        s.subscribe(&usb, "t").unwrap();
        assert!(searches(&s, &none, &none).unwrap().contains(&usb));
        assert_eq!(searches(&s, &none, &l).unwrap(), pairs(&[("a@x", "git"), ("b@x", "git")]));
    }
}
