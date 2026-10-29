import { useEffect, useState } from "preact/hooks";

import { when } from "../lib/format";
import type { Kind, Project, Status, Subscription } from "../protocol";
import type { Store } from "../store";

/** What each kind of subscription asks for, as `patchtrack follow` does. */
const KINDS: Record<Kind, { text: string; placeholder: string; inList: boolean }> = {
  author: { text: "An author", placeholder: "author@example.org", inList: true },
  list: { text: "Every series on a list", placeholder: "inbox, e.g. linux-usb", inList: false },
  query: { text: "A search", placeholder: "public-inbox query, e.g. dfn:drivers/usb/", inList: true },
  thread: { text: "One thread", placeholder: "message-id or its archive URL", inList: false },
};

/** What the bridge's syncs are doing, in a sentence. */
export function syncLine(s: Status["sync"] | undefined): string {
  if (!s) return "";
  if (s.running) return "Syncing…";
  if (s.asked) return "Sync asked for…";
  if (!s.finished) return "Not synced yet";
  return `${s.ok ? "Synced" : "Sync failed"} ${when(s.finished)}`;
}

type Props = {
  store: Store;
  status: Status | null;
  /** Ask for the status again, after asking for a sync. */
  onAsked: () => void;
  /** Changes when the store does: the page then asks again. */
  version: unknown;
};

export function Following({ store, status, onAsked, version }: Props) {
  const [subs, setSubs] = useState<Subscription[]>([]);
  const [kind, setKind] = useState<Kind>("author");
  const [projects, setProjects] = useState<Project[]>([]);
  const [project, setProject] = useState("");
  const [value, setValue] = useState("");
  const [list, setList] = useState("");
  const [error, setError] = useState<string | null>(null);

  const loadSubs = () => store.subscriptions().then(setSubs, (e: Error) => setError(e.message));
  useEffect(() => {
    loadSubs();
    store.projects().then((p) => setProjects(p.projects), () => setProjects([]));
  }, [version]);

  async function follow(e: Event) {
    e.preventDefault();
    setError(null);
    try {
      await store.follow(kind, value, KINDS[kind].inList ? list : "", project);
      setValue("");
      setList("");
      await loadSubs();
      onAsked();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  const sync = status?.sync;
  return (
    <div class="following">
      <h1>Following</h1>
      <p class="note">
        The bridge syncs what you follow on its own: when it starts, every few minutes (the <code>sync_every</code> setting), and as
        soon as you follow something. It plans every request as the command line does, one at a time.
      </p>
      {subs.length === 0 ? (
        <p class="muted">Nothing yet: follow an author's series, a list's, a search's or one thread.</p>
      ) : (
        <ul class="following-list">
          {subs.map((s) => (
            <li>
              {s.label}{" "}
              <button type="button" class="button" onClick={() => store.unfollow(s).then(loadSubs)}>
                Unfollow
              </button>
            </li>
          ))}
        </ul>
      )}
      <form class="follow" onSubmit={follow}>
        <select aria-label="What to follow" value={kind} onChange={(e) => setKind((e.target as HTMLSelectElement).value as Kind)}>
          {(Object.keys(KINDS) as Kind[]).map((k) => (
            <option value={k}>{KINDS[k].text}</option>
          ))}
        </select>
        <input
          required
          class="value"
          placeholder={KINDS[kind].placeholder}
          value={value}
          onInput={(e) => setValue((e.target as HTMLInputElement).value)}
        />
        {KINDS[kind].inList && (
          <input
            placeholder={project ? "list (default: the project's)" : "list (default: all)"}
            value={list}
            onInput={(e) => setList((e.target as HTMLInputElement).value)}
          />
        )}
        {projects.length > 0 && (
          <select aria-label="In project" value={project} onChange={(e) => setProject((e.target as HTMLSelectElement).value)}>
            <option value="">No project</option>
            {projects.map((p) => (
              <option value={p.name}>in {p.name}</option>
            ))}
          </select>
        )}
        <button type="submit" class="button">
          Follow
        </button>
      </form>
      {(kind === "list" || kind === "query") && (
        <p class="note">A list or a search finds far more than one author: its first sync goes back only as far as the list_since setting, and each thread it finds is a request.</p>
      )}
      {error && <p class="error">{error}</p>}
      <h2>Syncing</h2>
      <p>
        <span class="sync-state">{syncLine(sync)}</span>{" "}
        <button
          type="button"
          class="button"
          disabled={sync?.running || sync?.asked}
          onClick={() => store.sync().then(onAsked, (e: Error) => setError(e.message))}
        >
          Sync now
        </button>
      </p>
      {sync && sync.log.length > 0 && <pre class="log">{sync.log.join("\n")}</pre>}
    </div>
  );
}
