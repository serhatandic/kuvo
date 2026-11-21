import fs from "node:fs";
import path from "node:path";
import { draftReplayFence, MUTATION_EVIDENCE, mutationEvidenceKind } from "../ownership/forward-evidence";
import { supportsLegacyObjectVersion } from "../../shared/format-version";
import { submissionReference } from "../ownership/submission-address";
import type { DataReference } from "../ownership/resolve";
import { DATA_DECLARATIONS, type BackupSnapshot, type DataDeclaration, type DataKind } from "../../shared/data-declarations";
import { isObject, parsed } from "../../shared/json";
import { retainedArtifactCopies } from "../artifacts/inventory";
import { FILE_NAME, ITEM_ID } from "../../shared/work-queue-file";
import { APP_FOLDER, BOTS, ENGINE_DATA, K, MAC_ATTACHMENTS, SESSION_WAKES } from "../home";

export const roots: Record<string, string | null> = {
  home: K,
  bots: BOTS,
  wakes: SESSION_WAKES,
  engine: process.env.KUVO_ENGINE_DB_FOLDER || ENGINE_DATA,
  agents: process.env.KUVO_AGENTS_HOME || (APP_FOLDER ? path.join(APP_FOLDER, "agents") : path.join(K, "agents")),
  groups: process.env.KUVO_GROUPS || path.join(APP_FOLDER ?? K, "groups"),
  artifacts: process.env.KUVO_ARTIFACTS || path.join(K, "artifacts"),
  attachments: path.join(K, "attachments"),
  macAttachments: MAC_ATTACHMENTS,
};

