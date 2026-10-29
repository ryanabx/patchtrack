import io
import json
import os
import tempfile
import threading
import unittest
import urllib.error
import urllib.request
from unittest import mock

from patchtrack import api, cli, serve, store
from test_patchtrack import ME, mail

DIFF = "Fix it.\n\nSigned-off-by: D <{me}>\n---\ndiff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\n@@ -1 +1 @@\n-x\n+{new}\n"


def two_versions():
    """foo v1 and v2, two patches each; v2 changes patch 2 and gets a review and a comment."""
    msgs = []
    for v, date in ((1, "Mon, 05 Oct 2026 10:00:00 +0000"), (2, "Mon, 12 Oct 2026 10:00:00 +0000")):
        tag = f" v{v}" if v > 1 else ""
        root = f"v{v}-0@example.org"
        cover = "Do the thing.\n\n" + ("Changes in v2:\n- rework part 2\n\n" if v > 1 else "") + "base-commit: abc\n"
        msgs.append(mail(root, f"[PATCH{tag} 0/2] foo: do the thing", body=cover, date=date))
        for i in (1, 2):
            new = "y" if i == 1 or v == 1 else "z"
            msgs.append(mail(f"v{v}-{i}@example.org", f"[PATCH{tag} {i}/2] foo: part {i}", irt=root, date=date,
                             body=DIFF.format(me=ME, new=new)))
    msgs.append(mail("rev@x", "Re: [PATCH v2 1/2] foo: part 1", "Rev <rev@x>", irt="v2-1@example.org",
                     body="Reviewed-by: Rev <rev@x>\n", date="Tue, 13 Oct 2026 10:00:00 +0000"))
    msgs.append(mail("com@x", "Re: [PATCH v2 2/2] foo: part 2", "Sam <sam@x>", irt="v2-2@example.org",
                     body="> +z\nWhy z and not y here?\n", date="Wed, 14 Oct 2026 10:00:00 +0000"))
    return msgs


