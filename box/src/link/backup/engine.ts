import { exportTranscript, savedMessages, transcriptDatabase } from "../../agents/transcript";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { DATA_DECLARATIONS } from "../../shared/data-declarations";
import { backupFolderVersion } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { swapUnderWay } from "../../shared/engine-swap";
import { updateUnderWay } from "../../shared/updating";
import { held, ownSign } from "../documents/format";
import { exclusiveWhile, locked, privateFolder, writeDurablePrivate } from "../files";
import { APP_FOLDER, IS_MAC, K, RUN, homeFolder, located } from "../home";
import { QUEUES_LOCK } from "../work-queue/migrate";
import { macKeys, ownMacConfig, savedMacConfig } from "../mac/link";
import { portableApproval } from "../work/approval";
import { mayActivate } from "../ownership/resolve";
import { ownKey, peersOf } from "../peers/known";
import { leadershipDocuments, resolveCheckpoint } from "../setup/checkpoint";
import { followedSetup } from "../setup/record";
import { macLink, readLink, updateLink, type LinkState } from "../state";
import { backupDisk } from "../server/disk-worker";
import { archiveContext, newRecoveryKey, seal, type Entry } from "./archive";
import { backupSshAddress, destinationStore, expiredPoints, separateDisk, verifyBackupComputer, type Destination } from "./destination";
import { relayBackupAvailable } from "./relay-store";
import { projectEntries } from "./projects";
import { scrubEngine, snapshotDatabase } from "./sqlite";
import { attachmentKind, inside, queueBackupKind, receiptReference, recordReferences, roots, stat, storageInventory, type InventoryEntry } from "./locations";

export const BACKUP = path.join(K, "backup");
const CONFIG = path.join(BACKUP, "configuration.json");
export type Reason = "daily" | "wipe" | "update";
export const pointId = (reason: Reason | "vault", now: number) =>
  `${new Date(now).toISOString().replaceAll(":", "")}-${reason}-${crypto.randomBytes(8).toString("hex")}`;

export interface RestorePoint {
  id: string;
  at: number;
  version: number;
}
interface Config {
  version: 1;
  enabled: boolean;
  source: string;
  key: string;
  dest: Destination;
  disk?: number;
  projects: string[];
  last?: RestorePoint;
  lastDaily?: RestorePoint;
  error?: string;
}
export interface Manifest extends RestorePoint {
  format: 1;
  source: string;
  origin: string | null;
  signer: string | null;
  setup: string | null;
  paired: boolean;
  projects: string[];
}

export const bytes = (name: string, value: unknown): Entry => ({ name, bytes: Buffer.from(JSON.stringify(value)) });
export const backupLock = <T>(work: () => Promise<T>) => exclusiveWhile(path.join(BACKUP, ".operation"), async () => {
  for (const name of await fs.promises.readdir(privateFolder(BACKUP))) {
    if (name.startsWith(".scratch-")) await fs.promises.rm(path.join(BACKUP, name), { recursive: true, force: true });
  }
  return work();
});
export const scratchFolder = () => fs.mkdtempSync(path.join(privateFolder(BACKUP), ".scratch-"));

export function config(): Config | null {
  try {
    const value = JSON.parse(fs.readFileSync(CONFIG, "utf8"));
    if (value?.version !== 1) throw new Error("Unsupported backup configuration.");
    return value;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return null;
  }
}

export function usesAutomaticRelayBackup(): boolean {
  const kept = config();
  return Boolean(kept?.enabled && kept.dest.kind === "computer" && !kept.dest.ssh);
}

const save = (value: Config) => writeDurablePrivate(CONFIG, JSON.stringify(value), K);

function checkComputer(dest: Destination) {
  const link = readLink();
  if (dest.kind === "computer" && !peersOf(link).some(peer => peer.key === dest.computer)) {
    throw new Error("Choose a current member of this setup for backups.");
  }
}

