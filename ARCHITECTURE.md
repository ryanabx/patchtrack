# patchtrack: architecture

How patchtrack is built, and why: the owner's decisions and what was
measured. The Python version was the reference until the Rust version
matched it: its design rules, its calibrated heuristics and its test
cases carried over; its code did not. The tag `python-last` is the last
commit with both, for parity checks (AGENTS.md says how).

## What it is for

GitHub's workflow for patches sent by email. First "my pull requests": an
author's series and where each stands. Then the views and actions that
make that useful, for any series on any list.

1. **A web app, over your own machine.** The app is served from a static
   host (GitHub Pages) or by `patchtrack serve`, and shows the store of a
   machine of yours through `patchtrack bridge`, from that machine or,
   behind a tunnel, from your phone.
2. **From the archive or from your own mail.** Public-inbox archives
   (lore.kernel.org first), and your own mail (lei's output, Maildir,
   Thunderbird's mbox, `.eml`), so lore does not have to carry every
   client.
3. **A CLI and a TUI**, on the same store.
4. **Acting on mail.** Replies, reviews, tags, thanks and pings, composed
   as plain-text mail and sent by a person; b4 driven where it holds the
   state.
5. **Any series**, not only your own.

## Constraints

These decide the shape more than any preference does.

- **A page on another origin cannot read lore.** Measured 2026-10-28,
  with `Origin: https://ryanabx.github.io`: the help page (200), the
  search export (`POST ?x=m`) and a thread's `t.mbox.gz` (both 404, for
  a query that matched nothing) carried no `Access-Control-Allow-Origin`,
  so a browser will not hand them to a page served from GitHub Pages.
  lore also runs Anubis, which a `fetch()` cannot answer. **The app does
  not fetch lore, and we do not ask kernel.org for CORS headers**
  (decided 2026-10-29). A public proxy is ruled out too: it would carry
  every user's traffic and stand between lore's bot check and the
  clients it is meant to see.
- **A browser cannot send mail** (no SMTP, no raw sockets) **or run git**
  at the size of a kernel tree.

So the native binary does everything, and the web app is its client
(decided 2026-10-29): the store lives with `patchtrack bridge`, which
syncs it, derives what the app shows, and sends mail and runs b4 when you
allowed it. The app keeps nothing but the bridge's address and token.
An app with a store of its own (SQLite on OPFS, built and measured
2026-10-28) was not much use without a way to sync, which only the bridge
could do, and importing mail files into a browser was clumsy.

## Shape

```
        ┌──────────────── patchtrack-core (Rust: no I/O, no async) ────────────────┐
        │ model     Message, Thread, Series, Family, State and its Evidence       │
        │ derive    series, families, states; review and "applied" heuristics;    │
        │           project profiles; version diffs; nudges; news                 │
        │ sources   public-inbox (queries, windows, mbox.gz), mbox, Maildir,      │
        │           .eml; the polite request scheduler                            │
        │ compose   replies, reviews, tags, thanks, pings, as RFC 5322 bytes      │
        │ trees     which commits are which patches, given the commits            │
        └────────────────────────────────▲─────────────────────────────────────────┘
                     Request ──▶ host     │     commits, files, bytes
                     bytes   ◀── host     │
        ┌─────────────────────────────────┴────────────────────────────────────────┐
        │ the `patchtrack` binary                                                  │
        │   HTTP ureq · store SQLite (rusqlite) · git subprocess · git send-email  │
        │   CLI (clap) · TUI (ratatui) · bridge/serve: HTTP API, its own syncs     │
        └────────────────────────────────▲─────────────────────────────────────────┘
                                         │ JSON over HTTP (paired by a token)
        ┌────────────────────────────────┴─────────────────────────────────────────┐
        │ the web app: TypeScript, Preact, Vite; keeps nothing                     │
        └──────────────────────────────────────────────────────────────────────────┘
```

### The core does no I/O

