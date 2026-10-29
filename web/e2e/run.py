"""The web app in a real browser: headless Firefox, synthetic mail, no network.

    cd web && npm run build && cargo build -p patchtrack
    python3 e2e/run.py [--keep]

Starts, on free ports of 127.0.0.1: the built app (web/dist), a fake
public-inbox archive, `patchtrack bridge` (the debug binary) over a store
that follows an author on that archive, and Firefox with a throwaway
profile. Then it checks that the app asks to be paired, pairs from the
bridge's link and shows what the bridge synced on its own; that a change
made on the command line shows up without a reload; that mail dropped on
the app goes into the bridge's store; versions, closing, news, settings,
"mine", replies, drafts, sending, b4 branches; that following a list makes
the bridge sync it; a bridge behind an HTTPS tunnel; a phone's width; and
`patchtrack serve`. --keep leaves the temporary directory.
Exits non-zero if anything failed.
"""
import argparse
import http.server
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from marionette import Marionette  # noqa: E402

WEB = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ROOT = os.path.dirname(WEB)
BIN = os.environ.get("PATCHTRACK", os.path.join(ROOT, "target", "debug", "patchtrack"))
FIREFOX = os.environ.get("FIREFOX", "firefox")
DATE = "Mon, 05 Oct 2026 10:00:00 +0000"
MAILS = [
    f"Message-ID: <cover@x>\nFrom: Me <me@x.org>\nSubject: [PATCH 0/1] foo: do the thing\nDate: {DATE}\n\nCover.\n",
    f"Message-ID: <p1@x>\nFrom: Me <me@x.org>\nSubject: [PATCH 1/1] foo: part 1\nIn-Reply-To: <cover@x>\n"
    f"References: <cover@x>\nDate: {DATE}\n\nThe first part.\n\nSigned-off-by: Me <me@x.org>\n---\n a | 1 +\n\n"
    "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -0,0 +1 @@\n+x\n",
    "Message-ID: <rev@x>\nFrom: Rev <rev@x>\nSubject: Re: [PATCH 1/1] foo: part 1\nIn-Reply-To: <p1@x>\n"
    "References: <cover@x> <p1@x>\nDate: Tue, 06 Oct 2026 10:00:00 +0000\n\nReviewed-by: Rev <rev@x>\n",
]

# A stranger's series on linux-usb: a listing of the inbox finds the posting;
# only its thread, fetched when it is opened, has the reply.
STRANGER = [
    "Message-ID: <stranger@x>\nFrom: Stranger <stranger@y.org>\nTo: linux-usb@vger.kernel.org\n"
    f"Subject: [PATCH] usb: a stranger's fix\nDate: {DATE}\n\nFix.\n---\n s | 1 +\n",
    "Message-ID: <stranger-r@x>\nFrom: Rev <rev@x>\nSubject: Re: [PATCH] usb: a stranger's fix\nIn-Reply-To: <stranger@x>\n"
    "References: <stranger@x>\nDate: Tue, 06 Oct 2026 10:00:00 +0000\n\nOnly in the thread.\n",
]


def mbox(mails):
    return "".join(f"From mboxrd@z Thu Jan  1 00:00:00 1970\n{m}\n" for m in mails)


_ports = set()


def free_port():
    """A port nothing listens on, and not one handed out before: the ports are
    found before anything binds them, so two could otherwise be the same."""
    while True:
        with socket.socket() as s:
            s.bind(("127.0.0.1", 0))
            port = s.getsockname()[1]
        if port not in _ports:
            _ports.add(port)
            return port


class Quiet(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *a):
        pass


def serve(handler, port):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def static(directory, port):
    return serve(lambda *a, **kw: Quiet(*a, directory=directory, **kw), port)


