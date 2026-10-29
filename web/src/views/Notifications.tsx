// GitHub's notifications, for mail: each series that changed since you last
// read it (`patchtrack news` finds the same), and a tab of what needs you
// (`patchtrack nudge`). Opening a series marks it read.

import { useEffect, useState } from "preact/hooks";

import type { Notification, Notifications as Unread, Nudge } from "../protocol";
import { href } from "../route";
import type { Store } from "../store";
import { Label, Pill, Time } from "./common";

const familyHref = (sender: string, key: string) => href({ view: "family", sender, key, v: null, tab: "conversation", m: null });

/** What changed, in a few words each. */
function reasons(n: Notification): string[] {
  const out: string[] = [];
  if (n.newSeries) out.push(`new series, ${n.state}`);
  if (n.newVersion !== null) out.push(`v${n.newVersion} posted`);
  if (n.stateChange) out.push(`${n.stateChange[0]} → ${n.stateChange[1]}`);
  return out;
}

function Row({ n, onRead }: { n: Notification; onRead: (n: Notification) => void }) {
  return (
    <li class="row notification unread" data-key={n.key}>
      <span class="unread-dot" title="Unread" />
      <Pill state={n.state} />
      <div class="row-main">
        <div class="row-meta">
          {n.lists.map((l) => (
            <Label text={l} kind="list" />
          ))}{" "}
          {n.sender}
        </div>
        <div class="row-title">
          <a href={familyHref(n.sender, n.key)} onClick={() => onRead(n)}>
            <strong>{n.title}</strong>
          </a>{" "}
          <span class="muted">v{n.version}</span>
        </div>
        <ul class="news-lines">
          {reasons(n).map((r) => (
            <li>{r}</li>
          ))}
          {n.replies.map((r) => (
            <li>{r}</li>
          ))}
        </ul>
      </div>
      <div class="notification-side">
        <Time iso={n.updated} />
        <button type="button" class="button small mark-read" title="Mark as read" onClick={() => onRead(n)}>
          ✓ Mark as read
        </button>
      </div>
    </li>
  );
}

function UnreadList({ store, list, onChanged }: { store: Store; list: Unread; onChanged: () => void }) {
  const [error, setError] = useState<string | null>(null);
  // Gone at once; the store says so a moment later.
  const [gone, setGone] = useState<Set<string>>(new Set());
  useEffect(() => setGone(new Set()), [list]);
  const read = (n: Notification) => {
    setGone((g) => new Set(g).add(`${n.sender}\0${n.key}`));
    store.markRead(n.sender, n.key).then(onChanged, (e: Error) => setError(e.message));
  };
  const shown = list.notifications.filter((n) => !gone.has(`${n.sender}\0${n.key}`));
  if (error) return <p class="error">{error}</p>;
  if (!shown.length)
    return (
      <div class="blankslate">
        <h2>All caught up</h2>
        <p class="muted">New series and versions, state changes and replies in what you have show up here until you read them.</p>
      </div>
    );
  return (
    <>
      <div class="box">
        <div class="box-header notifications-head">
          <span>{shown.length} unread</span>
          <span class="spacer" />
          <button
            type="button"
            class="button small"
            onClick={() => {
              setGone(new Set(list.notifications.map((n) => `${n.sender}\0${n.key}`)));
              store.markAllRead().then(onChanged, (e: Error) => setError(e.message));
            }}
          >
            ✓ Mark all as read
          </button>
        </div>
        <ul class="rows">
          {shown.map((n) => (
            <Row key={`${n.sender}\0${n.key}`} n={n} onRead={read} />
          ))}
        </ul>
      </div>
      <p class="muted small">Opening a series marks it read, as reading it on GitHub does.</p>
    </>
  );
}

function NeedsYou({ nudges }: { nudges: Nudge[] }) {
  return (
    <>
      <p class="note">
        A reply is owed when someone asked or said something after your last message (a bare tag or an "Applied" asks nothing).
        A ping is due after a quiet week, a RESEND after two weeks with no human reply, as the kernel's submitting-patches.rst
        suggests; the ping_days and resend_days settings change that. Nothing is sent from here.
      </p>
      {nudges.length === 0 ? (
        <p class="blankslate">Nothing to nudge.</p>
      ) : (
        <ul class="rows box">
          {nudges.map((n) => (
            <li class="row">
              <Pill state={n.state} />
              <div class="row-main">
                <div class="row-title">
                  <strong class="kind">{n.kind}</strong> <a href={familyHref(n.sender, n.key)}>{n.title}</a>
                </div>
                <div class="row-meta">{n.why}</div>
              </div>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

type Props = {
  store: Store;
  tab: "unread" | "needs-you";
  /** Changes when the store does: the page then asks again. */
  version: unknown;
  /** Something was marked read: the unread count is asked again. */
  onChanged: () => void;
};

export function Notifications({ store, tab, version, onChanged }: Props) {
  const [list, setList] = useState<Unread | null>(null);
  const [nudges, setNudges] = useState<Nudge[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    store.notifications().then(
      (l) => {
        // Nothing read yet: start from what is here now, as a new GitHub account does.
        if (l.first) store.markAllRead().then(onChanged, (e: Error) => setError(e.message));
        setList(l);
      },
      (e: Error) => setError(e.message),
    );
    store.nudges().then(setNudges, (e: Error) => setError(e.message));
  }, [version]);
  const tabLink = (name: Props["tab"], text: string, count: number | null) => (
    <a class={`tab${tab === name ? " selected" : ""}`} href={href({ view: "notifications", tab: name })} data-tab={name}>
      {text}
      {count !== null && <span class="counter">{count}</span>}
    </a>
  );
  return (
    <div class="attention">
      <h1>Notifications</h1>
      <nav class="tabs underline">
        {tabLink("unread", "Unread", list ? list.notifications.length : null)}
        {tabLink("needs-you", "Needs you", nudges ? nudges.length : null)}
      </nav>
      {error ? (
        <p class="error">{error}</p>
      ) : tab === "needs-you" ? (
        nudges ? <NeedsYou nudges={nudges} /> : <p class="muted">Loading…</p>
      ) : list ? (
        <UnreadList store={store} list={list} onChanged={onChanged} />
      ) : (
        <p class="muted">Loading…</p>
      )}
    </div>
  );
}
