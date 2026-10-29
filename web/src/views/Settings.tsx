// The settings, with a control for each: what `patchtrack settings` shows,
// checked by the same core code, so the page accepts and refuses what the
// command line does. A change is saved when it is made.

import { useEffect, useState } from "preact/hooks";

import type { Project, Setting } from "../protocol";
import { href } from "../route";
import type { Store } from "../store";

type Save = (value: unknown) => Promise<void>;

/** The sections, and what goes in each; a key not named here goes under "Other". */
const GROUPS: Array<[string, string, string[]]> = [
  ["You", "Who you are: which series are yours, and how mail you write is signed.", ["identities", "name"]],
  ["Projects", "The archives and lists you work with, as GitHub has repositories.", ["projects"]],
  ["Syncing", "Where, how often and how far back the bridge syncs.", ["archive", "sync_every", "since", "list_since", "window_days", "refresh_days", "fetch_all_threads", "stale_minutes"]],
  ["Nudges", "When Needs you suggests a ping or a RESEND.", ["ping_days", "resend_days", "merge_window"]],
  ["b4", "", ["b4_prefix"]],
  ["Advanced", "How a project says approved and landed, and whose replies are not review.", ["profiles", "bots"]],
];

const TITLES: Record<string, string> = {
  identities: "Your addresses",
  name: "Your name",
  projects: "Projects",
  archive: "Default archive",
  since: "A first sync goes back",
  list_since: "A followed list's first sync goes back",
  window_days: "Search window, in days",
  refresh_days: "Refetch open series after, in days",
  sync_every: "Sync every, in minutes (0: only when asked)",
  fetch_all_threads: "Fetch every thread a followed list or search finds",
  stale_minutes: "Fetch a series again when opened, after this many minutes (0: only on Refresh)",
  ping_days: "Suggest a ping after, in days",
  resend_days: "Suggest a RESEND after, in days",
  merge_window: "Hold pings while the merge window is open",
  b4_prefix: "Where b4 prep branches are",
  profiles: "Project profiles",
  bots: "Bots",
};

const TIMES: Array<[string, string]> = [
  ["1.week.ago", "a week"],
  ["2.weeks.ago", "two weeks"],
  ["1.month.ago", "a month"],
  ["3.months.ago", "three months"],
  ["6.months.ago", "six months"],
  ["1.year.ago", "a year"],
];

/** Strings as chips, with one to add. */
export function StringList({
  values,
  onChange,
  placeholder,
  empty,
}: {
  values: string[];
  onChange: (v: string[]) => void;
  placeholder: string;
  empty?: string;
}) {
  const [text, setText] = useState("");
  const add = () => {
    const more = text.split(/[,\s]+/).map((t) => t.trim()).filter((t) => t && !values.includes(t));
    if (more.length) onChange([...values, ...more]);
    setText("");
  };
  return (
    <div class="string-list">
      {values.length === 0 && empty && <span class="muted">{empty}</span>}
      {values.map((v) => (
        <span class="chip">
          {v}
          <button type="button" aria-label={`Remove ${v}`} onClick={() => onChange(values.filter((x) => x !== v))}>
            ×
          </button>
        </span>
      ))}
      <input
        value={text}
        placeholder={placeholder}
        onInput={(e) => setText((e.target as HTMLInputElement).value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            add();
          }
        }}
      />
      <button type="button" class="button small" disabled={!text.trim()} onClick={add}>
        Add
      </button>
    </div>
  );
}

