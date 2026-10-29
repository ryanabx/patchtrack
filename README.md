# patchtrack

A pull-request list for email patches. Give it an address, and `patchtrack`
finds every series that address posted to a public-inbox archive such as
[lore.kernel.org](https://lore.kernel.org), groups the versions of each
series, and says where each one stands: applied, reviewed, commented, or
waiting. Every state comes with the message or commit that shows it.

```
$ patchtrack sync -a you@example.org
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
or both. For a web frontend, `export` writes the same data as JSON files
and `serve` answers with it over HTTP.

**Documentation: https://ryanabx.github.io/patchtrack**

## Install

Python 3.11 or later; `git` for the tree and branch commands. No other
dependencies.

```
pip install patchtrack          # or, from a checkout: pip install .
patchtrack sync -a you@example.org
patchtrack list
```

Every command explains itself: `patchtrack <command> --help`.

## Building the documentation

```
pip install -r docs/requirements.txt
sphinx-build -W -n -b html docs docs/_build/html
```

The command reference is generated from the code: each page is a command's
docstring in `src/patchtrack/cli.py` and its options.

## License

MIT
