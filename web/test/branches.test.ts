import { describe, expect, test } from "vitest";

import { branchFor, branchState } from "../src/lib/branches";
import type { Branch } from "../src/store";

const branch = (name: string, change_id: string, sent: Array<[number, string]>, revision: number): Branch => ({
  repo: "/r",
  name,
  subject: "",
  change_id,
  revision,
  sent: sent.map(([version, cover]) => ({ version, cover })),
  patches: 1,
});

describe("branches", () => {
  const all = [branch("b4/a", "20261001-a-1", [[1, "a1@x"]], 2), branch("b4/b", "20261001-b-2", [], 1)];
  test("a series' branch, by a cover it sent, else by change-id", () => {
    expect(branchFor(["v0@x", "a1@x"], "title:a", all)?.name).toBe("b4/a");
    expect(branchFor([], "20261001-b-2", all)?.name).toBe("b4/b");
    expect(branchFor(["other@x"], "title:c", all)).toBeUndefined();
  });
  test("where it stands", () => {
    expect(all.map(branchState)).toEqual(["v2 in preparation; v1 sent", "v1 never sent"]);
    expect(branchState(branch("b4/c", "", [[3, "c@x"]], 3))).toBe("as sent in v3");
  });
});
