// The session storage owner selects the payloads. The transcript supplies references for
// older flat attachments, inherited paths, and files whose retention has already expired.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { syncDurableFile } from "../../shared/files";
import { isObject } from "../../shared/json";
import { inside, roots, stat } from "../backup/locations";
import { K } from "../home";
import { ATTACHED_BLOCK, ATTACHED_HEADINGS, FROM_PHONE_BLOCK } from "../peers/naming";
import { measurePayload, payloadPath, validProof, verifyPayload, type FileProof } from "./payload";

const boxAttachments = roots.attachments!;
const stores = [boxAttachments, ...(roots.macAttachments ? [roots.macAttachments] : [])].map(root => path.resolve(root));
const sessionID = /^ses_[A-Za-z0-9_-]{1,120}$/;
const moveID = /^mov_[A-Za-z0-9]{1,40}$/;
const slotOf = (file: string) => crypto.createHash("sha256").update(file).digest("hex");
export const movedPayloadFolder = (name: string): boolean => /^move-mov_[A-Za-z0-9]{1,40}$/.test(name);

export interface SessionPayload extends FileProof {
  from: string;
  relative: string;
}

type Entry = { root: string; source: string; owner: string };
function inStore(file: string): { root: string; source: string } | undefined {
  const root = stores.find(root => file !== root && path.isAbsolute(file) && inside(root, file));
  return root ? { root, source: path.relative(root, file) } : undefined;
}

function attachedURI(file: unknown): string | undefined {
  if (!isObject(file)) return undefined;
  if (isObject(file.source) && typeof file.source.uri === "string") return file.source.uri;
  return typeof file.uri === "string" ? file.uri : typeof file.url === "string" ? file.url : undefined;
}

function inspectRecords(value: unknown, visit: (text: string) => void, file: (path: string) => void) {
  const visitFile = (record: unknown) => {
    const uri = attachedURI(record);
    if (!uri?.startsWith("file:")) return;
    let shown: string;
    try { shown = fileURLToPath(uri); }
    catch { throw new Error("A file attachment has an invalid local address."); }
    file(shown);
  };
  const inspect = (part: unknown) => {
    if (Array.isArray(part)) part.forEach(inspect);
    else if (isObject(part)) {
      if (part.type === "file") visitFile(part);
      if (Array.isArray(part.files)) part.files.forEach(visitFile);
      Object.values(part).forEach(inspect);
    } else if (typeof part === "string") visit(part);
  };
  inspect(value);
}

function attachmentNote(text: string): { path: string; stayed: boolean }[] {
  const note = ATTACHED_BLOCK.split(FROM_PHONE_BLOCK.split(text)?.before ?? text);
  let stayed = false;
  const paths: { path: string; stayed: boolean }[] = [];
  for (const line of note?.lines ?? []) {
    if ((ATTACHED_HEADINGS as readonly string[]).includes(line)) stayed = line === ATTACHED_HEADINGS[2];
    else if (path.isAbsolute(line)) paths.push({ path: line, stayed });
  }
  return paths;
}

/** A legacy sender with only a path in a historical prompt cannot certify that file. */
export function requireAttachmentReferences(records: unknown, mappings: ReadonlyMap<string, string>) {
  const requireFile = (file: string) => {
    if (!mappings.has(file)) throw new Error("A promised attachment has no file in the move. Update both computers and review the source files.");
  };
  inspectRecords(records, text => attachmentNote(text).forEach(entry => requireFile(entry.path)), requireFile);
}

/** Traverse real files under both declared roots, including leaves such as .lock. */
export function sessionInventory(sessions: readonly string[], records: readonly unknown[]) {
  const entries = new Map<string, Entry>();
  const unavailable = new Set<string>();
  const exportOnly = new Set<string>();
  const add = (file: string, location: Pick<Entry, "root" | "source">, owner: string) => {
    if (!entries.has(file)) entries.set(file, { ...location, owner });
  };
  const walk = (root: string, source: string, owner: string) => {
    const file = payloadPath(root, source);
    const info = stat(file);
    if (!info) return;
    if (info.isDirectory()) for (const name of fs.readdirSync(file).sort()) walk(root, source + "/" + name, owner);
    else if (info.isFile() && info.nlink === 1) add(file, { root, source }, owner);
    else unavailable.add(path.basename(file));
  };
  for (const owner of sessions) {
    if (!sessionID.test(owner)) throw new Error("Invalid session attachment owner.");
    for (const root of stores) walk(root, owner, owner);
  }
  const reference = (file: string) => {
    const location = inStore(file);
    if (!location || !sessions.length) return;
    const local = payloadPath(location.root, location.source);
    if (local !== file || !stat(local)?.isFile()) unavailable.add(path.basename(file));
    else add(file, location, sessions.includes(location.source.split("/")[0]!) ? location.source.split("/")[0]! : sessions[0]!);
  };
  const attached = (file: string) => {
    if (!inStore(file)) exportOnly.add(path.basename(file));
    else reference(file);
  };
  inspectRecords(records, value => {
    if (value.startsWith("file:")) {
      try { reference(fileURLToPath(value)); } catch (error) {
        if (!(error instanceof URIError) && !(error instanceof TypeError)) throw error;
      }
    } else if (!value.includes("\n") && path.isAbsolute(value)) reference(value);
    for (const file of attachmentNote(value)) {
      if (file.stayed) exportOnly.add(path.basename(file.path));
      else attached(file.path);
    }
  }, attached);
  return { entries, unavailable: [...unavailable], exportOnly: [...exportOnly] };
}

