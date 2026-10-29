// Mail you wrote: a reply box, and the drafts it keeps in the bridge's
// store. The bridge sends a draft with git send-email when it was started
// with --allow-send; otherwise a draft leaves as a file, the clipboard, or
// a mail client opened through mailto:.

import { useEffect, useState } from "preact/hooks";

import { emlName, mailtoHref } from "../lib/drafts";
import { plural, when } from "../lib/format";
import type { Draft } from "../protocol";
import type { Store } from "../store";

const TAGS = ["Reviewed-by", "Acked-by", "Tested-by", "Reported-by"];

function download(d: Draft) {
  const url = URL.createObjectURL(new Blob([d.raw], { type: "message/rfc822" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = emlName(d);
  a.click();
  URL.revokeObjectURL(url);
}

/** Send through the bridge (or your server): asked once more, with the recipients named. */
function SendHere({ store, d, onSent }: { store: Store; d: Draft; onSent: () => void }) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const all = [...(d.to ?? []), ...(d.cc ?? [])];
  const host = new URL(store.bridge.url).host;
  if (!asking)
    return (
      <button type="button" class="button primary" onClick={() => setAsking(true)}>
        Send…
      </button>
    );
  return (
    <div class="send-confirm">
      <p>
        Send this to {plural(all.length, "address", "addresses")} ({all.join(", ")}), through git send-email on {host}?
      </p>
      <button
        type="button"
        class="button primary"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          setError(null);
          try {
            await store.send(d.msgid);
            onSent();
          } catch (e) {
            setError(e instanceof Error ? e.message : String(e));
          }
          setBusy(false);
        }}
      >
        {busy ? "Sending…" : "Send it"}
      </button>{" "}
      <button type="button" class="button" disabled={busy} onClick={() => setAsking(false)}>
        Not now
      </button>
      {error && <p class="error">{error}</p>}
    </div>
  );
}

/** One draft, and the ways it can leave. */
export function DraftCard({ store, d, onChanged }: { store: Store; d: Draft; onChanged: () => void }) {
  const [note, setNote] = useState<string | null>(null);
  const canSend = store.hello.can.send;
  const mailto = mailtoHref(d);
  const act = (p: Promise<unknown>) => p.then(onChanged, (e: Error) => setNote(e.message));
  return (
    <div class="draft" data-msgid={d.msgid}>
      <dl class="draft-head">
        <dt>To</dt>
        <dd>{d.to?.join(", ")}</dd>
        {d.cc && d.cc.length > 0 && (
          <>
            <dt>Cc</dt>
            <dd>{d.cc.join(", ")}</dd>
          </>
        )}
        <dt>Subject</dt>
        <dd>{d.subject}</dd>
      </dl>
      <pre class="prose">{d.body}</pre>
      <div class="draft-actions">
        {d.sent_at ? (
          <span class="muted">Sent {when(d.sent_at)}.</span>
        ) : (
          <span class="muted">Not sent: nothing leaves until you send it.</span>
        )}
        <span class="spacer" />
        {!d.sent_at && canSend && <SendHere store={store} d={d} onSent={onChanged} />}
        <button type="button" class="button" onClick={() => download(d)} title="A message file your mail client opens, threading intact">
          Download .eml
        </button>
        <button
          type="button"
          class="button"
          onClick={() => navigator.clipboard.writeText(d.raw).then(() => setNote("Copied, headers and all."), (e: Error) => setNote(e.message))}
        >
          Copy
        </button>
        {mailto && (
          <a class="button" href={mailto} title="Opens your mail client; some drop the threading header">
            Open in mail app
          </a>
        )}
        {!d.sent_at && (
          <button type="button" class="button" onClick={() => act(store.markSent(d.msgid))}>
            Mark as sent
          </button>
        )}
        <button type="button" class="button danger" onClick={() => act(store.removeDraft(d.msgid))}>
          Delete
        </button>
      </div>
      {note && <p class="muted">{note}</p>}
      {!d.sent_at && !canSend && (
        <p class="note">
          To send from here, start the bridge with <code>--allow-send</code> (or your server, <code>patchtrack serve</code>). Or run{" "}
          <code>patchtrack send {d.msgid}</code> on its machine.
        </p>
      )}
    </div>
  );
}

/** A reply to one message: your words or a tag, kept as a draft. */
export function Composer({ store, to, onDone }: { store: Store; to: string; onDone: () => void }) {
  const [text, setText] = useState("");
  const [tag, setTag] = useState("");
  const [draft, setDraft] = useState<Draft | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (draft) return <DraftCard store={store} d={draft} onChanged={onDone} />;
  return (
    <form
      class="composer"
      onSubmit={(e) => {
        e.preventDefault();
        setError(null);
        store.reply(to, text, tag).then(setDraft, (err: Error) => setError(err.message));
      }}
    >
      <textarea
        rows={6}
        placeholder="Your reply, below a short quote. Leave it empty, with no tag, to quote the whole message and answer inline in your mail client."
        value={text}
        onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
      />
      <div class="composer-actions">
        <select aria-label="Tag" value={tag} onChange={(e) => setTag((e.target as HTMLSelectElement).value)}>
          <option value="">No tag</option>
          {TAGS.map((t) => (
            <option value={t}>{t}</option>
          ))}
        </select>
        <span class="spacer" />
        <button type="button" class="button" onClick={onDone}>
          Cancel
        </button>
        <button type="submit" class="button primary">
          Write draft
        </button>
      </div>
      {error && <p class="error">{error}</p>}
    </form>
  );
}

/** Every draft, newest first. */
export function DraftsPage({ store, version }: { store: Store; version: unknown }) {
  const [drafts, setDrafts] = useState<Draft[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = () => store.drafts().then((d) => setDrafts(d.reverse()), (e: Error) => setError(e.message));
  useEffect(() => {
    load();
  }, [version]);
  if (error) return <p class="error">{error}</p>;
  if (!drafts) return <p class="muted">Loading…</p>;
  return (
    <div class="drafts">
      <h1>Drafts</h1>
      <p class="note">
        Replies and tags you wrote from a series' conversation, kept in the bridge's store.{" "}
        {store.hello.can.send
          ? "The bridge sends them with git send-email, once you say so."
          : "Download a draft and open it in your mail client, copy it, or open it through a mail link."}
      </p>
      {drafts.length === 0 ? (
        <p class="blankslate">No drafts. Reply to a message in a series' conversation to write one.</p>
      ) : (
        drafts.map((d) => <DraftCard key={d.msgid} store={store} d={d} onChanged={load} />)
      )}
    </div>
  );
}
