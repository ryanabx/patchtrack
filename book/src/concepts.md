# Concepts

## Series and families

A **series** is one posting: a cover letter (or a lone first patch) and
the patches threaded under it. A **family** is every version of one
series, the nearest email has to a pull request: one sender's postings,
grouped by b4's `change-id` (from the cover letter's trailer or a b4
message-id), otherwise by the series title.

A family is judged on its latest version's patches, each as far as any
version of it got. A patch is followed from version to version by title,
then by an identical diff (a retitled patch), then by a similar title. A
patch a later version dropped still counts if it was applied somewhere:
dropping a patch because it went in is normal. One dropped and never
applied was abandoned and does not hold the family back.

## Who a series belongs to

A series is yours if you **posted** it or **wrote** it.

- **The sender** is `X-Original-From:` when a relay or list rewrote
  `From:` (b4's web relay sends as "X via B4 Relay <devnull+…>"),
  otherwise `From:`.
- **The author** is the in-body `From:` at the top of a patch (what
  `git am` records), otherwise the sender.

So a patch you resend for someone else is on your list ("written by …;
posted by you" in `show`), and so is your patch that a maintainer
resends. A family is one sender's, though: someone else's posting of your
patches is a family of its own, and does not supersede yours.

## States

A series is as far along as its least advanced patch, and every state
carries the message or commit that shows it.

| state | evidence |
|---|---|
| merged | every patch found in a `mainline` tree |
| in-next | every patch found in a `next` tree (or further) |
| queued | every patch found in a `maintainer` tree (or further) |
| applied | a reply from someone else says "Applied …", "Queued …", "Thanks, applied", "This was merged into …", or the patchwork-bot "This series was applied to …", covering every patch. A b4-ty `[2/3] … commit:` list names the patches; a bare reply to the cover covers all of them. |
| partially-applied | some patches got at least as far as applied, others did not (a b4-ty "(subset)", or "…, others to follow") |
| superseded | a later version, or a RESEND, of the same family exists |
| commented | a person's latest direct reply to a patch (or the cover) wrote prose below quoted text and gave no tag |
| reviewed | a `Reviewed-by:` or `Acked-by:`, or a project's approval (see [Other projects](projects.md)) |
| discussed | at least one reply from a person |
| bot-only | replies only from bots |
| no-reply | the thread was read and nobody else wrote |
| unfetched | no replies known, but the thread has never been read |
| closed | you closed it ([`close`](commands/close.md)) |

`applied` comes from the mail alone. When trees are configured and none
of them has the patches, the evidence says so: `"Applied to the for-next
branch…"; not in foo as of 2026-10-26 21:12`. That is the usual state
between a maintainer's reply and their push.

`commented` is the nearest email has to GitHub's "changes requested".
Mail cannot say whether the author is being asked for something, so the
state names what was done: someone commented inline. Only direct replies
to a posting count; replies to replies are mostly conversation. A later
direct reply carrying a tag settles that patch.

## As of

Every state is *as of* the time its thread was last read. "No reply" is a
timestamp, not a fact. A thread read only from your own mail says "from
your mailbox only", because local mail misses replies that dropped you
from Cc.

`unanswered` marks a series where a person wrote after you last did. It
is a fact, not a judgement: a bare `Reviewed-by` counts too. `nudge` is
stricter about what needs a reply.

## One store, everywhere

Everything patchtrack knows is in one SQLite file: the mail as received,
when each thread was read, what each tree check found, your settings and
what you follow. Series, families and states are derived again on every
command, so a better rule applies to old mail without fetching it again.
The web app shows this same file, through `patchtrack bridge` on the
machine that keeps it. `patchtrack export FILE` writes a copy another
patchtrack can import, and `export --public` leaves out what is yours
rather than the archive's.
