// The engine's database brought up to date in the staged copy (#713 §4.2): Kuvo Code 0.7.2 and
// 0.8.0-beta.1 ran engine 2.0.32, whose database the box's pinned engine migrates as it starts.
// Nothing else could: the schema is the engine's own. So the release's engine is started once on
// the staged data, with nothing else of yours (settings, home, state and cache of its own, nowhere
// to reach, as the check's is), and stopped as soon as it answers. The app's own database is never
// opened for writing: only its clone.
//
// It doesn't act on what it finds: a run that was cut short is marked suspended (as the engine does
// at every start), and messages still waiting stay waiting (tried with 2.2.6 on 2.0.32's).
// COMPAT(0.7 move): has the pinned engine bring the database of the engine Kuvo Code 0.7.2 and 0.8.0-beta.1 ran (2.0.32) up to date in the move's staged copy. Goes on the user's word, once no 0.7 Mac is expected to update.

import fs from "node:fs";
import path from "node:path";
import { databasesIn, KNOWN_SCHEMAS, schemaMark, tableCounts } from "./database";
import { startIsolated } from "./engine-check";

/** The engine databases' schemas this box's engine brings up to date itself (`<count> <last
 * migration>`): what each released Kuvo Code's engine left. Another (a newer engine's than the
 * box's, or one nobody tried) is refused before anything's cloned.
 * - `50 20261010180456_project_time_archived`: 2.0.32 (Kuvo Code 0.7.2, 0.8.0-beta.1). */
export const MIGRATED_SCHEMAS: readonly string[] = ["50 20261010180456_project_time_archived"];

/** Whether a schema is one the move takes: the pinned engine's own, or one it brings up to date. */
export const takenSchema = (mark: string | null): boolean => mark === null || KNOWN_SCHEMAS.includes(mark) || MIGRATED_SCHEMAS.includes(mark);

export interface Migrated { database: string; from: string; to: string }

/** Every staged engine database not yet of the pinned engine's schema, brought to it by `engine`
 * (the release's), in `data` (the staged `XDG_DATA_HOME`, with `opencode/` in it). Throws, in
 * words, when it isn't the pinned schema after, or holds fewer rows of anything it held before. */
export async function migrateDatabases({ engine, data, scratch }: { engine: string; data: string; scratch: string }): Promise<Migrated[]> {
  const folder = path.join(data, "opencode");
  const due = databasesIn(folder).map(database => ({ database, from: schemaMark(database) })).filter(each => each.from !== null && !KNOWN_SCHEMAS.includes(each.from));
  if (!due.length) return [];
  // (The engine opens its own channel's database, `opencode.db`: another channel's it wouldn't.)
  const other = due.find(each => path.basename(each.database) !== "opencode.db");
  if (other) throw new Error(`The engine's database ${path.basename(other.database)} is of an earlier version, and only opencode.db can be brought up to date.`);
  const [{ database, from }] = due;
  const before = tableCounts(database);
  fs.rmSync(scratch, { recursive: true, force: true });
  try {
    const started = await startIsolated({ engine, data, scratch, what: "your sessions (to bring them up to date)" });
    await started.stop();
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
  const to = schemaMark(database);
  if (to === null || !KNOWN_SCHEMAS.includes(to)) throw new Error(`The engine didn't bring your sessions up to date (they're at ${to ?? "nothing"}, from ${from}).`);
  // Nothing it held is gone: each table that's still there holds at least what it did.
  const after = tableCounts(database);
  const fewer = Object.entries(before).filter(([table, count]) => table in after && after[table] < count).map(([table, count]) => `${table} (${after[table]} of ${count})`);
  if (fewer.length) throw new Error(`Bringing your sessions up to date lost some of what they held: ${fewer.join(", ")}.`);
  return [{ database, from: from!, to }];
}
