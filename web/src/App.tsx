import { useEffect, useRef, useState } from "preact/hooks";

import type { Summary } from "./core/Summary";
import type { Scope, Status } from "./protocol";
import { href, parse } from "./route";
import { type Bridge, Store } from "./store";
import { Add } from "./views/Add";
import { Connect } from "./views/Connect";
import { FamilyView } from "./views/Family";
import { Following, syncLine } from "./views/Following";
import { List } from "./views/List";
import { Notifications } from "./views/Notifications";
import { People } from "./views/People";
import { DraftsPage } from "./views/Drafts";
import { Branches } from "./views/Branches";
import { Settings } from "./views/Settings";
import { pairing, saveBridge, savedBridge } from "./lib/bridge";

/** How often the page asks the bridge whether the store changed, while it is shown. */
const POLL_MS = 4000;

/** Pairing, then the app once the bridge answers. */
export function App() {
  const [bridge, setBridge] = useState<Bridge | null>(savedBridge());
  const [store, setStore] = useState<Store | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  // A bridge elsewhere waits for you to confirm it.
  const [pending, setPending] = useState<Bridge | null>(null);

  const connect = (b: Bridge | null) => {
    setStore(null);
    setError(null);
    if (!b) return;
    setBusy(true);
    Store.connect(b).then(
      (s) => {
        setStore(s);
        setBusy(false);
      },
      (e: Error) => {
        setError(e.message);
        setBusy(false);
      },
    );
  };
  const pair = (b: Bridge | null, said: string) => {
    saveBridge(b);
    setBridge(b);
    setNote(said);
    connect(b);
  };

  // A pairing link (`patchtrack bridge` prints it): keep the bridge, and
  // leave the token out of the address. True if it was one to act on.
  const takeLink = () => {
    const route = parse(location.hash);
    if (route.view !== "pair") return false;
    const b = pairing(route.url, route.token);
    history.replaceState(null, "", "#/");
    if ("error" in b) {
      setNote(b.error);
      return false;
    }
    if ("remote" in b) {
      setStore(null);
      setPending(b.remote);
    } else pair(b, `Paired with the bridge at ${b.url}.`);
    return true;
  };
  useEffect(() => {
    if (!takeLink()) connect(bridge);
    // A link opened in a tab that already shows the app changes only the hash.
    const onHash = () => takeLink();
    addEventListener("hashchange", onHash);
    return () => removeEventListener("hashchange", onHash);
  }, []);

  if (!store)
    return (
      <>
        <header class="appbar">
          <a class="brand" href="#/">
            patchtrack
          </a>
        </header>
        <main class="container">
          {busy && !pending ? (
            <p class="muted loading">Connecting to the bridge…</p>
          ) : (
            <Connect
              bridge={bridge}
              error={error}
              note={note}
              busy={busy}
              pending={pending}
              onConfirm={(yes) => {
                if (pending && yes) pair(pending, `Paired with the bridge at ${pending.url}.`);
                else {
                  setNote("Not paired: the link was ignored.");
                  connect(savedBridge());
                }
                setPending(null);
              }}
              onRetry={() => connect(bridge)}
              onForget={() => pair(null, "Forgotten. Open a bridge's link to pair again.")}
            />
          )}
        </main>
      </>
    );
  return <Main store={store} onLost={(why) => (setError(why), setStore(null))} />;
}