The core plans; the host does. A sync is a state machine: the core says
what to fetch and not before when, the host fetches it, and hands the
bytes back.

```rust
let mut sync = Sync::new(plan, known);
while let Some(effect) = sync.next(now) {   // Fetch(Request { id, url, not_before }), Log, or a fact to store
    ...                                      // the host fetches, and answers with sync.receive(id, outcome, now)
}
```

- **Politeness is written once.** Delays between requests, backoff,
  `Retry-After`, one request per thread, incremental searches and
  windows a result cap cannot truncate live in the core.
- **Tests are pure.** Bytes in, derived state out; sync is tested against
  an archive in memory and a simulated clock.

Mail parsing is the core's (`mail-parser`: no I/O). Reading a directory is
the host's: it hands the core each file's bytes.

## Layout

```
Cargo.toml          the workspace
crates/core/        patchtrack-core: everything above the host; no I/O
crates/store/       patchtrack-store: the schema, migrations and queries (rusqlite)
crates/cli/         the `patchtrack` binary: CLI and TUI; HTTP, git, sending;
                    the bridge and its API, for the web app
web/                the web app: TypeScript, Preact, Vite
tests/fixtures/     synthetic fixtures
tools/parity.py     (at `python-last`) the Python version against the Rust one
```

`rust-toolchain.toml` pins the compiler (1.98.1): `mail-parser` 1.0
needs 1.98.

## The store: one SQLite file

**The database file is the interchange format**: exporting is copying it
(`VACUUM INTO`), and any patchtrack that reads it reads everything.

- **Raw facts and your decisions, nothing derived.** Messages; when each
  thread was fetched and from where; tree check results; subscriptions;
  and what only you can say: closed families, what you last read
  (`seen`), drafts. Series, families and states are recomputed on every
  read, so a better heuristic applies to old data without a refetch.
- **Each message's raw bytes**, plus the header columns queries need.
  Re-parsing with a better parser needs no refetch, an exported message
  is byte for byte what was received, and a reply can quote exactly.
- **Importing merges.** Messages are keyed by message-id and the same
  bytes; facts keep the latest. Importing another store loses nothing of
  yours.
- **Versioned.** `PRAGMA user_version` counts the migrations, applied in
  order on open, appended only. A store from a newer patchtrack is
  refused, not guessed at. Opening writes nothing unless there is a
  migration to apply: the bridge watches `PRAGMA data_version`, and a
  commit of nothing would look like a change.
- **Your decisions stay yours.** An export for publishing (`--public`)
  leaves out drafts, what you read and closed reasons.

## Where mail comes from

| source | how |
|---|---|
| public-inbox over HTTP (lore) | `sync`, and the bridge's own syncs |
| lei output, Maildir, mbox, `.eml` directories | `source add PATH`: read on every sync, keeping what you follow |
| any mbox, `.eml` or patchtrack store | `import FILE`, or dropped on the web app (imported by the bridge) |

Local mail comes first. The archive fills in only threads local mail
cannot complete (a reply that did not copy you), and says so in the "as
of" text.

## Settings and subscriptions

