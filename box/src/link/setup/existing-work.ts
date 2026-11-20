import fs from "node:fs";
import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import path from "node:path";
import { queueFiles as sessionQueueFiles, readQueues, showsQueue } from "../../shared/queues";
import { isObject } from "../../shared/json";
import { UsageSettings } from "../../usage/settings";
import { APP_FOLDER, attachmentRoots, BOTS, CHATS, ENGINE_DATA, K, LINK_FILE, RUN, SESSION_WAKES } from "../home";
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
const categoriesFile = path.join(WORK_QUEUE, "categories.json");
const settings = [PERSONALITIES_FILE, LABELS_FILE, categoriesFile, ZONE_FILE, usageFile];
const sessionMetadata = ["session-settings.json", "named.json", "archived.json", "work/goals.json"].map(file => path.join(RUN, file));
const sessionQueues = sessionQueueFiles(RUN);

const payloads = [path.join(agents, "sessions"), CHATS, process.env.KUVO_GROUPS || path.join(APP_FOLDER ?? K, "groups"),
  SESSION_WAKES, process.env.KUVO_ARTIFACTS || path.join(K, "artifacts"), ...attachmentRoots(), path.join(WORK_QUEUE, "files"), path.join(RUN, "work", "notes")];

function databases(): string[] {
  return entries(engine).filter(name => name.startsWith("opencode") && name.endsWith(".db")).map(name => path.join(engine, name));
}

function engineHasSessions(files: string[]): boolean {
  if (entries(path.join(storage, "session")).length) return true;
  for (const file of files) {
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
    return id && !forwards.includes(id + ".answer.json") && pendingForward(jsonFile(path.join(FORWARDS, name)));
  });
  return waiting || [OUTBOX, sessionQueues.waiting].some(folder => entries(folder).some(name => name.endsWith(".json")))
    || entries(SENDING).some(name => name.endsWith(".json") || name.endsWith(".failed"))
    || Object.keys(jsonFile(WORK_WAKES) ?? {}).length > 0 || Object.keys(readBackground()).length > 0
    || Object.values(readQueues(sessionQueues, true)).some(state => showsQueue(state) || state.handoff?.length || state.inboxMoved?.length);
}

/** Record the work intentionally retained by a committed unlink. A changed file, an uncertain
 * read, or an older release marker without a baseline requires a merge when work is present. */
export function releasedWorkStamp(): string | undefined {
  try {
    const hash = createHash("sha256");
    function stamp(file: string): void {
      const info = fs.lstatSync(file, { throwIfNoEntry: false, bigint: true });
      if (!info) return;
      if (info.isDirectory()) {
        for (const name of entries(file)) {
          if (name !== ".DS_Store" && !(path.dirname(file) === BOTS && name === "workspace")) stamp(path.join(file, name));
        }
      } else {
        if (!info.isFile()) throw new Error("Invalid setup work store.");
        fs.accessSync(file, fs.constants.R_OK);
        hash.update(JSON.stringify([file, info.ino.toString(), info.size.toString(), info.ctimeNs.toString()]));
      }
    }
    for (const file of [...settings, ...sessionMetadata, ...queueFiles(), BOTS, storage, ...payloads,
      ...databases().flatMap(file => [file, file + "-wal"])]) stamp(file);
    return hash.digest("base64url");
  } catch {
    return undefined;
  }
}

/** The probe and both ordinary enrollment paths use the same work test. Historical
 * revocations grant no access and do not make a setup occupied. */
export function hasSetupWork(link: LinkState | null): boolean {
  if (link?.phones?.length || link?.preJoinPairings?.length || link?.setupMerge || link?.leadershipPending
    || installerInvitesLive(link) || (link?.pairing?.issuer !== undefined && link.pairing.expires > Date.now())
    || (link?.appPairing?.expires ?? 0) > Date.now()
    || link?.sharedSetup && link.sharedSetup === link.setup?.id && Object.keys(link.shared ?? {}).length
    || Object.keys(link?.sharedPending ?? {}).length || link?.untold?.length) return true;
  try {
    jsonFile(LINK_FILE);
    jsonFile(NUMBERS_FILE);
    const numbers = readNumbers();
    if ((numbers.next ?? 1) > 1 || numbers.leases.length || numbers.used?.length || numbers.asking || numbers.allocationSave
      || Object.keys(numbers.given ?? {}).length || Object.keys(numbers.granted ?? {}).length || pendingOwnerWork()
      || pendingSessionWork() || setupQueueItems().kept) return true;
    for (const file of settings) jsonFile(file);
    const files = databases();
    const library = readPersonalities();
    const labels = localLabels();
    const hasWork = labels.labels.length || labels.removed.length || !!jsonFile(categoriesFile)
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
      // Compare even if the work is now empty: a deletion after unlink is also a change.
      return releasedWorkStamp() !== link.releasedWork;
    }
    return Boolean(hasWork);
  } catch {
    // A damaged or unreadable store is not evidence of an unused computer.
    return true;
  }
}