class Archive(http.server.BaseHTTPRequestHandler):
    """Every search returns all the mail; a thread, the mail naming its root."""
    seen = []

    def log_message(self, *a):
        pass

    def answer(self, mails):
        body = mbox(mails).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/mbox")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        Archive.seen.append(("POST", self.path, self.headers.get("User-Agent", "")))
        # Browsing an inbox searches s:PATCH: it finds the stranger's posting too.
        self.answer(MAILS + STRANGER[:1] if "s%3APATCH" in self.path else MAILS)

    def do_GET(self):
        Archive.seen.append(("GET", self.path, self.headers.get("User-Agent", "")))
        if self.path == "/manifest.js.gz":
            # As lore lists its inboxes: each epoch a repository, with when it changed.
            body = json.dumps({"/netdev/git/0.git": {"modified": 1792000000, "description": "Networking"},
                               "/linux-usb/git/0.git": {"modified": 1791000000, "description": "Linux USB"},
                               "/linux-usb/git/1.git": {"modified": 1793000000, "description": "Linux USB"}}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        root = self.path.split("/")[2]
        self.answer([m for m in MAILS + STRANGER if root in m])


failures = []


def check(name, ok, detail=""):
    print(f"{'ok  ' if ok else 'FAIL'} {name}{': ' + str(detail) if detail and not ok else ''}")
    if not ok:
        failures.append(name)


def link_in(log, pattern=r"(http://127\.0\.0\.1:\d+/#/bridge\S+)"):
    """The first pairing link a server printed to its log, once it has."""
    for _ in range(100):
        found = re.search(pattern, open(log).read(), re.S)
        if found:
            return found.group(1)
        time.sleep(0.1)
    return None


def open_family(m, title, query=""):
    """Go to the series titled `title` from the list; its page's address."""
    m.js("location.hash = '/?who=everyone'; return 1")
    href = m.wait(f"return [...document.querySelectorAll('.row a')].find(a => a.textContent.includes({title!r}))?.getAttribute('href')")
    m.js(f"location.hash = {href!r}.replace(/^#/, '').split('?')[0] + {query!r}; return 1")
    return href


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()
    tmp = tempfile.mkdtemp(prefix="patchtrack-e2e-")
    site = os.path.join(tmp, "site")
    shutil.copytree(os.path.join(WEB, "dist"), site)
    os.makedirs(os.path.join(site, "data"))
    app_port, archive_port, bridge_port, marionette_port = (free_port() for _ in range(4))
    app = f"http://127.0.0.1:{app_port}/"
    origin = app.rstrip("/")
    procs = []
    try:
        static(site, app_port)
        serve(Archive, archive_port)

        # The bridge's machine: its archive is the fake one, it follows the
        # author, and it may send (from your identities) and run b4 on a
        # repository's prep branches. b4 here says what it was asked; mail goes to a file.
        db = os.path.join(tmp, "cli.sqlite3")
        cli = lambda *a: subprocess.run([BIN, "--db", db, *a], check=True, capture_output=True, text=True)
        cli("settings", "set", "archive", f"http://127.0.0.1:{archive_port}")
        cli("settings", "set", "window_days", "365")
        cli("settings", "set", "identities", '["me@x.org"]')
        cli("follow", "me@x.org")
        machine = machine_env(tmp)
        bridge_log = os.path.join(tmp, "bridge.log")
        procs.append(subprocess.Popen([BIN, "--db", db, "bridge", "--port", str(bridge_port), "--app", app,
                                       "--allow-send", "--repo", os.path.join(tmp, "repo")],
                                      stderr=open(bridge_log, "w"), env=machine))

        profile = os.path.join(tmp, "profile")
        os.makedirs(profile)
        with open(os.path.join(profile, "user.js"), "w") as f:
            f.write(f'user_pref("marionette.port", {marionette_port});\n'
                    'user_pref("devtools.console.stdout.content", true);\n')
        procs.append(subprocess.Popen([FIREFOX, "--headless", "--marionette", "--no-remote", "--profile", profile],
                                      stdout=open(os.path.join(tmp, "firefox.log"), "w"), stderr=subprocess.STDOUT))
        m = Marionette(marionette_port)

        # Unpaired, the app has nothing to show but how to connect.
        m.go(app)
        said = m.wait("return document.querySelector('.connect h1')?.textContent")
        check("unpaired, the app asks for a bridge", said == "Connect to your bridge", said)
        check("and shows no pages", m.js("return !document.querySelector('.appbar nav')"))

        # A bridge started for the published app, paired by editing its link to
        # point here: it refuses this origin, and both sides say how to fix it.
        other_port = free_port()
        other_log = os.path.join(tmp, "other-bridge.log")
        procs.append(subprocess.Popen([BIN, "--db", os.path.join(tmp, "other.sqlite3"), "bridge", "--port", str(other_port)],
                                      stderr=open(other_log, "w")))
        other = link_in(other_log, r"#/bridge\?(\S+)")
        m.go(app + "#/bridge?" + other)
        why = m.wait("return document.querySelector('.connect p.error')?.textContent")
        check("a bridge that does not serve this site says so", why and f"--app {origin}/" in why, why)
        for _ in range(50):
            logged = open(other_log).read()
            if "refused" in logged:
                break
            time.sleep(0.1)
        check("and its log says why", f"from {origin}: this bridge does not serve {origin}" in logged, logged[-300:])

        # The right link pairs, and the app shows what the bridge synced on its own.
        link = link_in(bridge_log)
        check("the bridge prints a pairing link", link)
        m.go(link)
        meta = m.wait("return document.querySelector('.row-meta')?.textContent.includes('reviewed') && "
                      "document.querySelector('.row-meta').textContent", 30)
        check("the link pairs, and the bridge's own sync is listed", meta and "reviewed" in meta, meta)
        check("the token is left out of the address", m.js("return location.hash") == "#/")
        check("the pairing is kept in this browser", (m.js("return localStorage.getItem('patchtrack-bridge')") or "").count("token") == 1)
        check("the archive saw one search and one thread, from patchtrack",
              [s[0] for s in Archive.seen[:2]] == ["POST", "GET"] and all(s[2].startswith("patchtrack/") for s in Archive.seen),
              Archive.seen)
        badge = m.wait("return document.querySelector('.sync-badge')?.textContent.startsWith('Synced') && "
                       "document.querySelector('.sync-badge').textContent")
        check("the header says when it last synced", badge and badge.startswith("Synced"), badge)
        m.reload(app)
        check("a reload connects again", m.wait("return !!document.querySelector('.row-meta')"))

        # The list shows what you follow; everyone's is a click away.
        m.js("location.hash = '/?who=everyone'; return 1")
        m.wait("return document.querySelector('.segmented .selected')?.textContent === 'Everyone'")
        # The command line changes the store; the page shows it without a reload.
        published = os.path.join(tmp, "published.mbox")
        with open(published, "w") as f:
            f.write(mbox([f"Message-ID: <pub@x>\nFrom: Other <other@x.org>\nSubject: [PATCH] bar: a published series\n"
                          f"Date: {DATE}\n\n---\n b | 1 +\n"]))
        cli("import", published)
        titles = m.wait("return [...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|').includes('bar: a published') && "
                        "[...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|')", 15)
        check("mail the command line imports shows up without a reload", titles and "bar: a published series" in titles, titles)

        # Adding mail with "only what I follow": into the bridge's store; a stranger's patch is left out; a long reply folds.
        dump = "".join(f"  bLength {i:3}  descriptor line\n" for i in range(100))
        with open(os.path.join(site, "data", "mixed.mbox"), "w") as f:
            f.write(mbox([
                f"Message-ID: <baz@x>\nFrom: Me <me@x.org>\nSubject: [PATCH] baz: mine too\nDate: {DATE}\n\n---\n c | 1 +\n",
                "Message-ID: <dump@x>\nFrom: Rev <rev@x>\nSubject: Re: [PATCH] baz: mine too\nIn-Reply-To: <baz@x>\n"
                f"References: <baz@x>\nDate: {DATE}\n\nThe device says:\n\n{dump}\nWhy?\n",
                f"Message-ID: <stranger@x>\nFrom: Stranger <stranger@x.org>\nSubject: [PATCH] qux: not followed\nDate: {DATE}\n\n"
                "---\n d | 1 +\n",
            ]))
        m.go(app + "#/add")
        m.wait("return document.querySelector('.add-mail .check input')?.disabled === false")
        m.js("document.querySelector('.add-mail .check input').click(); return 1")
        m.wait("return document.querySelector('.add-mail .check input').checked")
        drop(m, "data/mixed.mbox", "mixed.mbox")
        added = m.wait("return document.querySelector('.results li')?.textContent")
        check("only what you follow is kept", added and "3 messages, 2 kept, 2 new" in added, added)
        check("in the bridge's store", "baz: mine too" in cli("list", "--everyone").stdout)
        m.js("location.hash = '/?who=everyone'; return 1")
        rows = m.wait("return [...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|').includes('baz') && "
                      "[...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|')")
        check("and listed, without the stranger's", rows and "qux" not in rows, rows)
        open_family(m, "baz: mine too")
        fold = m.wait("return document.querySelector('.fold.long summary')?.textContent")
        check("a very long reply folds", fold == "… 74 more lines", fold)
        # Polling the bridge must not redraw a page whose store did not change: the
        # same elements, at the same place, two polls later.
        m.js("document.querySelector('.msg').dataset.mark = 'kept'; scrollTo(0, 200); return 1")
        before = m.js("return scrollY")
        time.sleep(9)
        kept = m.js("return document.querySelector('.msg')?.dataset.mark === 'kept' && scrollY")
        check("a page is not redrawn while nothing changes", kept == before and before > 0, (before, kept))

        # A second version: the Changes tab pairs its patches and shows the changelog.
        v2_date = "Mon, 12 Oct 2026 10:00:00 +0000"
        with open(os.path.join(site, "data", "v2.mbox"), "w") as f:
            f.write(mbox([
                f"Message-ID: <v2-cover@x>\nFrom: Me <me@x.org>\nSubject: [PATCH v2 0/1] foo: do the thing\nDate: {v2_date}\n\n"
                "Cover.\n\nChanges in v2:\n- Two lines now.\n\n---\n",
                f"Message-ID: <v2-p1@x>\nFrom: Me <me@x.org>\nSubject: [PATCH v2 1/1] foo: part 1\nIn-Reply-To: <v2-cover@x>\n"
                f"References: <v2-cover@x>\nDate: {v2_date}\n\nThe first part.\n\nSigned-off-by: Me <me@x.org>\n---\n a | 2 ++\n\n"
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -0,0 +1,2 @@\n+x\n+y\n",
            ]))
        m.go(app + "#/add")
        m.wait("return !!document.querySelector('.drop')")
        drop(m, "data/v2.mbox", "v2.mbox")
        m.wait("return document.querySelector('.results li')?.textContent.includes('new')")
        family = open_family(m, "foo: do the thing", "?tab=commits")
        rows = m.wait("return [...document.querySelectorAll('.commits .row.commit')].map(e => e.textContent).join('|')")
        check("Commits lists the latest version's patches", rows and "foo: part 1" in rows and "+2" in rows, rows)
        m.js("document.querySelector('.commits .row.commit a').click(); return 1")
        body = m.wait("return document.querySelector('.commit-body')?.textContent")
        check("a commit opens on its message and its diff, with line numbers", body and "The first part." in body
              and m.js("return [...document.querySelectorAll('.commit-body .file-lines tr.add td.num:nth-child(2)')].map(t => t.textContent).join(',')") == "1,2", body)
        m.js("[...document.querySelectorAll('.content a')].find(a => a.textContent.startsWith('Compare with v1')).click(); return 1")
        pairs = m.wait("return document.querySelector('.changes .pairs') && [...document.querySelectorAll('.changes .pair-head')].map(e => e.textContent).join('|')")
        check("Commits compares with an earlier version, patch by patch", pairs and "foo: part 1" in pairs and "changed" in pairs, pairs)
        log = m.js("return document.querySelector('.changes .changelog')?.textContent")
        check("with the author's changelog", log and "Two lines now" in log, log)
        m.js("document.querySelector('a.tab[data-tab=\"files\"]').click(); return 1")
        cmp = m.wait("return document.querySelector('.files-compared .pair')?.textContent")
        check("Files changed keeps the comparison, file by file", cmp and "changed" in cmp and "++y" in cmp, cmp)
        m.js("""const s = document.querySelector('select[aria-label="Compare with"]'); s.value = '';
                s.dispatchEvent(new Event('change', {bubbles: true})); return 1""")
        files = m.wait("return document.querySelector('.files .file') && [...document.querySelectorAll('.file')].map(f => f.dataset.path).join('|')")
        check("and without one lists each file the version touches", files == "a", files)
        # Review on the diff: a comment on a line, then the review, as mail.
        m.js("[...document.querySelectorAll('.file-lines tr.line.add')].find(r => r.textContent.endsWith('+y')).querySelector('.add-comment').click(); return 1")
        m.wait("return !!document.querySelector('.line-comment-form textarea')")
        m.js("""const t = document.querySelector('.line-comment-form textarea'); t.value = 'Why y?';
                t.dispatchEvent(new Event('input', {bubbles: true})); return 1""")
        time.sleep(0.3)
        m.js("document.querySelector('.line-comment-form button.primary').click(); return 1")
        pending = m.wait("return document.querySelector('.pending-comment')?.textContent")
        check("a comment on a line waits under it", pending and "Pending" in pending and "Why y?" in pending, pending)
        m.js("[...document.querySelectorAll('.review-bar button')].find(b => b.textContent.startsWith('Finish your review')).click(); return 1")
        m.wait("return !!document.querySelector('.review-panel')")
        m.js("""const t = document.querySelector('.review-panel textarea'); t.value = 'Nice series.';
                t.dispatchEvent(new Event('input', {bubbles: true}));
                document.querySelector('.verdicts input[value="Reviewed-by"]').click(); return 1""")
        time.sleep(0.3)
        m.js("document.querySelector('.review-panel button[type=submit]').click(); return 1")
        cards = m.wait("return document.querySelectorAll('.review-drafts .draft').length === 2 && "
                       "[...document.querySelectorAll('.review-drafts .draft')].map(d => d.textContent).join('|')")
        check("finishing the review writes a reply per patch and one to the cover",
              cards and "> +x\n> +y\n\nWhy y?" in cards and "Nice series." in cards and "Reviewed-by: Me <me@x.org>" in cards
              and not m.js("return !!document.querySelector('.pending-comment')"),
              cards or m.js("return document.querySelector('.review-bar')?.textContent"))
        check("kept as drafts in the bridge's store", cli("drafts").stdout.count(" review ") == 1)
        m.js("""const s = document.querySelector('select[aria-label="Version"]'); s.value = '1';
                s.dispatchEvent(new Event('change', {bubbles: true})); return 1""")
        older = m.wait("return location.hash.includes('v=1') && document.querySelector('p.viewing')?.textContent")
        m.js("document.querySelector('a.tab[data-tab=\"conversation\"]').click(); return 1")
        m.wait("return !!document.querySelector('.msg .on-archive')")
        check("the version picker shows an older version", older and "Viewing v1" in older, older)
        m.js("document.querySelector('.get-menu summary').click(); return 1")
        got = m.wait("return [...document.querySelectorAll('.get-row input')].map(i => i.value).join('|')")
        check("Get gives the version's message-id, its archive page and b4's commands",
              got == f"<cover@x>|http://127.0.0.1:{archive_port}/all/cover@x/|b4 shazam cover@x|b4 am cover@x", got)
        link = m.js("return document.querySelector('.msg .on-archive')?.getAttribute('href')")
        check("each message links to the archive", link == f"http://127.0.0.1:{archive_port}/all/cover@x/", link)
        m.js(f"location.hash = {family!r}.replace(/^#/, '').split('?')[0]; return 1")
        m.wait("return !!document.querySelector('.sidebar .close summary')")

        # Close it with a reason; it leaves the open list; reopen it.
        m.js("document.querySelector('.sidebar .close summary').click(); return 1")
        m.js("""const i = document.querySelector('.sidebar .close input'); i.value = 'went elsewhere';
                i.dispatchEvent(new Event('input', {bubbles: true})); return 1""")
        time.sleep(0.3)  # the page renders what was typed before it is submitted, as with a person
        m.js("document.querySelector('.sidebar .close button').click(); return 1")
        closed = m.wait("return document.querySelector('.sidebar')?.textContent.includes('Closed by you: went elsewhere')")
        check("a family can be closed", closed)
        check("in the bridge's store", "went elsewhere" in cli("show", "foo: do the thing", "--everyone").stdout)
        m.js("location.hash = '/'; return 1")
        rows = m.wait("return document.querySelector('.rows') && [...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|') || 'none'")
        check("and leaves the open series", "foo: do the thing" not in rows, rows)
        m.js(f"location.hash = {family!r}.replace(/^#/, ''); return 1")
        m.wait("return !!document.querySelector('.sidebar')")
        m.js("[...document.querySelectorAll('.sidebar button')].find(b => b.textContent === 'Reopen').click(); return 1")
        check("and reopened", m.wait("return !document.querySelector('.sidebar').textContent.includes('Closed by you')"))

        # Notifications: the first look starts from what is here; a new review then
        # shows up, counted in the bar, until it is read.
        m.go(app + "#/notifications")
        caught = m.wait("return document.querySelector('.blankslate h2')?.textContent")
        check("notifications start from what is here", caught == "All caught up", caught)
        # A review of your series, and a reply on someone else's: only yours is a notification.
        with open(os.path.join(site, "data", "review.mbox"), "w") as f:
            f.write(mbox(["Message-ID: <bazrev@x>\nFrom: Rev <rev@x>\nSubject: Re: [PATCH] baz: mine too\n"
                          "In-Reply-To: <baz@x>\nReferences: <baz@x>\nDate: Wed, 07 Oct 2026 10:00:00 +0000\n\n"
                          "Reviewed-by: Rev <rev@x>\n",
                          "Message-ID: <pubrev@x>\nFrom: Rev <rev@x>\nSubject: Re: [PATCH] bar: a published series\n"
                          "In-Reply-To: <pub@x>\nReferences: <pub@x>\nDate: Wed, 07 Oct 2026 10:00:00 +0000\n\n"
                          "Looks fine to me.\n"]))
        m.go(app + "#/add")
        m.wait("return !!document.querySelector('.drop')")
        drop(m, "data/review.mbox", "review.mbox")
        m.wait("return document.querySelector('.results li')?.textContent.includes('new')")
        m.js("location.hash = '/notifications'; return 1")
        lines = m.wait("return [...document.querySelectorAll('.notification .news-lines li')].map(e => e.textContent).join('|')")
        check("then a new review of your series is a notification", lines and "Rev Reviewed-By" in lines and "→ reviewed" in lines, lines)
        titles = m.js("return [...document.querySelectorAll('.notification .row-title')].map(e => e.textContent).join('|')")
        check("and a reply on someone else's is not", titles and "baz: mine too" in titles and "bar: a published" not in titles, titles)
        badge = m.wait("return document.querySelector('.unread-count')?.textContent")
        check("counted in the bar", badge == "1", badge)
        m.js("document.querySelector('.notification .mark-read').click(); return 1")
        read = m.wait("return document.querySelector('.blankslate h2')?.textContent === 'All caught up' && "
                      "!document.querySelector('.unread-count')")
        check("marked as read, it goes, and so does the count", read)
        said = cli("news", "--peek", "--everyone").stdout
        check("read in the bridge's store, as `news` sees it", "baz: mine too" not in said, said)
        with open(os.path.join(site, "data", "review2.mbox"), "w") as f:
            f.write(mbox(["Message-ID: <bazrev2@x>\nFrom: Ann <ann@x>\nSubject: Re: [PATCH] baz: mine too\n"
                          "In-Reply-To: <baz@x>\nReferences: <baz@x>\nDate: Thu, 08 Oct 2026 10:00:00 +0000\n\nWhy?\n"]))
        m.go(app + "#/add")
        m.wait("return !!document.querySelector('.drop')")
        drop(m, "data/review2.mbox", "review2.mbox")
        m.wait("return document.querySelector('.results li')?.textContent.includes('new')")
        badge = m.wait("return document.querySelector('.unread-count')?.textContent", 15)
        open_family(m, "baz: mine too")
        gone = m.wait("return !!document.querySelector('.msg') && !document.querySelector('.unread-count')", 15)
        m.js("location.hash = '/notifications'; return 1")
        caught = m.wait("return document.querySelector('.blankslate h2')?.textContent")
        check("opening a series reads its notification", badge == "1" and gone and caught == "All caught up", (badge, gone, caught))
        m.js("document.querySelector('a.tab[data-tab=\"needs-you\"]').click(); return 1")
        kinds = m.wait("return [...document.querySelectorAll('.attention .kind')].map(e => e.textContent).join('|')")
        check("quiet series need a nudge, in a tab of their own", kinds and ("resend" in kinds or "ping" in kinds), kinds)

        # Settings: a control for each, with the command line's checks, kept in the bridge's store.
        m.go(app + "#/settings")
        m.wait("return !!document.querySelector('#setting-name input')")
        m.js("""const i = document.querySelector('#setting-name input'); i.value = 'Me Myself';
                i.dispatchEvent(new Event('input', {bubbles: true})); return 1""")
        time.sleep(0.3)
        m.js("document.querySelector('#setting-name input').dispatchEvent(new Event('blur')); return 1")
        saved = m.wait("return document.querySelector('#setting-name .status')?.textContent.startsWith('Saved') && "
                       "document.querySelector('#setting-name .status').textContent")
        check("a setting is saved", saved and saved.startswith("Saved."), saved)
        check("in the bridge's store", "Me Myself" in cli("settings").stdout)
        cats = m.js("return [...document.querySelectorAll('.settings-nav a')].map(a => a.textContent).join('|')")
        check("settings come one category at a time, chosen on the left", cats and cats.startswith("You|Projects|Syncing")
              and not m.js("return !!document.querySelector('#setting-window_days')"), cats)
        m.js("document.querySelector('.settings-nav a[data-category=\"syncing\"]').click(); return 1")
        m.wait("return !!document.querySelector('#setting-window_days input') && location.hash.includes('c=syncing')")
        m.js("""const i = document.querySelector('#setting-window_days input'); i.value = '0';
                i.dispatchEvent(new Event('input', {bubbles: true})); return 1""")
        time.sleep(0.3)
        m.js("document.querySelector('#setting-window_days input').dispatchEvent(new Event('blur')); return 1")
        err = m.wait("return document.querySelector('#setting-window_days p.error')?.textContent")
        check("a bad value is refused, as the CLI refuses it", err and "more than 0" in err, err)
        check("sync_every has a control", m.js("return document.querySelector('#setting-sync_every input')?.type") == "number")
        m.js("document.querySelector('.settings-nav a[data-category=\"projects\"]').click(); return 1")
        m.wait("return !!document.querySelector('#setting-projects .presets button')")
        m.js("[...document.querySelectorAll('#setting-projects .presets button')].find(b => b.textContent === 'GCC').click(); return 1")
        card = m.wait("return document.querySelector('#setting-projects .card[data-project=\"GCC\"]')?.textContent")
        check("a built-in project is added", card and "gcc-patches" in card, card)
        m.go(app + "#/?who=everyone")
        tabs = m.wait("return [...document.querySelectorAll('.projects .project')].map(a => a.textContent).join('|')")
        check("the list has its projects", tabs and "All projects" in tabs and "GCC" in tabs, tabs)
        m.js("""const s = document.querySelector('select[aria-label="State"]'); s.value = 'reviewed';
                s.dispatchEvent(new Event('change', {bubbles: true})); return 1""")
        rows = m.wait("return document.querySelector('input.search').value === 'state:reviewed' && "
                      "[...document.querySelectorAll('.row')].map(r => r.querySelector('.row-meta').textContent).join('|')")
        check("the State menu filters by a qualifier in the search", rows and all("reviewed" in r for r in rows.split("|")), rows)
        m.go(app + "#/?who=everyone")
        count = m.wait("return [...document.querySelectorAll('.row')].find(r => r.textContent.includes('bar: a published'))?.querySelector('.row-count')?.textContent.trim()")
        check("a row counts the replies from people", count == "1", count)

        # With identities, the list narrows to your series ("sent or wrote"), and People to who replied to them.
        m.go(app + "#/?who=mine")
        rows = m.wait("return document.querySelector('.segmented .selected')?.textContent === 'Mine' && "
                      "[...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|')")
        check("Mine lists your series only", rows and "foo: do the thing" in rows and "bar: a published series" not in rows, rows)
        m.go(app + "#/people")
        who = m.wait("return [...document.querySelectorAll('.people-table td.who')].map(e => e.textContent).join('|')")
        check("People lists who replied", who and "Rev" in who, who)

        # Reply to the review from the conversation: a draft, addressed to the reviewer, kept under Drafts.
        # The review is on v1; the family opens on its latest version.
        open_family(m, "foo: do the thing", "?v=1")
        m.wait("return !!document.querySelector('.msg[data-msgid=\"rev@x\"] button.reply')")
        event = m.js("return document.querySelector('.version-event')?.textContent")
        check("an older version's conversation says a newer one was posted", event and "v2 was posted" in event and "Compare v1 with v2" in event, event)
        m.js("document.querySelector('.msg[data-msgid=\"rev@x\"] button.reply').click(); return 1")
        m.wait("return !!document.querySelector('.composer textarea')")
        m.js("""const t = document.querySelector('.composer textarea'); t.value = 'Thanks, will do.';
                t.dispatchEvent(new Event('input', {bubbles: true})); return 1""")
        time.sleep(0.3)
        m.js("document.querySelector('.composer button.primary').click(); return 1")
        card = m.wait("return document.querySelector('.draft')?.textContent")
        check("a reply is written as a draft, to the reviewer", card and "rev@x" in card and "Thanks, will do." in card, card)
        mailto = m.js("return document.querySelector('.draft a.button')?.getAttribute('href') || ''")
        check("with a mail link that threads it", mailto.startswith("mailto:rev%40x?") and "in-reply-to=%3Crev%40x%3E" in mailto, mailto)
        check("kept in the bridge's store", "to rev@x" in cli("drafts").stdout)
        m.js("location.hash = '/drafts'; return 1")
        m.wait("return !!document.querySelector('.drafts .draft')")
        m.js("[...document.querySelectorAll('.drafts .draft button')].find(b => b.textContent === 'Mark as sent').click(); return 1")
        sent = m.wait("return document.querySelector('.drafts .draft')?.textContent.includes('Sent ')")
        check("and kept under Drafts, marked sent when you say so", sent)

        # Sent through the bridge, which asks once more, naming the recipients. (To an address
        # git send-email takes: given "rev@x" it sends nothing.)
        open_family(m, "bar: a published")
        m.wait("return !!document.querySelector('.msg[data-msgid=\"pub@x\"] button.reply')")
        m.js("document.querySelector('.msg[data-msgid=\"pub@x\"] button.reply').click(); return 1")
        m.wait("return !!document.querySelector('.composer textarea')")
        m.js("""const t = document.querySelector('.composer textarea'); t.value = 'Sent from the browser.';
                t.dispatchEvent(new Event('input', {bubbles: true})); return 1""")
        time.sleep(0.3)
        m.js("document.querySelector('.composer button.primary').click(); return 1")
        m.wait("return [...document.querySelectorAll('.draft button')].some(b => b.textContent === 'Send…')")
        m.js("[...document.querySelectorAll('.draft button')].find(b => b.textContent === 'Send…').click(); return 1")
        asked = m.wait("return document.querySelector('.send-confirm p')?.textContent")
        m.js("[...document.querySelectorAll('.send-confirm button')].find(b => b.textContent === 'Send it').click(); return 1")
        if send_email():
            sent_mail = ""
            for _ in range(100):
                path = os.path.join(tmp, "sent.eml")
                sent_mail = open(path).read() if os.path.exists(path) else ""
                if "Sent from the browser." in sent_mail:
                    break
                time.sleep(0.1)
            check("a draft is sent through the bridge, once asked", asked and "other@x.org" in asked
                  and "Sent from the browser." in sent_mail and "In-Reply-To: <pub@x>" in sent_mail, (asked, sent_mail[:300]))
        else:
            # Nothing can be sent here: the app must say why, and keep the draft unsent.
            said = m.wait("return document.querySelector('.send-confirm p.error')?.textContent")
            check("without git send-email, the bridge says so and keeps the draft", asked and said
                  and "send-email is not installed" in said, (asked, said))
            print("skip a draft is sent through the bridge: git send-email is not installed here")

        # Branches: the bridge's b4 prep branches beside their series; a dry run, then for real.
        m.go(app + "#/branches")
        row = m.wait("return document.querySelector('.branch[data-branch=\"b4/foo\"]')?.textContent")
        check("a b4 branch is listed with its series", row and "foo: do the thing" in row and "v1 sent" in row, row)
        m.js("[...document.querySelectorAll('.branch button')].find(b => b.textContent === 'Preview v2').click(); return 1")
        out = m.wait("return document.querySelector('.b4-run .log')?.textContent")
        m.js("[...document.querySelectorAll('.b4-run button')].find(b => b.textContent.startsWith('Send v2')).click(); return 1")
        done = m.wait("return document.querySelector('.b4-run p.ok')?.textContent")
        b4_log = open(os.path.join(tmp, "b4.log")).read()
        check("b4 runs a dry run first, then for real", out and "send --dry-run" in out and done == "Done."
              and b4_log == "send --dry-run\nsend\n", (out, done, b4_log))
        m.js(f"location.hash = {family!r}.replace(/^#/, ''); return 1")
        side = m.wait("return [...document.querySelectorAll('.sidebar section')].find(s => s.textContent.startsWith('Branch'))?.textContent")
        check("a series' page shows the branch that posted it", side and "b4/foo" in side and "v2 in preparation" in side, side)

        # Follow a whole list: the bridge syncs it at once, searching the inbox by date alone.
        m.go(app + "#/following")
        m.wait("return !!document.querySelector('.following .follow select')")
        m.js("""const s = document.querySelector('.following .follow select'); s.value = 'list';
                s.dispatchEvent(new Event('change', {bubbles: true})); return 1""")
        m.wait("return document.querySelector('.following .follow input.value').placeholder.startsWith('inbox')")
        m.js("""const i = document.querySelector('.following .follow input.value'); i.value = 'linux-usb';
                i.dispatchEvent(new Event('input', {bubbles: true})); return 1""")
        time.sleep(0.3)
        before = len(Archive.seen)
        m.js("document.querySelector('.following .follow button').click(); return 1")
        labels = m.wait("return [...document.querySelectorAll('.following-list li')].map(e => e.textContent).join('|')"
                        " .includes('every series on linux-usb') && "
                        "[...document.querySelectorAll('.following-list li')].map(e => e.textContent).join('|')")
        check("a list can be followed", labels and "every series on linux-usb" in labels, labels)
        for _ in range(150):
            searches = [p for (meth, p, _) in Archive.seen[before:] if meth == "POST"]
            if any(p.startswith("/linux-usb/?x=m&q=rt%3A") for p in searches):
                break
            time.sleep(0.1)
        check("and the bridge syncs it at once, searching the inbox by date alone",
              any(p.startswith("/linux-usb/?x=m&q=rt%3A") for p in searches), searches)
        log = m.wait("return (document.querySelector('.following .log')?.textContent || '').includes('linux-usb') && "
                     "document.querySelector('.following .log').textContent", 30)
        check("and the page shows what that sync said", log, log)
        m.js("[...document.querySelectorAll('.following-list li')].find(li => li.textContent.includes('linux-usb'))"
             ".querySelector('button').click(); return 1")
        gone = m.wait("return ![...document.querySelectorAll('.following-list li')].some(li => li.textContent.includes('linux-usb'))")
        check("and unfollowed", gone)
        m.wait("return document.querySelector('.sync-badge')?.textContent.startsWith('Synced')", 30)
        before = len(Archive.seen)
        m.js("[...document.querySelectorAll('.following button')].find(b => b.textContent === 'Sync now').click(); return 1")
        for _ in range(100):
            if len(Archive.seen) > before:
                break
            time.sleep(0.1)
        check("Sync now syncs", len(Archive.seen) > before, Archive.seen[before:])

        # The inboxes, as the archive lists them, newest first; "all" is not one to list.
        m.go(app + "#/inboxes")
        names = m.wait("return [...document.querySelectorAll('.inbox-list li a')].map(a => a.textContent).join('|').includes('netdev') && "
                       "[...document.querySelectorAll('.inbox-list li a')].map(a => a.textContent).join('|')", 30)
        check("the inboxes come from the archive's list, newest first", names and names.index("linux-usb") < names.index("netdev"), names)
        manifests = [a for a in Archive.seen if a[1] == "/manifest.js.gz"]
        m.go(app + "#/inboxes/all")
        said = m.wait("return document.querySelector('.inbox-main p.error')?.textContent")
        check("all is refused, with why, and nothing is listed",
              said and "every inbox" in said and not m.js("return document.querySelector('.inbox-head')?.textContent.includes('Listing')"), said)
        m.go(app + "#/inboxes")
        m.wait("return !!document.querySelector('.inbox-list li a')")
        check("the archive's list is read once an hour, not on every visit", len([a for a in Archive.seen if a[1] == "/manifest.js.gz"]) == len(manifests) == 1, manifests)

        # An inbox, browsed: one search lists its series, and no thread is fetched
        # until one is opened, which fetches it.
        before = len(Archive.seen)
        m.go(app + "#/inboxes/linux-usb")
        row = m.wait("return [...document.querySelectorAll('.inbox-main .row')].find(r => r.textContent.includes(\"a stranger's fix\"))?.textContent", 30)
        asked = Archive.seen[before:]
        check("an inbox opened for the first time is listed, in one search", row and "not fetched" in row
              and [a[0] for a in asked] == ["POST"] and "s%3APATCH" in asked[0][1] and asked[0][1].startswith("/linux-usb/"), (row, asked))
        m.js("[...document.querySelectorAll('.inbox-main .row a.title-link')].find(a => a.textContent.includes(\"a stranger's fix\")).click(); return 1")
        reply = m.wait("return !!document.querySelector('.msg[data-msgid=\"stranger-r@x\"]') && !document.querySelector('.unfetched-note, .fetching')", 30)
        threads = [a[1] for a in Archive.seen[before:] if a[0] == "GET"]
        check("opening a series fetches its thread, once", reply and threads == ["/linux-usb/stranger@x/t.mbox.gz"], (reply, threads))
        time.sleep(1)
        m.reload(app, m.js("return location.hash"))
        m.wait("return !!document.querySelector('.msg[data-msgid=\"stranger-r@x\"]')")
        time.sleep(1)
        threads = [a[1] for a in Archive.seen[before:] if a[0] == "GET"]
        check("opened again soon after, it is not fetched again", len(threads) == 1, threads)
        m.js("document.querySelector('button.refresh').click(); return 1")
        for _ in range(100):
            threads = [a[1] for a in Archive.seen[before:] if a[0] == "GET"]
            if len(threads) == 2:
                break
            time.sleep(0.1)
        check("Refresh fetches it again", len(threads) == 2, threads)
        m.go(app + "#/inboxes/linux-usb")
        row = m.wait("return [...document.querySelectorAll('.inbox-main .row')].find(r => r.textContent.includes(\"a stranger's fix\"))?.textContent")
        check("and the inbox no longer says it is not fetched", row and "not fetched" not in row, row)
        m.go(app + "#/")
        mine = m.wait("return !!document.querySelector('.row') && [...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|')")
        m.go(app + "#/?who=everyone")
        everyone = m.wait("return [...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|').includes('stranger') && "
                          "[...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|')")
        check("the series list shows what you follow, and everyone's a click away", mine and "stranger" not in mine and everyone, (mine, everyone))
        m.go(app + "#/inboxes/linux-usb")
        listed = len(Archive.seen)
        m.wait("return [...document.querySelectorAll('.inbox-main button')].some(b => b.textContent === 'Load older' && !b.disabled)")
        m.js("[...document.querySelectorAll('.inbox-main button')].find(b => b.textContent === 'Load older').click(); return 1")
        for _ in range(100):
            if len(Archive.seen) > listed:
                break
            time.sleep(0.1)
        older = Archive.seen[listed:]
        rt = re.search(r"rt%3A(\d+)\.\.(\d+)", older[0][1]) if older else None
        check("Load older searches the two days before what was listed", rt and int(rt.group(2)) - int(rt.group(1)) == 2 * 86400
              and int(rt.group(2)) < time.time() - 86400, older)

        # A bridge reached from elsewhere: behind a tunnel that speaks HTTPS (here a proxy with a
        # certificate of its own) and named by --public-url. The app asks before it pairs.
        proxy_port, remote_port = free_port(), free_port()
        remote_log = os.path.join(tmp, "remote-bridge.log")
        procs.append(subprocess.Popen([BIN, "--db", db, "bridge", "--port", str(remote_port), "--app", app,
                                       "--public-url", f"https://127.0.0.1:{proxy_port}"], stderr=open(remote_log, "w")))
        https_proxy(tmp, proxy_port, remote_port)
        remote_link = link_in(remote_log, r"From anywhere.*?\n\n\s*(\S+)")
        check("a bridge with --public-url prints a link through it", remote_link and f"url=https://127.0.0.1:{proxy_port}&" in remote_link, remote_link)
        m.go(remote_link)
        asked = m.wait("return document.querySelector('.pair-confirm')?.textContent")
        m.js("[...document.querySelectorAll('.pair-confirm button')].find(b => b.textContent.startsWith('Don')).click(); return 1")
        m.wait("return !!document.querySelector('.row')")
        kept = m.js("return JSON.parse(localStorage.getItem('patchtrack-bridge')).url")
        check("a bridge elsewhere is asked about, and not taken unasked", asked and f"127.0.0.1:{proxy_port}" in asked
              and kept and kept.startswith("http://127.0.0.1:"), (asked, kept))
        m.go(remote_link)
        m.wait("return !!document.querySelector('.pair-confirm')")
        m.js("[...document.querySelectorAll('.pair-confirm button')].find(b => b.textContent === 'Connect').click(); return 1")
        rows = m.wait("return !!document.querySelector('.row') && JSON.parse(localStorage.getItem('patchtrack-bridge')).url")
        check("and once confirmed, it shows the store through the tunnel", rows == f"https://127.0.0.1:{proxy_port}", rows)

        # A phone: the pages fold into a menu, which a choice closes; nothing scrolls sideways.
        m.cmd("WebDriver:SetWindowRect", {"width": 500, "height": 800})
        m.go(app + "#/")
        # Choosing a page closes the menu: let the navigation land before opening it.
        m.wait("return location.hash === '#/' && !!document.querySelector('.row') && "
               "getComputedStyle(document.querySelector('.menu-toggle')).display !== 'none'")
        time.sleep(0.3)
        hidden = m.js("return getComputedStyle(document.querySelector('.appbar nav')).display === 'none'")
        m.js("document.querySelector('.menu-toggle').click(); return 1")
        shown = m.wait("return getComputedStyle(document.querySelector('.appbar nav')).display === 'flex'")
        m.js("[...document.querySelectorAll('.appbar nav a')].find(a => a.textContent.includes('People')).click(); return 1")
        closed = m.wait("return location.hash.startsWith('#/people') && getComputedStyle(document.querySelector('.appbar nav')).display === 'none'")
        wide = m.js("return document.documentElement.scrollWidth")
        check("on a narrow screen the pages are a menu", hidden and shown and closed and wide <= 500, (hidden, shown, closed, wide))

        # Last: it leaves this origin.
        # patchtrack serve: the app and its API on one origin, paired from its own link.
        serve_port = free_port()
        serve_log = os.path.join(tmp, "serve.log")
        procs.append(subprocess.Popen([BIN, "--db", db, "serve", "--listen", f"127.0.0.1:{serve_port}",
                                       "--public-url", f"http://127.0.0.1:{serve_port}", "--app-dir", site],
                                      stderr=open(serve_log, "w"), env=machine))
        m.go(link_in(serve_log))
        rows = m.wait("return [...document.querySelectorAll('.row-title')].map(e => e.textContent).join('|')")
        check("serve serves the app, and pairs with it on its own origin", rows and "foo: do the thing" in rows, rows)
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            try:
                p.wait(10)
            except subprocess.TimeoutExpired:
                p.kill()
        if args.keep:
            print("kept", tmp)
        else:
            shutil.rmtree(tmp, ignore_errors=True)
    print(f"{len(failures)} failed" if failures else "all passed")
    sys.exit(1 if failures else 0)


def https_proxy(tmp, port, target):
    """An HTTPS front for 127.0.0.1:target, as a tunnel is: a certificate of its own, the Host kept."""
    import http.client
    import ssl
    cert, key = os.path.join(tmp, "cert.pem"), os.path.join(tmp, "key.pem")
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", key, "-out", cert, "-days", "1",
                    "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1"], check=True, capture_output=True)

    class Proxy(http.server.BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def forward(self):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            c = http.client.HTTPConnection("127.0.0.1", target, timeout=60)
            c.request(self.command, self.path, body, dict(self.headers))
            r = c.getresponse()
            data = r.read()
            self.send_response(r.status)
            for k, v in r.getheaders():
                if k.lower() not in ("content-length", "connection", "transfer-encoding"):
                    self.send_header(k, v)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        do_GET = do_POST = do_OPTIONS = forward

    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Proxy)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def send_email():
    """Is git send-email here? (Debian and Fedora package it apart.)"""
    out = subprocess.run(["git", "--exec-path"], capture_output=True, text=True).stdout.strip()
    return os.path.exists(os.path.join(out, "git-send-email"))


