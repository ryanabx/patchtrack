# Quick start

## Your patches

```sh
patchtrack follow you@example.org
patchtrack sync
patchtrack list
```

The first sync searches the last three months and fetches the thread of
every series it finds; later ones fetch only what changed. `list` shows
one line per series family, with the evidence for its state underneath.

`follow` remembers the address, so later commands need no arguments.
`follow you@example.org -l linux-usb` searches one list instead of all.

## Projects

```sh
patchtrack project add Linux          # or U-Boot, QEMU, Git, Xen, buildroot, GCC, glibc
patchtrack project add GCC
patchtrack follow -p GCC you@example.org
patchtrack list -p GCC
```

A project is an archive and its lists, as GitHub has repositories: GCC
lives on sourceware, the kernel and U-Boot on lore. `sync` syncs each
archive in turn. Your own project: `project add NAME --archive URL -l
LIST`.

## Someone else's series

```sh
patchtrack follow --inbox linux-usb                 # every series on a list
patchtrack follow --query 'dfn:drivers/usb/typec/'  # what a search finds
patchtrack follow --thread https://lore.kernel.org/linux-usb/MESSAGE-ID/
```

A list or a search finds far more than one author, so its first sync goes
back only two weeks (the `list_since` setting), and each thread it finds
is a request. `list` with no `-a` shows what you follow and your own
series; `--everyone` shows everything in the store.

## Settings and trees

Settings live in the store: `patchtrack settings` shows them all (see
[Settings](settings.md)). Tell patchtrack which addresses are yours, and
the git trees in which to look for your patches:

```sh
patchtrack settings set identities '["you@example.org"]'
patchtrack tree add next --repo ~/linux --role next \
    --url https://git.kernel.org/pub/scm/linux/kernel/git/next/linux-next.git --branch master
```

## Your own mail

If your mail is already on disk (lei, mbsync, offlineimap, a mail client
that keeps Maildir or mbox), sync can read it first:

```sh
patchtrack source add ~/Mail/INBOX
```

## The daily round

```sh
patchtrack sync && patchtrack check --fetch   # fetch mail, look in the trees
patchtrack news                               # what changed since last time
patchtrack nudge                              # what needs a reply, a ping or a RESEND
patchtrack read gpio                          # read one series' conversation
patchtrack tui                                # or browse everything
```

[Running it on a schedule](scheduling.md) runs the first line on a timer.
