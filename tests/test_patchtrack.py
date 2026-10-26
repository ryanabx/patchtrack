import email.message
import unittest

from patchtrack import parse, series, store

ME = "dev@example.org"


def mail(mid, subject, frm=ME, irt=None, body="", date="Mon, 05 Oct 2026 10:00:00 +0000", refs=None):
    m = email.message.EmailMessage()
    m["Message-ID"] = f"<{mid}>"
    m["Subject"] = subject
    m["From"] = frm
    m["Date"] = date
    if irt:
        m["In-Reply-To"] = f"<{irt}>"
        m["References"] = " ".join(f"<{r}>" for r in (refs or [irt]))
    m.set_content(body)
    return m


def build(msgs):
    db = store.Store(":memory:")
    for m in msgs:
        db.add(m)
    msgs = db.messages()
    db.close()
    return series.build(msgs, [ME])


def posting(tag, version, n, date, cover_body=""):
    v = f" v{version}" if version > 1 else ""
    root = f"20261005-{tag}-v{version}-0-{'a' * 11}{version}@example.org"
    msgs = [mail(root, f"[PATCH{v} 0/{n}] foo: do the thing", body=cover_body, date=date)]
    for i in range(1, n + 1):
        msgs.append(mail(f"20261005-{tag}-v{version}-{i}-{'a' * 11}{version}@example.org",
                         f"[PATCH{v} {i}/{n}] foo: part {i}", irt=root, date=date))
    return root, msgs


class Subjects(unittest.TestCase):
    def test_patch(self):
        s = parse.parse_subject("[PATCH v3 2/4] drm/foo: unplug the device")
        self.assertEqual((s.is_patch, s.is_reply, s.version, s.index, s.total), (True, False, 3, 2, 4))
        self.assertEqual(s.title, "drm/foo: unplug the device")

    def test_reply_rfc_resend_subset(self):
        s = parse.parse_subject("Re: (subset) [RFC PATCH RESEND,v2,1/2] x: y")
        self.assertTrue(s.is_reply and s.rfc and s.resend and s.subset)
        self.assertEqual((s.version, s.index), (2, 1))

    def test_not_a_patch(self):
        self.assertFalse(parse.parse_subject("[GIT PULL] clk changes").is_patch)


class Evidence(unittest.TestCase):
    def test_trailers_skip_quotes(self):
        body = "> Reviewed-by: Old <old@x>\nLooks fine.\n\nAcked-by: Rev <rev@x>\n"
        self.assertEqual(parse.trailers(body), [("Acked-By", "Rev <rev@x>")])

    def test_b4_ty(self):
        body = ("On Mon, X wrote:\n> blah\n\nApplied, thanks!\n\n"
                "[2/3] mfd: add the thing\n      commit: 0123456789ab\n\nBest regards,\n")
        a = parse.applied(body, "Re: (subset) [PATCH 0/3] foo")
        self.assertEqual((a.indices, a.subset), ([2], True))

    def test_plain_applied(self):
        a = parse.applied("Applied to the for-next branch of foo.git.\n")
        self.assertIsNotNone(a)
        self.assertEqual(a.indices, [])

    def test_merged_into(self):
        self.assertIsNotNone(parse.applied("This was merged into master (0123456789abcdef).\n"))
        self.assertIsNone(parse.applied("This should be merged into the other series first.\n"))

    def test_patchwork_bot(self):
        self.assertIsNotNone(parse.applied("Hello:\n\nThis series was applied to netdev/net-next.git\n"))

    def test_bare_line_after_prose(self):
        self.assertIsNotNone(parse.applied("Fairly simple one and clearly a net gain.\nApplied.\n"))

    def test_after_a_quote(self):
        self.assertIsNotNone(parse.applied("> [PATCH] foo\nApplied to master, thanks!\n"))

    def test_not_applied(self):
        # The last three are wrapped sentences seen on gcc-patches, git@ and xen-devel.
        for body in ("> Applied, thanks!\nWhy?\n",
                     "Can this be applied to stable as well?\n",
                     "Thanks.\n-- \nApplied Signal Corp\n",
                     'The attribute is now\napplied to struct after its definition".\n',
                     "This one depends on the other being\napplied first.\n",
                     "Those tests will be\npicked up and run automatically in Gitlab CI.\n"):
            self.assertIsNone(parse.applied(body), body)

    def test_change_id(self):
        body = "---\nbase-commit: abc\nchange-id: 20260915-my-series-d35d2ac3d6f0\n"
        self.assertEqual(parse.change_id("whatever@x", body), "my-series")
        self.assertEqual(parse.change_id("20261025-my-series-v5-0-0123456789ab@example.org", ""), "my-series")