export function selectedProjects(given: unknown): string[] {
  if (given === undefined) return [];
  if (!Array.isArray(given) || given.length > 100) throw new Error("Choose up to 100 project folders.");
  return [...new Set(given.map(folder => {
    if (typeof folder !== "string" || !path.isAbsolute(folder)) throw new Error("Choose project folders by their absolute paths.");
    const real = fs.realpathSync(folder);
    const dataRoots = [...Object.values(roots), APP_FOLDER].filter((root): root is string => Boolean(root)).map(located);
    if (!fs.statSync(real).isDirectory() || [...dataRoots, located(homeFolder()), located(os.tmpdir())].some(root => inside(real, root)) ||
      dataRoots.some(root => inside(root, real))) throw new Error("Choose individual project folders outside Kuvo’s data.");
    return real;
  }))];
}

export function configure(dest: Destination, projects: string[]) {
  return backupLock(async () => {
    checkComputer(dest);
    if (dest.kind === "disk" && !separateDisk(dest.path)) throw new Error("Choose a folder on a separate disk.");
    if (dest.kind === "disk" && projects.some(folder => inside(folder, dest.path) || inside(dest.path, folder))) {
      throw new Error("Choose a backup folder separate from the selected projects.");
    }
    const previous = config();
    const same = previous && JSON.stringify(previous.dest) === JSON.stringify(dest);
    const value: Config = {
      version: 1, enabled: true, source: previous?.source ?? "b_" + crypto.randomBytes(16).toString("base64url"),
      key: previous?.key ?? newRecoveryKey(), dest, projects,
      ...(dest.kind === "disk" ? { disk: fs.statSync(dest.path).dev } : {}),
      ...(same ? { last: previous.last, lastDaily: previous.lastDaily } : {}),
    };
    if (dest.kind === "computer") await verifyBackupComputer(dest);
    // A failed destination check leaves the old configuration in place.
    await destinationStore(dest, value.source).list();
    save(value);
    return { source: value.source, recoveryKey: value.key };
  });
}

export function disable() {
  return backupLock(async () => {
    const kept = config();
    if (kept) save({ ...kept, enabled: false });
  });
}

export function backupStatus() {
  const kept = config();
  const link = readLink();
  const peers = peersOf(link);
  const dest = kept?.dest;
  const computers = peers.flatMap(({ key, name, id }) => {
    const relay = relayBackupAvailable(key, link);
    const ssh = dest?.kind === "computer" && dest.computer === key && dest.ssh || (backupSshAddress(id) ? id : undefined);
    return relay || ssh ? [{ key, name, ...(relay ? { relay } : {}), ...(ssh ? { ssh } : {}) }] : [];
  });
  let destinationAvailable: boolean | null = null;
  if (kept?.dest.kind === "disk") {
    try { destinationAvailable = separateDisk(kept.dest.path) && fs.statSync(kept.dest.path).dev === kept.disk; }
    catch { destinationAvailable = false; }
  }
  if (dest?.kind === "computer") destinationAvailable = dest.ssh
    ? peers.some(peer => peer.key === dest.computer) : relayBackupAvailable(dest.computer, link);
  return {
    computer: link && ownKey(link), enabled: kept?.enabled ?? false,
    computers, defaultDestination: computers.length ? "computer" : "disk",
    destination: kept?.dest ?? null, source: kept?.source ?? null, projects: kept?.projects ?? [],
    destinationAvailable, lastVerifiedRestorePoint: kept?.last ?? null,
    ...(kept?.error ? { error: kept.error } : {}),
    ...(!kept?.last ? { restorePointStatus: "no separate backup" } : {}),
  };
}

function layout(insideUpdate: boolean): number {
  if (fs.existsSync(path.join(RUN, "update-trial")) || fs.existsSync(path.join(RUN, "update-restore")) ||
    swapUnderWay(RUN) || (!insideUpdate && updateUnderWay(RUN))) {
    throw new Error("Wait for the update to finish before backing up.");
  }
  return backupFolderVersion(K);
}

// A Mac keeps its keys in the serving process. The disk worker reads the
// membership under its lock and needs the same identity to interpret it.
type BackupIdentity = Pick<LinkState, "key" | "sign">;