class Api(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.db = os.path.join(self.tmp.name, "t.sqlite3")
        db = store.Store(self.db)
        for m in two_versions():
            db.add(m)
        db.mark_thread("v2-0@example.org", "2026-10-15T00:00:00Z")
        db.commit()
        db.close()
        self.out = os.path.join(self.tmp.name, "site")

    def cli(self, *argv):
        out = io.StringIO()
        with mock.patch("sys.stdout", out), mock.patch("sys.stderr", io.StringIO()):
            self.assertEqual(cli.main(["--config", os.devnull, "--db", self.db, *argv]), 0)
        return out.getvalue()

    def read(self, path):
        with open(os.path.join(self.out, path)) as f:
            return json.load(f)

    def test_export(self):
        self.cli("export", "-a", ME, self.out)
        index = self.read("index.json")
        self.assertEqual((index["schema"], index["authors"]), (api.SCHEMA, [ME]))
        (fam,) = index["families"]
        self.assertEqual((fam["state"], fam["versions"], fam["head"], fam["open"], fam["closed"]),
                         ("commented", [1, 2], "v2-0@example.org", True, None))
        self.assertEqual(fam["evidence_msgids"], ["com@x"])
        self.assertIn("<com@x>", fam["evidence"])

        detail = self.read(f"families/{fam['id']}.json")["family"]
        self.assertEqual([s["version"] for s in detail["series"]], [1, 2])
        conv = detail["series"][1]["messages"]
        # Reply order: the cover, then each patch followed by its replies.
        self.assertEqual([(m["kind"], m["patch"], m["depth"]) for m in conv],
                         [("cover", 0, 0), ("patch", 1, 1), ("reply", 1, 2), ("patch", 2, 1), ("reply", 2, 2)])
        self.assertEqual(conv[0]["text"], "Do the thing.\n\nChanges in v2:\n- rework part 2")
        self.assertTrue(conv[1]["message"].endswith(f"Signed-off-by: D <{ME}>"))
        self.assertTrue(conv[1]["diff"].startswith("---\ndiff --git"))
        self.assertEqual(conv[2]["tags"], [{"kind": "Reviewed-By", "who": "Rev <rev@x>"}])
        self.assertEqual((conv[4]["direct"], conv[4]["comments"]), (True, ["Why z and not y here?"]))
        (d,) = detail["diffs"]
        self.assertEqual((d["from"], d["to"], d["changelog"]), (1, 2, ["Changes in v2:", "- rework part 2"]))
        self.assertEqual([p["marker"] for p in d["pairs"]], ["=", "!"])
        self.assertIn("++z", d["pairs"][1]["diff"])

        people = self.read("people.json")["people"]
        self.assertEqual([p["who"] for p in people], ["Sam <sam@x>", "Rev <rev@x>"])

    def test_export_removes_only_stale_family_files(self):
        os.makedirs(os.path.join(self.out, "families"))
        stale, mine = (os.path.join(self.out, "families", n) for n in ("0123456789ab.json", "notes.txt"))
        for path in (stale, mine):
            open(path, "w").close()
        self.cli("export", "-a", ME, self.out)
        self.assertFalse(os.path.exists(stale))
        self.assertTrue(os.path.exists(mine))
        self.assertEqual(len(os.listdir(os.path.join(self.out, "families"))), 2)

    def test_closed_family_is_exported_as_closed(self):
        self.cli("close", "-a", ME, "do the thing", "--reason", "folded")
        self.cli("export", "-a", ME, self.out)
        (fam,) = self.read("index.json")["families"]
        self.assertEqual((fam["state"], fam["closed"], fam["open"]), ("closed", "folded", False))

    def test_list_json_keeps_its_fields(self):
        (row,) = json.loads(self.cli("list", "-a", ME, "--json"))
        for k in ("root", "author", "title", "version", "state", "evidence", "patches", "stages", "trees", "tags"):
            self.assertIn(k, row)
        self.assertEqual(row["evidence_msgids"], ["com@x"])

    def test_unknown_paths(self):
        db = store.Store(self.db)
        self.addCleanup(db.close)
        data = cli._data(_args(ME), db)
        self.assertIsNotNone(api.document(data, "index.json"))
        for path in ("families/000000000000.json", "families/../index.json", "messages.json", "store.sqlite3"):
            self.assertIsNone(api.document(data, path), path)


def _args(author):
    from patchtrack import config
    args = cli.build_parser().parse_args(["export", "-a", author, "x"])
    args.settings = config.settings(os.devnull)
    args.bots_rules = args.settings.bots
    return args


class Serve(unittest.TestCase):
    def setUp(self):
        quiet = mock.patch("sys.stderr", io.StringIO())  # the server logs each request
        quiet.start()
        self.addCleanup(quiet.stop)
        db = store.Store(":memory:")
        for m in two_versions():
            db.add(m)
        self.data = cli._data(_args(ME), db)
        db.close()

    def start(self, **kw):
        server = serve.make_server(lambda: self.data, port=0, **kw)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        return f"http://127.0.0.1:{server.server_address[1]}"

    def get(self, url, host=None):
        req = urllib.request.Request(url, headers={"Host": host} if host else {})
        try:
            with urllib.request.urlopen(req) as r:
                return r.status, dict(r.headers), json.load(r)
        except urllib.error.HTTPError as e:
            with e:
                return e.code, dict(e.headers), json.load(e)

    def test_documents(self):
        base = self.start()
        status, headers, index = self.get(base + "/")
        self.assertEqual((status, index["schema"]), (200, api.SCHEMA))
        self.assertNotIn("Access-Control-Allow-Origin", headers)
        fam = index["families"][0]
        status, _, doc = self.get(f"{base}/families/{fam['id']}.json")
        self.assertEqual((status, doc["family"]["head"]), (200, "v2-0@example.org"))
        self.assertEqual(self.get(base + "/families/000000000000.json")[0], 404)

    def test_host_check(self):
        base = self.start()
        self.assertEqual(self.get(base + "/index.json", host="localhost:8080")[0], 200)
        status, _, doc = self.get(base + "/index.json", host="attacker.example:8080")
        self.assertEqual(status, 403)
        self.assertNotIn("families", doc)

    def test_cors_only_when_asked(self):
        base = self.start(cors="http://localhost:5173")
        _, headers, _ = self.get(base + "/index.json")
        self.assertEqual(headers["Access-Control-Allow-Origin"], "http://localhost:5173")

    def test_read_only(self):
        base = self.start()
        req = urllib.request.Request(base + "/index.json", data=b"{}", method="POST")
        with self.assertRaises(urllib.error.HTTPError) as e:
            urllib.request.urlopen(req)
        e.exception.close()
        self.assertEqual(e.exception.code, 501)


if __name__ == "__main__":
    unittest.main()
