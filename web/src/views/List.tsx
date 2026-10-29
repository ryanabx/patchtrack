import { useState } from "preact/hooks";

import type { Summary } from "../core/Summary";
import type { State } from "../core/State";
import { evidenceLine, filterFamilies, inProject, isOpen, listsOf, ofFamilies, plural, qualifier, whose, withQualifier } from "../lib/format";
import type { Scope, Who } from "../protocol";
import { href } from "../route";
import { Dot, Label, Time } from "./common";

/** One family in a list: its title, lists, state and evidence; "not fetched" while only a search's mail is known. */
export function Row({ f }: { f: Summary }) {
  const why = evidenceLine(f.evidence);
  return (
    <li class="row">
      <Dot state={f.state} />
      <div class="row-main">
        <div class="row-title">
          <a class="title-link" href={href({ view: "family", sender: f.sender, key: f.key, v: null, tab: "conversation", m: null })}>
            {f.title}
          </a>{" "}
          {f.lists.map((l) => (
            <Label text={l} kind="list" />
          ))}{" "}
          {f.unanswered && <Label text="unanswered" kind="flag" />}
          {!f.fetched && (
            <span class="label unfetched" title="Only what a search returned is here; opening the series fetches its thread">
              not fetched
            </span>
          )}
        </div>
        <div class="row-meta">
          v{f.version} · {plural(f.patches, "patch", "patches")} · {f.state} · {f.sender} · last activity{" "}
          <Time iso={f.last_activity} />
          {f.versions > 1 && ` · ${plural(f.versions, "version")}`}
        </div>
        {why && <div class="row-evidence">{why}</div>}
      </div>
      {f.replies > 0 && (
        <a
          class="row-count muted"
          href={href({ view: "family", sender: f.sender, key: f.key, v: null, tab: "conversation", m: null })}
          title={`${plural(f.replies, "reply", "replies")} from people to v${f.version}`}
        >
          <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
            <path fill="currentColor" d="M1 2.75C1 1.78 1.78 1 2.75 1h10.5c.97 0 1.75.78 1.75 1.75v7.5A1.75 1.75 0 0 1 13.25 12H9.06l-2.57 2.57A1.46 1.46 0 0 1 4 13.54V12H2.75A1.75 1.75 0 0 1 1 10.25Zm1.75-.25a.25.25 0 0 0-.25.25v7.5c0 .14.11.25.25.25h2c.41 0 .75.34.75.75v2.19l2.72-2.72a.75.75 0 0 1 .53-.22h4.5a.25.25 0 0 0 .25-.25v-7.5a.25.25 0 0 0-.25-.25Z" />
          </svg>{" "}
          {f.replies}
        </a>
      )}
    </li>
  );
}

const WHO: Array<[Who, string, string]> = [
  ["everyone", "Everyone", "Every series in the store"],
  ["followed", "Followed", "The series of what you follow (authors, lists, searches, threads), and yours"],
  ["mine", "Mine", "The series you sent or wrote, by your identities in Settings"],
];

type Props = {
  families: Summary[];
  scope: Scope;
  /** Your projects' names. */
  projects: string[];
  tab: "open" | "closed";
  q: string;
  who: Who;
  project?: string;
  list?: string;
};

const STATES: State[] = [
  "no-reply", "bot-only", "commented", "discussed", "reviewed", "partially-applied", "applied", "queued", "in-next", "merged", "superseded", "unfetched",
];
const SORTS: Array<[string, string]> = [["", "Newest"], ["updated", "Recently updated"], ["oldest", "Oldest"]];

/** How many of a project's lists show before "more". */
const LISTS_SHOWN = 8;