function acceptedBoundary(identity?: BackupIdentity) {
  let result: { origin: string | null; signer: string | null; setup: string | null; paired: boolean; entries: Entry[]; link: ReturnType<typeof readLink> } | undefined;
  updateLink(kept => {
    const link = kept && identity ? { ...kept, ...identity } : kept;
    const setup = followedSetup(link);
    const origin = ownKey(link ?? {});
    const entries: Entry[] = [];
    if (setup?.role === "primary" && DATA_DECLARATIONS.setupLeadership.backup && DATA_DECLARATIONS.setupReplica.backup) {
      const docs = new Map<string, unknown>();
      for (const envelope of Object.values(link?.leadership ?? {})) {
        const certificate = held(envelope);
        if (!certificate || (certificate.body.checkpoint !== undefined && !resolveCheckpoint(certificate))) {
          throw new Error("The accepted setup checkpoint is incomplete.");
        }
        for (const doc of leadershipDocuments(envelope)) docs.set(doc.d, doc);
      }
      for (const envelope of Object.values(link?.docs ?? {})) {
        const doc = held(envelope);
        if (doc?.origin === ownSign(link) && (doc.kind === "peers" || doc.kind === "personalities" || doc.kind === "setup-shared")) docs.set(envelope.d, envelope);
      }
      const shared = link?.sharedSetup === setup.id ? Object.fromEntries(Object.entries(link?.shared ?? {})
        .filter(([kind]) => DATA_DECLARATIONS[kind as keyof typeof DATA_DECLARATIONS]?.backup)) : {};
      entries.push(bytes("meta/accepted.json", { setup, shared, outcomes: link?.sharedOutcomes, history: [...docs.values()] }));
    }
    result = { origin, signer: ownSign(link), setup: setup?.id ?? null, paired: peersOf(link).length > 0 || setup?.role === "member", entries, link };
  });
  return result!;
}

export function captureBackup(file: string, key: string, source: string, id: string, now: number,
  projects: readonly string[], scratch: string, insideUpdate = false, attachmentHashes?: ReadonlyMap<string, string>,
  identity?: BackupIdentity): Manifest {
  const version = layout(insideUpdate);
  const boundary = acceptedBoundary(identity);
  const manifest: Manifest = { format: 1, source, id, at: now, version, origin: boundary.origin, signer: boundary.signer, setup: boundary.setup,
    paired: boundary.paired, projects: projects.map(project => path.basename(project)) };
  const link = boundary.link;
  const sessionFor = (root: string, id: string) => {
    const base = roots[root];
    const file = base && path.join(base, id, "session");
    return file && stat(file)?.isFile() ? fs.readFileSync(file, "utf8").trim() : id;
  };
  const ownedHere = (name: string) => recordReferences(name, sessionFor).every(ref => mayActivate(ref, { link }));
  const expectedFiles = attachmentHashes && new Map([...attachmentHashes].filter(([name]) => ownedHere(name)));
  const checkAttachment = ({ name, kind }: InventoryEntry) => {
    if (expectedFiles && attachmentKind(kind) && !expectedFiles.has(name)) {
      throw new Error(`Attachment added during export: ${name}.`);
    }
  };
  const queue = locked(QUEUES_LOCK, () => [...storageInventory(roots, queueBackupKind)].map((entry, index) => {
    if (!entry.file) throw new Error(`Work Queue attachment missing or unreadable: ${entry.name}.`);
    checkAttachment(entry);
    const selected = expectedFiles?.get(entry.name);
    if (entry.expectedSHA !== undefined && selected !== undefined && selected !== entry.expectedSHA) {
      throw new Error(`Work Queue attachment digest differs from its record: ${entry.name}.`);
    }
    const file = path.join(scratch, `queue-${index}`);
    fs.cpSync(entry.file, file, { dereference: false, force: false, errorOnExist: true });
    if (entry.expectedSize !== undefined && fs.lstatSync(file).size !== entry.expectedSize) {
      throw new Error(`Work Queue attachment size differs from its record: ${entry.name}.`);
    }
    return { ...entry, file };
  }), { exclusive: true });
  function* entries(): Generator<Entry> {
    yield bytes("meta/manifest.json", manifest);
    yield bytes("meta/attachment-roots.json", { attachments: roots.attachments, macAttachments: roots.macAttachments });
    yield* boundary.entries;
    let db = 0;
    const transcripts = new Set<string>();
    yield* queue;
    for (const item of storageInventory(roots, kind => !queueBackupKind(kind))) {
      if (!ownedHere(item.name)) continue;
      const receipt = item.file && receiptReference(item.name, item.file);
      // A tentative move can abort. Preserve its fence until ownership actually changes.
      if (receipt && !mayActivate(receipt, { link, ...(receipt.kind === "draftSubmissionReplayFences" ? { moves: [] } : {}) })) continue;
      if (!item.file) throw new Error(`Backup payload missing or unreadable: ${item.name}.`);
      checkAttachment(item);
      if (item.expectedSize !== undefined && fs.lstatSync(item.file).size !== item.expectedSize) {
        throw new Error(`Backup payload size differs from its record: ${item.name}.`);
      }
      if (item.snapshot === "agent transcript" || /^agents\/sessions\/[^/]+\/messages\.json$/.test(item.name)) {
        const name = path.posix.join(path.posix.dirname(item.name), "messages.json");
        if (transcripts.has(name)) continue;
        transcripts.add(name);
        if (fs.existsSync(path.join(path.dirname(item.file), transcriptDatabase))) {
          const copy = path.join(scratch, `transcript-${db++}`);
          exportTranscript(copy, savedMessages(path.dirname(item.file)));
          yield { name, file: copy };
          fs.rmSync(copy);
        } else yield { ...item, name, file: item.file };
      } else if (item.snapshot !== "bytes") {
        const copy = path.join(scratch, `db-${db++}`);
        snapshotDatabase(item.file, copy);
        if (item.snapshot === "engine database") scrubEngine(copy, [], false);
        yield { name: item.name, file: copy };
        fs.rmSync(copy);
      } else if (item.name === "home/box/approval.json") {
        yield bytes(item.name, portableApproval(JSON.parse(fs.readFileSync(item.file, "utf8"))));
      } else yield { ...item, file: item.file };
    }
    yield* projectEntries(projects);
  }
  seal(file, key, entries(), archiveContext(source, id), expectedFiles);
  if (layout(insideUpdate) !== version) throw new Error("The folder layout changed during backup. Try again.");
  return manifest;
}

