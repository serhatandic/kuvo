import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { exclusiveWhile, syncDurableFile, writeDurablePrivate } from "../files";
import { homeFolder, located } from "../home";
import { ownKey, peersOf, type MailSender } from "../peers/known";
import { acceptedMembership } from "../setup/membership";
import { followedSetup, SETUP_ID } from "../setup/record";
import { pendingShared, sharedValue } from "../setup/shared";
import { readLink } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { POINT, SOURCE, type DestinationStore } from "./destination";
import { outputPath, stat, tree } from "./locations";

export const RELAY_BACKUPS = "box.backups.relay";
const CHUNK = 256 * 1024;
const MAX_ARCHIVE = 2 * 1024 ** 3;
const MAX_STORED = 32 * 1024 ** 3;
const MAX_STAGING = 4 * 1024 ** 3;
const MAX_UPLOADS = 4;
const MAX_POINTS = 4096;
const STALE = 60 * 60_000;

const yes = (value: unknown = {}): Acted => ({ status: 200, value });
const no = (message: string, status = 409): Acted => ({ status, value: { message } });
const sizeOf = (value: unknown) => typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : -1;

async function hash(file: string): Promise<string> {
  const digest = crypto.createHash("sha256");
  const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
  for await (const data of fs.createReadStream(file, { fd })) digest.update(data);
  return digest.digest("hex");
}

export function relayBackupAvailable(key: string, link = readLink()): boolean {
  const setup = followedSetup(link);
  const membership = acceptedMembership(link);
  const own = ownKey(link ?? {});
  return Boolean(link?.relay && setup && !setup.conflict && membership?.boxes.some(box => box.key === own)
    && membership.boxes.some(box => box.key === key) && peersOf(link).some(peer => peer.key === key && peer.address)
    && takesForwarded(key, RELAY_BACKUPS));
}

/** Authorize each operation against accepted membership and the exact source namespace. */
function scope(sender: string, source: string, ownOnly: boolean, expected: unknown) {
  const link = readLink();
  const setup = followedSetup(link);
  const membership = acceptedMembership(link);
  const enrolled = (key: string | null) => membership?.boxes.some(box => box.key === key) && !membership.left.includes(key!);
  if (!setup || setup.id !== expected || !SETUP_ID.test(setup.id) || setup.conflict || link?.leadershipPending ||
      pendingShared(link, "membershipAndAccess") || pendingShared(link, "ownershipAssignments") ||
      !enrolled(sender) || !enrolled(ownKey(link ?? {}))) throw new Error("Wait for accepted backup membership.");
  const assignments = sharedValue(link, "ownershipAssignments");
  let owners = [sender];
  if (!ownOnly && isObject(assignments)) {
    owners = owners.concat(membership!.left.filter(old => {
      const match = assignments[`replacement:${old}`];
      return !enrolled(old) && isObject(match) && match.computer === sender && match.source === source;
    }));
  }
  return { setup: setup.id, owners };
}

const relative = (setup: string, owner: string, source: string) => [setup, owner, source];
const uploadOf = (root: string, setup: string, owner: string) => outputPath(root, [setup, owner, ".upload.json"]);
const partialOf = (upload: string) => path.join(path.dirname(upload), ".upload.partial");
const listing = (folder: string) => stat(folder)?.isDirectory() ? fs.readdirSync(folder) : [];
const archiveName = (id: string) => id + ".kuvo";

function syncRemoval(file: string, root: string) {
  let folder = path.dirname(file);
  while (folder !== root) {
    try { fs.rmdirSync(folder); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ENOTEMPTY") throw error;
      break;
    }
    folder = path.dirname(folder);
  }
  syncDurableFile(folder, root);
}

function removeUpload(upload: string, root: string) {
  fs.rmSync(partialOf(upload), { force: true });
  fs.rmSync(upload, { force: true });
  syncRemoval(upload, root);
}

/** Reserve against kept ciphertext and all unfinished uploads, including those of other peers. */
function reserve(root: string, requested: number, now: number) {
  let stored = 0, staging = 0, uploads = 0, points = 0, unwritten = 0;
  for (const { name, file } of tree(root, "", leaf => !leaf.startsWith(".") || leaf === ".upload.json")) {
    if (name.endsWith("/.upload.json")) {
      if (now - Math.max(fs.statSync(file).mtimeMs, stat(partialOf(file))?.mtimeMs ?? 0) >= STALE) {
        removeUpload(file, root);
        continue;
      }
      const value = parsed(fs.readFileSync(file));
      if (!isObject(value) || sizeOf(value.size) < 0) throw new Error("The backup storage needs attention.");
      staging += sizeOf(value.size);
      unwritten += Math.max(0, sizeOf(value.size) - (stat(partialOf(file))?.size ?? 0));
      uploads++;
    } else if (name.endsWith(".kuvo")) {
      points++;
      stored += fs.statSync(file).size;
    }
  }
  const space = fs.statfsSync(root);
  if (uploads >= MAX_UPLOADS || points + uploads >= MAX_POINTS || staging + requested > MAX_STAGING ||
      stored + staging + requested > MAX_STORED || requested + unwritten > space.bavail * space.bsize) throw new Error("The receiving computer doesn’t have enough backup capacity.");
}

