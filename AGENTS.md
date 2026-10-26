# patchtrack: orientation

The goal is a GitHub-like workflow for patches sent by email: first "my
pull requests" (an author's series and where each stands), then the views
and actions that make that useful. The archive is any public-inbox
instance, lore.kernel.org first. The neighbours are `b4` and `lei`: same
language, same archive, same conventions where we can match them.

## Layout

| path | what |
|---|---|
| `src/patchtrack/lore.py` | the only network code: public-inbox search export (`?x=m`) and thread `t.mbox.gz` |
| `src/patchtrack/parse.py` | mail to facts: subject prefixes, trailers, "applied" evidence, b4 change-ids |
| `src/patchtrack/thread.py` | `read`: reply order, depth, quote collapsing, patch and cover parts |
| `src/patchtrack/tui.py` | the curses browser: `Model` (keys, views; no curses) and a thin `draw`/`run` |
| `src/patchtrack/b4local.py` | `branches`: b4 prep tracking commits, patch-id comparison with the last version sent |
| `src/patchtrack/vdiff.py` | `diff`: pairing patches across versions, normalised diff of diffs, cover changelogs |
| `src/patchtrack/nudge.py` | which open series need a reply, ping or RESEND; drafted pings |
| `src/patchtrack/news.py` | what changed since you last looked: snapshots and their differences |
| `src/patchtrack/local.py` | your own mail: Maildir, `.eml` directories, mbox; header-first filtering |
| `src/patchtrack/trees.py` | git: fetch a tree, find posted patches in it (message-id trailer, patch-id, subject) |
| `src/patchtrack/store.py` | SQLite of raw facts: messages, fetch times, trees and per-patch tree results |
| `src/patchtrack/series.py` | messages to series, families and states |
| `src/patchtrack/cli.py` | `patchtrack sync`, `list`, `show`, `people`, `board`, `tree`, `check` |
| `src/patchtrack/board.py` | the static HTML board |
| `src/patchtrack/pretty.py` | terminal output for `list`: circles, colours, aligned columns |
| `docs/` | the Sphinx site: `conf.py` (generates the command pages), hand-written pages, `requirements.txt` |
| `src/patchtrack/config.py` | the TOML config file; bot rules |
| `tests/test_patchtrack.py` | unittest, synthetic mail only, no network |
| `tests/test_trees.py` | matchers against a throwaway git repo; stage derivation |
| `tests/test_sync.py` | what `sync` refetches and how it fails, against an in-memory fake archive |
| `tests/test_thread.py` | quote collapsing, sign-offs, patch and cover parts, reply order |
| `tests/test_tui.py` | the TUI model and layout on a fake window; a real run in a pty |
| `tests/test_b4local.py` | branches as sent, changed and never sent, in a throwaway repo |
| `tests/test_vdiff.py` | rebase-only is unchanged; added, dropped, retitled; changelogs |
| `tests/test_nudge.py` | nudge kinds at fixed dates, drafts, close/reopen |
| `tests/test_news.py` | `news`: first look, changes, `--peek`, `--since` |
| `tests/test_local.py` | local sources, incremental reads, `sync --offline`, local mail driving archive fetches |
| `tests/test_lore.py` | retries and backoff against a fake opener; windowed search |
| `tests/test_pretty.py`, `tests/test_config.py` | `list` output; the config file and bot rules |

## Documentation

The site is https://ryanabx.github.io/patchtrack, built with Sphinx (as
b4's is) by `.github/workflows/docs.yml` and published to GitHub Pages
from `main`.

