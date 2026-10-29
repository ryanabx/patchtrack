import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";

import type { Changes as ChangesData } from "../core/Changes";
import type { Entry } from "../core/Entry";
import type { Family } from "../core/Family";
import type { FileChange } from "../core/FileChange";
import type { Series } from "../core/Series";
import type { State } from "../core/State";
import { type DiffLine, type FileDiff, filesAcross, messageUrl, pathQuote } from "../lib/diff";
import {
  asOfText,
  bodyBlocks,
  diffLineClass,
  diffStats,
  displayName,
  evidenceParts,
  foldLong,
  hideAddresses,
  lastActivity,
  plural,
} from "../lib/format";
import { href, type Route } from "../route";
import type { Branch, FamilyPage, Store } from "../store";
import { Avatar, Label, Pill, Time } from "./common";
import { Composer, DraftCard } from "./Drafts";
import type { Draft, ReviewComment } from "../protocol";
import { branchFor, branchState } from "../lib/branches";

/** The bridge's branches, asked once for as long as the page lives. */
let branchCache: Promise<Branch[]> | null = null;
function cachedBranches(store: Store): Promise<Branch[]> {
  if (!branchCache) {
    branchCache = store.branches();
    branchCache.catch(() => (branchCache = null));
  }
  return branchCache;
}

/** The b4 prep branch that posted this series, where the bridge can see it. */
function BranchSection({ store, f }: { store: Store; f: Family }) {
  const [b, setB] = useState<Branch | null>(null);
  useEffect(() => {
    if (!store.hello.can.b4) return;
    cachedBranches(store).then(
      (all) => setB(branchFor(f.versions.map((v) => v.root), f.key, all) ?? null),
      () => setB(null),
    );
  }, [f.key]);
  if (!b) return null;
  return (
    <section>
      <h3>Branch</h3>
      <p class="small">
        <code>{b.name}</code> <span class="muted">in {b.repo}</span>
      </p>
      <p class="muted small">{branchState(b)}</p>
      <a class="button small" href={href({ view: "branches" })}>
        Send or resend with b4
      </a>
    </section>
  );
}

type FamilyRoute = Extract<Route, { view: "family" }>;

const QUOTE_KEEP = 3; // quoted lines left in view above a comment, as `read` does
const STAGES = ["not applied", "applied", "queued", "in-next", "merged"];
const STAGE_STATE: State[] = ["no-reply", "applied", "queued", "in-next", "merged"];

/** A cover letter up to b4's footer (base-commit:, change-id:) or the signature. */
function coverText(body: string): string {
  const out: string[] = [];
  for (const line of body.split("\n")) {
    if (/^(base-commit:|change-id:|prerequisite-)/.test(line) || line.trimEnd() === "--") break;
    out.push(line);
  }
  return out.join("\n").trimEnd();
}

/** The version whose conversation holds a message. */
function versionOf(f: Family, msgid: string): Series | undefined {
  return f.versions.find((s) => s.root === msgid || Object.values(s.patches).includes(msgid) || s.replies.some((r) => r.msgid === msgid));
}

/** Prose, with a very long run folded after its first lines. */
function Prose({ lines }: { lines: string[] }) {
  const { shown, folded } = foldLong(lines);
  if (!folded.length) return <pre class="prose">{lines.join("\n")}</pre>;
  return (
    <>
      <pre class="prose">{shown.join("\n")}</pre>
      <details class="fold long">
        <summary>… {plural(folded.length, "more line")}</summary>
        <pre class="prose">{folded.join("\n")}</pre>
      </details>
    </>
  );
}

function Body({ text }: { text: string }) {
  return (
    <div class="mailbody">
      {bodyBlocks(text).map((b) => {
        if (b.kind === "text") return <Prose lines={b.lines} />;
        if (b.kind === "signature")
          return (
            <details class="fold">
              <summary>signature</summary>
              <pre class="quote">{b.lines.join("\n")}</pre>
            </details>
          );
        if (b.lines.length <= QUOTE_KEEP + 1) return <pre class="quote">{b.lines.join("\n")}</pre>;
        const hidden = b.lines.slice(0, -QUOTE_KEEP);
        return (
          <>
            <details class="fold">
              <summary>… {plural(hidden.length, "quoted line")}</summary>
              <pre class="quote">{hidden.join("\n")}</pre>
            </details>
            <pre class="quote">{b.lines.slice(-QUOTE_KEEP).join("\n")}</pre>
          </>
        );
      })}
    </div>
  );
}

function Diff({ text }: { text: string }) {
  return (
    <pre class="diff">
      {text.split("\n").map((l) => (
        <span class={diffLineClass(l) || undefined}>{l + "\n"}</span>
      ))}
    </pre>
  );
}

/** "Reply", and the box it opens below the message. */
type ReplyBox = { open: boolean; toggle: () => void; box: ComponentChildren };

function ReplyButton({ r }: { r: ReplyBox }) {
  return (
    <button type="button" class="button small reply" onClick={r.toggle} aria-expanded={r.open}>
      Reply
    </button>
  );
}

/** A message's page on the archive, beside its Reply button. */
function OnArchive({ url }: { url: string }) {
  return (
    <a class="on-archive muted" href={url} target="_blank" rel="noreferrer" title="This message on the archive">
      archive ↗
    </a>
  );
}