function Main({ store, onLost }: { store: Store; onLost: (why: string) => void }) {
  const [families, setFamilies] = useState<Summary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [hash, setHash] = useState(location.hash);
  const [scope, setScope] = useState<Scope>({ mine: [], followed: [], authors: [] });
  const [projects, setProjects] = useState<string[]>([]);
  const [status, setStatus] = useState<Status | null>(null);

  const reload = () =>
    Promise.all([store.summaries(), store.scope(), store.projects()]).then(
      ([f, s, p]) => {
        setScope(s);
        setProjects(p.projects.map((x) => x.name));
        setFamilies(f);
      },
      (e: Error) => setError(e.message),
    );

  // The store changes under the page (the bridge's syncs, the command
  // line): ask whether it did, and read it again when it has. A bridge
  // that stops answering twice running is lost.
  const seen = useRef<number | null>(null);
  const failed = useRef(0);
  const poll = () =>
    store.status().then(
      (s) => {
        if (seen.current !== null && seen.current !== s.generation) reload();
        seen.current = s.generation;
        failed.current = 0;
        setStatus(s);
      },
      (e: Error) => {
        if (++failed.current >= 2) onLost(e.message);
      },
    );
  useEffect(() => {
    reload();
    poll();
    const timer = setInterval(() => document.visibilityState === "visible" && poll(), POLL_MS);
    const onShow = () => document.visibilityState === "visible" && poll();
    const onHash = () => setHash(location.hash);
    addEventListener("hashchange", onHash);
    document.addEventListener("visibilitychange", onShow);
    return () => {
      clearInterval(timer);
      removeEventListener("hashchange", onHash);
      document.removeEventListener("visibilitychange", onShow);
    };
  }, []);

  const route = parse(hash);
  useEffect(() => {
    if (!(route.view === "family" && route.m)) scrollTo(0, 0);
    setMenu(false);
  }, [hash]);
  // On a narrow screen the pages fold into a menu.
  const [menu, setMenu] = useState(false);

  async function exportStore() {
    const url = URL.createObjectURL(await store.exportFile());
    const a = document.createElement("a");
    a.href = url;
    a.download = "patchtrack.sqlite3";
    a.click();
    URL.revokeObjectURL(url);
  }

  // Views that read more than the list ask again when the store changed.
  const version = status?.generation;
  let main;
  if (error) main = <p class="error">{error}</p>;
  else if (!families) main = <p class="muted loading">Loading…</p>;
  else if (route.view === "add") main = <Add store={store} onAdded={reload} />;
  else if (route.view === "settings") main = <Settings store={store} onChanged={reload} />;
  else if (route.view === "notifications")
    main = <Notifications store={store} tab={route.tab} version={version} onChanged={poll} />;
  else if (route.view === "drafts") main = <DraftsPage store={store} version={version} />;
  else if (route.view === "branches") main = <Branches store={store} />;
  else if (route.view === "people")
    main = <People store={store} families={families} scope={scope} who={route.who} bots={route.bots} />;
  else if (route.view === "following") main = <Following store={store} status={status} onAsked={poll} version={version} />;
  else if (route.view === "family") main = <FamilyView store={store} route={route} onChanged={reload} onRead={poll} version={version} />;
  else if (route.view === "list" || route.view === "pair")
    main = (
      // A link to the list starts from its own query; typing in the box replaces the address without one.
      <List
        key={hash}
        families={families}
        scope={scope}
        projects={projects}
        tab={route.view === "list" ? route.tab : "open"}
        q={route.view === "list" ? route.q : ""}
        who={route.view === "list" ? route.who : "everyone"}
        project={route.view === "list" ? route.project : undefined}
        list={route.view === "list" ? route.list : undefined}
      />
    );
  else main = <p>Not found. <a href="#/">Back to the series</a></p>;

  const sync = status?.sync;
  return (
    <>
      <header class="appbar">
        <a class="brand" href="#/">
          patchtrack
        </a>
        <button
          type="button"
          class="menu-toggle"
          aria-label="Menu"
          aria-expanded={menu}
          aria-controls="pages"
          onClick={() => setMenu(!menu)}
        >
          {menu ? "✕" : "☰"}
        </button>
        <nav id="pages" class={menu ? "open" : ""}>
          <a href={href({ view: "list", tab: "open", q: "", who: "everyone" })} class={route.view === "list" || route.view === "family" ? "selected" : ""}>
            Series
          </a>
          <a href={href({ view: "notifications", tab: "unread" })} class={route.view === "notifications" ? "selected" : ""}>
            Notifications
            {status && status.unread > 0 && (
              <span class="unread-count" title={`${status.unread} unread`}>
                {status.unread}
              </span>
            )}
          </a>
          <a href={href({ view: "people", who: "everyone", bots: false })} class={route.view === "people" ? "selected" : ""}>
            People
          </a>
          <a href={href({ view: "branches" })} class={route.view === "branches" ? "selected" : ""}>
            Branches
          </a>
          <a href={href({ view: "drafts" })} class={route.view === "drafts" ? "selected" : ""}>
            Drafts
          </a>
          <a href={href({ view: "following" })} class={route.view === "following" ? "selected" : ""}>
            Following
          </a>
          <a href={href({ view: "add" })} class={route.view === "add" ? "selected" : ""}>
            Add mail
          </a>
          <a href={href({ view: "settings" })} class={route.view === "settings" ? "selected" : ""}>
            Settings
          </a>
        </nav>
        <span class="spacer" />
        <a
          class={`sync-badge${sync?.running ? " running" : ""}${sync?.ok === false ? " failed" : ""}`}
          href={href({ view: "following" })}
          title={sync?.log[sync.log.length - 1] ?? ""}
        >
          {syncLine(sync)}
        </a>
        {families && families.length > 0 && (
          <button type="button" class="button" onClick={exportStore} title="Download the store: any patchtrack can open or merge it">
            Export
          </button>
        )}
      </header>
      <main class="container">{main}</main>
    </>
  );
}
