// The store, which lives on the machine `patchtrack bridge` (or `patchtrack
// serve`) runs on: this app keeps none of its own. Every read is derived
// there, as the command line derives it, and every change is made there.

import type { Changes } from "./core/Changes";
import type { Entry } from "./core/Entry";
import type { Family } from "./core/Family";
import type { FileChange } from "./core/FileChange";
import type { Person } from "./core/Person";
import type { Summary } from "./core/Summary";
import type { Added, Draft, Kind, ReviewComment, Notifications, Nudge, Projects, Scope, Setting, Source, Status, Subscription, Who } from "./protocol";

/** Where the bridge listens, and the token that pairs this app with it. */
export type Bridge = { url: string; token: string };

/** What the bridge says about itself, and what it may do on its machine. */
export type Hello = { patchtrack: string; can: { send: boolean; b4: boolean } };

/** A family, with each version's archive and its page there (by root message-id). */
export type FamilyPage = Family & { archives: Record<string, string>; urls: Record<string, string> };

/** A b4 prep branch on the bridge's machine. */
export type Branch = {
  repo: string;
  name: string;
  /** The cover letter's subject. */
  subject: string;
  change_id: string;
  /** The version b4 sends next. */
  revision: number;
  /** Each version sent, and the message-id of its cover. */
  sent: Array<{ version: number; cover: string }>;
  patches: number;
};

/** What b4 printed, and whether it succeeded. */
export type B4Run = { ok: boolean; output: string };

export class Store {
  constructor(
    readonly bridge: Bridge,
    readonly hello: Hello,
  ) {}

  /** Ask the bridge at `bridge` who it is: the store it serves, or why it cannot be reached. */
  static async connect(bridge: Bridge): Promise<Store> {
    const hello = await answered<Hello>(await reach(bridge, "/v1/hello", { headers: { "X-Patchtrack-Token": bridge.token } }));
    return new Store(bridge, hello);
  }

  private async get<T>(path: string, args: Record<string, string> = {}): Promise<T> {
    const q = new URLSearchParams(args).toString();
    return answered(await reach(this.bridge, q ? `${path}?${q}` : path, authed(this.bridge)));
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    return answered(await reach(this.bridge, path, authed(this.bridge, { method: "POST", body: JSON.stringify(body) })));
  }

  /** The store's generation (it changes whenever the store does) and the bridge's own syncs. */
  status(): Promise<Status> {
    return this.get("/v1/status");
  }

  /** Ask the bridge to sync now. */
  sync(): Promise<{ sync: Status["sync"] }> {
    return this.post("/v1/sync", {});
  }

  /** One line per family, for the list. */
  summaries(): Promise<Summary[]> {
    return this.get("/v1/summaries");
  }

  /** One family, all of it. */
  family(sender: string, key: string): Promise<FamilyPage> {
    return this.get("/v1/family", { sender, key });
  }

  conversation(root: string): Promise<Entry[]> {
    return this.get("/v1/conversation", { root });
  }

  /** Your identities and the authors you follow. */
  scope(): Promise<Scope> {
    return this.get("/v1/scope");
  }

  /** Who replied to the series of `who`, most recent first. */
  people(who: Who, bots: boolean): Promise<Person[]> {
    return this.get("/v1/people", { who, bots: bots ? "1" : "0" });
  }

  /** What changed from the version rooted at `old` to the one at `new`. */
  changes(old: string, new_: string): Promise<Changes> {
    return this.get("/v1/changes", { old, new: new_ });
  }

  /** What became of each file from the version rooted at `old` to the one at `new`. */
  files(old: string, new_: string): Promise<FileChange[]> {
    return this.get("/v1/files", { old, new: new_ });
  }

  closeFamily(sender: string, key: string, reason: string): Promise<void> {
    return this.post("/v1/close", { sender, key, reason });
  }

  reopenFamily(sender: string, key: string): Promise<boolean> {
    return this.post("/v1/reopen", { sender, key });
  }

  nudges(): Promise<Nudge[]> {
    return this.get("/v1/nudges");
  }

  /** The series that changed since you last read them. */
  notifications(): Promise<Notifications> {
    return this.get("/v1/notifications");
  }

  /** Mark a series' notification read: what it shows now is what the next one compares with. */
  markRead(sender: string, key: string): Promise<{ marked: number }> {
    return this.post("/v1/notifications/read", { sender, key });
  }

  markAllRead(): Promise<{ marked: number }> {
    return this.post("/v1/notifications/read", { all: true });
  }

  settings(): Promise<Setting[]> {
    return this.get("/v1/settings");
  }

  /** Set a setting from what a person typed (JSON, or a bare string). */
  setSetting(key: string, value: string): Promise<void> {
    return this.post("/v1/settings", { key, value });
  }

  unsetSetting(key: string): Promise<void> {
    return this.post("/v1/settings", { key, value: null });
  }

  subscriptions(): Promise<Subscription[]> {
    return this.get("/v1/subscriptions");
  }

