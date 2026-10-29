# patchtrack

A pull-request list for email patches. Follow an address, and patchtrack
finds every series that address posted to a public-inbox archive such as
[lore.kernel.org](https://lore.kernel.org), groups the versions of each
series, and says where each one stands: applied, reviewed, commented, or
waiting. Every state comes with the message or commit that shows it.

```text
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

Beyond the list, it tells you [what changed](commands/news.md) since you
last looked, which series [need a reply, a ping or a RESEND](commands/nudge.md),
what [changed between versions](commands/diff.md), where your
[b4 branches](commands/branches.md) stand, and whether each patch is
[in a git tree](commands/check.md). It reads the archive, your
[own mailbox](commands/source.md), or both, and has a
[terminal browser](commands/tui.md).

## Why

Nothing else gives a contributor this view:

- **Patchwork** filters by submitter, but per project. Each patch appears
  once per list it was sent to, and its state changes only when a
  maintainer sets it.
- **b4 review** (b4 0.15+) tracks series for maintainers: series are added
  one at a time, and states move on b4's own actions.
- **lei** finds the mail but does not interpret it.

patchtrack reads the same mail and draws the conclusions a contributor
would.
