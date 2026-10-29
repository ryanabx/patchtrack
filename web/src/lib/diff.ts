// A posted patch's diff, read as GitHub's "Files changed" shows it: each
// file, its hunks, and each line's numbers on either side. No DOM here, so
// vitest runs it.

export type DiffLine = {
  kind: "add" | "del" | "ctx" | "hunk" | "note";
  text: string;
  /** Line numbers before and after, where the line has one. */
  old: number | null;
  new: number | null;
  /** Which line of the diff text it is, from 0: what a review comment names. */
  at: number;
};

export type FileDiff = {
  path: string;
  /** The path before, if the patch renamed the file. */
  from: string | null;
  status: "added" | "deleted" | "renamed" | "modified" | "binary";
  lines: DiffLine[];
  add: number;
  del: number;
};

const HUNK = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

/**
 * The files of a diff, from its first `diff --git` on. A hunk ends when its
 * header's line counts are used up, so what follows the last one (git's
 * "-- " signature) is not taken for a line of it.
 */
export function parseDiff(text: string): FileDiff[] {
  const files: FileDiff[] = [];
  let f: FileDiff | null = null;
  let oldLeft = 0, newLeft = 0, oldNo = 0, newNo = 0;
  for (const [at, line] of text.split("\n").entries()) {
    if (line.startsWith("diff --git ")) {
      const m = /^diff --git a\/(.*) b\/(.*)$/.exec(line);
      f = { path: m ? m[2] : line.slice(11), from: null, status: "modified", lines: [], add: 0, del: 0 };
      files.push(f);
      oldLeft = newLeft = 0;
      continue;
    }
    if (!f) continue;
    if (oldLeft > 0 || newLeft > 0) {
      const c = line[0];
      if (c === "+") {
        f.lines.push({ kind: "add", text: line.slice(1), old: null, new: newNo++, at });
        f.add++;
        newLeft--;
      } else if (c === "-") {
        f.lines.push({ kind: "del", text: line.slice(1), old: oldNo++, new: null, at });
        f.del++;
        oldLeft--;
      } else if (c === "\\") {
        f.lines.push({ kind: "note", text: line, old: null, new: null, at });
      } else {
        // A context line; an empty one may have lost its space on the way.
        f.lines.push({ kind: "ctx", text: line.slice(1), old: oldNo++, new: newNo++, at });
        oldLeft--;
        newLeft--;
      }
      continue;
    }
    const h = HUNK.exec(line);
    if (h) {
      [oldNo, newNo] = [Number(h[1]), Number(h[3])];
      [oldLeft, newLeft] = [h[2] === undefined ? 1 : Number(h[2]), h[4] === undefined ? 1 : Number(h[4])];
      f.lines.push({ kind: "hunk", text: line, old: null, new: null, at });
    } else if (line.startsWith("new file mode")) f.status = "added";
    else if (line.startsWith("deleted file mode")) f.status = "deleted";
    else if (line.startsWith("rename from ")) {
      f.status = "renamed";
      f.from = line.slice(12);
    } else if (line.startsWith("Binary files ") || line === "GIT binary patch") f.status = "binary";
  }
  return files;
}

/** A patch: its number, its message-id and its diff. */
export type PatchDiff = { patch: number; msgid: string; diff: string };

/** One file as all of a version's patches touch it, in patch order. */
export type FileAcross = { path: string; parts: Array<{ patch: number; msgid: string; file: FileDiff }>; add: number; del: number };

/** The files a version's patches touch, in the order the patches first touch them. */
export function filesAcross(patches: PatchDiff[]): FileAcross[] {
  const out: FileAcross[] = [];
  for (const p of patches) {
    for (const file of parseDiff(p.diff)) {
      let f = out.find((x) => x.path === file.path);
      if (!f) {
        f = { path: file.path, parts: [], add: 0, del: 0 };
        out.push(f);
      }
      f.parts.push({ patch: p.patch, msgid: p.msgid, file });
      f.add += file.add;
      f.del += file.del;
    }
  }
  return out;
}

/** A message-id as a URL path segment, as the core quotes it (Python's quote, safe "@+-._~="). */
export function pathQuote(msgid: string): string {
  const bytes = new TextEncoder().encode(msgid.replace(/^<+|>+$/g, ""));
  let out = "";
  for (const b of bytes) {
    const c = String.fromCharCode(b);
    out += /[A-Za-z0-9@+\-._~=]/.test(c) ? c : `%${b.toString(16).toUpperCase().padStart(2, "0")}`;
  }
  return out;
}

/** A message's page on its archive. */
export function messageUrl(archive: string, msgid: string): string {
  return `${archive.replace(/\/+$/, "")}/all/${pathQuote(msgid)}/`;
}
