import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { UsageSettings } from "../../usage/settings";
import { APP_FOLDER, BOTS, CHATS, ENGINE_DATA, K } from "../home";
import { readPersonalities } from "../personalities/library";
import { databasesIn } from "../rehome/database";
import type { LinkState } from "../state";
import { labels } from "../work-queue/labels";
import { readNumbers } from "../work-queue/numbers";
import { allItems } from "../work-queue/store";
import { readZoneSetting } from "../zone/setting";

function entries(folder: string): string[] {
  try { return fs.readdirSync(folder); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
}

function engineHasSessions(folder: string): boolean {
  if (entries(path.join(folder, "storage", "session")).length) return true;
  for (const file of databasesIn(folder)) {
    const require = createRequire(import.meta.url);
    const bun = !!process.versions.bun;
    const SQLite = bun ? require(["bun", "sqlite"].join(":")).Database : require("node:sqlite").DatabaseSync;
    const database = new SQLite(file, bun ? { readonly: true } : { readOnly: true });
    try {
      for (const table of ["session", "session_v2"]) {
        if (database.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?").get(table)
          && database.prepare("SELECT 1 FROM " + table + " LIMIT 1").get()) return true;
      }
    } finally { database.close(); }
  }
  return false;
}

/** Only an unused or explicitly unlinked primary can take ordinary enrollment.
 * This is checked again under the enrollment lock; a card is just a hint. */
export function hasSetupWork(link: LinkState | null, { legacyPairings = false } = {}): boolean {
  if (!legacyPairings && (link?.phones?.length || link?.preJoinPairings?.length || link?.removed?.length)
    || link?.sharedSetup && link.sharedSetup === link.setup?.id && Object.keys(link.shared ?? {}).length) return true;
  try {
    const numbers = readNumbers();
    if (allItems().length || (numbers.next ?? 1) > 1 || numbers.leases.length || numbers.used?.length || Object.keys(numbers.given ?? {}).length) return true;
    if (link?.releasedSetup && link.releasedSetup === link.setup?.id) return false;
    const library = readPersonalities();
    const usage = new UsageSettings(path.join(APP_FOLDER ?? K, "agents", "usage", "settings.json"));
    return Boolean(labels().length || library.personalities.length || library.removed.length || readZoneSetting()
      || Object.keys(usage.kept()).length || entries(path.join(APP_FOLDER ?? K, "agents", "sessions")).some(name => name.startsWith("ses_"))
      || entries(BOTS).some(name => fs.existsSync(path.join(BOTS, name, "bot.json")))
      || entries(CHATS).some(name => name !== ".DS_Store") || engineHasSessions(ENGINE_DATA));
  } catch {
    // A damaged or unreadable store is not evidence of an unused computer.
    return true;
  }
}
