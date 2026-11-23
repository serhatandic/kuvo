import { supportsLegacyObjectVersion } from "../../shared/format-version";
import fs from "node:fs";
import { createHash, type Hash } from "node:crypto";
import { createRequire } from "node:module";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { queueFiles as sessionQueueFiles, readQueues, showsQueue } from "../../shared/queues";
import { isObject } from "../../shared/json";
import { UsageSettings } from "../../usage/settings";
import { APP_FOLDER, attachmentRoots, BOTS, CHATS, ENGINE_DATA, GROUPS, K, LINK_FILE, RUN, SESSION_WAKES } from "../home";
import { OUTBOX, SENDING } from "../letters/letter";
import { installerInvitesLive } from "../add/invite-state";
import { readPersonalities, PERSONALITIES_FILE } from "../personalities/library";
import type { LinkState } from "../state";
import { LABELS_FILE, localLabels } from "../work-queue/labels";
import { NUMBERS_FILE, readNumbers } from "../work-queue/numbers";
import { FORWARD_FILE, FORWARDS, pendingForward } from "../work-queue/forward";
import { queueFileFormat } from "../work-queue/migrate";
import { WORK_QUEUE } from "../work-queue/store";
import { readBackground, WORK_BACKGROUND } from "../work/background";
import { WORK_ASKS } from "../work/asks";
import { WORK_WAKES } from "../work/wakes";
import { hasBootstrapMembership } from "./membership";
import { readZoneSetting, ZONE_FILE } from "../zone/setting";