/** A text or number saved when it is left or Enter is pressed. */
function Text({
  value,
  save,
  type = "text",
  placeholder,
  min = 1,
}: {
  value: string;
  save: Save;
  type?: "text" | "url" | "number";
  placeholder?: string;
  /** The least a number may be. */
  min?: number;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = () => {
    if (text === value) return;
    save(type === "number" ? Number(text) : text);
  };
  const events = {
    value: text,
    placeholder,
    onInput: (e: Event) => setText((e.target as HTMLInputElement).value),
    onBlur: commit,
    onKeyDown: (e: KeyboardEvent) => e.key === "Enter" && commit(),
  };
  if (type === "number") return <input type="number" min={min} {...events} />;
  if (type === "url") return <input type="url" {...events} />;
  return <input type="text" {...events} />;
}

function Time({ value, save }: { value: string; save: Save }) {
  const preset = TIMES.some(([v]) => v === value);
  const [custom, setCustom] = useState(!preset);
  return (
    <div class="inline">
      <select
        value={custom ? "" : value}
        onChange={(e) => {
          const v = (e.target as HTMLSelectElement).value;
          if (v) {
            setCustom(false);
            save(v);
          } else setCustom(true);
        }}
      >
        {TIMES.map(([v, text]) => (
          <option value={v}>{text}</option>
        ))}
        <option value="">a date, or N.days.ago…</option>
      </select>
      {custom && <Text value={value} save={save} placeholder="2026-10-01, or 10.days.ago" />}
    </div>
  );
}

type Profile = { name: string; recipients: string[]; approvals: string[]; landed: string[] };

function Profiles({ value, save }: { value: Profile[]; save: Save }) {
  const set = (i: number, p: Profile) => save(value.map((x, k) => (k === i ? p : x)));
  const [name, setName] = useState("");
  return (
    <div class="cards">
      {value.map((p, i) => (
        <div class="card">
          <div class="card-head">
            <strong>{p.name}</strong>
            <button type="button" class="button small danger" onClick={() => save(value.filter((_, k) => k !== i))}>
              Remove
            </button>
          </div>
          <label>Its postings go to (globs)</label>
          <StringList values={p.recipients} onChange={(v) => set(i, { ...p, recipients: v })} placeholder="*@gcc.gnu.org" />
          <label>A reviewer approves with (regexes)</label>
          <StringList values={p.approvals} onChange={(v) => set(i, { ...p, approvals: v })} placeholder="OK( for trunk)?[.!]?$" />
          <label>It landed when someone says (regexes)</label>
          <StringList values={p.landed} onChange={(v) => set(i, { ...p, landed: v })} placeholder="(Pushed|Committed)\b" />
        </div>
      ))}
      <div class="inline">
        <input value={name} placeholder="A profile's name" onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        <button
          type="button"
          class="button small"
          disabled={!name.trim()}
          onClick={() => {
            save([...value, { name: name.trim(), recipients: [], approvals: [], landed: [] }]);
            setName("");
          }}
        >
          Add a profile
        </button>
      </div>
    </div>
  );
}

type Bots = { builtin?: boolean; addresses?: string[]; patterns?: string[]; humans?: string[]; bodies?: string[] };

function BotsEditor({ value, save }: { value: Bots; save: Save }) {
  const set = (k: keyof Bots, v: unknown) => save({ ...value, [k]: v });
  return (
    <div class="cards">
      <label class="check">
        <input type="checkbox" checked={value.builtin !== false} onChange={(e) => set("builtin", (e.target as HTMLInputElement).checked)} /> The
        built-in rule ("bot", "robot", "noreply" in the address, and the kernel test robot)
      </label>
      <label>These addresses are bots</label>
      <StringList values={value.addresses ?? []} onChange={(v) => set("addresses", v)} placeholder="ci@example.org" />
      <label>Addresses like these are bots (globs)</label>
      <StringList values={value.patterns ?? []} onChange={(v) => set("patterns", v)} placeholder="*-ci@*" />
      <label>These are people, whatever the rules say</label>
      <StringList values={value.humans ?? []} onChange={(v) => set("humans", v)} placeholder="robot.person@example.org" />
      <label>A body like these is bot mail, from anyone (regexes)</label>
      <StringList values={value.bodies ?? []} onChange={(v) => set("bodies", v)} placeholder="^My bot found errors" />
    </div>
  );
}

function ProjectsEditor({ value, save, presets }: { value: Project[]; save: Save; presets: Project[] }) {
  const set = (i: number, p: Project) => save(value.map((x, k) => (k === i ? p : x)));
  const [name, setName] = useState("");
  const [archive, setArchive] = useState("");
  const more = presets.filter((p) => !value.some((v) => v.name.toLowerCase() === p.name.toLowerCase()));
  return (
    <div class="cards">
      {value.length === 0 && <p class="muted">None yet: everything is on the default archive, below.</p>}
      {value.map((p, i) => (
        <div class="card" data-project={p.name}>
          <div class="card-head">
            <strong>{p.name}</strong>
            <button type="button" class="button small danger" onClick={() => save(value.filter((_, k) => k !== i))}>
              Remove
            </button>
          </div>
          <label>Archive</label>
          <Text value={p.archive} type="url" save={async (v) => set(i, { ...p, archive: String(v) })} />
          <label>Its lists there</label>
          <StringList values={p.lists} onChange={(v) => set(i, { ...p, lists: v })} placeholder="linux-usb" empty="every list" />
          <label>Your own mail is in it when sent to (globs)</label>
          <StringList values={p.recipients} onChange={(v) => set(i, { ...p, recipients: v })} placeholder="*@vger.kernel.org" />
        </div>
      ))}
      {more.length > 0 && (
        <div class="presets">
          <span class="muted">Add a built-in project:</span>
          {more.map((p) => (
            <button type="button" class="button small" title={`${p.archive} ${p.lists.join(", ")}`} onClick={() => save([...value, p])}>
              {p.name}
            </button>
          ))}
        </div>
      )}
      <div class="inline">
        <input value={name} placeholder="Your own project's name" onInput={(e) => setName((e.target as HTMLInputElement).value)} />
        <input value={archive} type="url" placeholder="https://its.archive.example" onInput={(e) => setArchive((e.target as HTMLInputElement).value)} />
        <button
          type="button"
          class="button small"
          disabled={!name.trim() || !archive.trim()}
          onClick={() => {
            save([...value, { name: name.trim(), archive: archive.trim(), lists: [], recipients: [] }]);
            setName("");
            setArchive("");
          }}
        >
          Add
        </button>
      </div>
    </div>
  );
}

function Control({ s, value, save, presets }: { s: Setting; value: unknown; save: Save; presets: Project[] }) {
  switch (s.input) {
    case "addresses":
      return <StringList values={value as string[]} onChange={save} placeholder="you@example.org" empty="none yet" />;
    case "text":
    case "url":
      return <Text value={String(value)} type={s.input === "url" ? "url" : "text"} save={save} />;
    case "days":
      return <Text value={String(value)} type="number" save={save} />;
    case "minutes":
      return <Text value={String(value)} type="number" min={0} save={save} />;
    case "bool":
      return (
        <label class="check">
          <input type="checkbox" checked={Boolean(value)} onChange={(e) => save((e.target as HTMLInputElement).checked)} /> Yes
        </label>
      );
    case "time":
      return <Time value={String(value)} save={save} />;
    case "projects":
      return <ProjectsEditor value={value as Project[]} save={save} presets={presets} />;
    case "profiles":
      return <Profiles value={value as Profile[]} save={save} />;
    case "bots":
      return <BotsEditor value={value as Bots} save={save} />;
    default:
      return <Text value={JSON.stringify(value)} save={async (v) => save(JSON.parse(String(v)))} />;
  }
}

/** The settings, a section at a time, each with its control. */
/** A category's place in the address: its title, in lower case. */
const slug = (title: string) => title.toLowerCase();

export function Settings({ store, onChanged, category }: { store: Store; onChanged: () => void; category?: string }) {
  const [settings, setSettings] = useState<Setting[] | null>(null);
  const [presets, setPresets] = useState<Project[]>([]);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [saved, setSaved] = useState<string | null>(null);

  const load = () => store.settings().then(setSettings);
  useEffect(() => {
    load();
    store.projects().then((p) => setPresets(p.presets), () => setPresets([]));
  }, []);

  const save = (key: string) => async (value: unknown) => {
    try {
      await store.setSetting(key, JSON.stringify(value));
      setErrors((e) => ({ ...e, [key]: "" }));
      setSaved(key);
      await load();
      onChanged();
    } catch (e) {
      setErrors((err) => ({ ...err, [key]: e instanceof Error ? e.message : String(e) }));
    }
  };
  const reset = async (key: string) => {
    await store.unsetSetting(key);
    setErrors((e) => ({ ...e, [key]: "" }));
    await load();
    onChanged();
  };

  if (!settings) return <p class="muted">Loading…</p>;
  const named = new Set(GROUPS.flatMap(([, , keys]) => keys));
  const groups: Array<[string, string, Setting[]]> = GROUPS.map(([title, intro, keys]) => [
    title,
    intro,
    keys.flatMap((k) => settings.filter((s) => s.key === k)),
  ]);
  const other = settings.filter((s) => !named.has(s.key));
  if (other.length) groups.push(["Other", "", other]);
  // One category at a time, chosen on the left, as GitHub's settings are.
  const current = groups.find(([title]) => slug(title) === category) ?? groups[0];
  return (
    <div class="settings">
      <nav class="settings-nav" aria-label="Categories">
        {groups.map(([title, , list]) => (
          <a
            class={title === current[0] ? "selected" : undefined}
            href={href({ view: "settings", category: slug(title) })}
            data-category={slug(title)}
          >
            {title}
            {list.some((s) => errors[s.key]) && <span class="error"> !</span>}
          </a>
        ))}
      </nav>
      <div class="settings-main">
      <h1>{current[0]}</h1>
      {[current].map(([title, intro, list]) => (
        <section class="settings-group" data-category={slug(title)}>
          {intro && <p class="note">{intro}</p>}
          {list.map((s) => {
            const value = JSON.parse(s.value ?? s.default);
            return (
              <div class="setting" id={`setting-${s.key}`}>
                <h3>{TITLES[s.key] ?? s.key}</h3>
                <p class="note">{s.doc}</p>
                <Control s={s} value={value} save={save(s.key)} presets={presets} />
                <p class="muted status">
                  {s.value === null ? "The default." : saved === s.key ? "Saved. " : "Set. "}
                  {s.value !== null && (
                    <button type="button" class="link" onClick={() => reset(s.key)}>
                      Back to the default
                    </button>
                  )}
                </p>
                {errors[s.key] && <p class="error">{errors[s.key]}</p>}
              </div>
            );
          })}
        </section>
      ))}
      <p class="note">
        Kept in the bridge's store: the command line on that machine reads the same ones (<code>patchtrack settings</code>), and
        an exported store carries them (a public export does not).
      </p>
      </div>
    </div>
  );
}