function Message({ e, s, target, reply, archive }: { e: Entry; s: Series; target: boolean; reply: ReplyBox; archive: string }) {
  const of = s.total > 1 ? `${e.patch}/${s.total}` : "patch";
  const url = messageUrl(archive, e.msgid);
  const cls = `msg depth-${Math.min(e.depth, 4)} kind-${e.kind}${target ? " target" : ""}`;
  if (e.kind === "patch") {
    const st = diffStats(e.diff ?? "");
    const stage = s.stages[e.patch] ?? 0;
    return (
      <div class={cls} id={`m-${e.msgid}`} data-msgid={e.msgid}>
        <div class="msg-header commit">
          <span class="mono">{of}</span>
          <strong>{s.titles[e.patch] ?? e.subject}</strong>
          <Pill state={STAGE_STATE[stage]} text={STAGES[stage]} />
          <span class="spacer" />
          <Time iso={e.date} />
          <OnArchive url={url} />
          <ReplyButton r={reply} />
        </div>
        <div class="msg-body">
          <Prose lines={(e.message ?? "").split("\n")} />
          {e.diff && (
            <details class="fold diff-fold">
              <summary>
                {plural(st.files, "file")} changed, <span class="add">+{st.add}</span> <span class="del">−{st.del}</span>
              </summary>
              <Diff text={e.diff} />
            </details>
          )}
        </div>
        {reply.open && <div class="reply-box">{reply.box}</div>}
      </div>
    );
  }
  const who = e.from_name ? `${e.from_name} <${e.from_addr}>` : e.from_addr;
  return (
    <div class={cls} id={`m-${e.msgid}`} data-msgid={e.msgid}>
      <Avatar who={who} />
      <div class="msg-box">
        <div class="msg-header">
          <strong title={e.from_addr}>{displayName(who)}</strong>
          {e.mine && <Label text="author" />}
          {e.bot && <Label text="bot" />}
          <span class="muted">{e.kind === "cover" ? ` posted ${s.title}` : e.patch ? ` on ${of}` : " on the cover"}</span>
          <span class="spacer" />
          <Time iso={e.date} />
          <OnArchive url={url} />
          <ReplyButton r={reply} />
        </div>
        {(e.tags.length > 0 || e.applied) && (
          <div class="msg-events">
            {e.tags.map(([kind]) => (
              <Label text={kind} kind="tag" />
            ))}
            {e.applied && <Label text={`applied: “${e.applied}”`} kind="applied" />}
          </div>
        )}
        <div class="msg-body">
          <Body text={e.kind === "cover" ? coverText(e.body) : e.body} />
        </div>
        {reply.open && <div class="reply-box">{reply.box}</div>}
      </div>
    </div>
  );
}

type Loaded = { entries: Entry[] | null; error: string | null };

/** A version posted before or after this one, as GitHub marks a force-push in a pull request's timeline. */
function VersionEvent({ v, s, route, later }: { v: Series; s: Series; route: FamilyRoute; later: boolean }) {
  const [older, newer] = later ? [s, v] : [v, s];
  return (
    <div class="version-event muted" data-version={v.version}>
      <span class="shape">⟳</span>{" "}
      {later ? (
        <>
          <strong>v{v.version}</strong> was posted <Time iso={v.date} />, after this one.{" "}
          <a href={href({ ...route, v: v.version, tab: "conversation", vs: null, c: null, m: null })}>See v{v.version}</a>
        </>
      ) : (
        <>
          This is v{s.version}; <strong>v{v.version}</strong> was posted <Time iso={v.date} />.{" "}
          <a href={href({ ...route, v: v.version, tab: "conversation", vs: null, c: null, m: null })}>See v{v.version}</a>
        </>
      )}
      {" · "}
      <a href={href({ ...route, v: newer.version, tab: "commits", vs: older.version, c: null, m: null })}>
        Compare v{older.version} with v{newer.version}
      </a>
    </div>
  );
}

function Conversation({
  store,
  s,
  target,
  loaded,
  archive,
  before,
  after,
  route,
}: {
  store: Store;
  s: Series;
  target: string | null;
  loaded: Loaded;
  archive: string;
  /** The versions just before and after this one, if any. */
  before?: Series;
  after?: Series;
  route: FamilyRoute;
}) {
  const [replying, setReplying] = useState<string | null>(null);
  const reply = (msgid: string): ReplyBox => ({
    open: replying === msgid,
    toggle: () => setReplying(replying === msgid ? null : msgid),
    box: <Composer key={msgid} store={store} to={msgid} onDone={() => setReplying(null)} />,
  });
  // To the message a link names, once: not again whenever the store changes.
  const scrolled = useRef<string | null>(null);
  useEffect(() => {
    if (!loaded.entries || !target || scrolled.current === target) return;
    document.getElementById(`m-${target}`)?.scrollIntoView({ block: "start" });
    scrolled.current = target;
  }, [loaded.entries, target]);
  if (loaded.error) return <p class="error">{loaded.error}</p>;
  if (!loaded.entries) return <p class="muted">Loading the conversation…</p>;
  return (
    <div class="timeline">
      {before && <VersionEvent v={before} s={s} route={route} later={false} />}
      {loaded.entries.map((e) => (
        <Message key={e.msgid} e={e} s={s} target={e.msgid === target} reply={reply(e.msgid)} archive={archive} />
      ))}
      {after && <VersionEvent v={after} s={s} route={route} later={true} />}
    </div>
  );
}

