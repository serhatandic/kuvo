// What the vault did, kept by the box and never by an agent (PROTOCOL.md, "Vault"): every use, ask,
// answer, refusal and change, without a value, in `box/vault/activity.jsonl`, kept a year.

import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { newID, oneLine } from "../../shared/text";
import type { VaultActivity } from "../../shared/routes/vault";
import { KEPT_DAYS, VAULT_FOLDER } from "../home";
import { vaultWork } from "./work";

const ACTIVITY = path.join(VAULT_FOLDER, "activity.jsonl");
const DAY = 86_400_000;
const activityFormat = additiveObjectFormat("Vault activity");

/** A line as it's kept: with who did it on a client (`by`: a phone's link key, a Mac's), which
 * stays here; a client hears only whether it was itself (`mine`). */
export type KeptActivity = Omit<VaultActivity, "mine"> & { by?: string };

/** What's noted: everything but its id and time. */
export type Noted = Omit<KeptActivity, "id" | "at">;

// When the old lines were last dropped, by this process.
let prunedAt = 0;

/** A line of activity, kept. */
export function note(what: Noted, now = Date.now()): KeptActivity {
  const entry: KeptActivity = { id: newID("a_"), at: now, ...what };
  if (entry.why) entry.why = oneLine(entry.why, 300);
  vaultWork(() => {
    fs.mkdirSync(VAULT_FOLDER, { recursive: true, mode: 0o700 });
    const fd = fs.openSync(ACTIVITY, "a+", 0o600);
    try {
      const size = fs.fstatSync(fd).size;
      const tail = Buffer.alloc(1);
      if (size) fs.readSync(fd, tail, 0, 1, size - 1);
      const separator = size && tail[0] !== 10 ? "\n" : "";
      fs.writeFileSync(fd, separator + JSON.stringify(activityFormat.write(entry)) + "\n");
    } finally {
      fs.closeSync(fd);
    }
    if (now - prunedAt > DAY) {
      prunedAt = now;
      try { prune(now); } catch (error) {
        // Appending still works with a newer line present. Only compaction must stop,
        // since it would have to interpret or discard an entry this build cannot read.
        if (!(error instanceof UnsupportedFormatVersion)) throw error;
      }
    }
  });
  return entry;
}

// Lines older than a year, dropped.
function prune(now: number) {
  const lines = readLines();
  const kept = lines.filter(entry => now - entry.at < KEPT_DAYS * DAY);
  if (kept.length === lines.length) return;
  const temporary = ACTIVITY + ".new";
  fs.writeFileSync(temporary, kept.map(entry => JSON.stringify(activityFormat.write(entry)) + "\n").join(""), { mode: 0o600 });
  fs.renameSync(temporary, ACTIVITY);
}

function readLines(): KeptActivity[] {
  let text = "";
  try { text = fs.readFileSync(ACTIVITY, "utf8"); } catch { return []; }
  const entries: KeptActivity[] = [];
  for (const line of text.split("\n")) {
    if (!line) continue;
    let saved: unknown;
    try { saved = JSON.parse(line); } catch { continue; }
    if (isObject(saved)) entries.push(activityFormat.read(saved) as KeptActivity);
  }
  return entries;
}

/** What a client filters by: an item, a bot or a session; older than `before`; at most `limit`. */
export interface ActivityFilter {
  item?: string;
  bot?: string;
  session?: string;
  before?: number;
  limit?: number;
}

/** The activity, newest first. */
export function readActivity({ item, bot, session, before, limit = 100 }: ActivityFilter = {}, now = Date.now()): KeptActivity[] {
  return readLines()
    .filter(entry => now - entry.at < KEPT_DAYS * DAY)
    .filter(entry => (!item || entry.item === item) && (!bot || entry.bot === bot) && (!session || entry.session === session) && (!before || entry.at < before))
    .sort((a, b) => b.at - a.at)
    .slice(0, Math.max(1, Math.min(500, limit)));
}
