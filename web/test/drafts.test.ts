import { describe, expect, test } from "vitest";

import { emlName, mailtoHref } from "../src/lib/drafts";
import type { Draft } from "../src/protocol";

const draft: Draft = {
  msgid: "1793.abc.patchtrack@x.org",
  family: "",
  kind: "tag",
  created_at: "2026-10-29T00:00:00Z",
  sent_at: null,
  subject: "Re: [PATCH] a: b",
  to: ["dev@x.org", "m@x.org"],
  cc: ["list@vger.org"],
  in_reply_to: "p1@x",
  body: "> x\n\nReviewed-by: R <r@x.org>\n",
  raw: "",
};

describe("drafts", () => {
  test("a file name a file system takes", () => {
    expect(emlName({ ...draft, msgid: "a/b:c@x" })).toBe("tag-a_b_c.eml");
  });

  test("mailto: threads where it can, and gives up when too long", () => {
    const href = mailtoHref(draft)!;
    expect(href.startsWith("mailto:dev%40x.org,m%40x.org?subject=Re%3A%20%5BPATCH%5D%20a%3A%20b&cc=list%40vger.org")).toBe(true);
    expect(href).toContain("&in-reply-to=%3Cp1%40x%3E&body=%3E%20x%0A%0AReviewed-by");
    expect(mailtoHref({ ...draft, body: "x".repeat(2000) })).toBeNull();
    expect(mailtoHref({ ...draft, to: [] })).toBeNull();
  });
});