/** The latest posting of each version number, oldest first: a RESEND replaces its version. */
function byVersion(f: Family): Series[] {
  const latest = new Map<number, Series>();
  for (const v of [...f.versions].sort((a, b) => a.version - b.version || a.date.localeCompare(b.date))) latest.set(v.version, v);
  return [...latest.values()];
}

/** The patches of a version, in order, from its conversation. */
function patchesOf(entries: Entry[]): Entry[] {
  return entries.filter((e) => e.kind === "patch").sort((a, b) => a.patch - b.patch);
}

/** A patch's commit: its message and its diff, file by file. */
function CommitBody({ e, review }: { e: Entry; review: Review }) {
  return (
    <div class="commit-body">
      <Prose lines={(e.message ?? "").split("\n")} />
      <FileList files={filesAcross([{ patch: e.patch, msgid: e.msgid, diff: e.diff ?? "" }])} total={0} review={review} />
    </div>
  );
}

/** A pending comment, by the patch and the line of its diff it is on. */
type Pending = ReviewComment & { where: string };
const keyOf = (msgid: string, at: number) => `${msgid} ${at}`;

/** Comments waiting to become a review, and the line whose comment is being written. */
type Review = {
  pending: Map<string, Pending>;
  /** Keep a comment; an empty one is removed. */
  set: (c: Pending) => void;
  editing: string | null;
  edit: (key: string | null) => void;
};

function CommentForm({ review, c }: { review: Review; c: Pending }) {
  const [text, setText] = useState(c.text);
  const had = review.pending.has(keyOf(c.msgid, c.at));
  return (
    <form
      class="line-comment-form"
      onSubmit={(ev) => {
        ev.preventDefault();
        review.set({ ...c, text });
        review.edit(null);
      }}
    >
      <textarea rows={3} placeholder={`A comment on ${c.where}`} value={text} onInput={(ev) => setText((ev.target as HTMLTextAreaElement).value)} />
      <div class="composer-actions">
        {had && (
          <button type="button" class="button danger" onClick={() => (review.set({ ...c, text: "" }), review.edit(null))}>
            Delete
          </button>
        )}
        <span class="spacer" />
        <button type="button" class="button" onClick={() => review.edit(null)}>
          Cancel
        </button>
        <button type="submit" class="button primary" disabled={!text.trim()}>
          {had ? "Update comment" : "Add review comment"}
        </button>
      </div>
    </form>
  );
}

/** What a comment is on, for the person: the file and the line's number. */
const whereOf = (path: string, l: DiffLine) => `${path}:${l.new ?? l.old ?? ""}`;

/** The version's patches as GitHub lists a pull request's commits, each with where it stands. */
function Commits({ f, s, loaded, route, review }: { f: Family; s: Series; loaded: Loaded; route: FamilyRoute; review: Review }) {
  if (loaded.error) return <p class="error">{loaded.error}</p>;
  if (!loaded.entries) return <p class="muted">Loading the commits…</p>;
  const patches = patchesOf(loaded.entries);
  const head = f.versions[f.versions.length - 1];
  const byRoot = new Map(f.versions.map((v) => [v.root, v]));
  // Patches the family still counts from an older version: dropped once applied.
  const carried = s === head ? f.parts.filter((p) => p.root !== s.root) : [];
  if (!patches.length) return <p class="blankslate">This posting carries no patches: it is a cover letter alone.</p>;
  return (
    <div class="commits">
      <ul class="box rows">
        {patches.map((e) => {
          const stage = s.stages[e.patch] ?? 0;
          const tags = s.replies.filter((r) => r.index === e.patch).flatMap((r) => r.trailers);
          const said = s.replies.filter((r) => r.index === e.patch && !r.bot && !r.mine).length;
          const open = route.c === e.patch;
          const st = diffStats(e.diff ?? "");
          return (
            <li class={`row commit${open ? " open" : ""}`} data-patch={e.patch}>
              <div class="row-main">
                <div class="row-title">
                  <span class="muted mono">
                    {e.patch}/{s.total}
                  </span>{" "}
                  <a href={href({ ...route, c: open ? null : e.patch, m: null })} aria-expanded={open}>
                    <strong>{s.titles[e.patch] ?? e.subject}</strong>
                  </a>{" "}
                  <Pill state={STAGE_STATE[stage]} text={STAGES[stage]} />
                </div>
                <div class="row-meta">
                  {displayName(e.from_name ? `${e.from_name} <${e.from_addr}>` : e.from_addr)} · <Time iso={e.date} /> ·{" "}
                  <span class="add">+{st.add}</span> <span class="del">−{st.del}</span>
                  {said > 0 && (
                    <>
                      {" · "}
                      <a href={href({ ...route, tab: "conversation", c: null, m: e.msgid })}>{plural(said, "reply", "replies")}</a>
                    </>
                  )}
                  {tags.map(([kind, who]) => (
                    <>
                      {" "}
                      <Label text={kind} kind="tag" /> {displayName(who)}
                    </>
                  ))}
                </div>
                {(s.tree_rows[e.patch] ?? []).map((r) => (
                  <div class="row-meta mono">
                    {r.tree} @ {r.ref_sha.slice(0, 12)}: {r.commit_sha ? `${r.commit_sha.slice(0, 12)} (by ${r.how})` : "not found"}
                    {r.reverted_by && `, reverted by ${r.reverted_by.slice(0, 12)}`}, checked {r.checked_at.slice(0, 10)}
                  </div>
                ))}
                {open && <CommitBody e={e} review={review} />}
              </div>
            </li>
          );
        })}
      </ul>
      {carried.length > 0 && (
        <>
          <h3>Also counted</h3>
          <p class="note">Dropped from this version once applied: the family still counts them.</p>
          <ul class="box rows">
            {carried.map((p) => (
              <li class="row">
                <Pill state={STAGE_STATE[p.stage]} text={STAGES[p.stage]} />
                <div class="row-main">
                  <span class="muted mono">
                    v{p.version} {p.patch}/{p.total}
                  </span>{" "}
                  <strong>{byRoot.get(p.root)?.titles[p.patch] ?? ""}</strong>
                </div>
              </li>
            ))}
          </ul>
        </>
      )}
    </div>
  );
}

