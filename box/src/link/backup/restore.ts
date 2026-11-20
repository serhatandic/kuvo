import { permitsReplacement } from "../setup/computer-history";
import { recoveredQueueDecision } from "../work-queue/recovery";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { processRuns } from "../../shared/platform";
import { swapUnderWay } from "../../shared/engine-swap";
import { acceptedMembership } from "../setup/membership";
import { primaryFenced } from "../setup/lease";
import { followedSetup } from "../setup/record";
import { awaitSharedSave, pendingShared, sharedValue, stageShared, type Receipt } from "../setup/shared";
import { readLink, updateLink, type LinkState } from "../state";
import { ownSign } from "../documents/format";
import { mayActivate, replacementOwner, sessionsFencedHere } from "../ownership/resolve";
import { recordKey } from "../ownership/store";
import { fingerprint } from "../ownership/merge";
import { forwardAct, type Act, type Acted } from "../work-queue/forward";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { syncDurableFile, writeDurablePrivate } from "../files";
import { K, RUN } from "../home";
import { archiveContext, openArchive } from "./archive";
import { destinationStore, POINT, SOURCE, type Destination } from "./destination";
import { BACKUP, backupLock, scratchFolder, type Manifest } from "./engine";
import { prepareEngineBootstrap, scrubEngine } from "./sqlite";
import { outputPath, recordReferences, roots, stat, storageOf } from "./locations";
import { restoreProjects } from "./projects";
import { stageRestore } from "./stage";
import { portableApproval } from "../work/approval";

const JOURNAL = path.join(BACKUP, "restore.json");
const RESTORING = path.join(BACKUP, "restoring");

function json(file: string): unknown {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    return undefined;
  }
}

const validKey = (key: unknown): key is string => typeof key === "string" && KEY.test(key);
const backupSource = (value: unknown): value is string => typeof value === "string" && SOURCE.test(value);
const live = (link: LinkState | null, key: string) => acceptedMembership(link)?.boxes.some(box => box.key === key) ?? false;

function replacementEntry(link: LinkState | null, previous: string): unknown {
  const assignments = sharedValue(link, "ownershipAssignments");
  return isObject(assignments) ? assignments[`replacement:${previous}`] : undefined;
}

/** The primary's accepted state matches a named backup to a freshly enrolled replacement. */
export function matchReplacement(source: string, previous: string, replacement: string) {
  return backupLock(async () => {
    if (!backupSource(source) || !validKey(previous) || !validKey(replacement) || previous === replacement) throw new Error("Choose the old computer and its new membership.");
    let save: Receipt | null = null;
    let committed = false;
    updateLink(link => {
      const setup = followedSetup(link);
      if (!link || setup?.role !== "primary" || setup.conflict || primaryFenced(link) || link.leadershipPending ||
          !acceptedMembership(link)?.left.includes(previous) || live(link, previous) || !live(link, replacement) || ownKey(link) === replacement) {
        throw new Error("On the main computer, remove the old membership and add the replacement with fresh keys first.");
      }
      if (!permitsReplacement(acceptedMembership(link)?.computerHistory, previous, replacement)) {
        throw new Error("The selected replacement conflicts with the setup’s computer history.");
      }
      const kept = replacementEntry(link, previous);
      if (isObject(kept) && kept.source === source && kept.computer === replacement) { committed = true; return; }
      const assignments = sharedValue(link, "ownershipAssignments");
      save = stageShared(link, "ownershipAssignments", {
        ...(isObject(assignments) ? assignments : {}), [`replacement:${previous}`]: { computer: replacement, source },
      });
      return save ? link : undefined;
    }, { durable: true });
    if (committed) return null;
    if (!save) throw new Error("The main computer couldn’t save the replacement match.");
    return await awaitSharedSave(save);
  });
}

function ownershipBoundary(link: LinkState | null) {
  return fingerprint({
    assignments: sharedValue(link, "ownershipAssignments") ?? null,
    membership: acceptedMembership(link) ?? null,
    epoch: followedSetup(link)?.epoch,
  });
}

export function restoreForPeer(sender: MailSender, act: Act): Acted {
  const link = readLink();
  const setup = followedSetup(link);
  const source = act.body?.source;
  const old = act.body?.origin;
  const match = backupSource(source) && validKey(old) ? replacementEntry(link, old) : undefined;
  if (setup?.role !== "primary" || setup.conflict || primaryFenced(link) || link?.leadershipPending ||
      pendingShared(link, "ownershipAssignments") || pendingShared(link, "membershipAndAccess") || act.body?.setup !== setup.id ||
      !validKey(old) || !live(link, sender.key) || live(link, old) || !acceptedMembership(link)?.left.includes(old) ||
      !permitsReplacement(acceptedMembership(link)?.computerHistory, old, sender.key) || !isObject(match) || match.source !== source || replacementOwner(link, old) !== sender.key) {
    return { status: 409, value: { message: "The main computer has no current match for this replacement." } };
  }
  return { status: 200, value: { boundary: ownershipBoundary(link) } };
}