  /** Follow an author, a list, a search or a thread, in a project ("" for none); false if already. */
  follow(kind: Kind, value: string, list: string, project = ""): Promise<boolean> {
    return this.post("/v1/follow", { kind, value, list, project });
  }

  unfollow(s: Subscription): Promise<number> {
    return this.post("/v1/unfollow", { kind: s.kind, value: s.value, list: s.list, archive: s.archive });
  }

  /** The local mail the bridge reads on each sync. */
  sources(): Promise<Source[]> {
    return this.get("/v1/sources");
  }

  projects(): Promise<Projects> {
    return this.get("/v1/projects");
  }

  /** The family a version rooted at `root` is in, if the store has it. */
  familyOf(root: string): Promise<Summary | null> {
    return this.get("/v1/family-of", { root });
  }

  /**
   * A reply to the stored message `to`, kept as a draft: `text` below a short
   * quote (none: everything quoted), then `tag` ("" for none).
   */
  reply(to: string, text: string, tag: string): Promise<Draft> {
    return this.post("/v1/reply", { to, text, tag });
  }

  /**
   * A review written as mail, one draft per patch commented on (each line's
   * comment below its quoted hunk), with `summary` and `tag` on the cover
   * (`root`, the posting's root).
   */
  review(root: string, comments: ReviewComment[], summary: string, tag: string): Promise<Draft[]> {
    return this.post("/v1/review", { root, comments, summary, tag });
  }

  drafts(): Promise<Draft[]> {
    return this.get("/v1/drafts");
  }

  markSent(msgid: string): Promise<boolean> {
    return this.post("/v1/drafts/sent", { msgid });
  }

  removeDraft(msgid: string): Promise<boolean> {
    return this.post("/v1/drafts/remove", { msgid });
  }

  /** Send a draft with the bridge's git send-email; its Message-ID. */
  async send(msgid: string): Promise<string> {
    return (await this.post<{ sent: string }>("/v1/send", { msgid })).sent;
  }

  /**
   * Mail (an mbox or one message) or a store file, into the bridge's store.
   * With `onlyFollowed`, mail keeps only the series of the authors you
   * follow and your own, with their replies; a store file merges whole.
   */
  async add(bytes: Uint8Array, onlyFollowed = false): Promise<Added> {
    const path = `/v1/import${onlyFollowed ? "?only_followed=1" : ""}`;
    const init = authed(this.bridge, { method: "POST", body: bytes as BodyInit, headers: { "Content-Type": "application/octet-stream" } });
    return answered(await reach(this.bridge, path, init));
  }

  /** The whole store as one SQLite file, which any patchtrack can open or import. */
  async exportFile(): Promise<Blob> {
    const r = await reach(this.bridge, "/v1/export", authed(this.bridge));
    if (!r.ok) throw new Error(await refusal(r));
    return r.blob();
  }

  branches(): Promise<Branch[]> {
    return this.get("/v1/branches");
  }

  /** Run b4 on a branch there: "send", "resend" or "trailers"; a dry run unless `dryRun` is false. */
  runB4(b: Branch, action: "send" | "resend" | "trailers", dryRun: boolean): Promise<B4Run> {
    return this.post("/v1/b4", { repo: b.repo, branch: b.name, action, dry_run: dryRun });
  }
}

const authed = (bridge: Bridge, init: RequestInit = {}): RequestInit => ({
  ...init,
  headers: { "X-Patchtrack-Token": bridge.token, "Content-Type": "application/json", ...(init.headers ?? {}) },
});

async function answered<T>(r: Response): Promise<T> {
  if (!r.ok) throw new Error(await refusal(r));
  return r.json();
}

async function reach(bridge: Bridge, path: string, init: RequestInit): Promise<Response> {
  try {
    return await fetch(`${bridge.url}${path}`, init);
  } catch {
    throw new Error(await whyNoAnswer(bridge));
  }
}

/**
 * A refusal without CORS headers looks like no answer at all. A request
 * that needs no CORS (its answer stays opaque) tells the two apart: if it
 * gets through, the bridge is running and does not serve this site.
 */
async function whyNoAnswer(bridge: Bridge): Promise<string> {
  try {
    await fetch(`${bridge.url}/v1/hello`, { mode: "no-cors" });
  } catch {
    const host = new URL(bridge.url).hostname;
    if (host === "127.0.0.1" || host === "localhost")
      return "The bridge did not answer. Is `patchtrack bridge` running, and did you allow this site to reach your computer's local network?";
    return `The bridge at ${new URL(bridge.url).host} did not answer. Is \`patchtrack bridge\` running there, and the tunnel or proxy that gives it this address?`;
  }
  const here = location.origin;
  return `The bridge is running but does not serve this site (${here}). Start it with \`patchtrack bridge --app ${here}/\`, or add \`--origin ${here}\`.`;
}

async function refusal(r: Response): Promise<string> {
  try {
    return `The bridge refused: ${(await r.json()).error}`;
  } catch {
    return `The bridge answered ${r.status}.`;
  }
}