const MARKS: Record<string, string> = { "=": "unchanged", "!": "changed", "<": "dropped", ">": "added" };

/** One version against another, patch by patch, as `patchtrack diff` (and git range-diff) shows it. */
function CommitsCompared({ store, base, s }: { store: Store; base: Series; s: Series }) {
  const [changes, setChanges] = useState<ChangesData | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setChanges(null);
    setError(null);
    store.changes(base.root, s.root).then(setChanges, (e: Error) => setError(e.message));
  }, [base.root, s.root]);
  if (error) return <p class="error">{error}</p>;
  if (!changes) return <p class="muted">Comparing…</p>;
  return (
    <div class="changes">
      {changes.changelog.length > 0 && (
        <>
          <h3>The author's changelog</h3>
          <pre class="changelog">{changes.changelog.join("\n")}</pre>
        </>
      )}
      <ul class="pairs">
        {changes.pairs.map((p) => (
          <li class={`pair ${MARKS[p.marker]}`}>
            <div class="pair-head">
              <span class="mark" title={MARKS[p.marker]}>
                {p.marker}
              </span>{" "}
              <span class="muted">
                {p.old ? `${p.old.patch}/${base.total}` : "-"} → {p.new ? `${p.new.patch}/${s.total}` : "-"}
              </span>{" "}
              {(p.new ?? p.old)?.title} <span class="muted">{MARKS[p.marker]}</span>
            </div>
            {p.message.length + p.diff.length > 0 && <Diff text={[...p.message, ...p.diff].join("\n")} />}
          </li>
        ))}
      </ul>
      <p class="muted small">
        Hunk line numbers, git's function context and index lines are ignored, so a rebase alone is not a change.
      </p>
    </div>
  );
}

/** One file of one patch, with line numbers either side; with a review, each line takes a comment. */
function FileLines({ file, msgid, review }: { file: FileDiff; msgid: string; review?: Review }) {
  if (file.status === "binary") return <p class="muted small file-note">A binary file.</p>;
  return (
    <table class="file-lines">
      <tbody>
        {file.lines.map((l) => {
          const key = keyOf(msgid, l.at);
          const commentable = review && (l.kind === "add" || l.kind === "del" || l.kind === "ctx");
          const pending = review?.pending.get(key);
          const here: Pending = pending ?? { msgid, at: l.at, text: "", where: whereOf(file.path, l) };
          return (
            <>
              <tr class={`line ${l.kind}`} data-at={l.at}>
                <td class="num">{l.old ?? ""}</td>
                <td class="num">{l.new ?? ""}</td>
                <td class={`code${review ? " commentable" : ""}`}>
                  {commentable && (
                    <button type="button" class="add-comment" title="Comment on this line" aria-label="Comment on this line" onClick={() => review.edit(key)}>
                      +
                    </button>
                  )}
                  {l.kind === "add" ? "+" : l.kind === "del" ? "-" : l.kind === "ctx" ? " " : ""}
                  {l.text}
                </td>
              </tr>
              {review && review.editing === key ? (
                <tr class="line-comment">
                  <td colSpan={3}>
                    <CommentForm review={review} c={here} />
                  </td>
                </tr>
              ) : (
                pending && (
                  <tr class="line-comment" data-at={l.at}>
                    <td colSpan={3}>
                      <div class="pending-comment">
                        <Label text="Pending" kind="flag" /> <pre class="prose">{pending.text}</pre>
                        <button type="button" class="button small" onClick={() => review?.edit(key)}>
                          Edit
                        </button>
                      </div>
                    </td>
                  </tr>
                )
              )}
            </>
          );
        })}
      </tbody>
    </table>
  );
}

const VERDICTS: Array<[string, string, string]> = [
  ["", "Comment", "Your comments, without a tag."],
  ["Reviewed-by", "Approve", "Reviewed-by, on the cover: the series has your review."],
  ["Acked-by", "Ack", "Acked-by: you, as a maintainer of what it touches, agree."],
  ["Tested-by", "Tested", "Tested-by: you ran it."],
];

