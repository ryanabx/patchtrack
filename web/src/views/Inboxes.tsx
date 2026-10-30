// An inbox's newest threads, as lore's own pages list them, as GitHub lists
// pull requests. A page is one request by the bridge (the archive's Atom
// feed of a search: 50 threads, each by its newest message), and it stores
// no mail: a thread is fetched when you open it.

import { useEffect, useRef, useState } from "preact/hooks";

import { displayName, familyId, plural } from "../lib/format";
import type { Inbox, ListedRow, Listing, Status } from "../protocol";
import { href } from "../route";
import type { Store } from "../store";
import { Label, Time } from "./common";
import { Row } from "./List";

type Props = {
  store: Store;
  /** Your families, by `familyId`. */
  mine: string[];
  inbox?: string;
  project?: string;
  status: Status | null;
  /** Changes when the store does: the page then asks again. */
  version: unknown;
  /** A page was asked for: ask for the status again. */
  onAsked: () => void;
};

/** Inboxes shown per project before "All". */
const SHOWN = 15;

const inboxHref = (inbox: string, project: string) => href({ view: "inboxes", inbox, project: project || undefined });

/** A thread listed and not in the store yet: its title, and who wrote last, when. Opening it fetches it. */
function ListedThread({ row, inbox, project }: { row: ListedRow; inbox: string; project: string }) {
  const l = row.listed;
  const reply = l.subject !== row.title;
  return (
    <li class="row listed" data-msgid={l.msgid}>
      <span class="dot tone-muted" aria-hidden="true">
        ○
      </span>
      <div class="row-main">
        <div class="row-title">
          <a class="title-link" href={href({ view: "message", msgid: l.msgid, inbox, project: project || undefined })}>
            {row.title}
          </a>{" "}
          <span class="label unfetched" title="Opening it fetches its thread">
            not fetched
          </span>
        </div>
        <div class="row-meta">
          {reply ? "latest reply by" : "posted by"} {displayName(l.author)} · <Time iso={l.date} />
        </div>
      </div>
    </li>
  );
}

export function Inboxes({ store, mine, inbox, project = "", status, version, onAsked }: Props) {
  const [inboxes, setInboxes] = useState<Inbox[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [other, setOther] = useState("");
  const [all, setAll] = useState<Set<string>>(new Set());
  useEffect(() => {
    store.inboxes().then(setInboxes, (e: Error) => setError(e.message));
  }, [version]);
  // Each archive's list of inboxes, read again when it is an hour old.
  useEffect(() => {
    store.refreshInboxes().then(onAsked, (e: Error) => setError(e.message));
  }, []);
  const reading = status?.sync.inboxes ?? false;
  const here = inboxes?.find((i) => i.inbox === inbox && i.project === project);
  const listing = !!inbox && (status?.sync.browsing ?? []).includes(inbox);
  // Listing now, or waiting for the job before it (a sync, say).
  const doing = status?.sync.doing ?? "";
  const waiting = listing && !doing.includes(inbox ?? "");
  const ask = (older = false) => {
    if (!inbox) return;
    setError(null);
    store.browse(project, inbox, older).then(onAsked, (e: Error) => setError(e.message));
  };
  const [page, setPage] = useState<Listing | null>(null);
  useEffect(() => setPage(null), [inbox, project]);
  useEffect(() => {
    if (inbox) store.listing(project, inbox).then(setPage, (e: Error) => setError(e.message));
  }, [inbox, project, version]);
  // An inbox never listed is listed when you first open it.
  const asked = useRef<string | null>(null);
  useEffect(() => {
    const key = `${project}\0${inbox}`;
    if (!inbox || !inboxes || here?.listed || asked.current === key) return;
    asked.current = key;
    ask();
  }, [inbox, project, inboxes]);

  const rows = page?.rows ?? [];
  // Newest first, as the bridge sorts them; a project's section where its newest is.
  const filter = other.trim().toLowerCase();
  const byProject = new Map<string, Inbox[]>();
  for (const i of inboxes ?? [])
    if (!filter || i.inbox.includes(filter) || i.description.toLowerCase().includes(filter))
      byProject.set(i.project, [...(byProject.get(i.project) ?? []), i]);

  return (
    <div class="inboxes">
      <aside class="inbox-list">
        <form
          class="inbox-open"
          onSubmit={(e) => {
            e.preventDefault();
            const name = other.trim();
            // An inbox a project has, in that project; any other on the archive setting's archive.
            const known = inboxes?.find((i) => i.inbox === name);
            if (name) location.hash = inboxHref(name, known ? known.project : project);
          }}
        >
          <input
            placeholder="Filter, or any inbox's name"
            aria-label="Filter inboxes, or open one by name"
            value={other}
            onInput={(e) => setOther((e.target as HTMLInputElement).value)}
          />
        </form>
        {reading && <p class="muted small">Reading the list of inboxes…</p>}
        {!inboxes ? (
          <p class="muted">Loading…</p>
        ) : (
          [...byProject.entries()].map(([p, list]) => {
            const shown = all.has(p) || filter ? list : list.slice(0, SHOWN);
            return (
              <section data-project={p}>
                <h3>{p || "Other lists"}</h3>
                <ul class="plain">
                  {shown.map((i) => (
                    <li title={i.description || undefined}>
                      <a class={i.inbox === inbox && i.project === project ? "current" : undefined} href={inboxHref(i.inbox, i.project)}>
                        {i.inbox}
                      </a>{" "}
                      {i.series > 0 && <span class="count muted">{i.series}</span>}
                      {i.updated && (
                        <span class="muted small inbox-updated">
                          <Time iso={i.updated} />
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
                {shown.length < list.length && (
                  <button type="button" class="button small" onClick={() => setAll(new Set(all).add(p))}>
                    All {list.length}
                  </button>
                )}
              </section>
            );
          })
        )}
      </aside>
      <div class="inbox-main">
        {!inbox ? (
          <div class="blankslate">
            <h2>Browse an inbox</h2>
            <p class="muted">
              Its newest threads, as the archive lists them: 50 to a page, one request each, and no thread fetched until you open
              it. Your projects' lists are on the left, newest first; any other inbox by its name, "all" for every one.
            </p>
          </div>
        ) : (
          <>
            <div class="inbox-head">
              <h1>
                {inbox} {project && <span class="muted">in {project}</span>}
              </h1>
              {here?.description && <span class="muted inbox-description">{here.description}</span>}
              <span class="spacer" />
              <span class="muted small">
                {waiting ? (
                  `Waiting: ${doing.toLowerCase() || "another job"} first…`
                ) : listing ? (
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
                {plural(rows.length, "thread")}, newest first; <Label text="not fetched" kind="unfetched" /> until you open one
              </div>
              <ul class="rows">
                {rows.map((r) =>
                  r.family ? (
                    <Row key={r.listed.msgid} f={r.family} mine={mine.includes(familyId(r.family))} />
                  ) : (
                    <ListedThread key={r.listed.msgid} row={r} inbox={inbox} project={project} />
                  ),
                )}
                {!rows.length && <li class="row empty">{listing ? "Listing…" : page ? "Nothing listed yet." : "Loading…"}</li>}
              </ul>
            </div>
            <p class="inbox-more">
              <button type="button" class="button" disabled={listing || !page?.next} onClick={() => ask(true)}>
                Load older
              </button>{" "}
              <span class="muted small">The next 50 threads, in one request.</span>
            </p>
          </>
        )}
      </div>
    </div>
  );
}