class Series(unittest.TestCase):
    def test_states_and_versions(self):
        r1, v1 = posting("foo", 1, 3, "Mon, 05 Oct 2026 10:00:00 +0000")
        r2, v2 = posting("foo", 2, 3, "Mon, 12 Oct 2026 10:00:00 +0000")
        p1 = v2[1]["Message-ID"].strip("<>")
        replies = [
            mail("r1@x", "Re: [PATCH v2 1/3] foo: part 1", "Rev <rev@x>", irt=p1,
                 body="> code\nReviewed-by: Rev <rev@x>\n", date="Tue, 13 Oct 2026 10:00:00 +0000",
                 refs=[r2, p1]),
            mail("r2@x", "Re: (subset) [PATCH v2 0/3] foo: do the thing", "Max <max@x>", irt=r2,
                 body="Applied, thanks!\n\n[3/3] foo: part 3\n      commit: deadbeef\n",
                 date="Wed, 14 Oct 2026 10:00:00 +0000"),
        ]
        found = build(v1 + v2 + replies)
        self.assertEqual(set(found), {r1, r2})
        self.assertEqual(found[r1].state, "superseded")
        s = found[r2]
        self.assertEqual((s.state, s.applied, s.family), ("partially-applied", {3}, "foo"))
        self.assertTrue(s.unanswered)
        self.assertEqual([(i, k) for i, k, _, _ in s.tags], [(1, "Reviewed-By")])

    def test_single_patch_and_author_reply(self):
        root = "solo@example.org"
        found = build([
            mail(root, "[PATCH] bar: fix it"),
            mail("q@x", "Re: [PATCH] bar: fix it", "Maint <m@x>", irt=root, body="Why?\n",
                 date="Tue, 06 Oct 2026 10:00:00 +0000"),
            mail("a@x", "Re: [PATCH] bar: fix it", ME, irt="q@x", body="Because.\n",
                 date="Wed, 07 Oct 2026 10:00:00 +0000", refs=[root, "q@x"]),
            mail("b@x", "Re: [PATCH] bar: fix it", "ci-bot <ci-bot@x>", irt=root, body="ok\n",
                 date="Wed, 07 Oct 2026 11:00:00 +0000"),
        ])
        s = found[root]
        self.assertEqual((s.state, s.total, list(s.patches)), ("discussed", 1, [1]))
        self.assertFalse(s.unanswered)
        self.assertEqual([r.bot for r in s.replies], [False, False, True])

    def test_recipients_include_the_authors_replies(self):
        root = "solo@example.org"
        patch = mail(root, "[PATCH] bar: fix it")
        patch["To"] = "Maint <maint@x>"
        reply = mail("a@x", "Re: [PATCH] bar: fix it", ME, irt=root, body="Adding Max.\n",
                     date="Wed, 07 Oct 2026 10:00:00 +0000")
        reply["Cc"] = "Max <max@x>, list@x"
        self.assertEqual(build([patch, reply])[root].to, {"maint@x", "max@x", "list@x"})

    def test_applied_reply_to_single_patch(self):
        root = "solo@example.org"
        found = build([
            mail(root, "[PATCH] bar: fix it"),
            mail("y@x", "Re: [PATCH] bar: fix it", "Maint <m@x>", irt=root, body="Applied.\n",
                 date="Tue, 06 Oct 2026 10:00:00 +0000"),
        ])
        self.assertEqual(found[root].state, "applied")

    def test_partial_announcement_on_the_cover(self):
        root, msgs = posting("pa", 1, 3, "Mon, 05 Oct 2026 10:00:00 +0000")
        msgs.append(mail("y@x", "Re: [PATCH 0/3] foo: do the thing", "M <m@x>", irt=root,
                         body="Applied 1/3 to for-next, the rest will follow later.\n",
                         date="Tue, 06 Oct 2026 10:00:00 +0000"))
        s = build(msgs)[root]
        self.assertEqual((s.state, s.applied), ("partially-applied", set()))
        self.assertIn("which patches is not said", s.evidence)
        self.assertTrue(parse.partial("Committed to trunk as r17-1234, others to follow."))
        self.assertFalse(parse.partial("Applied to master, thanks. Other tests look fine."))

    def test_cover_applied_without_list_means_all(self):
        root, msgs = posting("baz", 1, 2, "Mon, 05 Oct 2026 10:00:00 +0000")
        msgs.append(mail("y@x", "Re: [PATCH 0/2] foo: do the thing", "M <m@x>", irt=root,
                         body="Applied to for-next, thanks.\n", date="Tue, 06 Oct 2026 10:00:00 +0000"))
        self.assertEqual(build(msgs)[root].applied, {1, 2})

    def test_no_reply(self):
        root, msgs = posting("q", 1, 2, "Mon, 05 Oct 2026 10:00:00 +0000")
        db = store.Store(":memory:")
        for m in msgs:
            db.add(m)
        db.mark_thread(root, "2026-10-26T00:00:00Z")
        fetched = series.build(db.messages(), [ME], db.thread_times())
        db.close()
        self.assertEqual(build(msgs)[root].state, "unfetched")
        self.assertEqual((fetched[root].state, fetched[root].unanswered), ("no-reply", False))


