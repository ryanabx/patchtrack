//! The binary, run as a user runs it, on throwaway stores. No network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let d = std::env::temp_dir().join(format!("patchtrack-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Dir(d)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(db: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_patchtrack")).arg("--db").arg(db).args(args).output().unwrap()
}

/// Runs and expects success; returns stdout.
fn ok(db: &Path, args: &[&str]) -> String {
    let out = run(db, args);
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn patch(id: &str, from: &str, title: &str) -> String {
    format!(
        "From mboxrd@z Thu Jan  1 00:00:00 1970\nMessage-ID: <{id}>\nFrom: {from}\nSubject: [PATCH] {title}\n\
         Date: Mon, 05 Oct 2026 10:00:00 +0000\n\n---\n a | 1 +\n\n"
    )
}

#[test]
fn settings_are_shown_set_and_unset() {
    let d = Dir::new("settings");
    let db = d.path("s.sqlite3");
    assert!(ok(&db, &["settings"]).contains("ping_days = 7  # default\n"));
    ok(&db, &["settings", "set", "ping_days", "3"]);
    ok(&db, &["settings", "set", "archive", "https://example.org"]);
    let shown = ok(&db, &["settings"]);
    assert!(shown.contains("ping_days = 3\n") && shown.contains("archive = \"https://example.org\"\n"), "{shown}");
    let bad = run(&db, &["settings", "set", "ping_dyas", "3"]);
    assert!(!bad.status.success() && String::from_utf8_lossy(&bad.stderr).contains("no setting called ping_dyas"));
    ok(&db, &["settings", "unset", "ping_days"]);
    assert!(ok(&db, &["settings"]).contains("ping_days = 7  # default\n"));
}

#[test]
fn series_lists_the_authors_you_follow() {
    let d = Dir::new("follow");
    let db = d.path("s.sqlite3");
    let mbox = d.path("mail.mbox");
    let mail = patch("1@x", "Me <me@x.org>", "foo: mine") + &patch("2@x", "Other <other@x.org>", "bar: theirs");
    std::fs::write(&mbox, mail).unwrap();
    ok(&db, &["import", mbox.to_str().unwrap()]);
    let all = ok(&db, &["list"]);
    assert!(all.contains("foo: mine") && all.contains("bar: theirs"), "{all}");
    ok(&db, &["follow", "Me@X.org"]);
    assert_eq!(ok(&db, &["follow"]), "author me@x.org\n");
    let mine = ok(&db, &["list"]);
    assert!(mine.contains("foo: mine") && !mine.contains("bar: theirs"), "{mine}");
    // -a overrides for one run.
    assert!(ok(&db, &["list", "-a", "other@x.org"]).contains("bar: theirs"));
    ok(&db, &["unfollow", "me@x.org"]);
    assert!(ok(&db, &["list"]).contains("bar: theirs"));
    assert!(!run(&db, &["unfollow", "me@x.org"]).status.success());
}

#[test]
fn exports_carry_settings_unless_public() {
    let d = Dir::new("export");
    let db = d.path("s.sqlite3");
    let mbox = d.path("mail.mbox");
    std::fs::write(&mbox, patch("1@x", "Me <me@x.org>", "foo: mine")).unwrap();
    ok(&db, &["import", mbox.to_str().unwrap()]);
    ok(&db, &["settings", "set", "identities", r#"["me@x.org"]"#]);
    ok(&db, &["follow", "me@x.org"]);
    ok(&db, &["source", "add", d.0.to_str().unwrap()]);
    let (full, public) = (d.path("full.sqlite3"), d.path("public.sqlite3"));
    ok(&db, &["export", full.to_str().unwrap()]);
    ok(&db, &["export", "--public", public.to_str().unwrap()]);
    assert!(!run(&db, &["export", full.to_str().unwrap()]).status.success()); // exists

    // Opened as a store of its own, and imported into a new one.
    for (file, identities) in [(&full, "[\"me@x.org\"]"), (&public, "[]  # default")] {
        assert!(ok(file, &["settings"]).contains(&format!("identities = {identities}\n")));
        assert_eq!(ok(file, &["source", "list"]), "");
        let other = d.path("other.sqlite3");
        let _ = std::fs::remove_file(&other);
        ok(&other, &["import", file.to_str().unwrap()]);
        assert!(ok(&other, &["settings"]).contains(&format!("identities = {identities}\n")));
        assert_eq!(ok(&other, &["follow"]), "author me@x.org\n");
        assert!(ok(&other, &["list"]).contains("foo: mine"));
    }
}

// ---- local mail ----

const ME: &str = "Me <me@x.org>";

fn msg(id: &str, from: &str, subject: &str, irt: Option<&str>, date: &str, body: &str) -> String {
    let irt = irt.map(|r| format!("In-Reply-To: <{r}>\nReferences: <{r}>\n")).unwrap_or_default();
    format!("Message-ID: <{id}>\nFrom: {from}\nSubject: {subject}\n{irt}Date: {date}\n\n{body}")
}

/// A cover and one patch, and a reviewer's reply to the cover.
fn series() -> (Vec<String>, String) {
    let d = "Mon, 05 Oct 2026 10:00:00 +0000";
    let posting = vec![
        msg("cover@x", ME, "[PATCH 0/1] foo: do the thing", None, d, "The thing.\n"),
        msg("p1@x", ME, "[PATCH 1/1] foo: part 1", Some("cover@x"), d, "---\n a | 1 +\n"),
    ];
    let reply = msg(
        "rev@x",
        "R <r@x>",
        "Re: [PATCH 0/1] foo: do the thing",
        Some("cover@x"),
        "Tue, 06 Oct 2026 10:00:00 +0000",
        "> The thing.\nWhy?\n",
    );
    (posting, reply)
}

fn noise() -> String {
    msg("other@x", "S <s@x>", "[PATCH] somebody else's patch", None, "Mon, 05 Oct 2026 10:00:00 +0000", "")
}

fn maildir(d: &Dir, name: &str, msgs: &[String]) -> PathBuf {
    let md = d.path(name);
    for sub in ["cur", "new", "tmp"] {
        std::fs::create_dir_all(md.join(sub)).unwrap();
    }
    for m in msgs {
        deliver(&md, m);
    }
    md
}

fn deliver(md: &Path, m: &str) {
    let n = std::fs::read_dir(md.join("new")).unwrap().count() + std::fs::read_dir(md.join("cur")).unwrap().count();
    // A Windows-style name, as mbsync writes there: '!' for ':'.
    std::fs::write(md.join("new").join(format!("1761000000.{n}.host!2,S")), m).unwrap();
}

/// Runs and expects success; returns stderr.
fn ok_err(db: &Path, args: &[&str]) -> String {
    let out = run(db, args);
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stderr).unwrap()
}

#[test]
fn a_maildir_source_keeps_only_whats_relevant() {
    let d = Dir::new("maildir");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let md = maildir(&d, "md", &[posting[0].clone(), posting[1].clone(), reply, noise()]);
    ok(&db, &["follow", "me@x.org"]);
    ok(&db, &["source", "add", md.to_str().unwrap()]);
    // Never touches the archive: there is none at this address.
    let err = ok_err(&db, &["sync", "--offline", "--archive", "http://127.0.0.1:9"]);
    assert!(err.contains("local mail: 4 messages read, 3 new"), "{err}");
    let all = ok(&db, &["list", "-a", "s@x"]);
    assert!(!all.contains("somebody else"), "{all}");
    // Seen only locally, and it says so.
    let json = ok(&db, &["list", "--json"]);
    assert!(json.contains("\"local_as_of\": \"20"), "{json}");
    assert!(ok(&db, &["source", "list"]).contains(" read 20"));
}

#[test]
fn identities_count_as_authors_for_local_mail() {
    let d = Dir::new("identities");
    let db = d.path("s.sqlite3");
    let (posting, _) = series();
    let md = maildir(&d, "md", &posting);
    ok(&db, &["settings", "set", "identities", r#"["me@x.org"]"#]);
    ok(&db, &["source", "add", md.to_str().unwrap()]);
    let err = ok_err(&db, &["sync", "--offline", "-a", "someone@else.org"]);
    assert!(err.contains("2 messages read, 2 new"), "{err}");
}

#[test]
fn an_mbox_and_an_eml_directory() {
    let d = Dir::new("mbox");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let mbox = d.path("sent.mbox");
    let from_line = "From mboxrd@z Thu Jan  1 00:00:00 1970\n";
    std::fs::write(&mbox, posting.iter().map(|m| format!("{from_line}{m}\n")).collect::<String>()).unwrap();
    let eml = d.path("eml");
    std::fs::create_dir(&eml).unwrap();
    std::fs::write(eml.join("reply.eml"), reply.replace('\n', "\r\n")).unwrap();
    ok(&db, &["follow", "me@x.org"]);
    ok(&db, &["source", "add", mbox.to_str().unwrap()]);
    ok(&db, &["source", "add", eml.to_str().unwrap()]);
    let err = ok_err(&db, &["sync", "--offline"]);
    assert!(err.contains("3 messages read, 3 new"), "{err}");
    assert!(ok(&db, &["list", "--json"]).contains("rev@x"));
    // Read again, the mbox unchanged since: it is skipped; the reply is known.
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600); // 2026-01-01
    std::fs::File::options().write(true).open(&mbox).unwrap().set_modified(old).unwrap();
    let err = ok_err(&db, &["sync", "--offline"]);
    assert!(err.contains("1 messages read, 0 new"), "{err}");
}

#[test]
fn a_maildir_is_read_incrementally() {
    let d = Dir::new("incremental");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let md = maildir(&d, "md", &posting);
    ok(&db, &["follow", "me@x.org"]);
    ok(&db, &["source", "add", md.to_str().unwrap()]);
    ok_err(&db, &["sync", "--offline"]);
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600); // 2026-01-01
    for f in std::fs::read_dir(md.join("new")).unwrap() {
        std::fs::File::options().write(true).open(f.unwrap().path()).unwrap().set_modified(old).unwrap();
    }
    deliver(&md, &reply); // delivered now
    let err = ok_err(&db, &["sync", "--offline"]);
    assert!(err.contains("1 messages read, 1 new"), "{err}"); // only the new file was opened
}

#[test]
fn import_only_followed_keeps_what_a_source_would() {
    let d = Dir::new("importfollowed");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let md = maildir(&d, "md", &[posting[0].clone(), posting[1].clone(), reply, noise()]);
    assert!(!run(&db, &["import", "--only-followed", md.to_str().unwrap()]).status.success()); // follows no one
    ok(&db, &["follow", "me@x.org"]);
    let err = ok_err(&db, &["import", "--only-followed", md.to_str().unwrap()]);
    assert!(err.contains("4 messages, 3 kept, 3 new"), "{err}");
    assert!(!ok(&db, &["list", "--everyone"]).contains("somebody else"));
}

#[test]
fn import_keeps_everything_it_is_given() {
    let d = Dir::new("import");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let md = maildir(&d, "md", &[posting[0].clone(), posting[1].clone(), reply, noise()]);
    let err = ok_err(&db, &["import", md.to_str().unwrap()]);
    assert!(err.contains("4 messages, 4 new"), "{err}");
    assert!(ok(&db, &["list"]).contains("somebody else"));
    // What it showed is marked as local mail, so no reply is a fact, not a guess.
    let json = ok(&db, &["list", "--json", "-a", "s@x"]);
    assert!(json.contains("\"state\": \"no-reply\""), "{json}");
}

// ---- sync against an archive on this machine ----

/// A public-inbox archive on 127.0.0.1: every search returns all its mail,
/// a thread the mail that names its root. Records each request as
/// "METHOD path" and the User-Agent it came with.
struct Archive {
    url: String,
    requests: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

impl Archive {
    fn start(mails: Vec<String>) -> Archive {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let seen = requests.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut conn) = conn else { continue };
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut agent = String::new();
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = h.split_once(':')
                        && name.eq_ignore_ascii_case("user-agent")
                    {
                        agent = value.trim().to_string();
                    }
                }
                let mut words = line.split_whitespace();
                let (method, path) = (words.next().unwrap_or("").to_string(), words.next().unwrap_or("").to_string());
                seen.lock().unwrap().push((format!("{method} {path}"), agent));
                let wanted: Vec<&String> = if method == "POST" {
                    mails.iter().collect()
                } else {
                    let root = path.trim_end_matches("/t.mbox.gz").rsplit('/').next().unwrap_or("").to_string();
                    mails.iter().filter(|m| m.contains(&root)).collect()
                };
                let body: String =
                    wanted.iter().map(|m| format!("From mboxrd@z Thu Jan  1 00:00:00 1970\n{m}\n")).collect();
                let _ = write!(
                    conn,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/mbox\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Archive { url, requests }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().iter().map(|(r, _)| r.clone()).collect()
    }
}

#[test]
fn sync_follows_subscriptions_with_the_archive_from_the_settings() {
    let d = Dir::new("sync");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let archive = Archive::start(vec![posting[0].clone(), posting[1].clone(), reply]);
    ok(&db, &["settings", "set", "archive", &archive.url]);
    ok(&db, &["settings", "set", "window_days", "365"]); // one search per list
    ok(&db, &["follow", "me@x.org", "-l", "one", "-l", "two"]);
    ok_err(&db, &["sync", "--delay", "0"]);
    let reqs = archive.requests();
    assert_eq!(reqs.len(), 3, "{reqs:?}");
    assert!(reqs[0].starts_with("POST /one/?x=m&q=") && reqs[1].starts_with("POST /two/?x=m&q="), "{reqs:?}");
    assert!(reqs[0].contains("me%40x.org"), "{reqs:?}");
    assert_eq!(reqs[2], "GET /one/cover@x/t.mbox.gz");
    // An honest User-Agent on every request.
    let agents = archive.requests.lock().unwrap().iter().map(|(_, a)| a.clone()).collect::<Vec<_>>();
    assert!(
        agents.iter().all(|a| a.starts_with("patchtrack/") && a.contains("github.com/ryanabx/patchtrack")),
        "{agents:?}"
    );
    let json = ok(&db, &["list", "--json"]);
    assert!(json.contains("\"state\": \"discussed\"") && json.contains("rev@x"), "{json}");

    // Nothing new: the next sync searches incrementally and fetches no thread.
    ok_err(&db, &["sync", "--delay", "0"]);
    assert_eq!(archive.requests().len(), 5);
    // -a and -l replace what is followed, for one run.
    ok_err(&db, &["sync", "--delay", "0", "-a", "other@x.org", "-l", "three"]);
    let reqs = archive.requests();
    assert!(reqs[5].starts_with("POST /three/") && reqs[5].contains("other%40x.org"), "{reqs:?}");
}

#[test]
fn follow_a_list_a_query_and_a_thread() {
    let d = Dir::new("follow-kinds");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let archive = Archive::start(vec![posting[0].clone(), posting[1].clone(), reply, noise()]);
    ok(&db, &["settings", "set", "archive", &archive.url]);
    ok(&db, &["settings", "set", "list_since", "2026-10-01"]);
    ok(&db, &["settings", "set", "window_days", "365"]);
    // Not an author anyone follows: the inbox brings it.
    ok(&db, &["follow", "--inbox", "one"]);
    ok(&db, &["follow", "--query", "dfn:drivers/usb/", "-l", "two"]);
    // Its URL on the archive this store syncs with: the archive setting's, so the setting it follows.
    ok(&db, &["follow", "--thread", &format!("{}/one/rev@x/", archive.url)]);
    assert!(!run(&db, &["follow", "--inbox", "a$(rm -rf ~)"]).status.success());
    assert_eq!(ok(&db, &["follow"]), "list one\nquery \"dfn:drivers/usb/\" in two\nthread rev@x in one\n");
    ok_err(&db, &["sync", "--delay", "0"]);
    let reqs = archive.requests();
    // The list's search is the window alone, from list_since; the query's is the query.
    assert!(reqs[0].starts_with("POST /one/?x=m&q=rt%3A1790812800.."), "{reqs:?}");
    assert!(reqs[1].starts_with("POST /two/?x=m&q=%28dfn%3Adrivers%2Fusb%2F%29+AND+rt%3A"), "{reqs:?}");
    // Each thread once, though the list, the query and the thread all name the same two.
    let threads: Vec<_> = reqs.iter().filter(|r| r.starts_with("GET")).collect();
    // From the inbox each was found in.
    assert_eq!(threads, ["GET /one/cover@x/t.mbox.gz", "GET /one/other@x/t.mbox.gz"], "{reqs:?}");
    // What you follow is what `list` shows without -a: here, everyone's series it found.
    let titles = ok(&db, &["list", "--color", "never"]);
    assert!(titles.contains("foo: do the thing") && titles.contains("somebody else's patch"), "{titles}");
    ok(&db, &["unfollow", "--inbox", "one", "--query", "dfn:drivers/usb/", "-l", "two"]);
    assert!(!run(&db, &["unfollow", "--inbox", "one"]).status.success());
    assert_eq!(ok(&db, &["follow"]), "thread rev@x in one\n");
    // Only the thread now: no search, and its root by name.
    ok_err(&db, &["sync", "--delay", "0"]);
    assert_eq!(archive.requests()[reqs.len()..], ["GET /one/cover@x/t.mbox.gz"]);
    let titles = ok(&db, &["list", "--color", "never"]);
    assert!(titles.contains("foo: do the thing") && !titles.contains("somebody else's patch"), "{titles}");
}

#[test]
fn projects_on_two_archives() {
    let d = Dir::new("projects");
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let lore = Archive::start(vec![posting[0].clone(), posting[1].clone(), reply]);
    let gcc_patch =
        msg("g1@x", "G <g@x.org>", "[PATCH] gcc: a fix", None, "Mon, 05 Oct 2026 10:00:00 +0000", "To the list.\n");
    let sourceware = Archive::start(vec![gcc_patch.replace("Subject:", "To: gcc-patches@gcc.gnu.org\nSubject:")]);
    ok(&db, &["settings", "set", "archive", &lore.url]);
    ok(&db, &["settings", "set", "window_days", "365"]);
    // Built in by name; your own with its archive. Listed with what could be added.
    ok(&db, &["project", "add", "U-Boot"]);
    ok(
        &db,
        &[
            "project",
            "add",
            "GCC",
            "--archive",
            &sourceware.url,
            "-l",
            "gcc-patches",
            "--to",
            "gcc-patches@gcc.gnu.org",
        ],
    );
    assert!(!run(&db, &["project", "add", "Mine"]).status.success(), "a project of your own needs an archive");
    let listed = ok(&db, &["project"]);
    assert!(listed.starts_with("U-Boot  https://lore.kernel.org  (u-boot)\n"), "{listed}");
    assert!(
        listed.contains(&format!("GCC  {}  (gcc-patches)\n    mail to gcc-patches@gcc.gnu.org\n", sourceware.url)),
        "{listed}"
    );
    assert!(listed.contains("Built in, to add by name: Linux, QEMU,") && !listed.contains("name: U-Boot"), "{listed}");
    ok(&db, &["project", "rm", "U-Boot"]);
    // Following in a project: on its archive, in its lists.
    ok(&db, &["follow", "me@x.org"]);
    ok(&db, &["follow", "-p", "gcc", "g@x.org"]);
    assert_eq!(ok(&db, &["follow"]), format!("author me@x.org\nauthor g@x.org in gcc-patches on {}\n", sourceware.url));
    ok_err(&db, &["sync", "--delay", "0"]);
    // Each archive saw its own search and thread.
    assert_eq!(lore.requests().len(), 2, "{:?}", lore.requests());
    let theirs = sourceware.requests();
    assert!(
        theirs[0].starts_with("POST /gcc-patches/?x=m&q=") && theirs[1] == "GET /gcc-patches/g1@x/t.mbox.gz",
        "{theirs:?}"
    );
    // A project narrows the list; the archive setting's mail is in no project here.
    let gcc = ok(&db, &["list", "-p", "GCC", "--color", "never"]);
    assert!(gcc.contains("gcc: a fix") && !gcc.contains("foo: do the thing"), "{gcc}");
    assert!(!run(&db, &["list", "-p", "Nope"]).status.success());
    ok(&db, &["unfollow", "-p", "GCC", "g@x.org"]);
    assert_eq!(ok(&db, &["follow"]), "author me@x.org\n");
}

#[test]
fn sync_with_no_authors_says_how_to_follow_one() {
    let d = Dir::new("noauthors");
    let out = run(&d.path("s.sqlite3"), &["sync"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("patchtrack follow"));
}

// ---- list, show, people, close, open ----

/// A store with two families from me: one with a review question on its
/// patch, sent to a maintainer; one nobody answered.
fn two_families(d: &Dir) -> PathBuf {
    let db = d.path("s.sqlite3");
    let date = "Mon, 05 Oct 2026 10:00:00 +0000";
    let mbox = d.path("two.mbox");
    let msgs = [
        "Message-ID: <a0@x>\nFrom: Me <me@x.org>\nTo: Maint <maint@example.org>\nSubject: [PATCH 0/1] foo: do the thing\n"
            .to_string()
            + &format!("Date: {date}\n\nCover.\n"),
        msg("a1@x", ME, "[PATCH 1/1] foo: part 1", Some("a0@x"), date, "---\n a | 1 +\n"),
        msg(
            "com@x",
            "Rev <rev@x>",
            "Re: [PATCH 1/1] foo: part 1",
            Some("a1@x"),
            "Tue, 06 Oct 2026 10:00:00 +0000",
            "> ---\nWhy is this needed?\n",
        ),
        msg("b0@x", ME, "[PATCH] bar: fix another thing", None, date, "---\n b | 1 +\n"),
    ];
    let from_line = "From mboxrd@z Thu Jan  1 00:00:00 1970\n";
    std::fs::write(&mbox, msgs.iter().map(|m| format!("{from_line}{m}\n")).collect::<String>()).unwrap();
    ok(&db, &["import", mbox.to_str().unwrap()]);
    ok(&db, &["follow", "me@x.org"]);
    db
}

#[test]
fn list_filters_combine() {
    let d = Dir::new("filters");
    let db = two_families(&d);
    let list = |args: &[&str]| ok(&db, &[&["list", "--color", "never"], args].concat());
    let all = list(&[]);
    assert!(all.contains("foo: do the thing") && all.contains("bar: fix another thing"), "{all}");
    assert!(all.contains("● 1 commented   ○ 1 no-reply"), "{all}");
    assert!(!all.contains('\x1b'));
    let only = |args: &[&str], foo: bool, bar: bool| {
        let out = list(args);
        assert_eq!(
            (out.contains("foo: do the thing"), out.contains("bar: fix another")),
            (foo, bar),
            "{args:?}: {out}"
        );
    };
    only(&["--state", "commented"], true, false);
    only(&["--state", "no-reply", "--state", "commented"], true, true);
    only(&["--unanswered"], true, false);
    only(&["--to", "maint@"], true, false);
    only(&["--replied", "rev"], true, false);
    only(&["--replied", "nobody"], false, false);
    only(&["--open"], true, true);
    only(&["-a", "other@x.org"], false, false);
    assert!(!run(&db, &["list", "--state", "bogus"]).status.success());
    // Addresses only with -v.
    assert!(!all.contains("<rev@x>") && list(&["-v"]).contains("<rev@x>"));
}

#[test]
fn list_json_keeps_its_fields() {
    let d = Dir::new("listjson");
    let db = two_families(&d);
    let rows: String = ok(&db, &["list", "--json", "--state", "commented"]);
    for k in ["root", "author", "title", "version", "state", "evidence", "patches", "stages", "trees", "tags", "url"] {
        assert!(rows.contains(&format!("\"{k}\":")), "{k}: {rows}");
    }
    assert!(rows.contains("\"evidence_msgids\": [\n      \"com@x\"\n    ]"), "{rows}");
    assert!(rows.contains("\"family_state\": \"commented\""));
}

#[test]
fn close_and_reopen() {
    let d = Dir::new("close");
    let db = two_families(&d);
    let list = |args: &[&str]| ok(&db, &[&["list", "--color", "never"], args].concat());
    // "thing" names both families: refused without --all.
    let vague = run(&db, &["close", "thing"]);
    assert!(!vague.status.success() && String::from_utf8_lossy(&vague.stderr).contains("2 families match"));
    assert!(ok(&db, &["close", "do the thing", "--reason", "went elsewhere"]).contains("closed: foo: do the thing"));
    assert!(!list(&[]).contains("do the thing"));
    let closed = list(&["--closed"]);
    assert!(closed.contains("closed by you: went elsewhere") && closed.contains('⊘'), "{closed}");
    assert!(list(&["--state", "closed"]).contains("do the thing"));
    assert!(ok(&db, &["reopen", "do the thing"]).contains("reopened"));
    assert!(list(&[]).contains("do the thing"));
}

#[test]
fn show_people_and_open() {
    let d = Dir::new("show");
    let db = two_families(&d);
    let shown = ok(&db, &["show", "do the THING"]);
    assert!(shown.starts_with("[PATCH] foo: do the thing (1 patch)\n  commented (unanswered), as of "), "{shown}");
    assert!(shown.contains("  1/1 <a1@x> [not applied]\n") && shown.contains(" on 1/1: Rev <rev@x>\n"), "{shown}");
    assert!(!run(&db, &["show", "no such series"]).status.success());
    let people = ok(&db, &["people"]);
    // last, series, replies, tags, comments, applied, who
    let row: Vec<&str> = people.lines().nth(1).unwrap().split_whitespace().collect();
    assert_eq!(row, ["2026-10-06", "1", "1", "0", "1", "0", "Rev", "<rev@x>"], "{people}");
    assert_eq!(ok(&db, &["open", "--print", "fix another"]), "https://lore.kernel.org/all/b0@x/T/#u\n");
}

// ---- read and diff ----

#[test]
fn read_and_diff_two_versions() {
    let d = Dir::new("readdiff");
    let db = d.path("s.sqlite3");
    let (v1, v2) = ("Mon, 05 Oct 2026 10:00:00 +0000", "Mon, 12 Oct 2026 10:00:00 +0000");
    let patch = |msg: &str, new: &str| {
        format!(
            "{msg}\n\nSigned-off-by: Me <me@x.org>\n---\n a.c | 2 +-\n\ndiff --git a/a.c b/a.c\nindex 1..2 100644\n--- a/a.c\n+++ b/a.c\n@@ -1,2 +1,2 @@\n keep\n-x\n+{new}\n"
        )
    };
    let msgs = [
        msg("v1-0@x", ME, "[PATCH 0/2] foo: do the thing", None, v1, "Cover.\n"),
        msg("v1-1@x", ME, "[PATCH 1/2] foo: part 1", Some("v1-0@x"), v1, &patch("Part one.", "y")),
        msg("v1-2@x", ME, "[PATCH 2/2] foo: drop me", Some("v1-0@x"), v1, &patch("Part two.", "q")),
        msg(
            "rev@x",
            "Rev <rev@x>",
            "Re: [PATCH 1/2] foo: part 1",
            Some("v1-1@x"),
            "Tue, 06 Oct 2026 10:00:00 +0000",
            &(String::from("On Mon, Me wrote:\n")
                + &(0..8).map(|i| format!("> l{i}\n")).collect::<String>()
                + "\nWhy y?\n\nThanks\n-- \nRev's signature\n"),
        ),
        msg(
            "v2-0@x",
            ME,
            "[PATCH v2 0/1] foo: do the thing",
            None,
            v2,
            "Cover.\n\nChanges in v2:\n- Use z.\n- Drop part 2.\n\n---\nMe (1):\n",
        ),
        msg("v2-1@x", ME, "[PATCH v2 1/1] foo: part 1", Some("v2-0@x"), v2, &patch("Part one.", "z")),
    ];
    let mbox = d.path("v.mbox");
    let from_line = "From mboxrd@z Thu Jan  1 00:00:00 1970\n";
    std::fs::write(&mbox, msgs.iter().map(|m| format!("{from_line}{m}\n")).collect::<String>()).unwrap();
    ok(&db, &["import", mbox.to_str().unwrap()]);

    let diff = ok(&db, &["diff", "do the thing", "--color", "never"]);
    let want = [
        "foo: do the thing: v1 → v2",
        "",
        "  The author's changelog:",
        "    Changes in v2:",
        "    - Use z.",
        "    - Drop part 2.",
        "",
        "  1/2 ! 1/1   foo: part 1",
        "      --- old diff",
        "      +++ new diff",
        "      @@ -5,3 +5,3 @@",
        "        keep",
        "       -x",
        "      -+y",
        "      ++z",
        "  2/2 < -     foo: drop me",
    ];
    assert_eq!(diff.lines().take(want.len()).collect::<Vec<_>>(), want, "{diff}");
    let err = run(&db, &["diff", "do the thing", "--from", "7"]);
    assert!(String::from_utf8_lossy(&err.stderr).contains("versions known: v1, v2"));

    // The latest version, unless --all.
    let read = ok(&db, &["read", "do the thing", "--color", "never"]);
    assert!(read.starts_with("[PATCH v2] foo: do the thing (1 patch)  ·  no-reply  ·  as of "), "{read}");
    assert!(
        read.contains("── patch  [PATCH v2 1/1] foo: part 1")
            && read.contains("[diff: 1 file, 2 lines changed; --diffs to show]")
    );
    let all = ok(&db, &["read", "do the thing", "--all", "--diffs", "--color", "never"]);
    assert!(
        all.contains("    ── Rev ")
            && all.contains("       [… 5 quoted lines]\n       > l5\n       > l6\n       > l7\n"),
        "{all}"
    );
    assert!(all.contains("Why y?") && !all.contains("signature") && all.contains("+y"));
}

// ---- news ----

fn add(db: &Path, d: &Dir, name: &str, msgs: &[String]) {
    let mbox = d.path(name);
    let from_line = "From mboxrd@z Thu Jan  1 00:00:00 1970\n";
    std::fs::write(&mbox, msgs.iter().map(|m| format!("{from_line}{m}\n")).collect::<String>()).unwrap();
    ok(db, &["import", mbox.to_str().unwrap()]);
}

fn news_posting(v: u32, date: &str) -> Vec<String> {
    let pre = if v > 1 { format!(" v{v}") } else { String::new() };
    vec![
        msg(&format!("nw{v}-0@x"), ME, &format!("[PATCH{pre} 0/1] foo: do the thing"), None, date, "Cover.\n"),
        msg(
            &format!("nw{v}-1@x"),
            ME,
            &format!("[PATCH{pre} 1/1] foo: part 1"),
            Some(&format!("nw{v}-0@x")),
            date,
            "---\n a | 1 +\n",
        ),
    ]
}

#[test]
fn news_first_look_then_changes() {
    let d = Dir::new("news");
    let db = d.path("s.sqlite3");
    add(&db, &d, "v1.mbox", &news_posting(1, "Mon, 05 Oct 2026 10:00:00 +0000"));
    let news = |extra: &[&str]| ok(&db, &[&["news", "--color", "never"], extra].concat());
    assert!(news(&[]).contains("First look: 1 series recorded"));
    assert!(news(&[]).contains("Nothing new."));
    let lock = "> +x\nThis needs a lock.\n";
    add(
        &db,
        &d,
        "r.mbox",
        &[msg(
            "c@x",
            "Rev <r@x>",
            "Re: [PATCH 1/1] foo: part 1",
            Some("nw1-1@x"),
            "Tue, 06 Oct 2026 10:00:00 +0000",
            lock,
        )],
    );
    let out = news(&["--peek"]);
    assert!(out.contains("○ no-reply → ● commented"), "{out}");
    assert!(out.contains("2026-10-06 Rev commented: \"This needs a lock.\""), "{out}");
    assert_eq!(out, news(&[])); // --peek did not mark it seen
    assert!(news(&[]).contains("Nothing new."));
}

#[test]
fn news_new_version_and_applied() {
    let d = Dir::new("news2");
    let db = d.path("s.sqlite3");
    add(&db, &d, "v1.mbox", &news_posting(1, "Mon, 05 Oct 2026 10:00:00 +0000"));
    ok(&db, &["news"]);
    let mut v2 = news_posting(2, "Mon, 12 Oct 2026 10:00:00 +0000");
    v2.push(msg(
        "a@x",
        "M <m@x>",
        "Re: [PATCH v2 0/1] foo: do the thing",
        Some("nw2-0@x"),
        "Tue, 13 Oct 2026 10:00:00 +0000",
        "Applied, thanks!\n",
    ));
    add(&db, &d, "v2.mbox", &v2);
    let out = ok(&db, &["news", "--color", "never"]);
    assert!(
        out.contains("v2 posted") && out.contains("→ ● applied") && out.contains("M \"Applied, thanks!\""),
        "{out}"
    );
}

#[test]
fn news_since() {
    let d = Dir::new("news3");
    let db = d.path("s.sqlite3");
    let mut msgs = news_posting(1, "Mon, 05 Oct 2026 10:00:00 +0000");
    msgs.push(msg(
        "c@x",
        "Rev <r@x>",
        "Re: [PATCH 0/1] foo: do the thing",
        Some("nw1-0@x"),
        "Tue, 06 Oct 2026 10:00:00 +0000",
        "Reviewed-by: Rev <r@x>\n",
    ));
    add(&db, &d, "all.mbox", &msgs);
    let out = ok(&db, &["news", "--color", "never", "--since", "2026-10-06T00:00:00Z"]);
    assert!(out.starts_with("Changes since 2026-10-06 00:00 UTC:\n"), "{out}");
    assert!(out.contains("no-reply → ● reviewed") && out.contains("Rev Reviewed-By"), "{out}");
    assert!(ok(&db, &["news", "--since", "2026-10-07T00:00:00Z"]).contains("Nothing new."));
    assert!(ok(&db, &["news"]).contains("First look")); // --since did not record a snapshot
}

// ---- trees and check ----

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Me")
        .env("GIT_AUTHOR_EMAIL", "me@x.org")
        .env("GIT_COMMITTER_NAME", "Maint")
        .env("GIT_COMMITTER_EMAIL", "maint@x.org")
        .env("GIT_CONFIG_GLOBAL", repo.join("../empty.gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn check_finds_a_patch_in_a_tree() {
    let d = Dir::new("check");
    let db = d.path("s.sqlite3");
    let repo = d.path("linux");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(d.path("empty.gitconfig"), "").unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.c"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    std::fs::write(repo.join("a.c"), "one\nTWO\n").unwrap();
    let diff = git(&repo, &["diff"]);
    // Posted before the commit below: commits are read from two days before the posting on.
    let posted_at = "Mon, 01 Jan 2024 00:00:00 +0000";
    let posted =
        msg("p1@x", ME, "[PATCH] a: shout two", None, posted_at, &format!("Shout.\n\nSigned-off-by: Me\n---\n{diff}"));
    add(&db, &d, "p.mbox", &[posted]);
    git(&repo, &["commit", "-q", "-a", "-m", "a: shout two\n\nLink: https://patch.msgid.link/p1@x\n"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]).trim().to_string();

    assert!(!run(&db, &["tree", "add", "next", "--repo", repo.to_str().unwrap()]).status.success()); // no --ref or --url
    ok(&db, &["tree", "add", "next", "--repo", repo.to_str().unwrap(), "--ref", "main", "--role", "next"]);
    assert!(ok(&db, &["tree", "list"]).starts_with("next             next       "));
    let err = ok_err(&db, &["check"]);
    assert!(err.contains("next @ ") && err.contains("1 of 1 posted patches found"), "{err}");
    let rows = ok(&db, &["list", "--json"]);
    assert!(rows.contains("\"family_state\": \"in-next\"") && rows.contains(&sha), "{rows}");
    let shown = ok(&db, &["show", "shout"]);
    assert!(shown.contains(&format!("next @ {}: {} (by message-id)", &sha[..12], &sha[..12])), "{shown}");
    ok(&db, &["tree", "rm", "next"]);
    assert!(!run(&db, &["tree", "rm", "next"]).status.success());
    assert!(ok(&db, &["list", "--json"]).contains("\"family_state\": \"in-next\"")); // results stay
    assert!(!run(&db, &["check"]).status.success()); // no trees left
}

// ---- nudge ----

#[test]
fn nudge_drafts_and_the_merge_window() {
    let d = Dir::new("nudge");
    let db = d.path("s.sqlite3");
    let cover = "Message-ID: <q0@x>\nFrom: Me <me@x.org>\nTo: Maint <maint@x.org>\nCc: list@x.org, me@x.org\n\
                 Subject: [PATCH 0/1] foo: a quiet series\nDate: Mon, 01 Jan 2024 00:00:00 +0000\n\nCover.\n";
    add(
        &db,
        &d,
        "q.mbox",
        &[
            cover.to_string(),
            msg("q1@x", ME, "[PATCH 1/1] foo: part 1", Some("q0@x"), "Mon, 01 Jan 2024 00:00:00 +0000", ""),
        ],
    );
    let nudge = |extra: &[&str]| ok(&db, &[&["nudge", "--color", "never"], extra].concat());
    let drafts = d.path("drafts");
    let out = nudge(&["--draft", drafts.to_str().unwrap()]);
    assert!(out.starts_with("○ resend             foo: a quiet series  v1 · no human reply in "), "{out}");
    assert!(out.contains("Drafts only: nothing was sent."), "{out}");
    let files: Vec<_> = std::fs::read_dir(&drafts).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1);
    assert!(files[0].file_name().unwrap().to_str().unwrap().starts_with("2024-01-01-title_foo_ a quiet series"));
    let draft = std::fs::read_to_string(&files[0]).unwrap();
    assert!(
        draft.starts_with(
            "From: me@x.org\nTo: maint@x.org\nCc: list@x.org\nSubject: Re: [PATCH 0/1] foo: a quiet series\n"
        ),
        "{draft}"
    );
    assert!(draft.contains("In-Reply-To: <q0@x>\n") && draft.contains("Content-Transfer-Encoding: 7bit\n"));
    assert!(draft.contains("b4 send --resend v1") && draft.contains("https://lore.kernel.org/all/q0@x/"));
    // Kept as a draft too, for `send`.
    let msgid = draft.lines().find_map(|l| l.strip_prefix("Message-ID: <")).unwrap().trim_end_matches('>');
    assert!(ok(&db, &["drafts"]).starts_with(&format!("{msgid}  ping ")), "{msgid}");

    // A mainline tree whose last tag is a final release from two days ago: the window is open.
    let repo = d.path("mainline");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(d.path("empty.gitconfig"), "").unwrap();
    git(&repo, &["init", "-q", "-b", "master"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "Linux 7.3"]);
    let two_days_ago = format!(
        "{} +0000",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() - 2 * 86_400
    );
    let out = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["tag", "-a", "-m", "Linux 7.3", "v7.3"])
        .env("GIT_COMMITTER_NAME", "Maint")
        .env("GIT_COMMITTER_EMAIL", "maint@x.org")
        .env("GIT_COMMITTER_DATE", &two_days_ago)
        .env("GIT_CONFIG_GLOBAL", d.path("empty.gitconfig"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    ok(&db, &["tree", "add", "mainline", "--repo", repo.to_str().unwrap(), "--ref", "master", "--role", "mainline"]);
    let out = nudge(&[]);
    assert!(out.starts_with("The merge window has been open since v7.3 ("), "{out}");
    assert!(out.contains("resend after -rc1") && out.contains("; merge window open"), "{out}");
    assert!(nudge(&["--ignore-merge-window"]).starts_with("○ resend "));
    ok(&db, &["settings", "set", "merge_window", "false"]);
    assert!(nudge(&[]).starts_with("○ resend "));

    ok(&db, &["close", "quiet"]);
    assert!(nudge(&[]).starts_with("Nothing to nudge (ping after 7 days, RESEND after 14)."));
}

// ---- branches ----

#[test]
fn branches_as_sent_changed_and_draft() {
    let d = Dir::new("branches");
    std::fs::write(d.path("empty.gitconfig"), "").unwrap();
    let repo = d.path("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.c"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let tracking = |revision: u32, history: &str| {
        format!(
            "feat: the cover\n\n--- b4-submit-tracking ---\n# internal\n{{\"series\": {{\"revision\": {revision}, \
             \"change-id\": \"20261001-feat-abcdef123456\", \"prefixes\": [], \"history\": {history}}}}}\n"
        )
    };
    let branch = |name: &str, revision: u32, history: &str| {
        git(&repo, &["checkout", "-q", "-b", name, "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", &tracking(revision, history)]);
        std::fs::write(repo.join("a.c"), "one\ntwo\n").unwrap();
        git(&repo, &["commit", "-q", "-a", "-m", "a: add two"]);
        let diff = git(&repo, &["show", "--format=", "HEAD"]);
        git(&repo, &["checkout", "-q", "main"]);
        diff
    };
    let diff = branch("b4/feat", 2, r#"{"v1": ["feat-v1-0@x"]}"#);
    branch("b4/new", 1, "{}");
    branch("other/feat", 1, "{}"); // not under the prefix

    let review = msg(
        "rev-1@x",
        "Rev <rev@x>",
        "Re: [PATCH 1/1] a: add two",
        Some("feat-v1-1@x"),
        "Tue, 06 Oct 2026 10:00:00 +0000",
        "Reviewed-by: Rev <rev@x>\n",
    );
    let run_with = |posted_diff: &str, replies: &[String]| {
        let db = d.path("s.sqlite3");
        let _ = std::fs::remove_file(&db);
        let date = "Mon, 05 Oct 2026 10:00:00 +0000";
        let mut mails = vec![
            msg("feat-v1-0@x", ME, "[PATCH 0/1] feat: the cover", None, date, ""),
            msg(
                "feat-v1-1@x",
                ME,
                "[PATCH 1/1] a: add two",
                Some("feat-v1-0@x"),
                date,
                &format!("Add two.\n---\n{posted_diff}"),
            ),
        ];
        mails.extend(replies.iter().cloned());
        add(&db, &d, "feat.mbox", &mails);
        ok(&db, &["branches", "--repo", repo.to_str().unwrap(), "--color", "never"])
    };
    let out = run_with(&diff, &[]);
    assert_eq!(out.lines().filter(|l| !l.starts_with(' ')).count(), 2, "{out}");
    assert!(out.contains("b4/feat\n                    as sent in v1\n"), "{out}");
    assert!(out.contains("◌ draft             b4/new\n                    v1 never sent; 1 patch\n"), "{out}");
    let out = run_with(&diff.replace("+two", "+TWO"), &[]);
    assert!(out.contains("v2 in preparation: 1 changed since v1"), "{out}");
    // A tag given since: b4 trailers -u would take it in; once the commit carries it, nothing to say.
    let out = run_with(&diff, std::slice::from_ref(&review));
    assert!(out.contains("as sent in v1; 1 new tag to take in: b4 trailers -u\n"), "{out}");
    git(&repo, &["checkout", "-q", "b4/feat"]);
    git(&repo, &["commit", "-q", "--amend", "-m", "a: add two\n\nReviewed-by: Rev <rev@x>"]);
    git(&repo, &["checkout", "-q", "main"]);
    let out = run_with(&diff, std::slice::from_ref(&review));
    assert!(out.contains("b4/feat\n                    as sent in v1\n"), "{out}");
}

fn reviewer(d: &Dir) -> PathBuf {
    let db = d.path("s.sqlite3");
    let (posting, reply) = series();
    let mbox = d.path("mail.mbox");
    let mails: String =
        posting.iter().chain([&reply]).map(|m| format!("From mboxrd@z Thu Jan  1 00:00:00 1970\n{m}\n")).collect();
    std::fs::write(&mbox, mails).unwrap();
    ok(&db, &["import", mbox.to_str().unwrap()]);
    ok(&db, &["settings", "set", "identities", "[\"r@x\"]"]);
    ok(&db, &["settings", "set", "name", "R Eviewer"]);
    db
}

#[test]
fn a_review_tag_is_a_draft_addressed_and_quoted_as_b4_would() {
    let d = Dir::new("reply-tag");
    let db = reviewer(&d);
    let out = ok(&db, &["reply", "foo: do", "-n", "1", "--tag", "reviewed-by", "--everyone"]);
    assert!(out.starts_with("From: R Eviewer <r@x>\nTo: me@x.org\n"), "{out}");
    assert!(
        out.contains("Subject: Re: [PATCH 1/1] foo: part 1\nIn-Reply-To: <p1@x>\nReferences: <cover@x> <p1@x>\n"),
        "{out}"
    );
    assert!(
        out.contains("\n\nOn Mon, 05 Oct 2026 10:00:00 +0000, Me wrote:\n>\n\nReviewed-by: R Eviewer <r@x>\n"),
        "{out}"
    );
    let id = out.lines().find_map(|l| l.strip_prefix("Draft ")).unwrap().split(' ').next().unwrap().to_string();
    let list = ok(&db, &["drafts"]);
    assert!(list.starts_with(&id) && list.contains(" tag ") && list.contains("to me@x.org"), "{list}");
    assert!(ok(&db, &["drafts", "show", &id[..12]]).contains("Reviewed-by: R Eviewer <r@x>"));
    // A reply to any stored message, by its message-id; a tag that is not one is refused.
    let out = ok(&db, &["reply", "<rev@x>", "-m", "Because.", "--everyone"]);
    assert!(out.contains("In-Reply-To: <rev@x>\n") && out.contains("\n\nBecause.\n"), "{out}");
    assert!(!run(&db, &["reply", "foo", "--tag", "Liked-by", "--everyone"]).status.success());
    ok(&db, &["drafts", "rm", &id]);
    assert!(!ok(&db, &["drafts"]).contains(&id));
}

#[test]
fn thanks_name_what_no_check_found() {
    let d = Dir::new("thanks");
    let db = reviewer(&d);
    let out = run(&db, &["thanks", "foo: do", "--everyone"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success() && String::from_utf8_lossy(&out.stderr).contains("no tree check found"));
    assert!(
        text.contains(
            "Applied, thanks!\n\n[1/1] foo: part 1\n      (no commit info)\n\nBest regards,\n-- \nR Eviewer <r@x>\n"
        ),
        "{text}"
    );
}

// ---- serve: the app, and what it may do on its machine ----

/// One HTTP request to 127.0.0.1:port: (status, body).
fn http(port: u16, method: &str, path: &str, token: &str, body: &str) -> (u16, Vec<u8>) {
    http_as(port, &format!("127.0.0.1:{port}"), method, path, token, body)
}

/// The same, naming another host, as a proxy in front of it would.
fn http_as(port: u16, host: &str, method: &str, path: &str, token: &str, body: &str) -> (u16, Vec<u8>) {
    use std::io::{Read, Write};
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let auth = if token.is_empty() { String::new() } else { format!("X-Patchtrack-Token: {token}\r\n") };
    write!(s, "{method} {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Length: {}\r\n\r\n{body}", body.len())
        .unwrap();
    let mut out = vec![];
    s.read_to_end(&mut out).unwrap();
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let status = String::from_utf8_lossy(&out[..split]).split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, out[split + 4..].to_vec())
}

#[test]
fn a_bridge_reachable_from_anywhere_answers_its_public_name() {
    let d = Dir::new("public-bridge");
    let db = d.path("s.sqlite3");
    // A page served over https cannot use a plain http bridge elsewhere.
    let refused = run(&db, &["bridge", "--port", "0", "--public-url", "http://desktop.example.org"]);
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("https"));
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let errlog = d.path("bridge.log");
    let mut bridge = Command::new(env!("CARGO_BIN_EXE_patchtrack"))
        .arg("--db")
        .arg(&db)
        .args(["bridge", "--port", &port.to_string(), "--public-url", "https://desktop.example.ts.net/"])
        .stderr(std::fs::File::create(&errlog).unwrap())
        .spawn()
        .unwrap();
    let mut said = String::new();
    for _ in 0..100 {
        said = std::fs::read_to_string(&errlog).unwrap();
        if said.matches("&token=").count() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let token = said.split("&token=").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
    let public = http_as(port, "desktop.example.ts.net", "GET", "/v1/hello", &token, "").0;
    let other = http_as(port, "elsewhere.example.org", "GET", "/v1/hello", &token, "").0;
    let local = http(port, "GET", "/v1/hello", &token, "").0;
    bridge.kill().unwrap();
    let _ = bridge.wait();
    assert!(said.contains("#/bridge?url=https://desktop.example.ts.net&token="), "{said}");
    assert_eq!((public, other, local), (200, 403, 200), "{said}");
}

// ---- what needs a Unix shell: an editor, a sendmail command, a fake b4 ----
//
// Together, so a helper only these use cannot be left in the Windows build.

#[cfg(unix)]
mod unix {
    use super::*;

    /// The patchtrack command, with git's configuration this test's own.
    fn with_git_config(d: &Dir, config: &str) -> Command {
        let file = d.path("gitconfig");
        std::fs::write(&file, config).unwrap();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_patchtrack"));
        cmd.env("GIT_CONFIG_GLOBAL", &file).env("GIT_CONFIG_NOSYSTEM", "1").env_remove("GIT_EDITOR");
        cmd
    }

    #[test]
    fn your_editor_writes_the_reply_below_the_whole_quote() {
        let d = Dir::new("reply-edit");
        let db = reviewer(&d);
        let config = "[core]\n\teditor = printf 'Answered inline.\\\\n' >>\n";
        let out =
            with_git_config(&d, config).arg("--db").arg(&db).args(["reply", "foo: do", "--everyone"]).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains("Me wrote:\n> The thing.\n\nAnswered inline.\n"), "{text}");
        // An editor that leaves only the quote keeps no draft.
        let out = with_git_config(&d, "[core]\n\teditor = true\n")
            .arg("--db")
            .arg(&db)
            .args(["reply", "foo: do", "--everyone"])
            .output()
            .unwrap();
        assert!(!out.status.success() && String::from_utf8_lossy(&out.stderr).contains("nothing written"));
        assert_eq!(ok(&db, &["drafts"]).matches(" reply ").count(), 1);
    }

    /// Is git send-email here? (Debian and Fedora package it apart.)
    fn send_email() -> bool {
        let out = Command::new("git").arg("--exec-path").output().unwrap();
        Path::new(String::from_utf8_lossy(&out.stdout).trim()).join("git-send-email").exists()
    }

    #[test]
    fn send_shows_the_draft_and_sends_only_on_yes() {
        use std::io::Write;
        if !send_email() {
            eprintln!("git send-email is not installed: not tested");
            return;
        }
        let d = Dir::new("send");
        let db = reviewer(&d);
        ok(&db, &["settings", "set", "name", "Rév Iewer"]);
        ok(&db, &["reply", "foo: do", "-m", "Why not?", "--everyone"]);
        let id = ok(&db, &["drafts"]).split(' ').next().unwrap().to_string();
        let sent = d.path("sent.eml");
        // A sendmail command in git's configuration, as a user might have; and
        // recipients there too, which must not be added.
        // git runs it with -i and the recipients after it: a function takes and drops them.
        let config = format!(
            "[sendemail]\n\tsendmailcmd = \"f() {{ cat > {}; }}; f\"\n\tto = config@x.org\n\tcc = config@x.org\n",
            sent.display()
        );
        let send = |answer: &str| {
            let mut child = with_git_config(&d, &config)
                .arg("--db")
                .arg(&db)
                .args(["send", &id])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            child.stdin.take().unwrap().write_all(answer.as_bytes()).unwrap();
            child.wait_with_output().unwrap()
        };
        let out = send("no\n");
        assert!(!out.status.success() && !sent.exists(), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains("Why not?"), "the draft is shown whole first");
        let out = send("yes\n");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let mail = std::fs::read_to_string(&sent).unwrap();
        assert!(mail.contains(&format!("Message-ID: <{id}>")) && mail.contains("In-Reply-To: <cover@x>"), "{mail}");
        assert!(mail.contains("To: me@x.org") && !mail.contains("config@x.org"), "{mail}");
        // A name that is not ASCII is encoded once, and the body starts with the reply, not a From: line.
        assert!(
            mail.contains("From: =?UTF-8?q?R=C3=A9v=20Iewer?= <r@x>") || mail.contains("From: =?utf-8?b?"),
            "{mail}"
        );
        assert!(mail.split_once("\n\n").unwrap().1.starts_with("On Mon, 05 Oct 2026"), "{mail}");
        // An address git send-email cannot read: it sends nothing and exits 0; that is not "sent".
        let date = "Wed, 07 Oct 2026 10:00:00 +0000";
        let from_bad =
            msg("bad1@x", "Bad <bad@x>", "Re: [PATCH 0/1] foo: do the thing", Some("cover@x"), date, "Hm.\n");
        std::fs::write(d.path("bad.mbox"), format!("From mboxrd@z Thu Jan  1 00:00:00 1970\n{from_bad}\n")).unwrap();
        ok(&db, &["import", d.path("bad.mbox").to_str().unwrap()]);
        let out = ok(&db, &["reply", "<bad1@x>", "-m", "To a bad address.", "--everyone"]);
        let bad = out.lines().find_map(|l| l.strip_prefix("Draft ")).unwrap().split(' ').next().unwrap().to_string();
        let _ = std::fs::remove_file(&sent);
        let mut child = with_git_config(&d, &config)
            .arg("--db")
            .arg(&db)
            .args(["send", &bad])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(b"yes\n").unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(!out.status.success(), "a send that sent nothing must fail");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("did not send it"),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!ok(&db, &["drafts"]).lines().any(|l| l.starts_with(&bad) && l.contains(" sent ")));
        // Sent once only; and it is in the store, as your mail.
        assert!(ok(&db, &["drafts"]).contains(" sent "));
        assert!(!send("yes\n").status.success());
        assert!(ok(&db, &["read", "foo: do", "--everyone", "--color", "never"]).contains("Why not?"));
    }

    #[test]
    fn serve_runs_b4_in_a_worktree_and_sends_only_your_mail() {
        use std::os::unix::fs::PermissionsExt;
        let d = Dir::new("serve");
        let db = reviewer(&d);
        std::fs::write(d.path("empty.gitconfig"), "").unwrap();
        // A repository with a b4 prep branch; main stays checked out.
        let repo = d.path("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        git(&repo, &["checkout", "-q", "-b", "b4/feat"]);
        let tracking = "feat: the cover\n\n--- b4-submit-tracking ---\n# internal\n{\"series\": {\"revision\": 1, \
                        \"change-id\": \"20261001-feat-abc\", \"prefixes\": [], \"history\": {}}}\n";
        git(&repo, &["commit", "-q", "--allow-empty", "-m", tracking]);
        git(&repo, &["checkout", "-q", "main"]);
        // A b4 that says what it was asked, on which branch, given what.
        let bin = d.path("bin");
        std::fs::create_dir(&bin).unwrap();
        let log = d.path("b4.log");
        std::fs::write(
            bin.join("b4"),
            format!("#!/bin/sh\nprintf '%s|%s|' \"$*\" \"$(git rev-parse --abbrev-ref HEAD)\" >> {0}\ncat >> {0}\necho '#' >> {0}\necho \"b4 did $*\"\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(bin.join("b4"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let app = d.path("app");
        std::fs::create_dir(&app).unwrap();
        std::fs::write(app.join("index.html"), "the app").unwrap();
        let sent = d.path("sent.eml");
        let config = format!("[sendemail]\n\tsendmailcmd = \"f() {{ cat > {}; }}; f\"\n", sent.display());
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let errlog = d.path("serve.log");
        let mut serve = with_git_config(&d, &config)
            .env("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()))
            .arg("--db")
            .arg(&db)
            .args([
                "serve",
                "--listen",
                &format!("127.0.0.1:{port}"),
                "--public-url",
                &format!("http://127.0.0.1:{port}"),
            ])
            .arg("--app-dir")
            .arg(&app)
            .args(["--allow-send", "--repo"])
            .arg(&repo)
            .stderr(std::fs::File::create(&errlog).unwrap())
            .spawn()
            .unwrap();
        let mut token = String::new();
        for _ in 0..100 {
            if let Some(t) = std::fs::read_to_string(&errlog).unwrap().split("&token=").nth(1) {
                token = t.split_whitespace().next().unwrap().to_string();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let checks = std::panic::catch_unwind(|| {
            assert_eq!(http(port, "GET", "/", "", ""), (200, b"the app".to_vec()));
            let (status, branches) = http(port, "GET", "/v1/branches", &token, "");
            let branches = String::from_utf8(branches).unwrap();
            assert!(
                status == 200 && branches.contains("\"name\":\"b4/feat\"") && branches.contains("feat: the cover"),
                "{branches}"
            );
            let b4 = |branch: &str, action: &str, dry: bool| {
                let body = format!(
                    r#"{{"repo": {:?}, "branch": "{branch}", "action": "{action}", "dry_run": {dry}}}"#,
                    repo.to_str().unwrap()
                );
                http(port, "POST", "/v1/b4", &token, &body)
            };
            let (status, out) = b4("b4/feat", "send", true);
            assert!(
                status == 200 && String::from_utf8_lossy(&out).contains("b4 did send --dry-run"),
                "{}",
                String::from_utf8_lossy(&out)
            );
            b4("b4/feat", "send", false);
            b4("b4/feat", "trailers", false);
            // In a worktree of the branch; your checkout stays where it was, and no worktree is left.
            assert_eq!(
                std::fs::read_to_string(&log).unwrap(),
                "send --dry-run|b4/feat|#\nsend|b4/feat|\n#\n-n trailers -u|b4/feat|#\n"
            );
            assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(), "main");
            assert_eq!(git(&repo, &["worktree", "list"]).lines().count(), 1);
            assert_eq!(b4("main", "send", true).0, 400, "not a prep branch");
            assert_eq!(b4("b4/feat", "rm -rf", true).0, 400, "not an action");
            let elsewhere = r#"{"repo": "/tmp", "branch": "b4/feat", "action": "send"}"#;
            assert_eq!(http(port, "POST", "/v1/b4", &token, elsewhere).0, 400, "not a repository it was given");
            // A reply written through the app is a draft in the store; it
            // goes out only from your identities.
            let (status, draft) =
                http(port, "POST", "/v1/reply", &token, r#"{"to": "p1@x", "text": "Looks good.", "tag": ""}"#);
            assert_eq!(status, 200, "{}", String::from_utf8_lossy(&draft));
            let draft: serde_json::Value = serde_json::from_slice(&draft).unwrap();
            let msgid = draft["msgid"].as_str().unwrap().to_string();
            assert!(ok(&db, &["drafts"]).contains(&msgid));
            let send = || http(port, "POST", "/v1/send", &token, &serde_json::json!({ "msgid": msgid }).to_string());
            let identities = |v: &str| {
                let body = serde_json::json!({"key": "identities", "value": v}).to_string();
                assert_eq!(http(port, "POST", "/v1/settings", &token, &body).0, 200);
            };
            identities(r#"["someone@else.org"]"#);
            let (status, body) = send();
            assert!(status == 400 && String::from_utf8_lossy(&body).contains("identities"));
            identities(r#"["r@x"]"#);
            if send_email() {
                let (status, body) = send();
                assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
                let mail = std::fs::read_to_string(&sent).unwrap();
                assert!(mail.contains(&format!("Message-ID: <{msgid}>")) && mail.contains("Looks good."), "{mail}");
                assert!(ok(&db, &["drafts"]).contains("sent"), "marked sent");
                assert_eq!(send().0, 400, "sent already");
            }
            let (status, store) = http(port, "GET", "/v1/export", &token, "");
            assert!(status == 200 && store.starts_with(b"SQLite format 3\0"));
        });
        serve.kill().unwrap();
        let _ = serve.wait();
        if let Err(e) = checks {
            eprintln!("{}", std::fs::read_to_string(&errlog).unwrap_or_default());
            std::panic::resume_unwind(e);
        }
    }
}