async function checkTrust(manifest: Manifest, signal?: AbortSignal): Promise<LinkState | null> {
  const link = readLink();
  const setup = followedSetup(link);
  const own = ownKey(link ?? {});
  if (manifest.origin && own === manifest.origin || manifest.signer && ownSign(link) === manifest.signer) throw new Error("Restore onto a replacement with fresh membership keys.");
  if (!manifest.paired && peersOf(link).length === 0 && setup?.role !== "member") return link;
  const primary = peersOf(link).find(peer => peer.key === setup?.primary);
  if (!primary || !own || !live(link, own) || setup?.conflict || setup?.id !== manifest.setup) {
    throw new Error("Enroll the replacement in the current setup and wait for its accepted membership.");
  }
  const reply = await forwardAct(primary, "Replacement approval", { method: "GET", area: "ownership", item: "backup-restore",
    body: { source: manifest.source, origin: manifest.origin, setup: manifest.setup } }, signal, link, Date.now(), { backgroundRelay: true });
  if ("refused" in reply) throw new Error(reply.refused);
  if (reply.status !== 200) throw new Error("Confirm the replacement on the main computer.");
  const current = readLink();
  if (!isObject(reply.value) || reply.value.boundary !== ownershipBoundary(current) ||
    followedSetup(current)?.primary !== primary.key || !live(current, own) || live(current, manifest.origin ?? "")) {
    throw new Error("Wait for the main computer’s ownership and membership to arrive.");
  }
  return current;
}

function readManifest(value: unknown, source: string, id: string): Manifest {
  if (!isObject(value) || value.format !== 1 || value.source !== source || value.id !== id || !Number.isSafeInteger(value.version) ||
    !Number.isSafeInteger(value.at) || typeof value.paired !== "boolean" ||
    !(value.origin === null || validKey(value.origin)) || !(value.signer === null || validKey(value.signer)) || !(value.setup === null || typeof value.setup === "string") ||
    !Array.isArray(value.projects) || !value.projects.every(name => typeof name === "string")) throw new Error("The archive doesn’t match that computer or restore point.");
  return value as unknown as Manifest;
}

function noWorkers() {
  if (fs.existsSync(path.join(RUN, "update-trial")) || fs.existsSync(path.join(RUN, ".updating")) || swapUnderWay(RUN)) {
    throw new Error("Finish the update before restoring.");
  }
  for (const [file, command] of [["engine", "opencode"], ["bridge", "kuvo-agents"], ["keeper", "kuvo-start"]]) {
    const record = json(path.join(RUN, file + ".json"));
    if (isObject(record) && processRuns(record.pid, command)) throw new Error("Stop the engine, agents, and keeper before restoring.");
  }
  let pid = "";
  try { pid = fs.readFileSync(path.join(RUN, "box.pid"), "utf8").trim(); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  if (processRuns(Number(pid), "kuvo-start")) throw new Error("Stop the keeper before restoring.");
}

interface Restored {
  id: string;
  at: number;
  installed: number;
  held: string[];
  unavailable: { entry: string; reason: string }[];
  savedAfterPoint: "unavailable unless verified elsewhere";
  credentials: "sign-in required";
}

export const restoreStatus = (): unknown => json(JOURNAL);

function sameFile(a: string, b: string) {
  const stamp = (file: string) => {
    const hash = crypto.createHash("sha256");
    const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
    const buffer = Buffer.allocUnsafe(256 * 1024);
    try {
      for (let n; (n = fs.readSync(fd, buffer));) hash.update(buffer.subarray(0, n));
      return hash.digest("hex");
    } finally { fs.closeSync(fd); }
  };
  return stat(a)?.isFile() && fs.statSync(a).size === fs.statSync(b).size && stamp(a) === stamp(b);
}

function place(file: string, target: string, replaceBootstrap = false) {
  fs.mkdirSync(path.dirname(target), { recursive: true, mode: 0o700 });
  const partial = target + ".restore-" + crypto.randomBytes(8).toString("hex");
  try {
    fs.copyFileSync(file, partial, fs.constants.COPYFILE_EXCL);
    syncDurableFile(partial);
    try {
      if (replaceBootstrap) fs.renameSync(partial, target);
      else fs.linkSync(partial, target);
    }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EEXIST" || !sameFile(target, file)) throw error;
    }
    syncDurableFile(target);
  } finally { fs.rmSync(partial, { force: true }); }
}

