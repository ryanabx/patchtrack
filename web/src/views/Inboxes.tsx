// An inbox's latest series, as lore's own pages list them, as GitHub lists
// pull requests. A page is one search by the bridge, and no thread is
// fetched: a series is fetched when you open it.

import { useEffect, useRef, useState } from "preact/hooks";

import type { Summary } from "../core/Summary";
import { plural } from "../lib/format";
import type { Inbox, Status } from "../protocol";
import { href } from "../route";
import type { Store } from "../store";
import { Time } from "./common";
import { Row } from "./List";

type Props = {
  store: Store;
  families: Summary[];
  inbox?: string;
  project?: string;
  status: Status | null;
  /** Changes when the store does: the page then asks again. */
  version: unknown;
  /** A page was asked for: ask for the status again. */
  onAsked: () => void;
};

const inboxHref = (inbox: string, project: string) => href({ view: "inboxes", inbox, project: project || undefined });

export function Inboxes({ store, families, inbox, project = "", status, version, onAsked }: Props) {
  const [inboxes, setInboxes] = useState<Inbox[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [other, setOther] = useState("");
  useEffect(() => {
    store.inboxes().then(setInboxes, (e: Error) => setError(e.message));
  }, [version]);
  const here = inboxes?.find((i) => i.inbox === inbox && i.project === project);
  const listing = !!inbox && (status?.sync.browsing ?? []).includes(inbox);
  const ask = (before?: string) => {
    if (!inbox) return;
    setError(null);
    store.browse(project, inbox, before).then(onAsked, (e: Error) => setError(e.message));
  };
  // An inbox never listed is listed when you first open it.
  const asked = useRef<string | null>(null);
  useEffect(() => {
    const key = `${project}\0${inbox}`;
    if (!inbox || !inboxes || here?.listed || asked.current === key) return;
    asked.current = key;
    ask();
  }, [inbox, project, inboxes]);

  const rows = inbox
    ? families
        .filter((f) => f.lists.includes(inbox) && (!project || f.projects.includes(project)))
        .sort((a, b) => b.posted.localeCompare(a.posted))
    : [];
  const byProject = new Map<string, Inbox[]>();
  for (const i of inboxes ?? []) byProject.set(i.project, [...(byProject.get(i.project) ?? []), i]);

  return (
    <div class="inboxes">
      <aside class="inbox-list">
        <form
          class="inbox-open"
          onSubmit={(e) => {
            e.preventDefault();
            if (other.trim()) location.hash = inboxHref(other.trim(), project);
          }}
        >
          <input placeholder="Any inbox, e.g. linux-usb" aria-label="Open an inbox" value={other} onInput={(e) => setOther((e.target as HTMLInputElement).value)} />
        </form>
        {!inboxes ? (
          <p class="muted">Loading…</p>
        ) : (
          [...byProject.entries()].map(([p, list]) => (
            <section>
              <h3>{p || "Other lists"}</h3>
              <ul class="plain">
                {list.map((i) => (
                  <li>
                    <a class={i.inbox === inbox && i.project === project ? "current" : undefined} href={inboxHref(i.inbox, i.project)}>
                      {i.inbox}
                    </a>{" "}
                    {i.series > 0 && <span class="count muted">{i.series}</span>}
                  </li>
                ))}
              </ul>
            </section>
          ))
        )}
      </aside>
      <div class="inbox-main">
        {!inbox ? (
          <div class="blankslate">
            <h2>Browse an inbox</h2>
            <p class="muted">
              Its latest series, as the archive lists them: one search per page, and no thread fetched until you open a series. Your
              projects' lists are on the left; any other inbox by its name.
            </p>
          </div>
        ) : (
          <>
            <div class="inbox-head">
              <h1>
                {inbox} {project && <span class="muted">in {project}</span>}
              </h1>
              <span class="spacer" />
              <span class="muted small">
                {listing ? (
                  "Listing…"
                ) : here?.listed ? (
                  <>
                    listed <Time iso={here.listed} />
                  </>
                ) : (
                  "not listed yet"
                )}
              </span>{" "}
              <button type="button" class="button" disabled={listing} onClick={() => ask()}>
                Fetch latest
              </button>
            </div>
            {error && <p class="error">{error}</p>}
            <div class="box">
              <div class="box-header">
                {plural(rows.length, "series", "series")} here{rows.some((r) => !r.fetched) && ", the ones not fetched yet as the search found them"}
              </div>
              <ul class="rows">
                {rows.map((f) => (
                  <Row key={`${f.sender}\0${f.key}`} f={f} />
                ))}
                {!rows.length && <li class="row empty">{listing ? "Listing…" : "No series on this inbox here yet."}</li>}
              </ul>
            </div>
            <p class="inbox-more">
              <button
                type="button"
                class="button"
                disabled={listing || !inboxes}
                onClick={() => ask(here?.since ?? new Date().toISOString().slice(0, 19) + "Z")}
              >
                Load older
              </button>{" "}
              <span class="muted small">
                {here?.since ? (
                  <>
                    Listed back to <Time iso={here.since} />; two days more, in one search.
                  </>
                ) : (
                  "Two days more, in one search."
                )}
              </span>
            </p>
          </>
        )}
      </div>
    </div>
  );
}