export function List({ families: all, scope, projects, tab, q, who: asked, project, list }: Props) {
  // Following nothing yet, "followed" is everyone's.
  const who = asked === "followed" && !scope.followed.length ? "everyone" : asked;
  const [query, setQuery] = useState(q);
  const [moreLists, setMoreLists] = useState(false);
  const mine = ofFamilies(all, whose(scope, who));
  const families = inProject(mine, project, list);
  const lists = listsOf(inProject(mine, project));
  const search = (v: string) => {
    setQuery(v);
    history.replaceState(null, "", href({ view: "list", tab, q: v, who: asked, project, list }));
  };
  const here = (p?: string, l?: string) => href({ view: "list", tab, q: query, who: asked, project: p, list: l });
  const shown = filterFamilies(families, tab, query);
  const open = families.filter(isOpen).length;
  const tabLink = (name: "open" | "closed", count: number, text: string) => (
    <a class={`tab${tab === name ? " selected" : ""}`} href={href({ view: "list", tab: name, q: query, who: asked, project, list })}>
      <span class="shape">{name === "open" ? "○" : "✓"}</span> {count} {text}
    </a>
  );
  // "Followed" includes your series, so with no followed series there is nothing to choose.
  const choices = scope.followed.length ? WHO.filter(([w]) => w !== "mine" || scope.mine.length) : [];
  if (!all.length) {
    return (
      <div class="blankslate">
        <h2>No mail yet</h2>
        <p>Add an mbox from lei, b4 or Thunderbird, single .eml messages, or a store patchtrack exported.</p>
        <p>
          <a class="button primary" href={href({ view: "add" })}>
            Add mail
          </a>
        </p>
      </div>
    );
  }
  return (
    <div>
      <div class="subnav">
        <input
          type="search"
          class="search"
          placeholder="Filter: words, state:reviewed author:dev@ list:linux-usb is:unanswered"
          title="Words match the title, state, sender or lists; state:, author:, list: and is:unanswered narrow it; a leading - turns any around; sort:updated or sort:oldest orders it."
          aria-label="Filter series"
          value={query}
          onInput={(e) => search((e.target as HTMLInputElement).value)}
        />
        {choices.length > 0 && (
          <div class="segmented" role="group" aria-label="Whose series">
            {choices.map(([w, text, title]) => (
              <a class={`segment${w === who ? " selected" : ""}`} title={title} href={href({ view: "list", tab, q: query, who: w, project, list })}>
                {text}
              </a>
            ))}
          </div>
        )}
      </div>
      {projects.length > 0 && (
        <nav class="projects" aria-label="Projects">
          <a class={`project${!project ? " selected" : ""}`} href={here()}>
            All projects <span class="count">{mine.length}</span>
          </a>
          {projects.map((p) => (
            <a class={`project${p === project ? " selected" : ""}`} href={here(p)}>
              {p} <span class="count">{inProject(mine, p).length}</span>
            </a>
          ))}
        </nav>
      )}
      {(project || list) && lists.length > 0 && (
        <div class="lists" aria-label="Lists">
          {(moreLists || list ? lists : lists.slice(0, LISTS_SHOWN)).map(([l, n]) => (
            <a class={`label list${l === list ? " selected" : ""}`} href={here(project, l === list ? undefined : l)}>
              {l} {n}
            </a>
          ))}
          {!moreLists && !list && lists.length > LISTS_SHOWN && (
            <button type="button" class="button small" onClick={() => setMoreLists(true)}>
              {lists.length - LISTS_SHOWN} more lists
            </button>
          )}
        </div>
      )}
      <div class="box">
        <div class="box-header tabs">
          {tabLink("open", open, "Open")}
          {tabLink("closed", families.length - open, "Closed")}
          <span class="spacer" />
          <select aria-label="State" class="menu" value={qualifier(query, "state")} onChange={(e) => search(withQualifier(query, "state", (e.target as HTMLSelectElement).value))}>
            <option value="">State</option>
            {STATES.map((st) => (
              <option value={st}>{st}</option>
            ))}
          </select>
          <select aria-label="Sort" class="menu" value={qualifier(query, "sort")} onChange={(e) => search(withQualifier(query, "sort", (e.target as HTMLSelectElement).value))}>
            {SORTS.map(([v, text]) => (
              <option value={v}>{v ? text : "Sort: newest"}</option>
            ))}
          </select>
        </div>
        <ul class="rows">
          {shown.map((f) => (
            <Row key={`${f.sender}\0${f.key}`} f={f} />
          ))}
          {!shown.length && (
            <li class="row empty">
              {query ? "No series match that filter." : `Nothing ${tab}.`}{" "}
              {inProject(all, project, list).length > families.length && (
                <a href={href({ view: "list", tab, q: query, who: "everyone", project, list })}>
                  Show everyone's ({plural(inProject(all, project, list).length - families.length, "more series", "more series")})
                </a>
              )}
            </li>
          )}
        </ul>
      </div>
    </div>
  );
}