class Relay(unittest.TestCase):
    def test_inbody_from_is_the_author(self):
        relay = mail("p@x", "[PATCH] foo: bar", "Dev via B4 Relay <devnull+dev.example.org@kernel.org>",
                     body="From: Dev <dev@example.org>\n\nThe change.\n---\n")
        direct = mail("p@x", "[PATCH] foo: bar", "Dev <dev@example.org>", body="The change.\n---\n")
        rows = []
        for m in (relay, direct):
            db = store.Store(":memory:")
            db.add(m)
            rows.append(db.messages()[0])
            db.close()
        same = ("author_addr", "author_name", "body")
        self.assertEqual([{k: r[k] for k in same} for r in rows][0], {k: rows[1][k] for k in same})
        self.assertEqual(rows[0]["author_addr"], "dev@example.org")
        self.assertEqual(rows[0]["from_addr"], "devnull+dev.example.org@kernel.org")  # sender: the relay

    def test_resent_patch_is_the_senders_and_the_authors(self):
        m = mail("p@x", "[PATCH v3] pci: quirk", ME, body="From: Alex <alex@x.org>\n\nQuirk.\n---\n")
        db = store.Store(":memory:")
        db.add(m)
        msgs = db.messages()
        db.close()
        self.assertEqual((msgs[0]["from_addr"], msgs[0]["author_addr"]), (ME, "alex@x.org"))
        s = series.build(msgs, [ME])["p@x"]
        self.assertEqual((s.author, s.written_by), (ME, {"Alex <alex@x.org>"}))
        self.assertIn("p@x", series.build(msgs, ["alex@x.org"]))

    def test_x_original_from(self):
        cover = mail("c@x", "[PATCH 0/2] foo", "Dev via B4 Relay <devnull+dev.example.org@kernel.org>")
        cover["X-Original-From"] = "Dev <dev@example.org>"
        self.assertEqual(parse.sender(cover), ("Dev", "dev@example.org"))
        self.assertEqual(parse.sender(mail("d@x", "x", "Dev <dev@example.org>")), ("Dev", "dev@example.org"))

    def test_replies_and_odd_lines_are_left_alone(self):
        self.assertEqual(parse.inbody_author("Re: [PATCH] x", "R", "r@x", "From: A <a@x>\nhi\n"),
                         ("R", "r@x", "From: A <a@x>\nhi\n"))
        self.assertEqual(parse.inbody_author("[PATCH] x", "R", "r@x", "From: nobody\n"),
                         ("R", "r@x", "From: nobody\n"))

    def test_migration_fixes_old_rows(self):
        import os
        import sqlite3
        import tempfile
        path = os.path.join(tempfile.mkdtemp(), "t.sqlite3")
        old = sqlite3.connect(path)  # a store as version 3 left it
        old.executescript(store.SCHEMA)
        for step in store.MIGRATIONS[:3]:
            old.executescript(step)
        old.execute("PRAGMA user_version = 3")
        old.executescript(
            "INSERT INTO messages (msgid, from_name, from_addr, subject, body, date, refs) VALUES"
            " ('p@x', 'Dev via B4 Relay', 'devnull+d@kernel.org', '[PATCH 1/1] x',"
            "  'From: Dev <dev@x.org>' || char(10) || char(10) || 'body' || char(10), '2026-10-01T00:00:00Z', ''),"
            " ('c@x', 'Dev via B4 Relay', 'devnull+d@kernel.org', '[PATCH 0/1] x', 'cover',"
            "  '2026-10-01T00:00:00Z', ''),"
            " ('r@x', 'R', 'r@x.org', 'Re: [PATCH 1/1] x', 'ok', '2026-10-02T00:00:00Z', '');")
        old.commit()
        old.close()
        db = store.Store(path)
        rows = {r["msgid"]: r for r in db.messages()}
        pending = db.pending_threads()
        db.close()
        self.assertEqual((rows["p@x"]["author_addr"], rows["p@x"]["body"]), ("dev@x.org", "body\n"))
        self.assertEqual(rows["r@x"]["author_addr"], "r@x.org")  # everyone else: author = sender
        self.assertEqual(pending, {"p@x", "c@x"})  # relay copies are refetched next sync


