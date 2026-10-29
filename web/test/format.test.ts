import { describe, expect, test } from "vitest";

import type { Summary } from "../src/core/Summary";
import {
  ago,
  bodyBlocks,
  diffLineClass,
  diffStats,
  displayName,
  evidenceLine,
  evidenceParts,
  familyId,
  filterFamilies,
  qualifier,
  withQualifier,
  foldLong,
  inProject,
  listsOf,
  hideAddresses,
  ofFamilies,
  stateStyle,
  whose,
} from "../src/lib/format";

const NOW = new Date("2026-10-28T12:00:00Z");

describe("format", () => {
  test("ago", () => {
    expect(ago("2026-10-28T11:59:30Z", NOW)).toBe("just now");
    expect(ago("2026-10-28T11:00:00Z", NOW)).toBe("1 hour ago");
    expect(ago("2026-10-27T11:00:00Z", NOW)).toBe("yesterday");
    expect(ago("2026-10-20T12:00:00Z", NOW)).toBe("8 days ago");
    expect(ago("2026-09-24T12:00:00Z", NOW)).toBe("on 2026-09-24");
  });

  test("states keep the CLI's shapes", () => {
    expect(stateStyle("merged")).toEqual({ shape: "●", tone: "merged" });
    expect(stateStyle("partially-applied")).toEqual({ shape: "◐", tone: "done" });
    expect(stateStyle("closed")).toEqual({ shape: "⊘", tone: "muted" });
  });

  test("names and addresses", () => {
    expect(displayName("Jane Reviewer <jane@x>")).toBe("Jane Reviewer");
    expect(displayName("<j@x>")).toBe("j@x");
    expect(hideAddresses("Kind: Jane <j@x> on 1/2")).toBe("Kind: Jane on 1/2");
  });

  test("evidence cites message-ids only where a clause ends", () => {
    const text = 'Rev <r@x>, 2026-10-06: "Applied" <a@x>; not in next as of 2026-10-07';
    expect(evidenceParts(text)).toEqual([
      { text: 'Rev <r@x>, 2026-10-06: "Applied"' },
      { msgid: "a@x" },
      { text: "; not in next as of 2026-10-07" },
    ]);
    expect(evidenceLine(text)).toBe('Rev, 2026-10-06: "Applied"; not in next as of 2026-10-07');
    expect(evidenceParts("by <v2@x>")).toEqual([{ text: "by" }, { msgid: "v2@x" }]);
  });

  test("bodies: text, quotes with their blank lines, the signature", () => {
    expect(bodyBlocks("On Mon, R wrote:\n> a\n>\n\n> b\n\nWhy?\n-- \nsig\n")).toEqual([
      { kind: "text", lines: ["On Mon, R wrote:"] },
      { kind: "quote", lines: ["> a", ">", "", "> b"] },
      { kind: "text", lines: ["Why?"] },
      { kind: "signature", lines: ["-- ", "sig"] },
    ]);
    // A diff's "---" is not a signature.
    expect(bodyBlocks("x\n---\n y").map((b) => b.kind)).toEqual(["text"]);
  });

  test("long bodies fold past a limit, short ones never", () => {
    const lines = (n: number) => Array.from({ length: n }, (_, i) => `line ${i}`);
    expect(foldLong(lines(60))).toEqual({ shown: lines(60), folded: [] });
    const long = foldLong(lines(61));
    expect([long.shown.length, long.folded.length, long.folded[0]]).toEqual([30, 31, "line 30"]);
  });

  test("whose series: the families the core names", () => {
    const f = (sender: string, key: string) => ({ sender, key }) as unknown as Summary;
    const families = [f("me@x", "a"), f("me@x", "b"), f("other@x", "a")];
    expect(familyId(families[2])).toBe("other@x\0a");
    const scope = { mine: ["me@x\0a"], followed: ["me@x\0a", "other@x\0a"], authors: [] };
    expect(ofFamilies(families, whose(scope, "mine")).map(familyId)).toEqual(["me@x\0a"]);
    expect(ofFamilies(families, whose(scope, "followed")).map(familyId)).toEqual(["me@x\0a", "other@x\0a"]);
    expect(ofFamilies(families, whose(scope, "everyone"))).toHaveLength(3);
  });

  test("the list's search: words, qualifiers, negation and sorting", () => {
    const f = (key: string, state: string, sender: string, lists: string[], unanswered: boolean, last: string, written_by: string[] = []) =>
      ({ key, title: `${key}: a fix`, state, sender, lists, unanswered, last_activity: last, written_by, closed: null }) as unknown as Summary;
    const all = [
      f("usb", "reviewed", "dev@x.org", ["linux-usb"], true, "2026-10-03"),
      f("net", "no-reply", "other@y.org", ["netdev"], false, "2026-10-05", ["Dev <dev@x.org>"]),
      f("gpio", "commented", "third@z.org", ["linux-gpio"], true, "2026-10-01"),
    ];
    const keys = (q: string) => filterFamilies(all, "open", q).map((s) => s.key);
    expect(keys("")).toEqual(["usb", "net", "gpio"]);
    expect(keys("state:reviewed")).toEqual(["usb"]);
    expect(keys("state:review")).toEqual([]); // a state, exactly
    expect(keys("author:dev@x")).toEqual(["usb", "net"]); // sent, or wrote
    expect(keys("list:linux- is:unanswered")).toEqual(["usb", "gpio"]);
    expect(keys("-state:no-reply fix")).toEqual(["usb", "gpio"]);
    expect(keys("sort:updated")).toEqual(["net", "usb", "gpio"]);
    expect(keys("sort:oldest -author:third")).toEqual(["net", "usb"]);
    expect(filterFamilies(all, "closed", "")).toEqual([]);
    expect(withQualifier("fix state:reviewed", "state", "applied")).toBe("fix state:applied");
    expect(withQualifier("fix state:reviewed", "state", "")).toBe("fix");
    expect(qualifier("Sort:Updated x", "sort")).toBe("updated");
  });

  test("projects and their lists", () => {
    const f = (key: string, projects: string[], lists: string[]) => ({ sender: "s", key, projects, lists }) as unknown as Summary;
    const all = [f("a", ["Linux"], ["linux-usb", "all"]), f("b", ["Linux"], ["netdev", "linux-usb"]), f("c", ["GCC"], ["gcc-patches"])];
    expect(inProject(all, "Linux").map((s) => s.key)).toEqual(["a", "b"]);
    expect(inProject(all, "Linux", "netdev").map((s) => s.key)).toEqual(["b"]);
    expect(inProject(all).length).toBe(3);
    expect(listsOf(inProject(all, "Linux"))).toEqual([["linux-usb", 2], ["netdev", 1]]);
  });

  test("diffs", () => {
    expect(diffStats("diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-x\n+y\n+z")).toEqual({ files: 1, add: 2, del: 1 });
    expect(["+++ b", "--- a", "@@ -1 @@", "+y", "-x", " c"].map(diffLineClass)).toEqual(["meta", "meta", "hunk", "add", "del", ""]);
  });
});
