"""Read your own mail: Maildir directories, mbox files, directories of .eml.

Only what matters is kept: patches the authors sent or wrote, and replies in
their threads. Headers are read first and only matches are parsed in full,
so a large mailbox stays cheap. A Maildir is read incrementally by file
time; an mbox whose size and time have not changed is skipped.
"""

import email.parser
import email.policy
import mailbox
import os

from . import parse

HEADERS = email.parser.BytesParser(policy=email.policy.compat32)
# Files touched within this long before the last import are read again:
# delivery may set a file's time a little before it appears.
SLACK = 86400


def _files(path):
    """(file, mtime) for every message file under a Maildir or .eml directory."""
    subdirs = [d for d in ("cur", "new") if os.path.isdir(os.path.join(path, d))]
    for d in subdirs or ["."]:
        base = os.path.join(path, d)
        for name in os.listdir(base):
            f = os.path.join(base, name)
            if os.path.isfile(f) and not name.startswith("."):
                yield f, os.path.getmtime(f)


def _messages(path, since):
    """(header-only message, loader of the full message) for each message in path."""
    if os.path.isdir(path):
        for f, mtime in _files(path):
            if since is not None and mtime < since - SLACK:
                continue
            with open(f, "rb") as fh:
                head = HEADERS.parse(fh, headersonly=True)

            def load(f=f):
                with open(f, "rb") as fh:
                    return HEADERS.parse(fh)
            yield head, load
    else:
        if since is not None and os.path.getmtime(path) < since:
            return
        for m in mailbox.mbox(path, create=False):
            yield m, (lambda m=m: m)


def read(db, paths, authors, last_read):
    """Store the relevant mail found in paths; returns (read, new message-ids).

    last_read(path) gives the epoch time a path was last read, or None.
    """
    authors = {a.lower() for a in authors}
    known = db.known_ids()
    patches, replies, count = {}, [], 0
    for path in paths:
        for head, load in _messages(path, last_read(path)):
            count += 1
            mid = parse.msgid(head["Message-ID"])
            if not mid or mid in known:
                continue
            s = parse.parse_subject(parse.decode(head["Subject"]))
            if s.is_patch and not s.is_reply and parse.sender(head)[1] in authors:
                patches[mid] = load
            else:
                refs = set(parse.msgids(head["References"])) | {parse.msgid(head["In-Reply-To"] or "")}
                replies.append((mid, refs, load))
    threads = known | set(patches)
    new = {}
    for mid, load in patches.items():
        new[mid] = load()
    for mid, refs, load in replies:
        if refs & threads:
            new[mid] = load()
    for m in new.values():
        db.add(m)
    db.commit()
    return count, set(new)