/**
 * GitHub's "Finish your review", for mail: the pending comments become one
 * reply per patch, each line's comment below its quoted hunk, and the
 * summary and tag a reply to the cover. Drafts, sent only when you say.
 */
function ReviewBar({ store, s, review, clear }: { store: Store; s: Series; review: Review; clear: () => void }) {
  const [open, setOpen] = useState(false);
  const [summary, setSummary] = useState("");
  const [tag, setTag] = useState("");
  const [drafts, setDrafts] = useState<Draft[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const n = review.pending.size;
  return (
    <div class="review-bar">
      <div class="review-head">
        <span class="muted small">
          {n ? `${plural(n, "pending comment")} on v${s.version}` : "Comment on a line with its +, then finish your review."}
        </span>
        <span class="spacer" />
        <button type="button" class="button primary" aria-expanded={open} onClick={() => setOpen(!open)}>
          Finish your review{n ? ` (${n})` : ""} ▾
        </button>
      </div>
      {open && (
        <form
          class="review-panel"
          onSubmit={(ev) => {
            ev.preventDefault();
            setError(null);
            const comments = [...review.pending.values()].map(({ msgid, at, text }) => ({ msgid, at, text }));
            store.review(s.root, comments, summary, tag).then(
              (d) => {
                setDrafts(d);
                setOpen(false);
                setSummary("");
                setTag("");
                clear();
              },
              (e: Error) => setError(e.message),
            );
          }}
        >
          <textarea rows={4} placeholder="Leave a comment on the whole series (optional)" value={summary} onInput={(ev) => setSummary((ev.target as HTMLTextAreaElement).value)} />
          <fieldset class="verdicts">
            {VERDICTS.map(([value, name, why]) => (
              <label>
                <input type="radio" name="verdict" value={value} checked={tag === value} onChange={() => setTag(value)} /> <strong>{name}</strong>{" "}
                <span class="muted small">{why}</span>
              </label>
            ))}
          </fieldset>
          <p class="muted small">
            Each patch you commented on gets a reply quoting the lines, your comment below each; the summary and the tag go to{" "}
            {s.patches[0] ? "the cover letter" : "the patch"}. They are kept as drafts: nothing is sent until you send it.
          </p>
          <div class="composer-actions">
            {n > 0 && (
              <button type="button" class="button danger" onClick={() => (clear(), setOpen(false))}>
                Discard comments
              </button>
            )}
            <span class="spacer" />
            <button type="submit" class="button primary">
              Write the review
            </button>
          </div>
          {error && <p class="error">{error}</p>}
        </form>
      )}
      {drafts && (
        <div class="review-drafts">
          <p class="ok">{plural(drafts.length, "draft")} written, also under Drafts:</p>
          {drafts.map((d) => (
            <DraftCard key={d.msgid} store={store} d={d} onChanged={() => store.drafts().then((all) => setDrafts(drafts.map((x) => all.find((a) => a.msgid === x.msgid)).filter((x): x is Draft => !!x)))} />
          ))}
        </div>
      )}
    </div>
  );
}

const STATUS: Record<FileDiff["status"], string> = { added: "added", deleted: "deleted", renamed: "renamed", modified: "", binary: "binary" };

/** Files, each a box: every patch's part of it, labelled by patch where there are several. */
function FileList({
  files,
  total,
  titles,
  review,
}: {
  files: ReturnType<typeof filesAcross>;
  total: number;
  titles?: Record<number, string>;
  review?: Review;
}) {
  return (
    <div class="files">
      {files.map((f) => (
        <details class="box file" open id={`file-${f.path}`} data-path={f.path}>
          <summary class="box-header file-header">
            <span class="mono">{f.path}</span>{" "}
            {f.parts.map((p) => STATUS[p.file.status]).filter(Boolean).map((t) => <Label text={t} />)}
            <span class="spacer" />
            <span class="add">+{f.add}</span> <span class="del">−{f.del}</span>
          </summary>
          {f.parts.map((p) => (
            <>
              {total > 0 && (
                <div class="file-part muted small">
                  {p.patch}/{total} {titles?.[p.patch] ?? ""}
                </div>
              )}
              <FileLines file={p.file} msgid={p.msgid} review={review} />
            </>
          ))}
        </details>
      ))}
    </div>
  );
}

/** Every file the version's patches touch, each with its hunks from every patch: GitHub's "Files changed". */
function Files({ s, loaded, review }: { s: Series; loaded: Loaded; review: Review }) {
  if (loaded.error) return <p class="error">{loaded.error}</p>;
  if (!loaded.entries) return <p class="muted">Loading the files…</p>;
  const files = filesAcross(patchesOf(loaded.entries).map((e) => ({ patch: e.patch, msgid: e.msgid, diff: e.diff ?? "" })));
  if (!files.length) return <p class="blankslate">No diff in this posting.</p>;
  const add = files.reduce((n, f) => n + f.add, 0);
  const del = files.reduce((n, f) => n + f.del, 0);
  return (
    <div>
      <p class="files-summary">
        {plural(files.length, "file")} changed, <span class="add">+{add}</span> <span class="del">−{del}</span>, in{" "}
        {plural(patchesOf(loaded.entries).length, "patch", "patches")}
      </p>
      <ul class="file-index">
        {files.map((f) => (
          <li>
            <a href={`#file-${f.path}`} onClick={(ev) => (ev.preventDefault(), document.getElementById(`file-${f.path}`)?.scrollIntoView())}>
              <span class="mono">{f.path}</span>
            </a>{" "}
            <span class="add">+{f.add}</span> <span class="del">−{f.del}</span>
          </li>
        ))}
      </ul>
      <FileList files={files} total={s.total} titles={s.titles} review={review} />
    </div>
  );
}

/** Each file from one version to another: its hunks across all patches, compared. */
function FilesCompared({ store, base, s }: { store: Store; base: Series; s: Series }) {
  const [files, setFiles] = useState<FileChange[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setFiles(null);
    setError(null);
    store.files(base.root, s.root).then(setFiles, (e: Error) => setError(e.message));
  }, [base.root, s.root]);
  if (error) return <p class="error">{error}</p>;
  if (!files) return <p class="muted">Comparing…</p>;
  const changed = files.filter((f) => f.marker !== "=");
  const where = (v: Series, ps: number[]) => (ps.length ? `v${v.version} ${ps.map((n) => `${n}/${v.total}`).join(", ")}` : `not in v${v.version}`);
  return (
    <div class="changes files-compared">
      <p class="files-summary">
        {changed.length ? `${plural(changed.length, "file")} differ` : "No file differs"} from v{base.version} to v{s.version}
        {files.length > changed.length ? `; ${plural(files.length - changed.length, "file")} the same` : ""}.
      </p>
      <ul class="pairs">
        {files.map((f) => (
          <li class={`pair ${MARKS[f.marker]}`} data-path={f.path}>
            <div class="pair-head">
              <span class="mark" title={MARKS[f.marker]}>
                {f.marker}
              </span>{" "}
              <span class="mono">{f.path}</span>{" "}
              <span class="muted">
                {f.marker === "<" ? "no longer touched" : f.marker === ">" ? "newly touched" : MARKS[f.marker]} · {where(base, f.old_patches)} →{" "}
                {where(s, f.new_patches)}
              </span>
            </div>
            {f.diff.length > 0 && <Diff text={f.diff.join("\n")} />}
          </li>
        ))}
      </ul>
      <p class="muted small">
        Each file's hunks from every patch, compared; hunk line numbers, git's function context and index lines are ignored, so a
        rebase alone is not a change.
      </p>
    </div>
  );
}

