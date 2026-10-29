// Which b4 prep branch posted a series: the cover message-ids b4 records
// for each version it sent, or else the series' b4 change-id. No DOM here.

import type { Branch } from "../store";

/** The branch that posted one of these versions (by root), or carries this change-id. */
export function branchFor(roots: string[], key: string, branches: Branch[]): Branch | undefined {
  return (
    branches.find((b) => b.sent.some((s) => roots.includes(s.cover))) ?? branches.find((b) => b.change_id !== "" && b.change_id === key)
  );
}

/** Where a branch stands against what was sent, as `patchtrack branches` says it. */
export function branchState(b: Branch): string {
  const last = b.sent[b.sent.length - 1];
  if (!last) return `v${b.revision} never sent`;
  return b.revision > last.version ? `v${b.revision} in preparation; v${last.version} sent` : `as sent in v${last.version}`;
}
