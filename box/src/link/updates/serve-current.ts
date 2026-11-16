// This Mac's own box's server (`serve`), kept to the box's files as an update leaves them
// (PROTOCOL.md, "Updates"). Its helper (Kuvo Helper) starts it and starts it again only when it
// ends, and the keeper's stop doesn't reach it: so it says the update it runs on is well (nothing
// else does on a Mac, where no machine's bridge is welcomed by the relay), and it goes once the files
// it runs changed under it, for its helper to start the new ones.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { K } from "../home";
import { boxVersion, isUpdating } from "./update";

/** The box's files a server runs on: its version (`bin/box-version`) and its link's SHA-256
 * (`bin/kuvo-link.js`), null for one that can't be read. */
export interface ServedFiles {
  version: string | null;
  link: string | null;
}

export const servedLink = (root = K): string => path.join(root, "bin", "kuvo-link.js");

export function servedFiles(root = K): ServedFiles {
  let link: string | null = null;
  try { link = crypto.createHash("sha256").update(fs.readFileSync(servedLink(root))).digest("hex"); } catch {}
  return { version: boxVersion(), link };
}

/** What a server that started on `started` does now: "leave" once the files it runs aren't what it
 * started on any more, no update being under way (one under way changes them as it goes, and may
 * put them back); otherwise "stay". `leaves`: whether it may go (one whose helper starts it again).
 * The rest: a test's. Whether an update is well isn't a server's to say (updates/health.ts). */
export function serveCheck(
  started: ServedFiles,
  { leaves = true, updating = isUpdating, now = servedFiles }: { leaves?: boolean; updating?: () => boolean; now?: () => ServedFiles } = {},
): "leave" | "stay" {
  if (updating() || !leaves) return "stay";
  const files = now();
  // (A file missing for a moment, as one is swapped in: not a change yet.)
  return files.version && files.link && (files.version !== started.version || files.link !== started.link) ? "leave" : "stay";
}