function entries(folder: string): string[] {
  try { return fs.readdirSync(folder).sort(); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
}

const agents = process.env.KUVO_AGENTS_HOME || path.join(APP_FOLDER ?? K, "agents");
const engine = process.env.KUVO_ENGINE_DB_FOLDER || ENGINE_DATA;
const storage = path.join(engine, "storage");
const usageFile = path.join(agents, "usage", "settings.json");
// Read when used: these come from modules that import this one through a cycle, so they may not
// be set yet while this module loads (restoring a backup loads it that way).
const categoriesFile = () => path.join(WORK_QUEUE, "categories.json");
const settings = () => [PERSONALITIES_FILE, LABELS_FILE, categoriesFile(), ZONE_FILE, usageFile];
const sessionMetadata = ["session-settings.json", "named.json", "archived.json"].map(file => path.join(RUN, file));
const sessionQueues = sessionQueueFiles(RUN);

const payloads = [path.join(agents, "sessions"), CHATS, GROUPS,
  SESSION_WAKES, process.env.KUVO_ARTIFACTS || path.join(K, "artifacts"), ...attachmentRoots(), path.join(WORK_QUEUE, "files"), path.join(RUN, "work", "notes")];

function databases(): string[] {
  return entries(engine).filter(name => name.startsWith("opencode") && name.endsWith(".db")).map(name => path.join(engine, name));
}

function databaseIdentity(file: string) {
  const info = (name: string) => {
    const stat = fs.lstatSync(name, { throwIfNoEntry: false, bigint: true });
    if (stat && !stat.isFile()) throw new Error("Invalid engine database.");
    return stat;
  };
  const main = info(file);
  if (!main) throw new Error("Missing engine database.");
  info(file + "-shm");
  return { main, wal: info(file + "-wal"), journal: info(file + "-journal") };
}

function openDatabase(file: string) {
  const identity = databaseIdentity(file);
  // Bun cannot read a closed WAL-mode database without sidecars. Immutable reads
  // are safe here only for an empty journal and with a physical recheck.
  const immutable = !identity.wal?.size && !identity.journal?.size;
  const source = immutable ? pathToFileURL(file).href + "?mode=ro&immutable=1" : file;
  const require = createRequire(import.meta.url);
  const bun = process.versions.bun
    ? require(["bun", "sqlite"].join(":")) as typeof import("bun:sqlite")
    : undefined;
  // Bun on Linux passes a URI through to SQLite only with SQLITE_OPEN_URI.
  const db = (bun
    ? new bun.Database(source, bun.constants.SQLITE_OPEN_READONLY | (immutable ? bun.constants.SQLITE_OPEN_URI : 0))
    : new (require("node:sqlite").DatabaseSync)(source, { readOnly: true })) as import("node:sqlite").DatabaseSync;
  return { db, identity, immutable };
}

const sqlName = (name: string) => '"' + name.replaceAll('"', '""') + '"';
const DB_SCAN_BYTES = 64n * 1024n * 1024n;
const DB_SCAN_MS = 1500;
class DatabaseScanLimit extends Error {}

/** Hash the committed logical contents. Checkpoints can move pages between the main file and WAL,
 * and a rebuild can change row order. Neither changes this fingerprint. */
function stampDatabase(hash: Hash, file: string, deadline: number) {
  const { db, identity } = openDatabase(file);
  try {
    const version = db.prepare("PRAGMA data_version");
    const before = version.get()?.data_version;
    db.exec("PRAGMA busy_timeout=0; PRAGMA trusted_schema=OFF; BEGIN");
    const schema = db.prepare("SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name").all()
      .filter(row => typeof row.tbl_name === "string" && !/^sqlite_stat[1-4]$/.test(row.tbl_name));
    hash.update(JSON.stringify([file, schema, db.prepare("PRAGMA user_version").get(), db.prepare("PRAGMA application_id").get()]));
    for (const { type, name } of schema) {
      if (type !== "table" || typeof name !== "string") continue;
      // Encode values by SQLite storage type, including 64-bit integers and raw text/blob bytes.
      const columns = db.prepare("SELECT name FROM pragma_table_xinfo(?) ORDER BY cid").all(name);
      const expressions = columns.map(({ name }, index) => {
        if (typeof name !== "string") throw new Error("Invalid engine database column.");
        const column = sqlName(name);
        return `(CASE typeof(${column})
          WHEN 'null' THEN 'n' WHEN 'integer' THEN 'i' || CAST(${column} AS TEXT)
          WHEN 'real' THEN 'r' || printf('%!.26g', ${column})
          WHEN 'text' THEN 't' || hex(${column}) ELSE 'b' || hex(${column}) END) AS v${index}`;
      });
      hash.update(JSON.stringify([name]));
      const rows: string[] = [];
      for (const row of db.prepare(`SELECT ${expressions.join(", ")} FROM ${sqlName(name)}`).iterate()) {
        if (performance.now() > deadline) throw new DatabaseScanLimit();
        rows.push(createHash("sha256").update(JSON.stringify(Object.values(row))).digest("base64url"));
      }
      for (const row of rows.sort()) hash.update(row);
      if (performance.now() > deadline) throw new DatabaseScanLimit();
    }
    db.exec("COMMIT");
    const after = fs.lstatSync(file, { bigint: true });
    if (version.get()?.data_version !== before || after.dev !== identity.main.dev || after.ino !== identity.main.ino) {
      throw new Error("The engine database changed while checking retained work.");
    }
  } finally { db.close(); }
}

function engineHasSessions(files: string[]): boolean {
  if (entries(path.join(storage, "session")).length) return true;
  for (const file of files) {
    const { db, identity, immutable } = openDatabase(file);
    try {
      for (const table of ["session", "session_v2"]) {
        if (db.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?").get(table)
          && db.prepare("SELECT 1 FROM " + table + " LIMIT 1").get()) return true;
      }
    } finally { db.close(); }
    if (immutable) {
      const after = databaseIdentity(file);
      const { main } = identity;
      if (after.wal?.size || after.journal?.size || after.main.dev !== main.dev || after.main.ino !== main.ino
        || after.main.size !== main.size || after.main.ctimeNs !== main.ctimeNs) throw new Error("The engine database changed while checking for sessions.");
    }
  }
  return false;
}

function jsonFile(file: string): Record<string, unknown> | undefined {
  try {
    const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK | fs.constants.O_NOFOLLOW);
    let value: unknown;
    try {
      if (!fs.fstatSync(fd).isFile()) throw new Error("Invalid setup work store.");
      value = JSON.parse(fs.readFileSync(fd, "utf8"));
    } finally { fs.closeSync(fd); }
    if (!isObject(value)) throw new Error("Invalid setup work store.");
    return value;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return undefined;
  }
}

const queues = path.join(WORK_QUEUE, "queues");
function queueFiles(): string[] {
  return entries(queues).filter(name => name.endsWith(".json")).map(name => path.join(queues, name));
}

function pendingOwnerWork(): boolean {
  return ["outbox", "edit-outbox"].some(family => {
    const folder = path.join(RUN, "owned-data", family);
    return entries(folder).filter(name => name.endsWith(".json")).some(name => {
      const record = jsonFile(path.join(folder, name));
      return record && (record.envelope || record.operation || !isObject(record.receipt) || record.receipt.status === "pending");
    });
  });
}

