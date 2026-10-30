// The archive searched, as lore's own search does: any query it takes, in an
// inbox or in all of them. A page is one request by the bridge (the
// archive's Atom feed of the search: 50 threads, each by its newest message
// that matched), kept apart from the mail: a thread is fetched when you open
// it. Nothing is asked as you type, only when you search.

import { useEffect, useRef, useState } from "preact/hooks";

import { familyId, plural } from "../lib/format";
import type { Listing, Projects, Status } from "../protocol";
import { href } from "../route";
import type { Store } from "../store";
import { Label, Time } from "./common";
import { ListedThread } from "./Inboxes";
import { Row } from "./List";

type Props = {
  store: Store;
  /** Your families, by `familyId`. */
  mine: string[];
  q: string;
  inbox?: string;
  project?: string;
  status: Status | null;
  /** Changes when the store does: the page then asks again. */
  version: unknown;
  /** A page was asked for: ask for the status again. */
  onAsked: () => void;
};

export function Search({ store, mine, q, inbox = "all", project = "", status, version, onAsked }: Props) {
  const query = q.trim();
  const [text, setText] = useState(q);
  const [where, setWhere] = useState(inbox);
  const [projects, setProjects] = useState<Projects | null>(null);
  const [page, setPage] = useState<Listing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [followed, setFollowed] = useState(false);
  useEffect(() => {
    store.projects().then(setProjects, (e: Error) => setError(e.message));
  }, []);
  useEffect(() => {
    setText(q);
    setWhere(inbox);
    setPage(null);
    setFollowed(false);
  }, [q, inbox, project]);
  useEffect(() => {
    if (query) store.listing(project, inbox, query).then(setPage, (e: Error) => setError(e.message));
  }, [query, inbox, project, version]);

  const searching = (status?.sync.searching ?? []).some((s) => s.inbox === inbox && s.query === query);
  const doing = status?.sync.doing ?? "";
  const waiting = searching && !doing.endsWith(` for ${query}`);
  const ask = (older = false) => {
    setError(null);
    store.browse(project, inbox, older, query).then(onAsked, (e: Error) => setError(e.message));
  };
  // A search never listed is asked for once, when the page first shows it.
  const asked = useRef<string | null>(null);
  useEffect(() => {
    const key = `${project}\0${inbox}\0${query}`;
    if (!query || !page || page.listed || page.failed || asked.current === key) return;
    asked.current = key;
    ask();
  }, [page]);

  const archive = projects?.projects.find((p) => p.name === project)?.archive ?? projects?.archive ?? "";
  const help = archive && `${archive}/${encodeURIComponent(inbox)}/_/text/help/`;
  const rows = page?.rows ?? [];
  const follow = () =>
    store.follow("query", query, inbox, project).then(
      () => setFollowed(true),
      (e: Error) => setError(e.message),
    );

  return (
    <div class="archive-search">
      <form
        class="search-form"
        role="search"
        onSubmit={(e) => {
          e.preventDefault();
          const next = { view: "search", q: text.trim(), inbox: where.trim() || "all", project: project || undefined } as const;
          if (next.q === query && next.inbox === inbox) ask();
          else if (next.q) location.hash = href(next);
        }}
      >
        <input
          class="search-query"
          placeholder="s:usb f:someone@example.org dfn:drivers/usb/ d:2.weeks.ago.."
          aria-label="Search the archive"
          value={text}
          onInput={(e) => setText((e.target as HTMLInputElement).value)}
        />
        <label>
          in{" "}
          <input
            class="search-inbox"
            aria-label="Inbox to search"
            value={where}
            onInput={(e) => setWhere((e.target as HTMLInputElement).value)}
          />
        </label>
        {projects && projects.projects.length > 0 && (
          <select
            class="menu"
            aria-label="Archive"
            value={project}
            onChange={(e) => {
              const p = (e.target as HTMLSelectElement).value;
              if (query) location.hash = href({ view: "search", q: query, inbox: where.trim() || "all", project: p || undefined });
            }}
          >
            <option value="">{projects.archive}</option>
            {projects.projects
              .filter((p) => p.archive !== projects.archive)
              .map((p) => (
                <option value={p.name}>{p.name}</option>
              ))}
          </select>
        )}
        <button type="submit" class="button primary" disabled={!text.trim() || searching}>
          Search
        </button>
      </form>
      <p class="muted small">
        The archive's own query language: <code>s:</code> subject, <code>f:</code> from, <code>tc:</code> to or Cc, <code>b:</code> body,{" "}
        <code>dfn:</code> a file a patch changes, <code>d:</code> dates; <code>s:PATCH</code> for patches only.{" "}
        {help && (
          <a href={help} target="_blank" rel="noreferrer">
            All of it
          </a>
        )}
      </p>
      {query && !query.includes(":") && (
        <p class="small">
          <a href={href({ view: "list", tab: "open", q: query, who: "everyone" })}>The series here</a> that match, without asking the
          archive.
        </p>
      )}
      {!query ? (
        <div class="blankslate">
          <h2>Search the archive</h2>
          <p class="muted">
            Any query the archive takes, in one inbox or in all of them ("all"): 50 threads to a page, one request each, and no thread
            fetched until you open it.
          </p>
        </div>
      ) : (
        <>
          <div class="inbox-head">
            <h1>
              <span class="muted">{inbox === "all" ? "Every inbox" : inbox}:</span> {query}
            </h1>
            <span class="spacer" />
            <span class="muted small">
              {waiting ? (
                `Waiting: ${doing.toLowerCase() || "another job"} first…`
              ) : searching ? (
                "Searching…"
              ) : page?.listed ? (
                <>
                  searched <Time iso={page.listed} />
                </>
              ) : null}
            </span>{" "}
            <button type="button" class="button" disabled={searching} onClick={() => ask()}>
              Search again
            </button>
            <button type="button" class="button follow-search" disabled={followed} onClick={follow} title="Synced with what you follow">
              {followed ? "Following" : "Follow this search"}
            </button>
          </div>
          {followed && <p class="muted small">Each sync searches it; its series join what you follow.</p>}
          {error && <p class="error">{error}</p>}
          {page?.failed && !searching && (
            <p class="error search-failed">
              {page.failed.includes("HTTP 400") ? "The archive could not parse that query." : "The archive did not answer."}{" "}
              <span class="muted small">{page.failed}</span>
            </p>
          )}
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
              {!rows.length && (
                <li class="row empty">
                  {searching ? "Searching…" : !page ? "Loading…" : page.listed ? "Nothing matched." : page.failed ? "Nothing listed." : "Searching…"}
                </li>
              )}
            </ul>
          </div>
          <p class="inbox-more">
            <button type="button" class="button" disabled={searching || !page?.next} onClick={() => ask(true)}>
              Load older
            </button>{" "}
            <span class="muted small">The next 50 threads, in one request.</span>
          </p>
        </>
      )}
    </div>
  );
}