export async function restore(dest: Destination, source: string, id: string, key: string, signal?: AbortSignal): Promise<Restored> {
  if (!POINT.test(id) || id.includes("-vault-")) throw new Error("Choose a dated computer backup.");
  return backupLock(async () => {
    noWorkers();
    const scratch = scratchFolder();
    try {
      const archived = openArchive(await destinationStore(dest, source).get(id, scratch), key, archiveContext(source, id), scratch);
      const manifestFile = archived.find(entry => entry.name === "meta/manifest.json")?.file;
      const manifest = readManifest(manifestFile && json(manifestFile), source, id);

      const link = await checkTrust(manifest, signal);
      noWorkers();
      const journal = restoreStatus();
      if (isObject(journal) && (journal.id !== id || journal.source !== source)) throw new Error("This replacement already has a restore point.");
      if (isObject(journal) && journal.state === "restored") {
        fs.rmSync(RESTORING, { force: true });
        return journal.result as Restored;
      }

      const byName = stageRestore(path.join(scratch, "layout"), manifest.version, archived);

      const result: Restored = { id, at: manifest.at, installed: 0, held: [], unavailable: [], savedAfterPoint: "unavailable unless verified elsewhere", credentials: "sign-in required" };
      const recovery = path.join(BACKUP, "recovered", id);
      const archivedJSON = (name: string) => byName.has(name) ? json(byName.get(name)!) : undefined;
      const savedFence = (family: "deletions" | "departures", id: string) => archivedJSON(`home/run/owned-data/edit-${family}/${recordKey(id)}.json`);
      const sessionFor = (root: string, id: string) => {
        const file = byName.get(`${root}/${id}/session`);
        return file ? fs.readFileSync(file, "utf8").trim() : id;
      };
      const authority = { link, origin: manifest.paired ? manifest.origin : null };
      const heldItems = new Set<string>();

      // Restored schedules live behind a native pause marker. Pending spools that lack a reliable
      // delivery answer remain reviewable recovery files, outside all executable spools.
      const placements: { file: string; target: string; name: string }[] = [];
      const frozen = new Set<string>();
      const removedItems = new Set<string>();
      const add = (file: string, target: string, name: string) => placements.push({ file, target, name });
      for (const [name, file] of byName) {
        if (name === "meta/manifest.json" || name === "meta/projects.json") continue;
        const [root, ...parts] = name.split("/");
        const storage = storageOf(name);
        const kind = storage?.kind;
        const base = roots[root];
        const id = root === "agents" ? parts[1] : parts[0];
        const queue = kind === "queueItems" ? parts.at(-1)!.slice(0, -5) : null;
        if (!recordReferences(name, sessionFor).every(ref => mayActivate(ref, authority))) {
          result.unavailable.push({ entry: name, reason: "moved" });
          continue;
        }

        if (queue) {
          const saved = json(file);
          if (!isObject(saved) || !Array.isArray(saved.items)) throw new Error("The backup contains an unreadable Work Queue.");
          const remaining = saved.items.filter(item => {
            if (!isObject(item) || typeof item.id !== "string") throw new Error("The backup contains an unreadable Work Queue item.");
            const decision = recoveredQueueDecision({ ...item, id: item.id }, queue, authority,
              { deletion: savedFence("deletions", item.id), departure: savedFence("departures", item.id) });
            if (decision !== "active") {
              removedItems.add(item.id);
              result.unavailable.push({ entry: item.id, reason: decision === "deleted" ? "deleted" : "moved or transfer needs review" });
              if (decision === "held") {
                heldItems.add(item.id);
                const entry = `work-queue-items/${recordKey(item.id)}.json`;
                const held = outputPath(recovery, entry.split("/"));
                const copy = path.join(scratch, recordKey(name, item.id));
                writeDurablePrivate(copy, JSON.stringify({ ...item, owner: queue }));
                add(copy, held, entry);
                result.held.push(entry);
              }
              return Boolean(item.removed); // Keep its deletion fence.
            }
            if (Array.isArray(item.files)) for (const attachment of item.files) {
              if (!isObject(attachment) || typeof attachment.name !== "string") continue;
              const entry = `home/run/work-queue/files/${item.id}/${attachment.name}`;
              if (!byName.has(entry)) result.unavailable.push({ entry, reason: "attachment missing from restore point" });
            }
            return true;
          });
          if (remaining.length !== saved.items.length) {
            fs.unlinkSync(file);
            writeDurablePrivate(file, JSON.stringify({ ...saved, items: remaining }));
          }
        }

        if (root === "agents" && parts[0] === "sessions" && parts[2] === "agent.json") {
          const record = json(file);
          if (!isObject(record)) throw new Error("The backup contains an unreadable agent record.");
          const { inbox, lateShells: _lateShells, revertNow: _revertNow, ...saved } = record;
          if (Array.isArray(inbox) && inbox.length) {
            const stopped = path.join(scratch, "inbox-" + id);
            const held = `agents/sessions/${id}/inbox.json`;
            writeDurablePrivate(stopped, JSON.stringify(inbox));
            add(stopped, outputPath(recovery, held.split("/")), held);
            result.held.push(held);
          }
          writeDurablePrivate(file, JSON.stringify(saved));
        }

        if (name === "home/box/approval.json") writeDurablePrivate(file, JSON.stringify(portableApproval(json(file))));
        if (root === "groups" && parts[1] === "group.json") {
          const record = json(file);
          const messages = archivedJSON(`groups/${id}/messages.json`);
          if (isObject(record) && (record.talking || record.paused || record.turning ||
              (isObject(record.group) && record.group.meeting) ||
              (Array.isArray(messages) && messages.some(message => isObject(message) && Array.isArray(message.tasks) &&
                message.tasks.some(task => isObject(task) && (task.state === "waiting" || task.state === "working")))))) {
            const { reviewReceipt: _previousReview, ...pending } = record;
            writeDurablePrivate(file, JSON.stringify({ ...pending, status: "held", restoreReview: true }));
            result.held.push(name);
          }
        }
        if (name === "groups/peer-memory.json") {
          const memory = json(file);
          if (!isObject(memory) || !roots.bots) throw new Error("The backup contains unreadable group memory.");
          const bots: Record<string, unknown> = {};
          for (const [bot, entries] of Object.entries(memory)) {
            if (!/^bot_[A-Za-z0-9]+$/.test(bot) || !isObject(entries) ||
                !mayActivate({ kind: "privateGroupSeats", id: bot }, authority) ||
                !mayActivate({ kind: "conversations", id: sessionFor("bots", bot) }, authority)) continue;
            bots[bot] = Object.fromEntries(Object.entries(entries).flatMap(([address, entry]) => {
              if (!isObject(entry)) return [];
              if (entry.forgotten === true) return [[address, entry]];
              if (typeof entry.file !== "string") return [];
              const leaf = path.basename(entry.file);
              const parts = [bot, "groups", leaf];
              if (!byName.has(`bots/${parts.join("/")}`)) return [];
              return [[address, { ...entry, file: outputPath(roots.bots!, parts) }]];
            }));
          }
          writeDurablePrivate(file, JSON.stringify(bots));
        }
        if (root === "engine" && name.endsWith(".db")) scrubEngine(file, sessionsFencedHere(authority), true);
        if (kind === "botSchedules" || kind === "sessionSchedules") {
          if (parts.length === 2 && parts[1] === "paused") continue;
          if (parts.length > 2 && base) frozen.add(outputPath(base, [parts[0], "paused"]));
        }
        if (storage?.restore !== "portable" || !base) {
          add(file, outputPath(recovery, name.split("/")), name);
          result.held.push(name);
        } else {
          add(file, outputPath(base, parts), name);
        }
      }
      // Install pause markers before introducing any wake/watch record. An existing pause stays.
      const paused = path.join(scratch, "paused");
      fs.writeFileSync(paused, "restore-review", { mode: 0o600 });
      writeDurablePrivate(JOURNAL, JSON.stringify({ state: "restoring", id, source }), K);
      writeDurablePrivate(RESTORING, id, K);
      noWorkers();
      if (ownershipBoundary(readLink()) !== ownershipBoundary(link)) throw new Error("Ownership changed during restore. Verify the replacement again.");
      for (const target of frozen) if (!stat(target)) place(paused, target);
      for (const { file, target, name } of placements) {
        if (name.startsWith("home/run/work-queue/files/") && removedItems.has(name.split("/")[4])) {
          if (heldItems.has(name.split("/")[4])) {
            place(file, outputPath(recovery, name.split("/")));
            result.held.push(name);
          }
          continue;
        }
        let replaceBootstrap = false;
        if (stat(target)) {
          if (sameFile(target, file)) {
            syncDurableFile(target);
            result.installed++;
            continue;
          }
          replaceBootstrap = name.startsWith("engine/") && name.endsWith(".db") && prepareEngineBootstrap(target);
          if (!replaceBootstrap) {
            addConflict(name, file);
            continue;
          }
        }
        place(file, target, replaceBootstrap);
        result.installed++;
      }
      function addConflict(name: string, file: string) {
        place(file, outputPath(recovery, name.split("/")));
        result.unavailable.push({ entry: name, reason: "existing local record; recovered copy held for review" });
      }
      result.unavailable.push(...restoreProjects(byName, recovery));
      writeDurablePrivate(JOURNAL, JSON.stringify({ state: "restored", id, source, result }), K);
      fs.rmSync(RESTORING, { force: true });
      syncDurableFile(BACKUP, K);
      return result;
    } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
  });
}
