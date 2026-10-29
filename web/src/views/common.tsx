import type { State } from "../core/State";
import { ago, displayName, stateStyle, when } from "../lib/format";

export function Pill({ state, text, big }: { state: State; text?: string; big?: boolean }) {
  const { shape, tone } = stateStyle(state);
  return (
    <span class={`pill tone-${tone}${big ? " big" : ""}`} title={text ?? state}>
      <span class="shape">{shape}</span>
      {text ?? state}
    </span>
  );
}

export function Dot({ state }: { state: State }) {
  const { shape, tone } = stateStyle(state);
  return (
    <span class={`dot tone-${tone}`} title={state} aria-label={state}>
      {shape}
    </span>
  );
}

export function Time({ iso }: { iso: string }) {
  return (
    <time dateTime={iso} title={when(iso)}>
      {ago(iso)}
    </time>
  );
}

/** No pictures in mail: a letter, coloured by name. */
export function Avatar({ who, small }: { who: string; small?: boolean }) {
  const name = displayName(who) || "?";
  let n = 0;
  for (const c of name) n = (n * 31 + (c.codePointAt(0) ?? 0)) % 8;
  return (
    <span class={`avatar av-${n}${small ? " small" : ""}`} aria-hidden="true">
      {name[0].toUpperCase()}
    </span>
  );
}

export function Label({ text, kind = "" }: { text: string; kind?: string }) {
  return <span class={`label ${kind}`}>{text}</span>;
}
