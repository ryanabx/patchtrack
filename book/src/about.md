# About

## Being a good citizen

patchtrack sends mail only when you tell it to: `reply`, `thanks` and
`nudge --draft` write drafts, and `send` shows a draft whole and sends it
through `git send-email` only when you type "yes", to the draft's
recipients and no one else. Against an archive it makes one search per
subscription (an author, a list or a search, in an inbox) and time
window, and one request per thread, at least a second
apart (`sync --delay`). It identifies itself as `patchtrack/<version>` with a
link to this project, and retries politely, honouring `Retry-After`. It
never works around a bot check.

## How it knows

The store keeps raw facts only: messages, when each thread was read, and
what each tree check found, including "not found". Series, families and
states are derived again on every command. So a better rule applies to
old mail without a refetch, and every state can point at its evidence.

## Source and changes

The source is at <https://github.com/ryanabx/patchtrack>, with the
[changelog](https://github.com/ryanabx/patchtrack/blob/main/CHANGELOG.md).
patchtrack is MIT-licensed.