/** Copy to the clipboard, saying so for a moment. */
function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      type="button"
      class="button small copy"
      onClick={() =>
        navigator.clipboard.writeText(text).then(() => {
          setDone(true);
          setTimeout(() => setDone(false), 1500);
        })
      }
    >
      {done ? "Copied" : label}
    </button>
  );
}

/** GitHub's "Code" button, for mail: the posting's message-id, its archive page, and b4's commands for it. */
function GetMenu({ s, archive, url }: { s: Series; archive: string; url: string }) {
  const id = s.root;
  const rows: Array<[string, string]> = [
    ["Message-ID", `<${id}>`],
    ["Archive", url],
    ["Apply with b4", `b4 shazam ${id}`],
    ["As an mbox", `b4 am ${id}`],
  ];
  return (
    <details class="get-menu">
      <summary class="button primary">Get v{s.version} ▾</summary>
      <div class="get-panel">
        {rows.map(([what, text]) => (
          <div class="get-row" data-what={what}>
            <div class="muted small">{what}</div>
            <div class="get-line">
              <input readOnly value={text} onFocus={(e) => (e.target as HTMLInputElement).select()} />
              <CopyButton text={text} />
            </div>
          </div>
        ))}
        <p class="small">
          <a href={url} target="_blank" rel="noreferrer">
            Open on the archive ↗
          </a>{" "}
          ·{" "}
          <a href={`${archive.replace(/\/+$/, "")}/all/${pathQuote(id)}/t.mbox.gz`} rel="noreferrer">
            Download the thread
          </a>
        </p>
      </div>
    </details>
  );
}

/** Close a family (with your reason) or reopen it: kept in the store, as `patchtrack close` keeps it. */
function CloseButton({ store, f, onChanged }: { store: Store; f: Family; onChanged: () => void }) {
  const [reason, setReason] = useState("");
  if (f.closed !== null)
    return (
      <button type="button" class="button" onClick={() => store.reopenFamily(f.sender, f.key).then(onChanged)}>
        Reopen
      </button>
    );
  return (
    <details class="close">
      <summary class="button">Close…</summary>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          store.closeFamily(f.sender, f.key, reason).then(onChanged);
        }}
      >
        <input placeholder="why (optional)" value={reason} onInput={(e) => setReason((e.target as HTMLInputElement).value)} />
        <button type="submit" class="button">
          Close
        </button>
      </form>
    </details>
  );
}