class Comments(unittest.TestCase):
    def test_inline_comments(self):
        body = ("On Mon, 5 Oct 2026, Dev wrote:\n> +\tval = readl(x);\n\n"
                "This needs a lock here.\n\n> +\treturn 0;\n\nThanks for the patch!\n"
                "Reviewed-by: R <r@x>\nok\n-- \nsome signature with many words\n")
        self.assertEqual(parse.inline_comments(body), ["This needs a lock here."])

    def test_top_post_is_not_inline(self):
        self.assertEqual(parse.inline_comments("Looks fine to me overall.\n\n> +\tx = 1;\n"), [])

    def thread(self, *replies):
        root = "solo@example.org"
        msgs = [mail(root, "[PATCH] bar: fix it")]
        for n, (frm, irt, body) in enumerate(replies):
            msgs.append(mail(f"r{n}@x", "Re: [PATCH] bar: fix it", frm, irt=irt or root, body=body,
                             date=f"Tue, 06 Oct 2026 1{n}:00:00 +0000",
                             refs=[root] if not irt else [root, irt]))
        return build(msgs)[root]

    def test_direct_comment(self):
        s = self.thread(("R <r@x>", None, "> +\tx = 1;\nWhy is this not checked?\n"))
        self.assertEqual(s.state, "commented")
        self.assertIn('"Why is this not checked?"', s.evidence)

    def test_comment_on_a_reply_is_conversation(self):
        s = self.thread(("Dev <%s>" % ME, None, "Ping, any thoughts on this?\n"),
                        ("R <r@x>", "r0@x", "> Ping, any thoughts on this?\nIt is in my queue now.\n"))
        self.assertEqual(s.state, "discussed")

    def test_later_tag_settles_it(self):
        s = self.thread(("R <r@x>", None, "> +\tx = 1;\nWhy is this not checked?\n"),
                        ("R <r@x>", None, "> +\tx = 1;\nFine after all.\n\nReviewed-by: R <r@x>\n"))
        self.assertEqual(s.state, "reviewed")


