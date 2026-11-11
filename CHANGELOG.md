# Changelog

## Unreleased

- **`tree discover`** finds the maintainer trees for your open series:
  the MAINTAINERS sections covering their files (as get_maintainer.pl
  reads them), their `T:` trees, and the branch from linux-next's
  `Next/Trees` where `T:` names none. In a kernel checkout,
  `tree discover --add` needs no options: MAINTAINERS comes from the
  mainline tree or the current directory, Next/Trees from linux-next. For
  a tree whose branch neither file names, it lists the branches and asks
  which one (in a terminal), or prints the `tree add` command to finish.

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