**Settings live in the store**, in a `settings` table of JSON values the
core validates: your addresses, the archive, sync windows and frequency,
nudge thresholds, project profiles, bot rules. There is no config file,
so an exported store carries them, and the CLI and the web app accept the
same values. On import, a store's settings fill only the keys you have
not set, and its subscriptions are taken only into a store that follows
nothing; a `--public` export leaves settings out. A key from a newer
patchtrack is skipped with a warning, not refused. What is
machine-specific (local mail paths, git trees, the bridge's token) lives
in tables that are never exported. Command-line options override
settings for one run.

**What a file brings in**: `import FILE` keeps everything in it, since a
file you name is something you want to see (someone else's thread from
`b4 mbox`, a list's archive); `import --only-followed` and the `source`
paths keep only the series of the authors you follow and your
identities, and the replies in their threads.

### Projects

Several archives at once, as GitHub has repositories: a project is a
setting with a name, an archive, its inboxes there and recipient globs
for your own mail; the core has presets (Linux, U-Boot, QEMU, Git, Xen,
buildroot, GCC, glibc). Which projects a series is in is derived, never
stored. Every subscription names its archive, and a sync runs per
archive, so politeness is per host; a thread is fetched from the inbox it
was found in, as not every archive serves lore's `/all/`. For the kernel,
a project's lists are its subsystems: the web app filters by them.

### What you follow

The working set is the union of subscriptions:

- an **author**: their series, and replies to them ("my pull requests");
- a **list**: every series posted to it (a maintainer's queue);
- a **query**: a public-inbox search;
- a **thread**: one series, from a pasted URL or message-id.

"Mine" is a filter by your configured addresses, not the model. Sync
records are kept per subscription, and which subscription found a thread
is a stored fact (`found_by`), so "what you follow" is one rule
(`views::follows`) for the CLI's default list, the web app's Followed
filter and People. A list's or a query's first sync goes back only
`list_since` (two weeks by default).

## The bridge and the web app

`patchtrack bridge` (on 127.0.0.1) and `patchtrack serve` (behind your
HTTPS proxy, serving the built app too) are one server:

- **It answers from the store** (`crates/cli/src/api.rs`): `GET`s read
  and derive, as the command line does; `POST`s of JSON change it
  (settings, follows, closing, drafts, reviews, what you read, imports).
  It derives once per change of the store: `PRAGMA data_version`, on a
  connection of its own, says when any other connection (its own
  requests' or a command's in a terminal) committed. The same number, in
  `/v1/status`, tells the app to read again; the app asks every few
  seconds while it is shown, and refreshes in place.
- **It syncs on its own**: when it starts, every `sync_every` minutes (a
  setting, 30 by default; 0 for only when asked), after a follow, and on
  "Sync now"; one sync at a time, the same code as `patchtrack sync`.
  Its log is in `/v1/status`.
- **Who may ask**: requests naming another Host than 127.0.0.1, localhost
  or the names it was given are refused (DNS rebinding); CORS is
  answered for the app's origins only, never `*`; every API request
  carries the pairing token, and one without it is refused before its
  body is read. The token reads and changes the store: the link that
  carries it is to be kept to yourself.
- **What this machine does for the app** only when the command allowed
  it: send a stored draft with `git send-email` (`--allow-send`, and only
  From one of your identities), and run b4 on the repositories it was
  given (`--repo`).

### The bridge in today's browsers

Measured 2026-10-28, headless Chrome 156 and Firefox 158; a page over
https on one port fetching a stand-in bridge over http on another, both
on 127.0.0.1:

- *Mixed content is not the obstacle.* Both let an https page fetch
  `http://127.0.0.1`: loopback counts as potentially trustworthy.
- *From a local page* the request is ordinary CORS: a preflight
  (`OPTIONS`, for the token header or a JSON body), then the request.
- *From a public page* (GitHub Pages; simulated with Chrome's
  `--ip-address-space-overrides` and Firefox's
  `network.lna.address_space.public.override`), both browsers gate it on
  a per-site **loopback-network permission**, a prompt for a real user.
  Until it is granted nothing reaches the bridge, not even the
  preflight. Once it is granted, the same plain CORS flow works. Firefox
  logs every such request to the console ("Local Network Access
  detected … prompt action: auto_allow") once it is allowed.
- So the app asks to be paired from a link the bridge prints, and says
  why a prompt follows. `serve` avoids the question altogether: the app
  and its API share an origin.

### From anywhere, and your own server

The owner's case (2026-10-29): the bridge runs on a desktop, and the
published web app on a phone uses it from anywhere. A page served over
https reaches plain http only on its own computer, so the bridge is put
behind a tunnel or proxy that speaks HTTPS (`tailscale serve`,
Cloudflare Tunnel, Caddy) and told that address (`--public-url`): it
still listens on 127.0.0.1, and also answers requests naming that host.
The app takes a bridge on its own computer, or the server that served
it, as it is; a bridge elsewhere only over https and once the person
confirms it, naming it, since a link could otherwise point the app at
someone else's server.

## Acting on mail

The core composes; the binary delivers; a person decides.

- **Composed in the core**: replies (In-Reply-To, References, reply-all
  as b4 addresses it, quoting as `b4 ty` does), review tags from your
  configured identity, reviews on the diff (each comment below its hunk,
  quoted up to its line), b4-style thanks, pings. Plain text, 7bit or
  8bit, never quoted-printable or flowed.
- **Kept as drafts** in the store (a decision of yours, like `closed`;
  never in a public export), so a closed tab loses nothing.
- **Delivered through `git send-email`**, with your `sendemail.*`
  configuration and git's credential handling: no SMTP library, no
  password in patchtrack, the same on Windows. Recipients come from the
  draft alone (`--no-to --no-cc --no-bcc`), and the draft's own headers
  thread it. `send` shows the whole message and sends only when the
  person types "yes"; the web app asks once more, naming the recipients.
  send-email exits 0 having sent nothing when it cannot read an address,
  so delivery requires its "Result: OK" (or 2xx) line.
- **Or handed over** from the web app as a `.eml` file, the clipboard or
  a `mailto:` link (short bodies only; clients may drop threading
  headers).

### What goes through b4

The owner's direction (2026-10-29): an integrated GitHub-like experience
that uses b4 wherever b4 does the job. b4 owns a series' life in the
author's git repository; patchtrack owns what the mail says about it.

| action | who does it | why |
|---|---|---|
| a new version, a RESEND | **b4** (`b4 send`, `b4 send --resend`), run by the bridge on the prep branch | b4 keeps the branch, cover and revision |
| tags received into the branch | **b4** (`b4 trailers -u`) | b4 rewrites the commits; patchtrack says which tags are new since the branch was sent |
| apply a series (a maintainer) | **b4** (`b4 shazam MSGID`, the series page's "Get") | patchtrack knows the message-id and the latest version |
| thanks for applied patches | **patchtrack**, in `b4 ty`'s exact form | `b4 ty` only knows what `b4 shazam`/`am` applied; the tree check knows every commit, however it landed |
| a reply, a review tag, a review on the diff | **patchtrack** | b4 has no reply command |
| a ping | **patchtrack** (`nudge`) | |

The bridge runs each b4 action first as a dry run, in a worktree of the
branch (your checkout is never switched), feeding one Enter so failing
pre-flight checks stop a send.

## Tree checks

The binary runs git (whole branches, never shallow) and hands the core
each commit's sha, subject, trailers and patch-id; the core decides which
posted patch each one is, matching only on what names the patch (a
`Link:`/`Message-ID:` trailer, patch-id over the full diff, subject).

## Frontends

- **CLI and TUI** are one binary: `clap` for commands, `ratatui` for the
  TUI. Each command's documentation is its help text; the book (mdBook)
  generates a page per command from it.
- **The web app**: TypeScript with Preact, built by Vite, over the
  bridge's API. The model's types are generated from the Rust (`ts-rs`,
  behind the core's `ts` feature), so the core defines the contract and
  the compiler checks it. The boundary is coarse: a request returns a
  whole view (a family with its versions, a conversation, a version
  diff). It has GitHub's shapes: a list with qualifiers, a series as a
  pull request (Conversation, Commits, Files changed, versions compared),
  review on the diff, notifications.
- **Mail is text, never markup**, in every frontend.
- The Python version's `board` and JSON API (`export DIR`, `serve`) are
  not ported; `export` and `serve` name other things now.

## Design rules

- **Every state carries its evidence**: the message-id and the line that
  justifies it, and the time the thread was last fetched. A state the
  tool cannot point at a message for is labelled as such.
- **Say only what the mail says.** `unanswered` means someone wrote after
  the author, not that the author owes a reply.
- **Be polite to the archive.** Read-only, one search per subscription,
  one request per thread, a delay between requests, an honest
  `User-Agent`. Never work around a bot check; report it.
- **Never make a git history shallow**, and match commits only on what
  names the patch.
- **Few dependencies**, each earning its place; none that pulls in an
  async runtime or does I/O behind the core's back.

## Documentation and releases

- **Documentation is an mdBook** (`book/`): hand-written pages for
  concepts, quick start, projects, scheduling and the web app, and
  generated pages for each command and for the settings.
- **Releases are GitHub releases**, built by a release workflow, with
  artifacts for Linux x86_64, Windows x86_64 and the web app, and
  checksums. **The native binary must work on Windows**: paths through a
  platform directories crate, no Unix tools shelled out to, CI on
  Windows too.
- **crates.io** gets `patchtrack-core`, `patchtrack-store` and the
  `patchtrack` binary, once the owner decides to publish.

## Testing

- **The core**: pure tests on synthetic mail. A new heuristic gets a
  positive and a negative case, and its test must fail when the
  heuristic is broken (plant the bug, watch it fail, revert).
- **Parity with the Python version**: one mailbox through both, families
  and states compared, from a worktree of `python-last` (AGENTS.md).
- **The binary**: integration tests against an archive on 127.0.0.1 and
  throwaway git repositories; the bridge's API tested on a temporary
  store.
- **The web app**: unit tests for its logic without the DOM, and
  `web/e2e/run.py`: the built app in headless Firefox against a real
  bridge and a fake archive.

## Measurements and validation

- **Grouping matches the Python version.** One day of linux-usb from lore
  (109 messages, 11 families, two with two versions): the same families,
  versions, roots and patch numbers from both.
- **States and evidence match the Python version.** The derivation
  (replies, tags, "applied" lines, inline comments, tree stages,
  superseding, families, profiles, bots, closing) is ported with its
  tests, plus three the Python suite lacked. On a week each of
  linux-usb, u-boot and gcc-patches (2,970 messages, 408 families), every
  family's state, evidence, and every version's state, evidence and
  `unanswered` are the same, except 45 that differ only by a posting
  someone else sent (below). The same run found a crash in the Python
  version (a part in charset `unknown-8bit`, on u-boot) that the Rust
  parser does not have.
- **Sync matches the Python version.** Its tests are the Python version's
  sync and retry tests. Live against lore, a small author's sync made the
  same requests in the same order as the Python version's and derived
  the same states.
- **Speed.** At 20,000 synthetic messages (1,000 series of a cover, 4
  patches and 15 replies; 22.5 MB of mbox), the store imports them in
  0.34 s and reads and groups them in 0.08 s (2026-10-28; the Python
  version built them in 0.22 s and stored them in 10 s). The bridge
  derives once per change of the store, so the app's many reads of an
  unchanged store cost nothing. The store is 2.4 times its mbox:
  compressing `raw`, read only to re-parse, export or quote, is the
  obvious saving.
- **Deliberate differences from the Python version:**
  - *Families are per sender.* A patch joins its parent's series only
    when both have the same sender, and a family is one sender's
    versions. The Python version grouped by tracked author, counting a
    posting the author wrote but someone else sent as theirs. On
    gcc-patches that let a forge's test repost of 59 patches supersede
    their authors' own postings. "Sent or wrote" stays the rule for what
    is *yours*, as a filter when listing.
  - *Anyone else's patch in your thread is a reply* to your series, as
    well as a series of its own.
  - *The first of a repeated header counts.* public-inbox gives a
    second copy of a message-id a `Message-ID:` of its own after the
    sender's; mail-parser reads the last, so the core asks for the first.

## Open questions

- The browsers' loopback-network prompts: what they look like to a
  person, and whether a refusal is remembered.
- A Thunderbird add-on as another client of the bridge?
- Can b4's web submission endpoint carry replies, or only series?