type ExportBoundary = { attachmentHashes: ReadonlyMap<string, string>; checkSource: () => void | Promise<void> };

async function writeVerifiedPoint(store: ReturnType<typeof destinationStore>, source: string, key: string, id: string,
  now: number, projects: string[], scratch: string, insideUpdate = false, boundary?: ExportBoundary): Promise<RestorePoint> {
  const file = path.join(scratch, "archive");
  await boundary?.checkSource();
  const link = IS_MAC ? readLink() : null;
  const identity = link?.key && link.sign ? { key: link.key, sign: link.sign } : undefined;
  const { version, setup } = await backupDisk.run("captureBackup", file, key, source, id, now, projects, scratch,
    insideUpdate, boundary?.attachmentHashes, identity);
  if (store.setup !== undefined && store.setup !== setup) throw new Error("Backup membership changed during capture.");
  await store.put(id, file);
  try { await backupDisk.run("authenticate", await store.get(id, scratch), key, archiveContext(source, id)); }
  catch (error) { await store.erase(id).catch(() => {}); throw error; }
  return { id, at: now, version };
}

export type ExportPoint = { source: string; id: string; at: number };
type ExportTarget = { destination: Destination; recoveryKey: string };

export function verifyExport(point: ExportPoint, target: ExportTarget, commit?: () => void): Promise<void> {
  return backupLock(async () => {
    const store = destinationStore(target.destination, point.source);
    const scratch = scratchFolder();
    try {
      await backupDisk.run("authenticate", await store.get(point.id, scratch), target.recoveryKey, archiveContext(point.source, point.id));
    } catch (error) {
      throw new Error("The backup could not be read back and verified. Check its destination and retry.", { cause: error });
    } finally { await fs.promises.rm(scratch, { recursive: true, force: true }); }
    commit?.();
  });
}