- **A command's documentation is its docstring.** Each `cmd_*` function
  in `cli.py` has a reStructuredText docstring. `build_parser()` passes
  it to argparse as the subcommand's description, so `patchtrack CMD
  --help` and the website show the same text: `--help` through `_Help`,
  which strips the markup and turns `:doc:` references into URLs, and
  the website through `sphinx-argparse`. `docs/conf.py` writes one page
  per subcommand at build time (`docs/commands/`, ignored by git), so a
  new command gets its page without anyone adding one.
- **The configuration reference is `config.py`'s module docstring**
  (`docs/configuration.rst` is an `automodule`).
- **Hand-written pages** (`docs/*.rst`) are for what no single command
  owns: concepts, quick start, other projects, scheduling, about.
- **Help strings must not depend on the machine**: they end up on the
  website, so show `$XDG_DATA_HOME/...`, not a resolved path.
- **Build strictly before committing**: `sphinx-build -W --keep-going -n
  -b html docs docs/_build/html` (`pip install -r docs/requirements.txt`).
  The tests check that every command has a one-line help and a docstring.

## Design rules

- **Store raw facts, derive everything else.** The database holds
  messages, when each thread was fetched, and the latest result of each
  (patch, tree) check, including "not found", with the ref sha. Series, families and states
  are recomputed on every read, so a better heuristic applies to old data
  without a refetch. Do not cache derived state.
- **Every state carries its evidence.** That is the message-id and the
  line that justifies it, plus the time the thread was last fetched. A
  state the tool cannot point at a message for (`unfetched`) is labelled
  as such, never shown as `no-reply`.
- **Say only what the mail says.** `unanswered` means "someone wrote after
  the author", not "the author owes a reply". Prefer a factual label to a
  judgement the tool cannot make.
- **Be polite to the archive.** Read-only, one search per author, one
  request per thread, `--delay` between requests, an honest
  `User-Agent`. Never work around a bot check (lore runs Anubis); if one
  blocks us, report it.
- **Never send mail.** Any future action that would (replies, `b4 ty`
  style thanks) prepares the message and stops; a person sends it.
- **Standard library only** until a dependency clearly earns its place;
  git is called as a subprocess.
- **Never make a history shallow.** Fetches take whole branches into
  `refs/patchtrack/<name>`. On a shallow history, "not found" means nothing.
- **Match only on what names the patch.** A message-id counts only in a
  `Link:`/`Message-ID:` trailer: commit bodies cite *other* postings in
  reference links (the first live run matched one patch to a later
  commit that merely cited it). `git log -p` over a path filter needs
  `--full-diff`, or a larger commit is trimmed into a false patch-id match.

## Sync robustness and identity

Measured 2026-10-26/27, and each now has a test:
- **No export cap below 1652 results.** Windows split at 1000, so a cap
  above that cannot matter.
- **Pending fetches are a flag, not a time comparison.**
  `threads.wanted_at` is set when a sync chooses a thread and cleared by
  the fetch. Comparing it with `fetched_at` at one-second resolution lost
  a retry when two syncs fell in the same second.
- **lore keeps two copies of b4-relay mail under one message-id**: the
  relay's (`From: X via B4 Relay <devnull+…>`, `X-Original-From: X`, an
  in-body `From:` on patches) and X's own. Whichever copy was stored last
  decided whose series it was, and a full and an incremental sync
  disagreed. Sender and author are now separate columns, resolved the
  same way for both copies.
- **Ownership is "sent or wrote".** A patch a contributor resent for its
  original author dropped off the contributor's list when the in-body
  author was used alone.

## News

`seen` holds what `news` last *showed* for each family. That is a record
of the user, not of the mail, and it is the one derived thing the store
keeps. `news --since` rebuilds only the mail as of a time. A first
version also rebuilt tree checks and fetches as of then, and on a store
created after that time it reported every tree result as fresh news
("rtc: applied → in-next" for a patch in next long before).

## Nudges and closing

`nudge`'s "reply owed" is narrower than `[unanswered]`. The first live run
listed three series whose last word was an Ack or a Reviewed-by as "reply
owed". A tag asks nothing; those series wait on a maintainer, so they
are pings. Drafts are 7bit/8bit plain text: the first came out
quoted-printable, which breaks lines mid-word on the lists and in an
editor. `closed` is, with `seen`, the only user decision the store keeps.

## Families

A family is judged on its head (latest) version's patches. Each patch counts
as far as any version of it got. A patch is followed from version to
version by `vdiff.links`: the same title, then the same diff (a retitled
patch), then a similar title. A patch a later version
dropped still counts if it was applied somewhere: dropping it because it
went in is normal: v4 3/3 applied alone, then a v5 of the other two. A patch dropped and never applied was abandoned and does not
hold the family back. A patch dropped after several versions counts once: every copy is grouped
by the last version that still had it.

## Review states

`commented` was calibrated on 39 real human replies (2026-10-26). "Prose
below a quoted diff" missed two objections that quoted the commit message,
so any quoted text counts. "Any reply" flagged routing mail between
maintainers and an author's "thanks for explaining", so only direct
replies to a posting count, and the latest per patch wins. A reply with a
tag is approval whatever it says. Known false positive: a process note
replying to a cover letter ("I will fold this into my series"). A fix was tried and rejected (2026-10-27): counting cover replies as
comments only when they ask something ("?", "please", "should", "why"…)
would have demoted 32 of 72 real cover replies across nine projects,
including "I've got two more small nits on the first patch" and "With
this series applied I see a bunch of qom-test failures".

## Sync

The first sync of an (author, list) covers the window; later ones are
incremental (see the ``sync`` docstring). "As of" stays the time a *whole thread* was last
fetched: an incremental search that finds nothing new is not a fetch of
the thread, because a reply that does not copy the author would not show
up in it. The weekly refresh bounds how stale that can get. Validated
2026-10-26: after an incremental sync, all 34 versions had the same state
as in a fresh full sync; a quiet incremental sync took 0.5 s against 43 s.

## Configuration

User preferences go in the TOML config (`config.py`), data in the store.
`config.settings()` is loaded once in `main()` as `args.settings`.
Command-line options override it, and it overrides the built-in defaults.
Every section and key is declared in `config.KNOWN` (trees in
`TREE_KEYS`), with types in `TYPES`; unknown ones are errors. Tests that
run the CLI point `XDG_CONFIG_HOME` at an empty directory, so a real
config cannot leak in. The `bodies` bot rule (for a maintainer whose bot mail, such as a
binding check, comes from their own address) is unit-tested only: no such mail was in the
validation data.

## Schema

`store.MIGRATIONS` is applied in order on open, counted by
`PRAGMA user_version`. Add to the end, never edit a shipped entry. A new
column is empty on old rows until their mail is fetched again, so say in
the migration's comment what repopulates it (usually `sync --full`).

## Performance (measured 2026-10-27)

Series are rebuilt from the stored mail on every command. With 20,000
messages (1,000 series of 4 patches and 15 replies), `build()` takes 0.22
s and `list` 0.49 s end to end. Storing them took 10 s (0.5 ms each).
Nothing needs caching at contributor scale.

## Testing

`PYTHONPATH=src:tests python3 -m unittest discover -s tests -v`. Tests use synthetic mail built
with `email.message`; nothing touches the network. When adding a
heuristic, add a positive *and* a negative case, and check that the test
fails when the heuristic is broken (plant the bug, watch it fail, revert).
The anchored regexes in `parse.py` already reject `>`-quoted lines, so a
test of quote stripping alone proves nothing.

CI (`.github/workflows/tests.yml`) runs the suite on Python 3.11–3.15
and checks that the installed command starts. 3.11 is the floor (for
`tomllib`); the whole suite has passed on 3.11 in the `python:3.11`
container and on 3.15 locally.

For a live check, sync a real author into a scratch `--db` and compare
against what is known by hand. The first validation (2026-10-26, one
contributor's kernel series over two months) agreed with a hand-kept
status file on every applied, reviewed and no-reply row. The tree check
found exactly the 9 commits the status file listed as applied, each
confirmed by subject. It also caught one sha the status file had wrong:
it had recorded a mainline merge commit instead of the patch.

## Which projects it fits (measured 2026-10-26)

One week of `[PATCH]` mail from each archive, with `patchtrack`'s own parser:

| archive | posts with inline diff | replies with a tag | "applied" replies `patchtrack` sees | how the project actually says it |
|---|---|---|---|---|
| lore: a kernel subsystem list | 100% | 10% | 24 | "Applied to the … branch of ….git" |
| lore: u-boot | 99% | 27% | 64 | "Applied to u-boot/main, thanks!" |
| lore: buildroot | 100% | 4% | 177 | "Applied to master, thanks!" |
| lore: qemu-devel | 100% | 42% | 26 | "Applied to …", "This was merged into master (sha)"; mostly pull requests |
| lore: xen-devel | 100% | 28% | 0 | committers commit without replying |
| lore: git | 99% | 0% | 2 | the maintainer's "What's cooking" mails, not thread replies |
| lore: openembedded-core | 100% | 0% | 0 | not announced in the thread |
| sourceware: gcc-patches | 95% | 0% | 1 | reviewer says "OK" (42), author "pushed" (14) |
| sourceware: libc-alpha | 95% | 30% | 0 | committers commit without replying |

Unreachable for `patchtrack`: DPDK (`inbox.dpdk.org`) puts its search behind an
Anubis challenge for honest clients, and we do not work around bot
checks. GNU Mailman archives (emacs-devel) and PostgreSQL's own archive
are not public-inbox.

So the mail layer (series, versions, replies, tags, `commented`) and the
tree check are generic to public-inbox projects that use
git-send-email. The "applied" mail heuristics are kernel/U-Boot/buildroot
style. For gcc, git, xen, glibc and OE the tree check is the signal that
works. The same survey found five false "applied" matches, all wrapped
sentences ("…\napplied first."). An announcement must now start a
paragraph or be a bare "Applied."; one real wrapped announcement in
buildroot is missed as a result.

### Profiles (2026-10-27)

`config.SOURCEWARE` was calibrated on that gcc-patches week. Approvals are
a bare OK/LGTM, optionally "for <branch>", then the end of the line or
`.`/`!`. "OK for mainline?" (an author asking), "ok, will do." and "OK,
not really wrong, but…" are not approvals. Landing lines start with
"Pushed", "Committed", "Now pushed" or "OK and pushed". Of 9 such lines
with a parent, 8 answered someone else's mail (the author answering the
approval), so they count from the author. "I've pushed a small
refactoring…" (first person, something else) does not match. With the
profile, that week's series went from 1 reviewed / 0 applied to 16 / 7,
and every matched line read as genuine. Known limit: "Committed …, others
to follow" on a cover used to mark the whole series applied; such
announcements are now partial (1 of 319 surveyed announcements matches,
the one it was written for).

## Known gaps and next steps

Done: sync that cannot silently miss mail, your own mailbox as a source,
config for authors/lists/trees, `news`, `read`/`open`, `nudge` and
`close`, `diff`, `branches`, GitHub readiness, project profiles and the
TUI. What is left, roughly in order of value:

1. **Heuristics with known misses:**
   - `commented`, and so `nudge`'s "reply owed": a process note replying
     to a cover letter ("I will fold this into my series") counts as a
     comment.
   - An "applied" announcement wrapped onto a sentence's second line is
     missed (one in a week of buildroot).
2. **Reverts** are found when the revert says "This reverts commit <sha>",
   and a chain of reverts is followed (a revert of the revert puts the
   patch back). A revert squashed into another change is not found.
3. **Mail sources**: Maildir, mbox and `.eml`. IMAP is left to `mbsync`
   and friends on purpose, so `patchtrack` never holds a mail password.
   Local mail never shows a reply that dropped you from Cc, and the "as
   of" text says so.
4. **Other archives**: git's "What's cooking" is not parsed (use its
   branches as trees). Mailman and PostgreSQL archives are not
   public-inbox. DPDK's search is behind a bot check.
5. **Board**: per list works (`-l`); `--all-versions` does not exist yet.
   The docs have a systemd timer for `sync && check --fetch; board`
   (`docs/scheduling.rst`).
6. **Old rows** stored before a migration lack its columns until they are
   fetched again (`sync --full`, `check`).
7. **Releasing**: `.github/workflows/release.yml` publishes to PyPI when
   a GitHub release is published, through Trusted Publishing (no token).
   It refuses a tag that does not match `__version__`, runs the tests,
   and checks the build with `twine check --strict`. Publishing a release
   is the owner's step.

## Contributions

No LLM trailers in commits (`Assisted-by:`, `Co-authored-by:` naming a
model or assistant). If a change was developed with an LLM coding
assistant, say so in prose in the commit message or PR description.
