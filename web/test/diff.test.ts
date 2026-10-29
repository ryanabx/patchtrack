import { describe, expect, test } from "vitest";

import { filesAcross, messageUrl, parseDiff, pathQuote } from "../src/lib/diff";

const PATCH = `---
 a.c | 3 ++-
 b.c | 1 +
 2 files changed

diff --git a/a.c b/a.c
index 1111111..2222222 100644
--- a/a.c
+++ b/a.c
@@ -10,3 +10,4 @@ int main(void)
 keep
-old
+new
+more
 tail
diff --git a/b.c b/b.c
new file mode 100644
--- /dev/null
+++ b/b.c
@@ -0,0 +1 @@
+-- not a signature
--${" "}
2.51.0
`;

describe("diffs", () => {
  test("files, hunks and line numbers", () => {
    const [a, b] = parseDiff(PATCH);
    expect([a.path, a.status, a.add, a.del]).toEqual(["a.c", "modified", 2, 1]);
    expect(a.lines.map((l) => [l.kind, l.old, l.new])).toEqual([
      ["hunk", null, null],
      ["ctx", 10, 10],
      ["del", 11, null],
      ["add", null, 11],
      ["add", null, 12],
      ["ctx", 12, 13],
    ]);
    // Each line knows where it is in the text, which is what a review comment names.
    const lines = PATCH.split("\n");
    expect(a.lines.every((l) => lines[l.at].endsWith(l.text))).toBe(true);
    expect(lines[a.lines[3].at]).toBe("+new");
    // A line that looks like git's signature is a line while its hunk lasts; the signature is not.
    expect([b.path, b.status, b.lines.map((l) => l.text)]).toEqual(["b.c", "added", ["@@ -0,0 +1 @@", "-- not a signature"]]);
  });

  test("a rename", () => {
    const [f] = parseDiff("diff --git a/x.c b/y.c\nsimilarity index 90%\nrename from x.c\nrename to y.c\n");
    expect([f.path, f.from, f.status]).toEqual(["y.c", "x.c", "renamed"]);
  });

  test("a file across a version's patches", () => {
    const files = filesAcross([
      { patch: 1, msgid: "p1@x", diff: PATCH },
      { patch: 2, msgid: "p2@x", diff: "diff --git a/a.c b/a.c\n--- a/a.c\n+++ b/a.c\n@@ -1 +1 @@\n-x\n+y\n" },
    ]);
    expect(files.map((f) => [f.path, f.parts.map((p) => p.patch), f.add, f.del])).toEqual([
      ["a.c", [1, 2], 3, 2],
      ["b.c", [1], 1, 0],
    ]);
  });

  test("archive links quote as the core does", () => {
    // crates/core/src/views.rs: message_url("https://lore.kernel.org/", "a/b@x")
    expect(messageUrl("https://lore.kernel.org/", "a/b@x")).toBe("https://lore.kernel.org/all/a%2Fb@x/");
    expect(pathQuote("<20261005.abc-1+x=y~z@x.org>")).toBe("20261005.abc-1+x=y~z@x.org");
    expect(pathQuote("é@x")).toBe("%C3%A9@x");
  });
});