export function setupQueueItems(): { active: number; kept: number } {
  const byID = new Map<string, { removed: boolean; updated: number }>();
  for (const file of queueFiles()) {
    const queue = jsonFile(file);
    const items = queue && queueFileFormat.read(queue).items;
    if (!Array.isArray(items)) throw new Error("Invalid Work Queue store.");
    for (const item of items) {
      if (!isObject(item) || typeof item.id !== "string") throw new Error("Invalid Work Queue item.");
      const updated = isObject(item.updated) && typeof item.updated.at === "number" ? item.updated.at : 0;
      if (updated >= (byID.get(item.id)?.updated ?? 0)) byID.set(item.id, { removed: Boolean(item.removed), updated });
    }
  }
  return { active: [...byID.values()].filter(item => !item.removed).length, kept: byID.size };
}

function pendingSessionWork(): boolean {
  const asks = jsonFile(WORK_ASKS);
  if (asks && (!Array.isArray(asks.asks) || asks.asks.some(ask => !isObject(ask) || !ask.answered || ask.late && !ask.toldLate))) return true;
  jsonFile(sessionQueues.file);
  jsonFile(WORK_BACKGROUND);
  const forwards = entries(FORWARDS);
  const waiting = forwards.some(name => {
    const id = FORWARD_FILE.exec(name)?.[1];
    if (!id) return false;
    const reply = id + ".answer.json";
    if (forwards.includes(reply)) {
      const answer = jsonFile(path.join(FORWARDS, reply));
      return !answer || !supportsLegacyObjectVersion(answer);
    }
    return pendingForward(jsonFile(path.join(FORWARDS, name)));
  });
  return waiting || [OUTBOX, sessionQueues.waiting].some(folder => entries(folder).some(name => name.endsWith(".json")))
    || entries(SENDING).some(name => name.endsWith(".json") || name.endsWith(".failed"))
    || Object.keys(jsonFile(WORK_WAKES) ?? {}).length > 0 || Object.keys(readBackground()).length > 0
    || Object.values(readQueues(sessionQueues, true)).some(state => showsQueue(state) || state.handoff?.length || state.inboxMoved?.length);
}

function workInventory(omitUsage = false) {
  const hash = createHash("sha256");
  function stamp(file: string, stable?: Hash, skipEmpty = false): bigint {
    const info = fs.lstatSync(file, { throwIfNoEntry: false, bigint: true });
    if (!info) return 0n;
    if (info.isDirectory()) {
      for (const name of entries(file)) {
        if (name !== ".DS_Store" && !(path.dirname(file) === BOTS && name === "workspace")) stamp(path.join(file, name));
      }
      return 0n;
    }
    if (!info.isFile()) throw new Error("Invalid setup work store.");
    fs.accessSync(file, fs.constants.R_OK);
    const entry = JSON.stringify([file, info.ino.toString(), info.size.toString(), info.ctimeNs.toString()]);
    hash.update(entry);
    if (!skipEmpty || info.size > 0n) stable?.update(entry);
    return info.size;
  }
  for (const file of [...settings(), ...sessionMetadata, ...queueFiles(), BOTS, storage, ...payloads]) if (!omitUsage || file !== usageFile) stamp(file);
  const content = hash.copy();
  const stable = hash.copy();
  const files = databases();
  let size = 0n;
  for (const file of files) {
    const { journal } = databaseIdentity(file);
    // A read-only SQLite connection can create an empty WAL.
    size += stamp(file, stable) + stamp(file + "-wal", stable, true);
    if (journal?.size) {
      stable.update(JSON.stringify([file + "-journal", journal.ino.toString(), journal.size.toString(), journal.ctimeNs.toString()]));
      size += journal.size;
    }
  }
  return { physical: hash.digest("base64url"), stability: stable.digest("base64url"), content, files, size, omitUsage };
}

let lastDatabaseStamp: { stability: string; logical: string } | undefined;
function databaseStamp(inventory: ReturnType<typeof workInventory>) {
  const { physical, stability, files, size, content } = inventory;
  if (lastDatabaseStamp?.stability === stability) return { physical, logical: lastDatabaseStamp.logical };
  if (size > DB_SCAN_BYTES) return undefined;
  const deadline = performance.now() + DB_SCAN_MS;
  try {
    for (const file of files) stampDatabase(content, file, deadline);
  } catch (error) {
    if (error instanceof DatabaseScanLimit) return undefined;
    throw error;
  }
  // The link lock doesn't cover the database, session, or bot writers.
  const after = workInventory(inventory.omitUsage);
  if (after.stability !== stability) throw new Error("Retained work changed during the check.");
  const logical = content.digest("base64url");
  lastDatabaseStamp = { stability, logical };
  return { physical: after.physical, logical };
}

