// Handing a draft to whatever sends it: a file, the clipboard, or a mail
// client through mailto:. No DOM here, so vitest runs it.

import type { Draft } from "../protocol";

/** A file name for the draft: what kind, and a part of its Message-ID. */
export function emlName(d: Draft): string {
  return `${d.kind}-${d.msgid.split("@")[0].replace(/[^A-Za-z0-9.-]/g, "_")}.eml`;
}

/**
 * A mailto: link that opens the draft in a mail client (RFC 6068), with
 * In-Reply-To so it threads where the client honours it; null when it is
 * too long for one, which clients and browsers cut short.
 */
export function mailtoHref(d: Draft, max = 1800): string | null {
  if (!d.to?.length) return null;
  const q = (s: string) => encodeURIComponent(s);
  const params = [`subject=${q(d.subject ?? "")}`];
  if (d.cc?.length) params.push(`cc=${q(d.cc.join(","))}`);
  if (d.in_reply_to) params.push(`in-reply-to=${q(`<${d.in_reply_to}>`)}`);
  params.push(`body=${q(d.body ?? "")}`);
  const href = `mailto:${d.to.map(q).join(",")}?${params.join("&")}`;
  return href.length > max ? null : href;
}
