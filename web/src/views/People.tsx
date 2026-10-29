// Who replied, and how: `patchtrack people`.

import { useEffect, useState } from "preact/hooks";

import type { Person } from "../core/Person";
import { displayName, plural } from "../lib/format";
import type { Scope, Who } from "../protocol";
import { href } from "../route";
import type { Store } from "../store";
import { Avatar, Label, Time } from "./common";

const WHOSE: Record<Who, string> = { everyone: "every series", followed: "the series you follow and yours", mine: "your series" };

/** `families` changes when the store does: the page then asks again. */
export function People({ store, families, scope, who, bots }: { store: Store; families: unknown; scope: Scope; who: Who; bots: boolean }) {
  const [people, setPeople] = useState<Person[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Blank only for another question; a change to the store is answered in place.
  useEffect(() => setPeople(null), [who, bots]);
  useEffect(() => {
    store.people(who, bots).then(setPeople, (e: Error) => setError(e.message));
  }, [families, who, bots]);
  if (error) return <p class="error">{error}</p>;
  const link = (w: Who, text: string) => (
    <a class={`segment${w === who ? " selected" : ""}`} href={href({ view: "people", who: w, bots })}>
      {text}
    </a>
  );
  return (
    <div class="people">
      <h1>People</h1>
      <p class="note">
        Everyone who replied to {WHOSE[who]}, most recent first: how many series and replies, the tags they gave (Reviewed-by, Acked-by
        and the like), their inline reviews without a tag, and the "applied" mails they sent.
      </p>
      <div class="subnav">
        {scope.followed.length > 0 && (
          <div class="segmented" role="group" aria-label="Whose series">
            {link("everyone", "Everyone")}
            {link("followed", "Followed")}
            {scope.mine.length > 0 && link("mine", "Mine")}
          </div>
        )}
        <label class="check">
          <input type="checkbox" checked={bots} onChange={() => (location.hash = href({ view: "people", who, bots: !bots }))} /> Show bots
        </label>
      </div>
      {!people ? (
        <p class="muted">Loading…</p>
      ) : people.length === 0 ? (
        <p class="blankslate">No one has replied yet.{bots ? "" : " Bots are hidden."}</p>
      ) : (
        <div class="scroll-x">
        <table class="box people-table">
          <thead>
            <tr>
              <th>who</th>
              <th>series</th>
              <th>replies</th>
              <th>tags</th>
              <th>comments</th>
              <th>applied</th>
              <th>last</th>
            </tr>
          </thead>
          <tbody>
            {people.map((p) => (
              <tr>
                <td class="who" title={p.who}>
                  <Avatar who={p.who} small /> {displayName(p.who)} {p.bot && <Label text="bot" />}
                </td>
                <td title={plural(p.series.length, "series", "series")}>{p.series.length}</td>
                <td>{p.replies}</td>
                <td>{p.tags || ""}</td>
                <td>{p.comments || ""}</td>
                <td>{p.applied || ""}</td>
                <td>
                  <Time iso={p.last} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
      )}
    </div>
  );
}