export async function verifyConfiguredWipe(point: ExportPoint, commit?: () => void): Promise<void> {
  const kept = config();
  if (!kept || kept.source !== point.source) throw new Error("The pre-wipe backup configuration changed.");
  if (kept.dest.kind === "disk" && fs.statSync(kept.dest.path).dev !== kept.disk) {
    throw new Error("Reconnect the selected backup disk.");
  }
  await verifyExport(point, { destination: kept.dest, recoveryKey: kept.key }, commit);
}

/** A one-time backup with its own retention and project selection. The caller journals the point
 * before using it, so a crash after publication resumes by authenticating the same archive. */
export function exportBeforeWipe(point: ExportPoint, target: ExportTarget, projects: string[], boundary?: ExportBoundary): Promise<void> {
  return backupLock(async () => {
    const { destination, recoveryKey } = target;
    checkComputer(destination);
    if (destination.kind === "computer") await verifyBackupComputer(destination);
    if (destination.kind === "disk" && projects.some(folder => inside(folder, destination.path) || inside(destination.path, folder))) {
      throw new Error("Choose an export folder separate from the selected projects.");
    }
    const scratch = scratchFolder();
    try {
      const store = destinationStore(destination, point.source);
      if ((await store.list()).includes(point.id)) {
        await backupDisk.run("authenticate", await store.get(point.id, scratch), recoveryKey, archiveContext(point.source, point.id));
      } else {
        await writeVerifiedPoint(store, point.source, recoveryKey, point.id, point.at, projects, scratch, false, boundary);
      }
    } finally { await fs.promises.rm(scratch, { recursive: true, force: true }); }
  });
}

export async function backupBefore(reason: Reason, { now = Date.now(), insideUpdate = false } = {}): Promise<RestorePoint | null> {
  if (!config()?.enabled) return null;
  return backupLock(async () => {
    const kept = config();
    if (!kept?.enabled) return null;
    const today = new Date(now).toISOString().slice(0, 10);
    if (reason === "daily" && kept.lastDaily?.id.startsWith(today) && !kept.error) return kept.lastDaily;
    let scratch: string | undefined;
    try {
      if (IS_MAC && kept.dest.kind === "computer" && !kept.dest.ssh && !macLink.overlay) {
        macLink.config ??= savedMacConfig() ?? ownMacConfig();
        if (!await macKeys({ needed: true }) || !macLink.overlay) throw new Error("The Mac’s Box identity is unavailable for Relay backup.");
      }
      checkComputer(kept.dest);
      if (kept.dest.kind === "disk" && fs.statSync(kept.dest.path).dev !== kept.disk) throw new Error("Reconnect the selected backup disk.");
      layout(insideUpdate);
      if (kept.dest.kind === "computer") await verifyBackupComputer(kept.dest);
      const store = destinationStore(kept.dest, kept.source);
      scratch = scratchFolder();
      const point = await writeVerifiedPoint(store, kept.source, kept.key, pointId(reason, now), now, kept.projects, scratch, insideUpdate);
      save({ ...kept, last: point, ...(reason === "daily" ? { lastDaily: point } : {}), error: undefined });
      for (const expired of expiredPoints(await store.list(), now)) await store.erase(expired);
      return point;
    } catch (error) {
      const current = config() ?? kept;
      save({ ...current, error: (error as Error).message });
      throw error;
    } finally {
      if (scratch) await fs.promises.rm(scratch, { recursive: true, force: true });
    }
  });
}

let timer: ReturnType<typeof setInterval> | undefined;
export function keepDailyBackups(): void {
  if (timer) return;
  const check = () => { void backupBefore("daily").catch(() => {}); };
  setTimeout(check, 60_000).unref();
  timer = setInterval(check, 15 * 60_000);
  timer.unref();
}

export async function erasePoint(dest: Destination, source: string, id: string) {
  return backupLock(async () => {
    await destinationStore(dest, source).erase(id);
    const kept = config();
    if (kept?.source === source && JSON.stringify(kept.dest) === JSON.stringify(dest)) {
      save({ ...kept, ...(kept.last?.id === id ? { last: undefined } : {}), ...(kept.lastDaily?.id === id ? { lastDaily: undefined } : {}) });
    }
  });
}
