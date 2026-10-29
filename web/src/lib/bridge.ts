// Pairing with the bridge: where it listens and its token, kept in this
// browser. A bridge on this computer, or the server this app was served
// from (`patchtrack serve`), is taken as it is. A bridge elsewhere (yours,
// on your desktop behind a tunnel, reached from your phone) only over
// https, and only once you confirm it: a link that named someone else's
// server would have this app sync from it, and store what it said.

import type { Bridge } from "../store";

const KEY = "patchtrack-bridge";

/**
 * The bridge a pairing link names; `remote` when it is elsewhere and must be
 * confirmed first; or why it is not acceptable. `here` is this app's own origin.
 */
export function pairing(
  url: string,
  token: string,
  here = globalThis.location?.origin ?? "",
): Bridge | { remote: Bridge } | { error: string } {
  let u: URL;
  try {
    u = new URL(url);
  } catch {
    return { error: "The link does not name a bridge." };
  }
  const loopback = u.protocol === "http:" && ["127.0.0.1", "localhost"].includes(u.hostname);
  const ownServer = u.origin === here && (u.protocol === "https:" || loopback);
  const elsewhere = u.protocol === "https:" && !ownServer;
  if ((!loopback && !ownServer && !elsewhere) || u.pathname !== "/" || u.search || u.hash || u.username || u.password) {
    return {
      error: "A bridge is on this computer (http://127.0.0.1:PORT), or elsewhere over https (its --public-url).",
    };
  }
  if (!/^[0-9a-f]{32}$/.test(token)) return { error: "The link's token is not one `patchtrack bridge` makes." };
  const b = { url: u.origin, token };
  return elsewhere ? { remote: b } : b;
}

export function savedBridge(): Bridge | null {
  try {
    const b = JSON.parse(localStorage.getItem(KEY) ?? "null");
    return b && typeof b.url === "string" && typeof b.token === "string" ? b : null;
  } catch {
    return null;
  }
}

export function saveBridge(b: Bridge | null) {
  if (b) localStorage.setItem(KEY, JSON.stringify(b));
  else localStorage.removeItem(KEY);
}