export async function relayBackupAct(sender: MailSender, act: Act): Promise<Acted> {
  const source = act.item;
  const { action, id, offset: givenOffset, size: givenSize, data, sha256, setup: expected } = act.body ?? {};
  if (!SOURCE.test(source) || (id !== undefined && (typeof id !== "string" || !POINT.test(id)))) return no("Choose a backup source and point.", 400);
  const offset = sizeOf(givenOffset), size = sizeOf(givenSize);
  const root = path.join(located(homeFolder()), ".kuvo-backups", "relay");
  try {
    scope(sender.key, source, act.method === "POST", expected);
    fs.mkdirSync(path.dirname(root), { mode: 0o700, recursive: true });
    if (located(path.dirname(root)) !== path.dirname(root)) throw new Error("The backup storage contains a link.");
    fs.mkdirSync(root, { mode: 0o700, recursive: true });
    const folder = fs.lstatSync(root);
    if (located(root) !== root || !folder.isDirectory() || (folder.mode & 0o077) !== 0 ||
        (process.getuid && folder.uid !== process.getuid())) throw new Error("The backup storage must be private.");
    return await exclusiveWhile(path.join(root, ".operation"), async () => {
      const { setup, owners } = scope(sender.key, source, act.method === "POST", expected);
      const folders = owners.map(owner => outputPath(root, relative(setup, owner, source)));
      const files = (folder: string) => listing(folder).filter(name => name.endsWith(".kuvo") && POINT.test(name.slice(0, -5))
        && stat(path.join(folder, name))?.isFile());
      if (act.method === "GET" && action === "list") return yes([...new Set(folders.flatMap(files))].map(name => name.slice(0, -5)).sort().reverse());
      if (typeof id !== "string") return no("Choose a backup point.", 400);
      const paths = owners.map(owner => outputPath(root, [...relative(setup, owner, source), archiveName(id)]));
      if (act.method === "GET" || act.method === "DELETE") {
        const found = paths.filter(file => stat(file)?.isFile());
        const file = found[0];
        if (found.length > 1) return no("The backup point has ambiguous ownership.");
        if (!file) return act.method === "DELETE" ? yes() : no("The backup point is unavailable.", 404);
        if (act.method === "DELETE") {
          fs.unlinkSync(file);
          syncRemoval(file, root);
          return yes();
        }
        if (action !== "read" || offset < 0) return no("Invalid backup read.", 400);
        const fd = fs.openSync(file, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
        try {
          const info = fs.fstatSync(fd);
          if (!info.isFile() || offset > info.size) return no("Invalid backup read.", 400);
          const chunk = Buffer.alloc(Math.min(CHUNK, info.size - offset));
          const n = fs.readSync(fd, chunk, 0, chunk.length, offset);
          return yes({ size: info.size, data: chunk.subarray(0, n).toString("base64url") });
        } finally { fs.closeSync(fd); }
      }
      if (act.method !== "POST") return no("Invalid backup deposit.", 400);
      const file = paths[0]!;
      const upload = uploadOf(root, setup, sender.key);
      const partial = partialOf(upload);
      const spec = JSON.stringify({ source, id, size, sha256 });
      const kept = stat(upload)?.isFile() ? fs.readFileSync(upload, "utf8") : undefined;
      const info = stat(partial);
      if (action === "abort") {
        if (kept === spec) removeUpload(upload, root);
        return yes();
      }
      if (size <= 0 || size > MAX_ARCHIVE || typeof sha256 !== "string" || !/^[a-f0-9]{64}$/.test(sha256)) {
        return no("The encrypted backup exceeds the Relay archive limit or has an invalid digest.");
      }
      const published = stat(file);
      if (published) {
        if (action !== "commit" || !published.isFile() || published.size !== size || await hash(file) !== sha256) {
          return no("That backup point already exists.");
        }
        if (scope(sender.key, source, true, expected).setup !== setup) return no("Backup membership changed during the deposit.");
        syncDurableFile(file, path.dirname(path.dirname(root)));
        if (kept === spec) removeUpload(upload, root);
        return yes();
      }
      if (action === "begin") {
        if (kept === spec && info?.isFile()) return yes({ offset: info.size });
        reserve(root, size, Date.now());
        if (stat(upload) || stat(partial)) return no("This computer already has an upload in progress.");
        fs.mkdirSync(path.dirname(upload), { recursive: true, mode: 0o700 });
        writeDurablePrivate(upload, spec, root);
        try { fs.closeSync(fs.openSync(partial, "wx", 0o600)); }
        catch (error) { removeUpload(upload, root); throw error; }
        return yes({ offset: 0 });
      }
      if (kept !== spec || !info?.isFile()) return no("The backup deposit is no longer open.");
      if (action === "append") {
        if (typeof data !== "string" || data.length > Math.ceil(CHUNK * 4 / 3) || !/^[A-Za-z0-9_-]+$/.test(data)) return no("Invalid backup chunk.", 400);
        const bytes = Buffer.from(data, "base64url");
        if (offset < 0 || offset > info.size || offset + bytes.length > size) return no("Invalid backup offset.", 400);
        const fd = fs.openSync(partial, fs.constants.O_RDWR | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
        try {
          if (offset < info.size) {
            const old = Buffer.alloc(bytes.length);
            if (fs.readSync(fd, old, 0, old.length, offset) !== old.length || !old.equals(bytes)) return no("The repeated backup chunk differs.");
          } else if (fs.writeSync(fd, bytes, 0, bytes.length, offset) !== bytes.length) throw new Error("The receiving computer couldn’t write the full chunk.");
        } catch (error) { removeUpload(upload, root); throw error; }
        finally { fs.closeSync(fd); }
        return yes({ offset: offset + bytes.length });
      }
      if (action !== "commit" || info.size !== size) return no("The encrypted deposit is incomplete.");
      if (await hash(partial) !== sha256) {
        removeUpload(upload, root);
        return no("The encrypted deposit failed its checksum.");
      }
      if (scope(sender.key, source, true, expected).setup !== setup) return no("Backup membership changed during the deposit.");
      fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
      syncDurableFile(partial, root);
      fs.linkSync(partial, file);
      syncDurableFile(path.dirname(file), path.dirname(path.dirname(root)));
      removeUpload(upload, root);
      return yes();
    });
  } catch (error) {
    return no((error as NodeJS.ErrnoException).code === "ENOSPC" ? "The receiving computer is out of backup space." : (error as Error).message);
  }
}

export function relayStore(key: string, source: string): DestinationStore {
  const setup = followedSetup(readLink())?.id;
  if (!setup) throw new Error("Wait for accepted backup membership.");
  const ask = async (method: Act["method"], body: Record<string, unknown>) => {
    const link = readLink();
    const computer = peersOf(link).find(peer => peer.key === key);
    if (followedSetup(link)?.id !== setup) throw new Error("Backup membership changed during transfer.");
    if (!computer || !relayBackupAvailable(key, link)) throw new Error("Choose an enrolled computer that supports Relay backups.");
    const reply = await forwardAct(computer, "Encrypted backup", { area: "backup", method, item: source, body: { ...body, setup } },
      undefined, link, Date.now(), { backgroundRelay: true });
    if ("refused" in reply) throw new Error(reply.refused);
    if (reply.status !== 200) throw new Error(isObject(reply.value) && typeof reply.value.message === "string" ? reply.value.message : "The receiving computer couldn’t use the backup.");
    return reply.value;
  };
  return {
    setup,
    async list() {
      const value = await ask("GET", { action: "list" });
      if (!Array.isArray(value) || value.some(id => typeof id !== "string" || !POINT.test(id))) throw new Error("The receiving computer returned an invalid list.");
      return value;
    },
    async put(id, file) {
      const size = fs.statSync(file).size;
      if (size > MAX_ARCHIVE) throw new Error("This encrypted backup exceeds the Relay archive limit.");
      const sha256 = await hash(file);
      const request = { id, size, sha256 };
      try {
        const started = await ask("POST", { ...request, action: "begin" });
        let offset = isObject(started) ? sizeOf(started.offset) : -1;
        if (offset < 0 || offset > size) throw new Error("The receiving computer returned an invalid upload offset.");
        const fd = fs.openSync(file, "r");
        try {
          while (offset < size) {
            const bytes = Buffer.alloc(Math.min(CHUNK, size - offset));
            if (fs.readSync(fd, bytes, 0, bytes.length, offset) !== bytes.length) throw new Error("The encrypted backup changed while sending.");
            const reply = await ask("POST", { ...request, action: "append", offset, data: bytes.toString("base64url") });
            offset += bytes.length;
            if (!isObject(reply) || reply.offset !== offset) throw new Error("The receiving computer didn’t confirm the chunk.");
          }
        } finally { fs.closeSync(fd); }
        await ask("POST", { ...request, action: "commit" });
      } catch (error) {
        await ask("POST", { ...request, action: "abort" }).catch(() => {});
        throw error;
      }
    },
    async get(id, scratch) {
      const file = path.join(scratch, "received");
      const fd = fs.openSync(file, "wx", 0o600);
      try {
        let offset = 0, size: number | undefined;
        do {
          const reply = await ask("GET", { action: "read", id, offset });
          if (!isObject(reply) || typeof reply.data !== "string" || reply.data.length > Math.ceil(CHUNK * 4 / 3)) throw new Error("Invalid encrypted backup chunk.");
          size ??= sizeOf(reply.size);
          const bytes = Buffer.from(reply.data, "base64url");
          if (size <= 0 || size > MAX_ARCHIVE || reply.size !== size || !bytes.length || offset + bytes.length > size) throw new Error("Incomplete encrypted backup.");
          if (fs.writeSync(fd, bytes) !== bytes.length) throw new Error("Couldn’t save the encrypted backup.");
          offset += bytes.length;
        } while (offset < size);
      } catch (error) {
        fs.rmSync(file, { force: true });
        throw error;
      } finally { fs.closeSync(fd); }
      return file;
    },
    async erase(id) { await ask("DELETE", { id }); },
  };
}