function usageWork() {
  // Validate the JSON before UsageSettings reads it. Its ordinary UI reader can
  // recover from malformed files, but enrollment must keep unreadable work.
  jsonFile(usageFile);
  const usage = new UsageSettings(usageFile);
  return { scope: usage.setup, digest: usage.digest(), imported: usage.imported };
}

function workStamp(omitUsage: boolean) {
  const inventory = workInventory(omitUsage);
  const stamped = databaseStamp(inventory);
  return { physical: stamped?.physical ?? inventory.physical, logical: stamped?.logical };
}

/** Keep the historical stamp for earlier boxes. The companion compares the
 * inventory separately from Usage's setup-scoped projection. */
export function releasedWorkBaseline(): Pick<LinkState, "releasedWork" | "releasedDatabaseWork" | "releasedScopedWork"> {
  try {
    const before = workStamp(false);
    return {
      releasedWork: before.physical, releasedDatabaseWork: before.logical,
      releasedScopedWork: { ...workStamp(true), usage: usageWork() },
    };
  } catch {
    return { releasedWork: null };
  }
}

function scopedWorkChanged(baseline: NonNullable<LinkState["releasedScopedWork"]>): boolean {
  const before = baseline.usage;
  const now = usageWork();
  const same = now.digest === before.digest && now.imported === before.imported;
  // Usage resets on a scope change, except the first time it binds unscoped settings.
  const reset = before.scope !== undefined && now.scope !== before.scope
    && !now.imported && now.digest === new UsageSettings().digest();
  if (!same && !reset) return true;
  const inventory = workInventory(true);
  return inventory.physical !== baseline.physical
    && (!baseline.logical || databaseStamp(inventory)?.logical !== baseline.logical);
}

/** The probe and both ordinary enrollment paths use the same work test. Historical
 * revocations grant no access and do not make a setup occupied. */
export function hasSetupWork(link: LinkState | null): boolean {
  if (link?.phones?.length || link?.preJoinPairings?.length || link?.setupMerge
    || installerInvitesLive(link) || (link?.pairing?.issuer !== undefined && link.pairing.expires > Date.now())
    || (link?.appPairing?.expires ?? 0) > Date.now()
    || link?.sharedSetup && link.sharedSetup === link.setup?.id && Object.keys(link.shared ?? {}).length && !hasBootstrapMembership(link)
    || link?.untold?.length) return true;
  try {
    jsonFile(LINK_FILE);
    jsonFile(NUMBERS_FILE);
    const numbers = readNumbers();
    if ((numbers.next ?? 1) > 1 || numbers.leases.length || numbers.used?.length || numbers.asking
      || Object.keys(numbers.given ?? {}).length || Object.keys(numbers.granted ?? {}).length || pendingOwnerWork()
      || pendingSessionWork() || setupQueueItems().kept) return true;
    for (const file of settings()) jsonFile(file);
    const files = databases();
    const library = readPersonalities();
    const labels = localLabels();
    const hasWork = labels.labels.length || labels.removed.length || !!jsonFile(categoriesFile())
      || library.personalities.length || library.removed.length || readZoneSetting()
      || Object.keys(new UsageSettings(usageFile).kept()).length
      || sessionMetadata.some(file => Object.keys(jsonFile(file) ?? {}).length)
      || payloads.some(folder => entries(folder).some(name => name !== ".DS_Store"))
      || entries(BOTS).some(name => {
        const folder = path.join(BOTS, name);
        const info = fs.lstatSync(folder, { throwIfNoEntry: false });
        return info?.isSymbolicLink() || info?.isDirectory() && entries(folder).includes("bot.json");
      })
      || engineHasSessions(files);
    if (link?.releasedSetup === link?.setup?.id && link?.releasedWork === null) return true;
    if (link?.releasedSetup === link?.setup?.id && link?.releasedWork) {
      if (link.releasedScopedWork) return scopedWorkChanged(link.releasedScopedWork);
      // Compare even if the work is now empty: a deletion after unlink is also a change.
      const inventory = workInventory();
      return inventory.physical !== link.releasedWork && (!link.releasedDatabaseWork || databaseStamp(inventory)?.logical !== link.releasedDatabaseWork);
    }
    return Boolean(hasWork);
  } catch {
    // A damaged or unreadable store is not evidence of an unused computer.
    return true;
  }
}