def machine_env(tmp):
    """A git config with a sendmail command writing sent mail to tmp/sent.eml; a fake b4 on
    PATH logging to tmp/b4.log; and a repository with a b4 prep branch whose v1 is cover@x."""
    with open(os.path.join(tmp, "gitconfig"), "w") as f:
        f.write(f'[user]\n\tname = Me\n\temail = me@x.org\n[sendemail]\n'
                f'\tsendmailcmd = "f() {{ cat > {tmp}/sent.eml; }}; f"\n')
    env = dict(os.environ, GIT_CONFIG_GLOBAL=os.path.join(tmp, "gitconfig"), GIT_CONFIG_NOSYSTEM="1")
    os.makedirs(os.path.join(tmp, "bin"))
    b4 = os.path.join(tmp, "bin", "b4")
    with open(b4, "w") as f:
        f.write(f"#!/bin/sh\necho \"$*\" >> {tmp}/b4.log\necho \"b4 would $*\"\n")
    os.chmod(b4, 0o755)
    env["PATH"] = os.path.join(tmp, "bin") + os.pathsep + env["PATH"]
    repo = os.path.join(tmp, "repo")
    git = lambda *a: subprocess.run(["git", "-C", repo, *a], check=True, capture_output=True, env=env)
    os.makedirs(repo)
    git("init", "-q", "-b", "main")
    git("commit", "-q", "--allow-empty", "-m", "base")
    git("checkout", "-q", "-b", "b4/foo")
    git("commit", "-q", "--allow-empty", "-m", "foo: do the thing\n\n--- b4-submit-tracking ---\n# internal\n"
        '{"series": {"revision": 2, "change-id": "20261001-foo-abc", "prefixes": [], "history": {"v1": ["cover@x"]}}}\n')
    git("checkout", "-q", "main")
    return env


def drop(m, url, name):
    m.js("""(async () => {
      const buf = await (await fetch(arguments[0])).arrayBuffer();
      const dt = new DataTransfer();
      dt.items.add(new File([buf], arguments[1]));
      document.querySelector(".drop").dispatchEvent(new DragEvent("drop", {dataTransfer: dt, bubbles: true, cancelable: true}));
    })(); return 1;""".replace("arguments[0]", repr(url)).replace("arguments[1]", repr(name)))


if __name__ == "__main__":
    main()
