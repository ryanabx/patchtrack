// Until it is paired with a bridge that answers, the app has nothing to
// show: the store is the bridge's.

import type { Bridge } from "../store";

type Props = {
  /** The bridge this app is paired with, if any. */
  bridge: Bridge | null;
  /** Why it could not be reached, or a note about the last pairing. */
  error: string | null;
  note: string | null;
  busy: boolean;
  /** A bridge elsewhere a link named, waiting for you to confirm it. */
  pending: Bridge | null;
  onConfirm: (yes: boolean) => void;
  onRetry: () => void;
  onForget: () => void;
};

export function Connect({ bridge, error, note, busy, pending, onConfirm, onRetry, onForget }: Props) {
  return (
    <div class="connect">
      <h1>Connect to your bridge</h1>
      {note && <p class="note">{note}</p>}
      {pending && (
        <div class="pair-confirm">
          <p>
            A link asks to connect this app to the bridge at <strong>{new URL(pending.url).host}</strong>. Connect only if it is
            yours (a <code>patchtrack bridge --public-url</code> you started): this app then shows that machine's store, changes it,
            and with your leave sends your drafts through it.
          </p>
          <button type="button" class="button primary" onClick={() => onConfirm(true)}>
            Connect
          </button>{" "}
          <button type="button" class="button" onClick={() => onConfirm(false)}>
            Don't connect
          </button>
        </div>
      )}
      <p>
        patchtrack's store lives on your computer, and <code>patchtrack bridge</code> serves it to this app: the mail, what you follow,
        your settings and drafts. It syncs with the archive on its own, sends your mail and runs b4 when you allow it. This page keeps
        nothing but the address of the bridge and its token.
      </p>
      {bridge ? (
        <>
          <p>
            Paired with <code>{bridge.url}</code>.{" "}
            <button type="button" class="button" disabled={busy} onClick={onRetry}>
              {busy ? "Connecting…" : "Try again"}
            </button>{" "}
            <button type="button" class="button" disabled={busy} onClick={onForget}>
              Forget it
            </button>
          </p>
          {error && <p class="error">{error}</p>}
        </>
      ) : (
        !pending && (
          <ol class="steps">
            <li>
              Install patchtrack on your computer (see <a href="./book/install.html">Install</a>).
            </li>
            <li>
              Run <code>patchtrack bridge</code>, with <code>--allow-send</code> to send mail from here and{" "}
              <code>--repo PATH</code> to run b4 on your branches.
            </li>
            <li>Open the link it prints. Your browser asks once whether this site may reach your computer's local network: that is the bridge.</li>
          </ol>
        )
      )}
    </div>
  );
}
