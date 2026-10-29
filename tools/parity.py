"""States and evidence: the Python version against the Rust one, family by family.

    cargo build --release -p patchtrack
    python3 tools/parity.py SOME.mbox /tmp/scratch.sqlite3 [N]

The mbox is real mail you fetched (a week of a list, say); it is imported
into the scratch store, which should not exist yet. N differences are
printed in full. The Python version decides "mine" by a list of tracked
authors, so it runs once per sender, with that sender tracked, and only that
sender's families compare. Families are per sender in the Rust version, so a
posting a tracked author wrote but someone else sent is counted apart.
"""
import json, mailbox, os, subprocess, sys
from collections import Counter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, "src"))
from patchtrack import parse, series, store

# The Python version crashes on a charset it does not know ("unknown-8bit",
# seen on u-boot); decode those as UTF-8 here, as the Rust parser does.
_text_body = parse.text_body
def text_body(msg):
    for part in msg.walk():
        try:
            "".encode(part.get_content_charset() or "utf-8")
        except LookupError:
            part.set_param("charset", "utf-8")
    return _text_body(msg)
parse.text_body = text_body

BIN = os.path.join(ROOT, "target", "release", "patchtrack")


def rust(path, db):
    """Every family, as {sender, key, state, evidence, versions (oldest first)}."""
    subprocess.run([BIN, "--db", db, "import", path], check=True, capture_output=True)
    out = subprocess.run([BIN, "--db", db, "list", "--json", "--all-versions", "--closed", "--everyone"],
                         check=True, capture_output=True, text=True).stdout
    families = {}
    for row in json.loads(out):  # each family's versions, newest first
        f = families.setdefault((row["sender"], row["family"]), {
            "sender": row["sender"], "key": row["family"], "state": row["family_state"],
            "evidence": row["family_evidence"], "versions": []})
        f["versions"].insert(0, row)
    return list(families.values())


def key_rs(f):
    return (f["sender"], f["key"])


def view_rs(f):
    return (f["state"], f["evidence"], [(v["version"], v["root"], v["state"], v["evidence"], v["unanswered"])
                                        for v in f["versions"]])


def view_py(f):
    return (f.state, f.evidence, [(s.version, s.root, s.state, s.evidence, s.unanswered) for s in f.versions])


path, db = sys.argv[1], sys.argv[2]
rust_fams = rust(path, db)
rs = {key_rs(f): view_rs(f) for f in rust_fams}
# Which sender Rust gives each posting: families are per sender in Rust; the
# Python version also took in postings a tracked author wrote but someone
# else sent ("sent or wrote").
rs_sender = {mid: f["sender"] for f in rust_fams for v in f["versions"] for mid in [v["root"], *v["patches"].values()]}
def other_sender(k, view):
    return any(rs_sender.get(root, k[0]) != k[0] for _, root, *_ in view[2])
py_db = store.Store(":memory:")
for m in mailbox.mbox(path, create=False):
    py_db.add(m)
msgs = py_db.messages()
# `import` marks every thread it shows as seen in local mail; tell the Python
# version the same, or every quiet series is "unfetched" there and "no-reply" here.
local = {m["msgid"]: "2026-01-01T00:00:00Z" for m in msgs}
same, differ, only_rs, only_py, explained = 0, [], [], [], []
for sender in sorted({k[0] for k in rs}):
    py = {(f.author, f.key): view_py(f) for f in series.families(series.build(msgs, [sender], local_times=local))}
    py = {k: v for k, v in py.items() if k[0] == sender}
    for k in sorted(set(rs) | set(py)):
        if k[0] != sender:
            continue
        if k not in py:
            only_rs.append(k)
        elif k not in rs:
            (explained if other_sender(k, py[k]) else only_py).append(k)
        elif rs[k] == py[k]:
            same += 1
        elif other_sender(k, py[k]):
            explained.append(k)
        else:
            differ.append((k, py[k], rs[k]))
states = Counter(v[0] for v in rs.values())
print(f"{path}: {len(rs)} families; identical {same}; differ only by a posting someone else sent {len(explained)}; "
      f"other differences {len(differ)}, only rust {len(only_rs)}, only python {len(only_py)}")
print("  states:", dict(states.most_common()))
for k, p, r in differ[:int(sys.argv[3]) if len(sys.argv) > 3 else 5]:
    print("  DIFF", k[1])
    print("    py:", p[0], "|", p[1][:150]);  print("    rs:", r[0], "|", r[1][:150])
    for a, b in zip(p[2], r[2]):
        if a != b:
            print("    py v:", a[0], a[2], a[4], "|", a[3][:120]); print("    rs v:", b[0], b[2], b[4], "|", b[3][:120])
for k in only_rs[:3]: print("  ONLY RUST", k)
for k in only_py[:3]: print("  ONLY PY", k)