function Sidebar({ f, s, store, onChanged }: { f: Family; s: Series; store: Store; onChanged: () => void }) {
  const cite = (msgid: string) => {
    const v = versionOf(f, msgid);
    return v ? href({ view: "family", sender: f.sender, key: f.key, v: v.version, tab: "conversation", m: msgid }) : null;
  };
  const reviewers = new Map<string, { kind: string; who: string }>();
  for (const v of [...f.versions].reverse())
    for (const r of v.replies)
      for (const [kind, who] of r.trailers) {
        const k = `${kind}\0${who.toLowerCase()}`;
        if (!reviewers.has(k)) reviewers.set(k, { kind, who });
      }
  return (
    <aside class="sidebar">
      <section>
        <h3>State</h3>
        <Pill state={f.state} />
        {f.evidence && (
          <p class="evidence">
            {evidenceParts(f.evidence).map((p) => {
              if ("text" in p) return <span title={p.text}>{hideAddresses(p.text)}</span>;
              const to = cite(p.msgid);
              return to ? (
                <>
                  {" "}
                  <a href={to} title={p.msgid}>
                    mail
                  </a>
                </>
              ) : null;
            })}
          </p>
        )}
        {f.closed !== null && <p class="muted">Closed by you{f.closed ? `: ${f.closed}` : ""}</p>}
        <CloseButton store={store} f={f} onChanged={onChanged} />
      </section>
      <section>
        <h3>Reviewers</h3>
        {reviewers.size ? (
          <ul class="plain">
            {[...reviewers.values()].map((r) => (
              <li>
                <Avatar who={r.who} small /> <span title={r.who}>{displayName(r.who)}</span> <Label text={r.kind} kind="tag" />
              </li>
            ))}
          </ul>
        ) : (
          <p class="muted">No tags yet</p>
        )}
      </section>
      <BranchSection store={store} f={f} />
      <section>
        <h3>Versions</h3>
        <ul class="plain">
          {[...f.versions].reverse().map((v) => (
            <li>
              <a
                class={v === s ? "current" : undefined}
                href={href({ view: "family", sender: f.sender, key: f.key, v: v.version, tab: "conversation", m: null })}
              >
                v{v.version}
                {v.resend ? " RESEND" : ""}
              </a>{" "}
              <Pill state={v.state} /> <Time iso={v.date} />
            </li>
          ))}
        </ul>
      </section>
      {s.lists.length > 0 && (
        <section>
          <h3>Lists</h3>
          {s.lists.map((l) => (
            <Label text={l} kind="list" />
          ))}
        </section>
      )}
      {s.to.length > 0 && (
        <section>
          <h3>Sent to (v{s.version})</h3>
          <ul class="plain small">
            {s.to.map((a) => (
              <li>{a}</li>
            ))}
          </ul>
        </section>
      )}
      {s.written_by.length > 0 && (
        <section>
          <h3>Written by</h3>
          <ul class="plain small">
            {s.written_by.map((a) => (
              <li>{a}</li>
            ))}
          </ul>
        </section>
      )}
      <section>
        <h3>Data</h3>
        <p class="muted small">{asOfText(s)}</p>
      </section>
    </aside>
  );
}

