# patchtrack

A pull-request list for email patches. Follow an address, and `patchtrack`
finds every series that address posted to a public-inbox archive such as
[lore.kernel.org](https://lore.kernel.org), groups the versions of each
series, and says where each one stands: applied, reviewed, commented, or
waiting. Every state comes with the message or commit that shows it.

```
$ patchtrack follow you@example.org
$ patchtrack sync
$ patchtrack list
○ no-reply   spi: foo: handle zero-length tr…    v1 · 1 patch · 2026-10-26
● applied    hwmon: add the Foo PMIC sensor …  v5 · 2 patches · 2026-10-25
             v5 1/2 applied, v5 2/2 applied, v4 3/3 in-next
● reviewed   clk: fix the divider rounding o…  v1 · 2 patches · 2026-10-21  unanswered
             Reviewed-By: Jane Reviewer on 2/2
● commented  gpio: foo: restore the interrup…    v1 · 1 patch · 2026-10-20  unanswered
             Sam Maintainer on 1/1: "Why not handle this in the irqchip core instead?"
● merged     rtc: foo: keep the alarm across…    v1 · 1 patch · 2026-09-24
             1/1 is 8d0e4b1f6a2c in mainline (by message-id), as of 2026-10-27 00:55 …

● 1 merged   ● 1 applied   ● 1 commented   ● 1 reviewed   ○ 1 no-reply
```

It also shows what changed since you last looked (`news`), which series
need a reply, a ping or a RESEND (`nudge`), what changed between versions
(`diff`), where your b4 branches stand (`branches`), and whether each
patch is in a git tree (`check`). It reads the archive, your own mailbox,
or both, and has a terminal browser (`tui`). A web app (`web/`) shows the
same store through `patchtrack bridge`, which keeps it synced on its own,
from your desktop or, behind a tunnel, from your phone.

**Documentation: https://ryanabx.github.io/patchtrack**

## Install

Linux or Windows; `git` for the tree and branch commands. From a checkout,
with the toolchain `rust-toolchain.toml` names:

```
cargo install --locked --path crates/cli
patchtrack follow you@example.org
patchtrack sync
patchtrack list
```

Every command explains itself: `patchtrack <command> --help`.

## Layout

| path | what |
|---|---|
| `crates/core` | parsing mail and deriving series, families and states; sync as a state machine; no I/O |
| `crates/store` | the SQLite store and its migrations |
| `crates/cli` | the `patchtrack` binary: CLI and TUI; HTTP, git and files; the bridge the web app talks to |
| `web/` | the web app: Preact and TypeScript, over `patchtrack bridge` |
| `book/` | the documentation (mdBook); command pages come from the help text |

`ARCHITECTURE.md` explains the design. The Python version it replaces is
at the tag `python-last` (and released as 0.1.0).

## Building the documentation

```
cargo run -q -p patchtrack -- book-pages book
mdbook build book
```

## License

MIT
