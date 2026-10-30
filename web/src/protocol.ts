// The shapes the bridge answers with that are not the core's model (those
// are generated, in core/).

import type { Listed } from "./core/Listed";
import type { State } from "./core/State";
import type { Summary } from "./core/Summary";

/** A setting: what the core says of it, and its stored JSON (null: the default). */
export type Setting = {
  key: string;
  shape: string;
  /** The control it takes: addresses, text, url, time, days, minutes, bool, projects, profiles, bots. */
  input: "addresses" | "text" | "url" | "time" | "days" | "minutes" | "bool" | "projects" | "profiles" | "bots";
  default: string;
  doc: string;
  value: string | null;
};

/** An open family that needs something of you. */
export type Nudge = { sender: string; key: string; title: string; state: State; kind: string; days: number; why: string };

/** A family that changed since you last read it, and how. */
export type Notification = {
  sender: string;
  key: string;
  title: string;
  state: State;
  /** The latest version, its lists, and its last activity. */
  version: number;
  lists: string[];
  updated: string;
  newSeries: boolean;
  newVersion: number | null;
  stateChange: [State, State] | null;
  /** A line for each new reply. */
  replies: string[];
};
/** `first` before anything was ever read: no baseline to compare with yet. */
export type Notifications = { first: boolean; notifications: Notification[] };

/** Whose series a view shows: "followed" is the authors you follow and your identities. */
export type Who = "everyone" | "followed" | "mine";

/**
 * The families (by `familyId`) of "mine" and "followed"; and the addresses
 * of the authors you follow and your identities, whose mail "only what you
 * follow" keeps.
 */
export type Scope = { mine: string[]; followed: string[]; authors: string[] };

/** What adding mail did: messages read, kept (all, unless only followed), and new; `store` for a store file merged. */
export type Added = { read: number; kept: number; new: number; store: boolean };

/** Mail you composed, kept until you remove it. */
export type Draft = {
  msgid: string;
  /** The family it is about (sender NUL key), or "". */
  family: string;
  kind: "reply" | "tag" | "review" | "thanks" | "ping";
  created_at: string;
  sent_at: string | null;
  subject: string | null;
  to: string[] | null;
  cc: string[] | null;
  in_reply_to: string | null;
  body: string | null;
  /** The message as it will be sent. */
  raw: string;
};

/** A comment on a line of a patch: its message-id, which line of its diff (from 0), and what to say. */
export type ReviewComment = { msgid: string; at: number; text: string };

/** Kinds of subscription: an author, every series on a list, a search, one thread. */
export type Kind = "author" | "list" | "query" | "thread";

/** What this store follows. */
export type Subscription = { kind: Kind; value: string; list: string; archive: string; label: string };

/** A project: an archive, its lists there (none: every one), and recipients that place your own mail in it. */
export type Project = { name: string; archive: string; lists: string[]; recipients: string[] };

/** Your projects, the built-in ones, and the archive setting's archive. */
export type Projects = { projects: Project[]; presets: Project[]; archive: string };

/**
 * An inbox to browse: in a project ("" for none), how many series it has
 * here, when it was last listed, and how far back it has been listed.
 */
export type Inbox = {
  project: string;
  inbox: string;
  /** What the archive says it is, and when it last changed. */
  description: string;
  updated: string | null;
  /** When its newest page was listed here, and how many threads it has listed. */
  listed: string | null;
  series: number;
};

/** A thread an inbox listed: its newest message, its title, and its series where the store has it. */
export type ListedRow = { listed: Listed; title: string; family: Summary | null };

/**
 * An inbox's listed threads (its patches, or what a search of it found),
 * newest first; when its newest page was listed, where the next starts, and
 * why the archive last would not list it.
 */
export type Listing = { listed: string | null; next: number; failed: string | null; rows: ListedRow[] };

/** A listed thread, opened: its series, or it is being fetched, or the archive no longer has it. */
export type Opened = { family: { sender: string; key: string; version: number } } | { fetching: true } | { missing: true };

/** Local mail the bridge reads on each sync (`patchtrack source add`). */
export type Source = { path: string; read_at: string | null };

/** The bridge's own jobs: a sync running, or when the last finished and how; threads and inbox pages. */
export type SyncStatus = {
  running: boolean;
  /** A sync is wanted as soon as the job running is done. */
  asked: boolean;
  /** What the bridge does now, in a few words, if anything. */
  doing: string | null;
  /** Threads opened in the app are being fetched, or wait to be. */
  threads: boolean;
  /** Inboxes being listed, or waiting to be. */
  browsing: string[];
  /** Searches being listed, or waiting to be. */
  searching: { inbox: string; query: string }[];
  /** An archive's list of inboxes is being read, or waits to be. */
  inboxes: boolean;
  started: string | null;
  finished: string | null;
  /** Whether the last sync finished with nothing failed. */
  ok: boolean | null;
  /** What the current or last sync said. */
  log: string[];
};

/** The store's generation (it changes whenever the store does), and the syncs. */
export type Status = { generation: number; messages: number; sync: SyncStatus; unread: number };