export function FamilyView({
  store,
  route,
  onChanged,
  onRead,
  version,
}: {
  store: Store;
  route: FamilyRoute;
  onChanged: () => void;
  /** Its notification was marked read. */
  onRead: () => void;
  /** Changes when the store does: the family is fetched again. */
  version: unknown;
}) {
  // Fetched on its own: the list holds only summaries.
  const [f, setF] = useState<FamilyPage | null | undefined>(undefined);
  useEffect(() => {
    store.family(route.sender, route.key).then(setF, () => setF(null));
  }, [route.sender, route.key, version]);
  const found = f && f.sender === route.sender && f.key === route.key ? f : null;
  // Reading the conversation reads its notification, as on GitHub; whatever
  // arrives while you are here is read too.
  useEffect(() => {
    if (found && route.tab === "conversation") store.markRead(found.sender, found.key).then((r) => r.marked && onRead(), () => {});
  }, [found?.sender, found?.key, route.tab, version]);
  const head = found ? found.versions[found.versions.length - 1] : null;
  const s = found ? ([...found.versions].reverse().find((v) => v.version === route.v) ?? head) : null;
  // The version's conversation: its messages, and its patches for Commits and Files changed.
  const [loaded, setLoaded] = useState<Loaded>({ entries: null, error: null });
  // Blank for another version; a change to the store is shown in place, where you are.
  useEffect(() => setLoaded({ entries: null, error: null }), [s?.root]);
  useEffect(() => {
    if (!s) return;
    store.conversation(s.root).then(
      (entries) => setLoaded({ entries, error: null }),
      (e: Error) => setLoaded({ entries: null, error: e.message }),
    );
  }, [s?.root, version]);
  // A review in progress, on this version; leaving the page asks first.
  const [pending, setPending] = useState<Map<string, Pending>>(new Map());
  const [editing, setEditing] = useState<string | null>(null);
  useEffect(() => {
    setPending(new Map());
    setEditing(null);
  }, [s?.root]);
  useEffect(() => {
    if (!pending.size) return;
    const ask = (ev: BeforeUnloadEvent) => ev.preventDefault();
    addEventListener("beforeunload", ask);
    return () => removeEventListener("beforeunload", ask);
  }, [pending.size]);
  const review: Review = {
    pending,
    editing,
    edit: setEditing,
    set: (c) =>
      setPending((old) => {
        const next = new Map(old);
        if (c.text.trim()) next.set(keyOf(c.msgid, c.at), c);
        else next.delete(keyOf(c.msgid, c.at));
        return next;
      }),
  };
  const bar = s && <ReviewBar store={store} s={s} review={review} clear={() => setPending(new Map())} />;
  if (f === undefined) return <p class="muted loading">Loading…</p>;
  if (!found || !head || !s) {
    return (
      <div class="blankslate">
        <h2>No such series</h2>
        <p>
          <a href={href({ view: "list", tab: "open", q: "", who: "everyone" })}>Back to the series</a>
        </p>
      </div>
    );
  }
  const versions = byVersion(found);
  const base = route.vs !== null && route.vs !== undefined && route.vs !== s.version ? versions.find((v) => v.version === route.vs) : undefined;
  const n = Math.max(1, Object.keys(head.patches).filter((i) => i !== "0").length);
  const tab = (name: FamilyRoute["tab"], text: string, count: number | null) => (
    <a class={`tab${route.tab === name ? " selected" : ""}`} href={href({ ...route, tab: name, m: null, c: null })} data-tab={name}>
      {text}
      {count !== null && <span class="counter">{count}</span>}
    </a>
  );
  const patches = Object.keys(s.patches).filter((i) => i !== "0").length;
  const files = loaded.entries
    ? filesAcross(patchesOf(loaded.entries).map((e) => ({ patch: e.patch, msgid: e.msgid, diff: e.diff ?? "" }))).length
    : null;
  const go = (r: Partial<FamilyRoute>) => (location.hash = href({ ...route, ...r }));
  const archive = found.archives[s.root] ?? "";
  // Where this posting is among the versions; -1 for a posting a RESEND replaced.
  const at = versions.indexOf(s);
  const comparing = route.tab !== "conversation";
  return (
    <div>
      <div class="pr-header">
        <div class="pr-title-row">
          <h1>
            {head.title} <span class="muted">v{s.version}</span>
          </h1>
          <GetMenu s={s} archive={archive} url={found.urls[s.root] ?? messageUrl(archive, s.root)} />
        </div>
        <div class="pr-meta">
          <Pill state={found.state} big /> <strong>{found.sender}</strong> posted {plural(n, "patch", "patches")} in{" "}
          {plural(found.versions.length, "version")} · last activity <Time iso={lastActivity(head)} />
          {head.unanswered && <Label text="unanswered" kind="flag" />}
        </div>
      </div>
      <nav class="tabs underline">
        {tab("conversation", "Conversation", s.replies.length + Object.keys(s.patches).length)}
        {tab("commits", "Commits", patches)}
        {tab("files", "Files changed", files)}
        <span class="spacer" />
        <span class="version-bar">
          {comparing && versions.length > 1 && (
            <label class="muted small">
              {base ? "Changes from" : "Compare with"}{" "}
              <select
                aria-label="Compare with"
                value={base ? String(base.version) : ""}
                onChange={(e) => {
                  const v = (e.target as HTMLSelectElement).value;
                  go({ vs: v === "" ? null : Number(v), c: null });
                }}
              >
                <option value="">—</option>
                {versions
                  .filter((v) => v.version !== s.version)
                  .map((v) => (
                    <option value={String(v.version)}>v{v.version}</option>
                  ))}
              </select>
              {base && " to"}
            </label>
          )}{" "}
          <select
            aria-label="Version"
            class={s !== head ? "not-latest" : undefined}
            value={String(s.version)}
            onChange={(e) => go({ v: Number((e.target as HTMLSelectElement).value), vs: null, c: null, m: null })}
          >
            {[...versions].reverse().map((v) => (
              <option value={String(v.version)}>
                v{v.version}
                {v.resend ? " RESEND" : ""}
                {v === head ? " (latest)" : ""}
              </option>
            ))}
          </select>
        </span>
      </nav>
      {s !== head && (
        <p class="viewing">
          Viewing v{s.version}, not the latest.{" "}
          <a href={href({ ...route, v: null, vs: null, c: null, m: null })}>See v{head.version}</a>
        </p>
      )}
      <div class="layout">
        <div class="content">
          {route.tab === "commits" ? (
            base ? (
              <CommitsCompared store={store} base={base} s={s} />
            ) : (
              <>
                {at > 0 && (
                  <p class="note small">
                    <a href={href({ ...route, vs: versions[at - 1].version, c: null })}>Compare with v{versions[at - 1].version}</a>
                    : what changed in each patch, and the author's changelog.
                  </p>
                )}
                {bar}
                <Commits f={found} s={s} loaded={loaded} route={route} review={review} />
              </>
            )
          ) : route.tab === "files" ? (
            base ? (
              <FilesCompared store={store} base={base} s={s} />
            ) : (
              <>
                {bar}
                <Files s={s} loaded={loaded} review={review} />
              </>
            )
          ) : (
            <Conversation
              store={store}
              s={s}
              target={route.m}
              loaded={loaded}
              archive={archive}
              before={at >= 0 ? versions[at - 1] : undefined}
              after={at >= 0 ? versions[at + 1] : undefined}
              route={route}
            />
          )}
        </div>
        <Sidebar f={found} s={s} store={store} onChanged={onChanged} />
      </div>
    </div>
  );
}
