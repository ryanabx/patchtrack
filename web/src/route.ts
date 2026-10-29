// Routes live in the location hash, so any static host serves the app:
//   #/            #/?tab=closed&q=clk&who=mine&p=GCC&l=gcc-patches
//   #/f/SENDER/KEY?v=2&tab=commits&vs=1&c=2&m=MSGID   (vs: compare with; c: an open commit)
//   #/add
//   #/following
//   #/settings
//   #/drafts
//   #/branches
//   #/people?who=followed&bots=1
//   #/notifications?tab=needs-you
//   #/bridge?url=http://127.0.0.1:47130&token=...   (the link `patchtrack bridge` prints)

import type { Who } from "./protocol";

export type Route =
  | { view: "list"; tab: "open" | "closed"; q: string; who: Who; project?: string; list?: string }
  | {
      view: "family";
      sender: string;
      key: string;
      v: number | null;
      tab: "conversation" | "commits" | "files";
      m: string | null;
      /** The version Commits and Files changed compare with. */
      vs?: number | null;
      /** The commit (patch number) open in Commits. */
      c?: number | null;
    }
  | { view: "add" }
  | { view: "following" }
  | { view: "settings" }
  | { view: "drafts" }
  | { view: "branches" }
  | { view: "people"; who: Who; bots: boolean }
  | { view: "notifications"; tab: "unread" | "needs-you" }
  | { view: "pair"; url: string; token: string }
  | { view: "missing" };

export function parse(hash: string): Route {
  const raw = hash.replace(/^#/, "");
  const at = raw.indexOf("?");
  const path = (at < 0 ? raw : raw.slice(0, at)).split("/").filter(Boolean).map(decodeURIComponent);
  const params = new URLSearchParams(at < 0 ? "" : raw.slice(at + 1));
  const who: Who = params.get("who") === "mine" ? "mine" : params.get("who") === "followed" ? "followed" : "everyone";
  if (path.length === 0)
    return {
      view: "list",
      tab: params.get("tab") === "closed" ? "closed" : "open",
      q: params.get("q") ?? "",
      who,
      project: params.get("p") ?? undefined,
      list: params.get("l") ?? undefined,
    };
  if (path[0] === "add" && path.length === 1) return { view: "add" };
  // "sync" was its name once.
  if ((path[0] === "following" || path[0] === "sync") && path.length === 1) return { view: "following" };
  if (path[0] === "settings" && path.length === 1) return { view: "settings" };
  if (path[0] === "drafts" && path.length === 1) return { view: "drafts" };
  if (path[0] === "branches" && path.length === 1) return { view: "branches" };
  if (path[0] === "people" && path.length === 1) return { view: "people", who, bots: params.get("bots") === "1" };
  if (path[0] === "notifications" && path.length === 1)
    return { view: "notifications", tab: params.get("tab") === "needs-you" ? "needs-you" : "unread" };
  // News and Needs you were pages of their own once.
  if (path[0] === "news" && path.length === 1) return { view: "notifications", tab: "unread" };
  if (path[0] === "nudge" && path.length === 1) return { view: "notifications", tab: "needs-you" };
  if (path[0] === "bridge" && path.length === 1) return { view: "pair", url: params.get("url") ?? "", token: params.get("token") ?? "" };
  if (path[0] === "f" && path.length === 3) {
    const int = (name: string) => {
      const n = Number.parseInt(params.get(name) ?? "", 10);
      return Number.isInteger(n) ? n : null;
    };
    const t = params.get("tab");
    return {
      view: "family",
      sender: path[1],
      key: path[2],
      v: int("v"),
      // "patches" and "changes" were the tabs' names once.
      tab: t === "commits" || t === "patches" || t === "changes" ? "commits" : t === "files" ? "files" : "conversation",
      m: params.get("m"),
      vs: int("vs"),
      c: int("c"),
    };
  }
  return { view: "missing" };
}

export function href(r: Route): string {
  const params = new URLSearchParams();
  let path = "/";
  if (r.view === "family") {
    path = `/f/${encodeURIComponent(r.sender)}/${encodeURIComponent(r.key)}`;
    if (r.v !== null) params.set("v", String(r.v));
    if (r.tab !== "conversation") params.set("tab", r.tab);
    if (r.vs !== null && r.vs !== undefined) params.set("vs", String(r.vs));
    if (r.c !== null && r.c !== undefined) params.set("c", String(r.c));
    if (r.m) params.set("m", r.m);
  } else if (r.view === "add") {
    path = "/add";
  } else if (r.view === "following") {
    path = "/following";
  } else if (r.view === "settings") {
    path = "/settings";
  } else if (r.view === "people") {
    path = "/people";
    if (r.who !== "everyone") params.set("who", r.who);
    if (r.bots) params.set("bots", "1");
  } else if (r.view === "notifications") {
    path = "/notifications";
    if (r.tab !== "unread") params.set("tab", r.tab);
  } else if (r.view === "drafts" || r.view === "branches") {
    path = `/${r.view}`;
  } else if (r.view === "list") {
    if (r.tab !== "open") params.set("tab", r.tab);
    if (r.q) params.set("q", r.q);
    if (r.who !== "everyone") params.set("who", r.who);
    if (r.project) params.set("p", r.project);
    if (r.list) params.set("l", r.list);
  }
  const q = params.toString();
  return `#${path}${q ? "?" + q : ""}`;
}
