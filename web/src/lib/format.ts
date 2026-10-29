// How things read on screen: no DOM here, so vitest runs it.

import type { Series } from "../core/Series";
import type { State } from "../core/State";
import type { Summary } from "../core/Summary";
import type { Scope, Who } from "../protocol";

/** The same circles and colours as `patchtrack list`. */
const STYLE: Record<State, [string, string]> = {
  "merged": ["●", "merged"],
  "in-next": ["●", "done"],
  "queued": ["●", "done"],
  "applied": ["●", "done"],
  "partially-applied": ["◐", "done"],
  "commented": ["●", "attention"],
  "reviewed": ["●", "approved"],
  "discussed": ["●", "discussed"],
  "bot-only": ["●", "muted"],
  "no-reply": ["○", "waiting"],
  "unfetched": ["○", "muted"],
  "superseded": ["○", "muted"],
  "closed": ["⊘", "muted"],
};

export function stateStyle(state: State): { shape: string; tone: string } {
  const [shape, tone] = STYLE[state] ?? ["○", "muted"];
  return { shape, tone };
}

/** Still waiting on someone: not in, not replaced, not closed. */
export function isOpen(f: { state: State }): boolean {
  return !["applied", "queued", "in-next", "merged", "superseded", "partially-applied", "closed"].includes(f.state);
}