class Board(unittest.TestCase):
    def test_render(self):
        from patchtrack import board
        root, msgs = posting("bd", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        msgs.append(mail("y@x", "Re: [PATCH 0/1] foo: do the thing", "M <m@x>", irt=root,
                         body="Applied <b>now</b>, thanks.\n", date="Tue, 06 Oct 2026 10:00:00 +0000"))
        page = board.render(series.families(build(msgs)), "T & <t>", "2026-10-26 00:00")
        self.assertIn("<h2>Applied (1)</h2>", page)
        self.assertIn("&lt;b&gt;now&lt;/b&gt;", page)  # mail text is escaped
        self.assertIn('href="https://lore.kernel.org/all/y@x/">mail</a>', page)
        self.assertIn("<title>T &amp; &lt;t&gt;</title>", page)


class Families(unittest.TestCase):
    def family(self, msgs, tree_results=None):
        db = store.Store(":memory:")
        for m in msgs:
            db.add(m)
        found = series.build(db.messages(), [ME], {}, None, tree_results)
        db.close()
        (fam,) = series.families(found)
        return fam

    def test_dropped_because_applied(self):
        r1, v1 = posting("fam", 1, 3, "Mon, 05 Oct 2026 10:00:00 +0000")
        r2, v2 = posting("fam", 2, 2, "Mon, 12 Oct 2026 10:00:00 +0000")
        p3 = v1[3]["Message-ID"].strip("<>")
        applied = mail("ap@x", "Re: [PATCH v2 0/2] foo: do the thing", "M <m@x>", irt=r2,
                       body="Applied, thanks.\n", date="Tue, 13 Oct 2026 10:00:00 +0000")
        tree = {p3: [{"tree": "next", "role": "next", "ref_sha": "f" * 40, "commit_sha": "c" * 40,
                      "how": "patch-id", "checked_at": "2026-10-26T00:00:00Z"}]}
        fam = self.family(v1 + v2 + [applied], tree)
        self.assertEqual(fam.state, "applied")
        self.assertEqual(fam.evidence, "v2 1/2 applied, v2 2/2 applied, v1 3/3 in-next")

    def test_abandoned_patch_does_not_count(self):
        r1, v1 = posting("ab", 1, 3, "Mon, 05 Oct 2026 10:00:00 +0000")
        r2, v2 = posting("ab", 2, 2, "Mon, 12 Oct 2026 10:00:00 +0000")
        applied = mail("ap@x", "Re: [PATCH v2 0/2] foo: do the thing", "M <m@x>", irt=r2,
                       body="Applied, thanks.\n", date="Tue, 13 Oct 2026 10:00:00 +0000")
        fam = self.family(v1 + v2 + [applied])
        self.assertEqual((fam.state, len(fam.parts)), ("applied", 2))
        self.assertEqual(fam.head.root, r2)

    def test_older_version_with_the_same_commits_is_superseded(self):
        r1, v1 = posting("dup", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        r2, v2 = posting("dup", 2, 1, "Mon, 12 Oct 2026 10:00:00 +0000")
        hit = [{"tree": "mainline", "role": "mainline", "ref_sha": "f" * 40, "commit_sha": "c" * 40,
                "how": "patch-id", "checked_at": "2026-10-26T00:00:00Z", "reverted_by": None}]
        p1, p2 = (v[1]["Message-ID"].strip("<>") for v in (v1, v2))
        fam = self.family(v1 + v2, {p1: hit, p2: hit})
        self.assertEqual([s.state for s in fam.versions], ["superseded", "merged"])
        self.assertEqual(fam.state, "merged")

    def versions(self, specs, tree=None):
        """specs: per version, a list of (title, diff text). Returns the family."""
        msgs, roots = [], []
        for v, patches in enumerate(specs, 1):
            pre = f"v{v} " if v > 1 else ""
            root = f"fam-v{v}-0@x"
            roots.append(root)
            date = f"Mon, {5 + 7 * v:02d} Oct 2026 10:00:00 +0000"
            msgs.append(mail(root, f"[PATCH {pre}0/{len(patches)}] foo: series", date=date,
                             body="---\nchange-id: 20261001-famtest-abcdef123456\n"))
            for i, (title, diff) in enumerate(patches, 1):
                body = f"Text.\n---\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-x\n+{diff}\n"
                msgs.append(mail(f"fam-v{v}-{i}@x", f"[PATCH {pre}{i}/{len(patches)}] {title}", irt=root,
                                 date=date, body=body))
        rows = {}
        for (v, i), stage_role in (tree or {}).items():
            rows[f"fam-v{v}-{i}@x"] = [{"tree": "t", "role": stage_role, "ref_sha": "f" * 40,
                                        "commit_sha": f"{v}{i}" * 20, "how": "patch-id",
                                        "checked_at": "2026-12-01T00:00:00Z", "reverted_by": None}]
        return self.family(msgs, rows)

    def test_retitled_patch_keeps_its_history(self):
        fam = self.versions([[("a: one", "a1"), ("b: old title", "b1")],
                             [("a: one", "a1"), ("b: quite another title", "b1")]],
                            tree={(1, 2): "next", (2, 1): "next"})
        # v2 2/2 is v1 2/2 under a new title (same diff): it is in next, not a dropped patch.
        self.assertEqual(fam.state, "in-next")
        self.assertEqual(len(fam.parts), 2)

    def test_patch_dropped_after_several_versions_counts_once(self):
        fam = self.versions([[("a: one", "a1"), ("m: cell", "m1")],
                             [("a: one", "a2"), ("m: cell", "m1")],
                             [("a: one", "a3"), ("m: cell", "m1")],
                             [("a: one", "a4")]],
                            tree={(1, 2): "next", (2, 2): "next", (3, 2): "next"})
        self.assertEqual([(s.version, i) for s, i, _ in fam.parts], [(4, 1), (3, 2)])

    def test_older_version_applied_head_not_yet(self):
        r1, v1 = posting("old", 1, 1, "Mon, 05 Oct 2026 10:00:00 +0000")
        r2, v2 = posting("old", 2, 1, "Mon, 12 Oct 2026 10:00:00 +0000")
        applied = mail("ap@x", "Re: [PATCH 0/1] foo: do the thing", "M <m@x>", irt=r1,
                       body="Applied, thanks.\n", date="Tue, 13 Oct 2026 10:00:00 +0000")
        fam = self.family(v1 + v2 + [applied])
        self.assertEqual((fam.state, fam.evidence), ("applied", "v1 1/1 applied"))


if __name__ == "__main__":
    unittest.main()
