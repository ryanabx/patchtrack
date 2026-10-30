// A thread listed by an inbox, opened: the bridge fetches it by this message
// (one request), and this page gives way to its series once the store has
// it. A thread the store has already opens at once.

import { useEffect, useState } from "preact/hooks";

import type { Opened } from "../protocol";
import { href } from "../route";
import type { Store } from "../store";

type Props = {
  store: Store;
  msgid: string;
  inbox?: string;
  project?: string;
  /** Changes when the store does: the thread may have arrived. */
  version: unknown;
  onAsked: () => void;
};

export function OpenThread({ store, msgid, inbox = "", project = "", version, onAsked }: Props) {
  const [opened, setOpened] = useState<Opened | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    store.openMessage(msgid, project, inbox).then(
      (o) => {
        if ("family" in o) {
          const to = href({ view: "family", sender: o.family.sender, key: o.family.key, v: o.family.version, tab: "conversation", m: msgid });
          // The series in its place, not after it in the history.
          location.replace(to);
          return;
        }
        if ("fetching" in o && !opened) onAsked();
        setOpened(o);
      },
      (e: Error) => setError(e.message),
    );
  }, [msgid, version]);
  const back = inbox ? href({ view: "inboxes", inbox, project: project || undefined }) : href({ view: "inboxes" });
  return (
    <div class="blankslate open-thread">
      {error ? (
        <p class="error">{error}</p>
      ) : opened && "missing" in opened ? (
        <>
          <h2>Not on the archive</h2>
          <p class="muted">The archive has no thread with &lt;{msgid}&gt; any more.</p>
        </>
      ) : (
        <>
          <h2>Fetching the thread…</h2>
          <p class="muted">
            The bridge fetches it from the archive, by &lt;{msgid}&gt;, in one request; its series opens here when it arrives.
          </p>
        </>
      )}
      <p>
        <a href={back}>Back to {inbox || "the inboxes"}</a>
      </p>
    </div>
  );
}