export function plural(n: number, one: string, many = one + "s"): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "3 days ago"; past a month, the date itself. */
export function ago(iso: string, now = new Date()): string {
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return iso;
  const s = Math.max(0, (now.getTime() - then.getTime()) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${plural(Math.floor(s / 60), "minute")} ago`;
  if (s < 86400) return `${plural(Math.floor(s / 3600), "hour")} ago`;
  const days = Math.floor(s / 86400);
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  return `on ${then.toISOString().slice(0, 10)}`;
}

/** "2026-10-05 10:00 UTC" */
export function when(iso: string | null): string {
  return iso ? `${iso.slice(0, 16).replace("T", " ")} UTC` : "";
}

export function lastActivity(s: Series): string {
  return s.replies.reduce((latest, r) => (r.date > latest ? r.date : latest), s.date);
}

/** Series::as_of_text, for the page. */
export function asOfText(s: Series): string {
  const at = (t: string) => t.slice(0, 16).replace("T", " ");
  let text: string;
  if (s.as_of && (!s.local_as_of || s.as_of >= s.local_as_of)) text = `as of ${at(s.as_of)} UTC`;
  else if (s.local_as_of) {
    // Local mail holds what was sent to you; a reply that did not copy you is missed.
    text = `as of ${at(s.local_as_of)} UTC, from your mailbox only`;
    if (s.as_of) text += ` (archive ${at(s.as_of)})`;
  } else text = "thread never fetched";
  return s.pending ? text + "; a fresher fetch is pending" : text;
}

/** "Jane Reviewer <jane@x>" -> "Jane Reviewer"; a bare address stays. */
export function displayName(who: string): string {
  const m = /^(.*?)\s*<([^<>]+)>$/.exec(who);
  return m ? m[1].replace(/^"|"$/g, "") || m[2] : who;
}

/** Addresses are noise beside names, as in `patchtrack list` without -v. */
export function hideAddresses(text: string): string {
  return text.replace(/ <[^<>\s]+@[^<>\s]+>(?!;|$)/g, "");
}

// The message-id that closes an evidence clause: "... <id>" or "... <id>; ..."
const CITED = / <([^<>\s]+)>(?=;|$)/g;

/** Evidence as text and the messages it cites, in order. */
export function evidenceParts(text: string): Array<{ text: string } | { msgid: string }> {
  const out: Array<{ text: string } | { msgid: string }> = [];
  let last = 0;
  for (const m of text.matchAll(CITED)) {
    if (m.index > last) out.push({ text: text.slice(last, m.index) });
    out.push({ msgid: m[1] });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last) });
  return out;
}

/** Evidence on one line of a list: without message-ids or addresses. */
export function evidenceLine(text: string): string {
  return hideAddresses(
    evidenceParts(text)
      .map((p) => ("text" in p ? p.text : ""))
      .join(""),
  ).trim();
}

/** A family's id, as the core's `Family::id`: sender NUL key. */
export function familyId(f: { sender: string; key: string }): string {
  return `${f.sender}\0${f.key}`;
}

/** The families `who` shows; null for everyone's. */
export function whose(scope: Scope, who: Who): Set<string> | null {
  return who === "mine" ? new Set(scope.mine) : who === "followed" ? new Set(scope.followed) : null;
}

/** The families among `ids` (null: all of them). */
export function ofFamilies(families: Summary[], ids: Set<string> | null): Summary[] {
  return ids ? families.filter((f) => ids.has(familyId(f))) : families;
}

/** A project's series (none named: all of them); and of those, a list's. */
export function inProject(families: Summary[], project?: string, list?: string): Summary[] {
  return families.filter((f) => (!project || f.projects.includes(project)) && (!list || f.lists.includes(list)));
}

/** The lists these series went to, the busiest first: a project's subsystems. */
export function listsOf(families: Summary[]): Array<[string, number]> {
  const n = new Map<string, number>();
  for (const f of families) for (const l of f.lists) if (l !== "all") n.set(l, (n.get(l) ?? 0) + 1);
  return [...n.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
}

/**
 * The query with its `key:` qualifier set to `value` ("" removes it), as the
 * list's State and Sort menus change it.
 */
export function withQualifier(q: string, key: string, value: string): string {
  const words = q.split(/\s+/).filter((w) => w && !w.toLowerCase().startsWith(`${key}:`));
  if (value) words.push(`${key}:${value}`);
  return words.join(" ");
}

/** A qualifier's value in the query, or "". */
export function qualifier(q: string, key: string): string {
  const w = q.split(/\s+/).find((w) => w.toLowerCase().startsWith(`${key}:`));
  return w ? w.slice(key.length + 1).toLowerCase() : "";
}

/**
 * The families of one tab that match a search: words are found in the
 * title, state, sender or lists; `state:reviewed` (a state, exactly),
 * `author:dev@` (who sent or wrote it), `list:linux-usb` and `is:unanswered`
 * narrow it, and a leading "-" turns any of them around; `sort:updated`
 * puts the latest activity first and `sort:oldest` the oldest series.
 */
export function filterFamilies(families: Summary[], tab: "open" | "closed", q: string): Summary[] {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  let sort = "";
  const tests: Array<(f: Summary) => boolean> = [];
  for (const raw of words) {
    const not = raw.startsWith("-") && raw.length > 1;
    const w = not ? raw.slice(1) : raw;
    const [key, value] = w.includes(":") ? [w.slice(0, w.indexOf(":")), w.slice(w.indexOf(":") + 1)] : ["", w];
    let t: (f: Summary) => boolean;
    if (key === "sort") {
      sort = value;
      continue;
    } else if (key === "state") t = (f) => f.state === value;
    else if (key === "author") t = (f) => [f.sender, ...f.written_by].some((a) => a.toLowerCase().includes(value));
    else if (key === "list") t = (f) => f.lists.some((l) => l.toLowerCase().includes(value));
    else if (key === "is" && value === "unanswered") t = (f) => f.unanswered;
    else t = (f) => `${f.title} ${f.state} ${f.sender} ${f.lists.join(" ")}`.toLowerCase().includes(w);
    tests.push(not ? (f) => !t(f) : t);
  }
  const out = families.filter((f) => (tab === "open") === isOpen(f) && tests.every((t) => t(f)));
  if (sort === "updated") out.sort((a, b) => b.last_activity.localeCompare(a.last_activity));
  else if (sort === "oldest") out.reverse();
  return out;
}

export type Block = { kind: "text" | "quote" | "signature"; lines: string[] };

/** A mail body as blocks: text, quotes (lines starting ">") and the signature. */
export function bodyBlocks(body: string): Block[] {
  const lines = body.replace(/\s+$/, "").split("\n");
  const sig = lines.findIndex((l) => l.trimEnd() === "--");
  const main = sig < 0 ? lines : lines.slice(0, sig);
  const out: Block[] = [];
  for (const line of main) {
    const kind = /^\s*>/.test(line) ? "quote" : "text";
    const last = out[out.length - 1];
    // A blank line inside a quote belongs to it.
    if (last && (last.kind === kind || (last.kind === "quote" && !line.trim()))) last.lines.push(line);
    else out.push({ kind, lines: [line] });
  }
  for (const b of out) {
    if (b.kind === "quote") while (b.lines.length && !b.lines[b.lines.length - 1].trim()) b.lines.pop();
  }
  if (sig >= 0) out.push({ kind: "signature", lines: lines.slice(sig) });
  return out.filter((b) => b.lines.some((l) => l.trim()));
}

/**
 * A long run of lines as what stays in view and what folds away: past
 * `max` lines, the first `keep`. One real reply carried a several-thousand-line
 * USB descriptor dump.
 */
export function foldLong(lines: string[], keep = 30, max = 60): { shown: string[]; folded: string[] } {
  if (lines.length <= max) return { shown: lines, folded: [] };
  return { shown: lines.slice(0, keep), folded: lines.slice(keep) };
}

export function diffLineClass(line: string): string {
  if (line.startsWith("+++") || line.startsWith("---")) return "meta";
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "del";
  if (line.startsWith("diff --git") || line.startsWith("index ")) return "meta";
  return "";
}

export function diffStats(diff: string): { files: number; add: number; del: number } {
  let files = 0, add = 0, del = 0;
  for (const line of diff.split("\n")) {
    if (line.startsWith("diff --git ")) files++;
    else if (line.startsWith("+") && !line.startsWith("+++")) add++;
    else if (line.startsWith("-") && !line.startsWith("---")) del++;
  }
  return { files, add, del };
}