const component = (pattern: string) => new RegExp("^" + pattern.split("*")
  .map(part => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("[^/]*") + "$");

const selectors = Object.entries(DATA_DECLARATIONS).flatMap(([key, policy]: [string, DataDeclaration]) =>
  !policy.backup ? [] : (policy.storage ?? []).map(location => {
    const pattern = typeof location === "string" ? location : location.pattern;
    const options = typeof location === "string" ? undefined : location;
    const [root, ...parts] = pattern.replace(/\/$/, "").split("/");
    return { kind: key as DataKind, restore: options?.restore ?? policy.restore, snapshot: options?.snapshot ?? "bytes",
      inventory: options?.inventory, root, parts: parts.map(component), tree: pattern.endsWith("/") };
  }));

const temporary = (name: string) => name.startsWith(".") || /\.(?:tmp|new|lock)$/.test(name);
const queueFileRoot = "home/run/work-queue/files";
function queueAttachment(name: string): boolean {
  const parts = name.split("/");
  return parts.length === 6 && parts.slice(0, 4).join("/") === queueFileRoot
    && ITEM_ID.test(parts[4]!) && FILE_NAME.test(parts[5]!);
}

export function storageOf(name: string) {
  const [root, ...parts] = name.split("/");
  if (!parts.length) return undefined;
  return selectors.find(rule => rule.root === root && (rule.tree ? parts.length >= rule.parts.length : parts.length === rule.parts.length)
    && rule.parts.every((match, i) => match.test(parts[i]!))
    && (rule.inventory === "session payloads" || !parts.some(temporary)
      || (rule.inventory === "queue payloads" && queueAttachment(name))));
}

export const kindOf = (name: string): DataKind | undefined => storageOf(name)?.kind;

export const queueBackupKind = (kind: DataKind): boolean => kind.startsWith("queue") && kind !== "queueNumberAllocation";
export const attachmentKind = (kind: DataKind): boolean => kind === "sessionFiles" || kind === "queueFiles";

const SUBMISSION_EVIDENCE = /^home\/run\/owned-data\/submissions\/([A-Za-z0-9_-]{43})\.json$/;

/** An old opaque claim has no authority address. Preserve it without guessing from the slot. */
export function receiptReference(name: string, file: string): DataReference | undefined {
  const submission = SUBMISSION_EVIDENCE.exec(name)?.[1];
  const mutation = MUTATION_EVIDENCE.exec(name)?.[1];
  if (!submission && !mutation) return;
  const value = parsed(fs.readFileSync(file));
  if (!isObject(value) || !supportsLegacyObjectVersion(value)) return;
  if (submission) return submissionReference(value.submission, submission);
  const fence = draftReplayFence(value.draftReplayFence, mutation!);
  if (fence && typeof value.digest === "string" && /^scoped:[A-Za-z0-9_-]{43}$/.test(value.digest) && value.answer === undefined)
    return { kind: "draftSubmissionReplayFences", id: String(fence.session), setup: String(fence.setup) };
  return undefined;
}

/** Map individual files to the same record address used by writes. Aggregate
 * stores (for example SQLite) resolve their record IDs with their own reader. */
export function recordReferences(name: string, sessionFor: (root: string, id: string) => string): DataReference[] {
  const [root, first, second] = name.split("/");
  const noteSession = /^home\/run\/work\/notes\/(ses_[A-Za-z0-9_-]{1,95})\.ordinary\.json$/.exec(name)?.[1];
  if (noteSession) return [{ kind: "sessionNotes", id: noteSession }];
  if (root === "bots" && second) return [
    { kind: "botSettings", id: first }, { kind: "conversations", id: sessionFor(root, first) },
  ];
  if (root === "groups" && second) return [{ kind: "groupSettings", id: first }];
  if (root === "agents" && first === "sessions" && second) return [{ kind: "conversations", id: second }];
  if (root === "wakes") return [{ kind: "conversations", id: sessionFor(root, first) }];
  if ((root === "attachments" || root === "macAttachments") && first.startsWith("ses_"))
    return [{ kind: "sessionFiles", id: first }];
  return [];

}

export function stat(file: string): fs.Stats | undefined {
  return fs.lstatSync(file, { throwIfNoEntry: false });
}

export function* tree(file: string, name: string, include: (name: string) => boolean): Generator<{ file: string; name: string }> {
  const info = stat(file);
  if (info?.isFile()) yield { file, name };
  if (info?.isDirectory()) for (const child of fs.readdirSync(file).sort()) {
    if (include(child)) yield* tree(path.join(file, child), `${name}/${child}`, include);
  }
}

/** An explicit item manifest selects its payloads, including temporary-looking names.
 * Older items without a file list retain the ordinary durable-file walk. */
type Expected = { expectedSize?: number; expectedSHA?: string };
function queueFileManifests(base: string): Map<string, Map<string, Expected>> {
  const lists = new Map<string, Map<string, Expected>>();
  const run = path.join(base, "run"), queue = path.join(run, "work-queue");
  const manifests = path.join(queue, "queues");
  if (![base, run, queue, manifests].every(folder => stat(folder)?.isDirectory())) return lists;
  for (const manifest of fs.readdirSync(manifests).sort()) {
    const file = path.join(manifests, manifest);
    if (temporary(manifest) || !manifest.endsWith(".json") || !stat(file)?.isFile()) continue;
    const saved = parsed(fs.readFileSync(file));
    if (!isObject(saved) || !Array.isArray(saved.items)) throw new Error(`Unreadable Work Queue inventory: ${manifest}.`);
    for (const item of saved.items) {
      if (!isObject(item) || typeof item.id !== "string" || !ITEM_ID.test(item.id) || !Array.isArray(item.files)) continue;
      const names = lists.get(item.id) ?? new Map<string, Expected>();
      lists.set(item.id, names);
      for (const attachment of item.files) {
        if (!isObject(attachment) || typeof attachment.name !== "string" || !FILE_NAME.test(attachment.name)) continue;
        const size = attachment.size;
        const digest = attachment.sha256;
        const expectedSize = typeof size === "number" && Number.isSafeInteger(size) && size >= 0 ? size : undefined;
        if (digest !== undefined && (typeof digest !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(digest))) {
          throw new Error(`Invalid Work Queue attachment digest: ${item.id}/${attachment.name}.`);
        }
        const prior = names.get(attachment.name);
        if ((prior?.expectedSize !== undefined && expectedSize !== undefined && prior.expectedSize !== expectedSize) ||
            (prior?.expectedSHA !== undefined && digest !== undefined && prior.expectedSHA !== digest)) {
          throw new Error(`Conflicting Work Queue attachment inventory: ${item.id}/${attachment.name}.`);
        }
        names.set(attachment.name, { expectedSize: expectedSize ?? prior?.expectedSize, expectedSHA: digest ?? prior?.expectedSHA });
      }
    }
  }
  return lists;
}

type Payload = Expected & { name: string; file: string | null };

function* queuePayloads(base: string): Generator<Payload> {
  const manifests = queueFileManifests(base);
  const run = path.join(base, "run"), queue = path.join(run, "work-queue"), files = path.join(queue, "files");
  const readable = [base, run, queue, files].every(folder => stat(folder)?.isDirectory());
  for (const id of [...manifests.keys()].sort()) {
    const folder = path.join(files, id);
    const present = readable && stat(folder)?.isDirectory();
    const names = manifests.get(id)!;
    for (const leaf of [...names.keys()].sort()) {
      const file = present ? path.join(folder, leaf) : null;
      yield { name: `${queueFileRoot}/${id}/${leaf}`, file: file && stat(file)?.isFile() ? file : null, ...names.get(leaf) };
    }
  }
  // Legacy items without a file list keep the old walk. An explicit list (including
  // an empty one) excludes unlisted bytes left behind by an interrupted queue write.
  if (readable) for (const id of fs.readdirSync(files).sort()) {
    if (!manifests.has(id) && !temporary(id)) yield* tree(path.join(files, id), `${queueFileRoot}/${id}`, leaf => !temporary(leaf));
  }
}

export type InventoryEntry = Payload & { kind: DataKind; snapshot: BackupSnapshot };

/** A null file is a promised payload whose regular file is unavailable. */
export function* storageInventory(layout: Readonly<Record<string, string | null>> = roots,
  include: (kind: DataKind) => boolean = () => true): Generator<InventoryEntry> {
  const seen = new Set<string>();
  for (const { kind, root, parts, tree: recursive, snapshot, inventory } of selectors) {
    if (kind !== "legacyMutationEvidence" && !include(kind)) continue;
    const base = layout[root];
    if (!base) continue;
    function* walk(file: string, name: string, at: number): Generator<{ file: string; name: string }> {
      if (at === parts.length) {
        // These dedicated roots hold the session payloads themselves, including
        // Mac drafts and the older flat layout. They have no separate file list.
        if (recursive) {
          if (stat(file)?.isDirectory()) yield* tree(file, name, child => inventory === "session payloads" || !temporary(child));
        } else if (stat(file)?.isFile()) yield { file, name };
        return;
      }
      if (!stat(file)?.isDirectory()) return;
      for (const child of fs.readdirSync(file).sort()) {
        if (!temporary(child) && parts[at]!.test(child)) yield* walk(path.join(file, child), `${name}/${child}`, at + 1);
      }
    }
    if (kind === "managedArtifacts" && !seen.has("artifacts")) {
      seen.add("artifacts");
      for (const [name, expected] of retainedArtifactCopies(base)) {
        const file = outputPath(base, name.split("/").slice(1));
        const found = stat(file);
        seen.add(name);
        yield { name, file: found?.isFile() ? file : null, ...expected, kind, snapshot };
      }
    }
    for (const item of inventory === "queue payloads" ? queuePayloads(base) : walk(base, root, 0)) {
      if (seen.has(item.name)) continue;
      seen.add(item.name);
      const key = MUTATION_EVIDENCE.exec(item.name)?.[1];
      const actual = key && item.file ? mutationEvidenceKind(item.file, key) : kind;
      if (DATA_DECLARATIONS[actual].backup && include(actual)) yield { ...item, kind: actual, snapshot };
    }
  }
}

export function inside(parent: string, child: string): boolean {
  const relative = path.relative(parent, child);
  return !relative || (!relative.startsWith(".." + path.sep) && relative !== ".." && !path.isAbsolute(relative));
}

/** Walk before writing so a link elsewhere cannot turn a restored file into an overwrite. */
export function outputPath(base: string, parts: string[]): string {
  const root = path.resolve(base);
  const target = path.resolve(root, ...parts);
  if (target === root || !inside(root, target)) throw new Error("Invalid recovery path.");
  for (let at = path.dirname(target); inside(path.resolve(base), at); at = path.dirname(at)) {
    const info = stat(at);
    if (info && !info.isDirectory()) throw new Error("The recovery path contains a link or non-directory.");
    if (at === root) break;
  }
  return target;
}
