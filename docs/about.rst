About
=====

Being a good citizen
--------------------

patchtrack is read-only: it never sends mail. ``nudge --draft`` writes
pings as files for you to send yourself. Against an archive it makes one
search per author, list and time window, and one request per thread, at
least a second apart (``--delay``). It identifies itself as
``patchtrack/<version>`` and retries politely, honouring ``Retry-After``.
It never works around a bot check.

How it knows
------------

The store keeps raw facts only: messages, when each thread was read, and
what each tree check found, including "not found". Series, families and
states are derived again on every command. So a better rule applies to
old mail without a refetch, and every state can point at its evidence.

Source and changes
------------------

The source is at https://github.com/ryanabx/patchtrack, with the
`changelog <https://github.com/ryanabx/patchtrack/blob/main/CHANGELOG.md>`_.
patchtrack is MIT-licensed.
