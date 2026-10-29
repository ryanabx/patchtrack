# Changelog

## Unreleased: the rewrite

patchtrack is rewritten in Rust, as one core and the `patchtrack`
binary (Linux and Windows), with a web app over it. States and evidence
match the Python version on real mail, except where noted below.

- **A web app over your own machine**: `patchtrack bridge` serves the
  store to the web app (published on GitHub Pages, or your own copy with
  `patchtrack serve`), which keeps nothing of its own. The bridge reads,
  derives and changes the store, and syncs it on its own: when it
  starts, every `sync_every` minutes, after you follow something, and on
  "Sync now". The app shows any change within seconds, without a reload.
  Mail dropped on the app is imported into that store.
- **A series' page is a pull request's**: Conversation, Commits (each
  patch with where it stands, its tags and tree checks, opening on its
  message and diff) and Files changed (every file the version touches,
  with line numbers, each patch's part labelled). A version picker serves
  all three, and Commits and Files changed compare with another version:
  patch by patch with the author's changelog, or file by file across the
  whole series, a rebase alone being no change. "Get" gives the posting's
  message-id, its archive page and `b4 shazam`/`b4 am` lines to copy, and
  every message links to its archive page. A conversation says which
  version came before and after it.
- **Review on the diff**: comment on any line in Files changed or a
  commit, then "Finish your review" (Comment, Approve with Reviewed-by,
  Ack, Tested) writes the mail a reviewer on the lists would: one reply
  per patch quoting each comment's hunk up to its line, the comment
  below, and the summary and tag in a reply to the cover. They are
  drafts, sent only when you say.
- **Notifications**: each series that changed since you read it (a new
  version, a state change, new replies), counted in the bar, until you
  mark it read or open it; what needs a reply, a ping or a RESEND in a
  second tab. `news` in a terminal reads the same record.
- **The list as GitHub's**: the search box takes `state:`, `author:`,
  `list:`, `is:unanswered`, `-` to turn any around, and `sort:updated` or
  `sort:oldest`, which State and Sort menus set; each row counts the
  replies from people.
- **Settings live in the store**, not in a config file: `settings`,
  `follow`/`unfollow` (what `sync` searches, per list), `source` (your
  local mail) and `tree` (git trees). An exported store carries its
  settings and what it follows; `export --public` leaves out what is yours.
  The Python version's `config.toml` and `config` command are gone.
- **`export FILE`** now writes a copy of the store, and `import` merges
  one. The JSON documents of `export DIR` and `serve`, and the HTML
  `board`, are not ported: the web app takes their place. The old
  documentation's URLs change with the move to mdBook.
- **The web app sends mail and runs b4** through `patchtrack bridge
  --allow-send --repo PATH`: a draft's Send button (asked once more), and
  a Branches page that runs `b4 send`, `b4 send --resend` and `b4 trailers
  -u` on your prep branches, each first as a dry run.
- **The bridge from anywhere**: `bridge --public-url https://...` behind
  a tunnel or proxy (`tailscale serve`, Cloudflare Tunnel, Caddy), so the
  published web app on your phone uses your desktop's bridge; the app
  asks before it connects to a bridge elsewhere.
- **`patchtrack serve`**: the web app and its API from your own server,
  behind your HTTPS proxy.
- `send` no longer trusts git send-email's exit status alone: given an
  address it cannot read, send-email sends nothing and exits 0.
- **Projects**, as GitHub has repositories: `project add Linux` (or
  U-Boot, QEMU, Git, Xen, buildroot, GCC, glibc, or your own with
  `--archive`), `follow -p GCC ADDR`, `list -p GCC`. Each archive is
  synced in turn, and a thread is fetched from the inbox it was found in.
  The web app shows a project's series and, for the kernel, its lists.
- **A settings page with controls** in the web app (chips, numbers,
  menus, project cards), and a menu instead of a clipped bar on a phone.
- `reply`'s patch number is `-n`, as `-p` now means a project.
- **Write and send mail**: `reply` (a reply, or a review tag with
  `--tag`, addressed and quoted as b4 would), `thanks` (b4 ty's "Applied,
  thanks!", with the commits `check` found), `drafts`, and `send`, which
  shows a draft whole and sends it through `git send-email` only when you
  type "yes". `nudge --draft` keeps its pings as drafts too. The web app
  writes replies and tags from a conversation, and hands a draft over as
  a file, the clipboard or a mail link. A new setting, `name`, signs them.
- `branches` says when reviewers gave tags a prep branch does not carry
  yet, and that `b4 trailers -u` takes them in.
- **Follow more than authors**: `follow --inbox LIST` (every series on a
  list, a maintainer's queue), `follow --query QUERY` (a public-inbox
  search) and `follow --thread URL` (one thread, by any of its messages).
  A list's or a query's first sync goes back two weeks (`list_since`).
  `list` with no `-a` shows what you follow and your own series.
- **Families are per sender**: someone else's posting of your patches is a
  family of its own and no longer supersedes yours. "Sent or wrote" still
  decides what is yours.
- `import` marks the threads it shows as seen in local mail, so a series
  without replies is `no-reply` rather than `unfetched`.
- `read`'s diff summary no longer counts blank lines as changed lines.
- Local mail: a reply whose parent comes later in the same mailbox is
  kept; Maildir names with `!` (as mbsync writes them on Windows) work.

## Never released: the last of the Python version

- **Data for a web frontend**: `export DIR` writes JSON documents (every
  family; each family's versions, conversations and version diffs; people),
  and `serve` answers with the same documents over HTTP, read-only, on
  localhost. The frontend is a separate project; the documents are
  described at https://ryanabx.github.io/patchtrack/api.html.
- `list --json` gains fields (`label`, `titles`, `url`, `to`,
  `evidence_msgids`, …); none is removed.

## 0.1.0

The first version.

- **Sync** an author's mail from a public-inbox archive (lore.kernel.org by
  default): windowed searches that a result cap cannot truncate, retries
  with backoff, incremental runs, and threads whose fetch failed retried on
  the next run. Your own **Maildirs and mbox files** too, alongside the
  archive or alone (`sync --offline`, `import`).
- **Series and families**: versions grouped by b4 change-id or title;
  sender and author kept apart (b4 relay copies, patches resent for
  someone else).
- **States from evidence**: applied, queued, in-next and merged (from
  maintainers' mail and from git trees, by message-id trailer, patch-id or
  subject), commented, reviewed, discussed, bot-only, no-reply, closed,
  each with the message or commit that shows it and an "as of" time.
- **Commands**: `list` (coloured, aligned, filterable), `show`, `read` (the
  conversation in the terminal), `open`, `news` (what changed since you
  last looked), `diff` (between versions), `nudge` (reply, ping or RESEND,
  with drafted pings), `branches` (your b4 prep branches), `close` and
  `reopen`, `people`, `board` (static HTML), `tui` (interactive),
  `check`, `tree`, `import`, `config`.
- **Project profiles** for how a project says "approved" and "landed";
  built in for gcc and sourceware.
- **Across versions**, a patch is followed by title, identical diff or
  similar title, so a retitled patch keeps its history.
- **Trees**: a revert of a revert puts a patch back.
- **`nudge` holds pings and RESENDs during an open merge window**, read
  from a mainline tree's tags.
- **Documentation** at https://ryanabx.github.io/patchtrack, built with
  Sphinx. Each command's page is its docstring, the same text
  `patchtrack <command> --help` shows.
- **Config** for authors, lists, trees, mail sources, b4 repositories,
  nudge thresholds and bot rules.