/** These JSON records are the bridge's portable transcript, already hashed by its carry manifest. */
export function carriedSessionRecords(carry: string, sessions: readonly string[]): unknown[] {
  return sessions.flatMap(session => ["session.json", "messages.json"].map(name =>
    JSON.parse(fs.readFileSync(payloadPath(carry, `sessions/${session}/${name}`), "utf8"))));
}

export function stageSessionPayloads(id: string, staging: string, sessions: readonly string[], records: readonly unknown[], most: number): SessionPayload[] {
  if (!moveID.test(id)) throw new Error("Invalid move for session attachments.");
  const { entries, unavailable, exportOnly } = sessionInventory(sessions, records);
  if (unavailable.length) throw new Error(`Session attachments are missing, expired or unreadable: ${unavailable.join(", ")}. Review the move and removal plan.`);
  if (exportOnly.length) throw new Error(`Some attachment references are outside the managed roots: ${exportOnly.join(", ")}. Review the export-only entries.`);
  const payloads: SessionPayload[] = [];
  for (const [from, { root, source, owner }] of entries) {
    const relative = `${owner}/move-${id}/${slotOf(from)}/${path.basename(from)}`;
    const proof = measurePayload(root, source, payloadPath(staging, relative), most);
    most -= proof.size;
    payloads.push({ from, relative, ...proof });
  }
  return payloads;
}

export function checkedSessionPayloads(id: string, value: unknown, sessions: readonly string[]): SessionPayload[] {
  if (!moveID.test(id) || !Array.isArray(value)) throw new Error("Invalid session attachment inventory.");
  const seen = new Set<string>();
  const sources = new Set<string>();
  for (const entry of value) {
    if (!isObject(entry) || typeof entry.relative !== "string" || typeof entry.from !== "string" || !path.isAbsolute(entry.from)
      || /[\r\n\0]/.test(entry.from) || !validProof(entry as unknown as FileProof)) throw new Error("Invalid session attachment inventory.");
    const [session, folder, slot, leaf, ...extra] = entry.relative.split("/");
    if (!session || !sessionID.test(session) || !sessions.includes(session) || folder !== "move-" + id || slot !== slotOf(entry.from)
      || leaf !== path.basename(entry.from) || extra.length || seen.has(entry.relative) || sources.has(entry.from)) {
      throw new Error("Invalid session attachment inventory.");
    }
    seen.add(entry.relative);
    sources.add(entry.from);
  }
  return value as SessionPayload[];
}

export function installSessionPayloads(payloads: readonly SessionPayload[], staged: string): Map<string, string> {
  const placed = new Map<string, string>();
  for (const payload of payloads) verifyPayload(staged, payload.relative, payload);
  for (const payload of payloads) {
    const destination = payloadPath(boxAttachments, payload.relative);
    verifyPayload(staged, payload.relative, payload, destination);
    syncDurableFile(destination, K);
    placed.set(payload.from, destination);
  }
  verifySessionPayloads(payloads);
  return placed;
}

export function verifySourceSessionPayloads(payloads: readonly SessionPayload[]): void {
  for (const payload of payloads) {
    const location = inStore(payload.from);
    if (!location) throw new Error("A source attachment has no managed location.");
    verifyPayload(location.root, location.source, payload);
  }
}

export function verifySessionPayloads(payloads: readonly SessionPayload[]): void {
  for (const payload of payloads) verifyPayload(boxAttachments, payload.relative, payload);
}

export function removeSessionPayloads(payloads: readonly SessionPayload[]): void {
  for (const { relative } of payloads) fs.rmSync(payloadPath(boxAttachments, relative), { force: true });
}

export function attachmentMappings(payloads: readonly SessionPayload[]): [string, string][] {
  return payloads.map(({ from, relative }) => [from, payloadPath(boxAttachments, relative)]);
}
