// Your b4 prep branches, on the machine the bridge (or your server) runs
// on: each beside the series it posted, with what b4 can do next. b4 does
// it; every action is first a dry run, whose output you read before the
// button that does it for real appears.

import { useEffect, useState } from "preact/hooks";

import type { Summary } from "../core/Summary";
import { plural } from "../lib/format";
import { href } from "../route";
import type { B4Run, Branch, Store } from "../store";
import { Pill } from "./common";

type Action = "send" | "resend" | "trailers";

/** What each action is called, before and after its dry run. */
function names(b: Branch, a: Action): { preview: string; doIt: string } {
  const last = b.sent.length ? b.sent[b.sent.length - 1].version : 0;
  switch (a) {
    case "send":
      return { preview: `Preview v${b.revision}`, doIt: `Send v${b.revision} with b4` };
    case "resend":
      return { preview: `Preview a RESEND of v${last}`, doIt: `Resend v${last} with b4` };
    case "trailers":
      return { preview: "Look for new tags", doIt: "Take them in (b4 trailers -u)" };
  }
}

function BranchRow({ store, b, onDone }: { store: Store; b: Branch; onDone: () => void }) {
  const [family, setFamily] = useState<Summary | null>(null);
  const [run, setRun] = useState<{ action: Action; dry: boolean; result: B4Run } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const last = b.sent.length ? b.sent[b.sent.length - 1] : null;
  useEffect(() => {
    if (last) store.familyOf(last.cover).then(setFamily, () => setFamily(null));
  }, [b.name]);
  const go = async (action: Action, dry: boolean) => {
    setBusy(true);
    setError(null);
    try {
      setRun({ action, dry, result: await store.runB4(b, action, dry) });
      if (!dry) onDone();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    setBusy(false);
  };
  const actions: Action[] = last ? (b.revision > last.version ? ["send", "resend", "trailers"] : ["resend", "trailers"]) : ["send"];
  return (
    <li class="row branch" data-branch={b.name}>
      <div class="row-main">
        <div class="row-title">
          <code>{b.name}</code> {b.subject}
        </div>
        <div class="row-meta">
          {last ? `v${last.version} sent` : "never sent"} · v{b.revision} next · {plural(b.patches, "patch", "patches")} ·{" "}
          <span class="mono">{b.repo}</span>
        </div>
        {family && (
          <div class="row-evidence">
            <Pill state={family.state} />{" "}
            <a href={href({ view: "family", sender: family.sender, key: family.key, v: null, tab: "conversation", m: null })}>
              {family.title}
            </a>
          </div>
        )}
        <div class="branch-actions">
          {actions.map((a) => (
            <button type="button" class="button small" disabled={busy} onClick={() => go(a, true)}>
              {names(b, a).preview}
            </button>
          ))}
        </div>
        {busy && <p class="muted">b4 is running…</p>}
        {error && <p class="error">{error}</p>}
        {run && (
          <div class="b4-run">
            <pre class={`log ${run.result.ok ? "" : "failed"}`}>{run.result.output || "(b4 printed nothing)"}</pre>
            {run.dry && run.result.ok && (
              <p>
                <button type="button" class="button primary" disabled={busy} onClick={() => go(run.action, false)}>
                  {names(b, run.action).doIt}
                </button>{" "}
                <span class="muted">This runs b4 for real, on {b.repo}.</span>
              </p>
            )}
            {!run.dry && <p class={run.result.ok ? "ok" : "error"}>{run.result.ok ? "Done." : "b4 did not finish: see its output."}</p>}
          </div>
        )}
      </div>
    </li>
  );
}

export function Branches({ store }: { store: Store }) {
  const [list, setList] = useState<Branch[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = () => {
    if (store.hello.can.b4) store.branches().then(setList, (e: Error) => setError(e.message));
  };
  useEffect(load, []);
  const intro = (
    <p class="note">
      Your b4 prep branches, on the computer <code>patchtrack bridge</code> (or your <code>patchtrack serve</code>) runs on, each with
      the series it posted. b4 sends, resends and takes in tags; you see each as a dry run first.
    </p>
  );
  if (!store.hello.can.b4) {
    return (
      <div class="branches">
        <h1>Branches</h1>
        {intro}
        <p>
          The bridge runs no b4. Start it with your repositories: <code>patchtrack bridge --repo ~/linux</code>.
        </p>
      </div>
    );
  }
  return (
    <div class="branches">
      <h1>Branches</h1>
      {intro}
      {error && <p class="error">{error}</p>}
      {!list ? (
        <p class="muted">Loading…</p>
      ) : list.length === 0 ? (
        <p class="blankslate">No b4 prep branches in the repositories the bridge was given (b4 prep -n NAME makes one).</p>
      ) : (
        <ul class="rows box">
          {list.map((b) => (
            <BranchRow key={`${b.repo}\0${b.name}`} store={store} b={b} onDone={load} />
          ))}
        </ul>
      )}
    </div>
  );
}
