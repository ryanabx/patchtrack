import { describe, expect, test } from "vitest";

import { pairing } from "../src/lib/bridge";

const TOKEN = "0123456789abcdef0123456789abcdef";

describe("pairing", () => {
  test("a bridge on this computer, with its token", () => {
    expect(pairing("http://127.0.0.1:47130", TOKEN)).toEqual({ url: "http://127.0.0.1:47130", token: TOKEN });
    expect(pairing("http://localhost:47130/", TOKEN)).toEqual({ url: "http://localhost:47130", token: TOKEN });
  });

  test("the server this app came from", () => {
    const here = "https://pt.example.org";
    expect(pairing("https://pt.example.org", TOKEN, here)).toEqual({ url: here, token: TOKEN });
    // Another server is a bridge elsewhere: confirmed first, never taken as it is.
    expect(pairing("https://other.example.org", TOKEN, here)).toHaveProperty("remote");
    // Not over plain http, unless on this computer.
    expect(pairing("http://pt.example.org", TOKEN, "http://pt.example.org")).toHaveProperty("error");
  });

  test("a bridge elsewhere, over https, to be confirmed", () => {
    expect(pairing("https://desktop.example.ts.net", TOKEN, "https://ryanabx.github.io")).toEqual({
      remote: { url: "https://desktop.example.ts.net", token: TOKEN },
    });
    expect(pairing("https://desktop.example.ts.net:8443/", TOKEN)).toHaveProperty("remote");
  });

  test("anything else is refused", () => {
    for (const url of [
      "http://evil.example:47130",
      "http://127.0.0.1.evil.example:47130",
      "http://127.0.0.1:47130/v1/hello",
      "http://127.0.0.1:47130/?x=1",
      "https://user:pass@desktop.example.ts.net",
      "https://desktop.example.ts.net/v1/",
      "javascript:alert(1)",
      "",
    ]) {
      expect(pairing(url, TOKEN), url).toHaveProperty("error");
    }
    expect(pairing("http://127.0.0.1:47130", "short")).toHaveProperty("error");
    expect(pairing("http://127.0.0.1:47130", TOKEN.toUpperCase())).toHaveProperty("error");
  });
});
