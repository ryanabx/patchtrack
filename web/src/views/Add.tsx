import { useEffect, useState } from "preact/hooks";

import { plural, when } from "../lib/format";
import type { Source } from "../protocol";
import type { Store } from "../store";

type Result = { name: string; text: string; ok: boolean };

/** The local mail the bridge reads on every sync: set on its machine, with `patchtrack source add`. */
function Sources({ store }: { store: Store }) {
  const [sources, setSources] = useState<Source[] | null>(null);
  useEffect(() => {
    store.sources().then(setSources, () => setSources([]));
  }, []);
  return (
    <>
      <h2>Your mailboxes</h2>
      <p class="note">
        A Maildir (from lei, mbsync, or Thunderbird's Maildir mode), an mbox or a directory of .eml files on the bridge's machine is
        read on every sync, keeping only the series of the authors you follow and yours, with their replies. Add one there with{" "}
        <code>patchtrack source add PATH</code>.
      </p>
      {sources && sources.length > 0 && (
        <ul class="rows box">
          {sources.map((s) => (
            <li class="row" data-source={s.path}>
              <div class="row-main">
                <div class="row-title mono">{s.path}</div>
                <div class="row-meta">{s.read_at ? `read ${when(s.read_at)}` : "not read yet"}</div>
              </div>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

export function Add({ store, onAdded }: { store: Store; onAdded: () => void }) {
  const [results, setResults] = useState<Result[]>([]);
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  // Like `patchtrack import --only-followed`: off by default, as there.
  const [onlyFollowed, setOnlyFollowed] = useState(false);
  const [follows, setFollows] = useState(false);
  useEffect(() => {
    store.scope().then((s) => setFollows(s.authors.length > 0), () => setFollows(false));
  }, []);

  async function add(files: FileList | File[]) {
    setBusy(true);
    const out: Result[] = [];
    for (const file of Array.from(files)) {
      try {
        const a = await store.add(new Uint8Array(await file.arrayBuffer()), onlyFollowed && follows);
        const text = a.store
          ? `${plural(a.new, "new message")} (merged a store)`
          : `${plural(a.read, "message")}${a.kept < a.read ? `, ${a.kept} kept` : ""}, ${a.new} new`;
        out.push({ name: file.name, ok: true, text });
      } catch (e) {
        out.push({ name: file.name, ok: false, text: e instanceof Error ? e.message : String(e) });
      }
      setResults([...out]);
    }
    setBusy(false);
    onAdded();
  }

  return (
    <div class="add-mail">
      <h1>Add mail</h1>
      <p class="note">What you add goes into the bridge's store, as <code>patchtrack import</code> on its machine would put it.</p>
      <label
        class={`drop${over ? " over" : ""}${busy ? " busy" : ""}`}
        onDragOver={(e) => {
          e.preventDefault();
          setOver(true);
        }}
        onDragLeave={() => setOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setOver(false);
          if (e.dataTransfer?.files.length) add(e.dataTransfer.files);
        }}
      >
        <input
          type="file"
          multiple
          onChange={(e) => {
            const files = (e.target as HTMLInputElement).files;
            if (files?.length) add(files);
          }}
        />
        <strong>{busy ? "Reading…" : "Drop files here, or choose them"}</strong>
        <span class="muted">mbox files, .eml messages, or a patchtrack store (.sqlite3)</span>
      </label>
      <label class={`check${follows ? "" : " muted"}`}>
        <input
          type="checkbox"
          checked={onlyFollowed && follows}
          disabled={!follows}
          onChange={(e) => setOnlyFollowed((e.target as HTMLInputElement).checked)}
        />{" "}
        Keep only the series of the authors you follow and your own, with their replies
        {!follows && " (follow an author, or set your identities in Settings, first)"}
      </label>
      {results.length > 0 && (
        <ul class="results">
          {results.map((r) => (
            <li class={r.ok ? "ok" : "error"}>
              <strong>{r.name}</strong>: {r.text}
            </li>
          ))}
        </ul>
      )}
      <Sources store={store} />
      <h2>Where else mail comes from</h2>
      <ul class="sources">
        <li>
          <strong>lei</strong>: <code>lei q -o mboxrd:/tmp/mine.mbox 'f:you@example.org'</code>, then drop the file.
        </li>
        <li>
          <strong>b4</strong>: <code>b4 mbox MESSAGE-ID</code> writes a thread as an mbox.
        </li>
        <li>
          <strong>Thunderbird</strong>: folders are mbox files in its profile directory; or save messages as .eml.
        </li>
        <li>
          <strong>Another patchtrack</strong>: a store it exported merges in.
        </li>
      </ul>
    </div>
  );
}
