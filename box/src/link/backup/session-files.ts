import fs from "node:fs";
import path from "node:path";
import { visitAttachmentPaths, type AttachmentFormat } from "../../shared/attachment-paths";
import { isObject, parsed } from "../../shared/json";
import { writeDurablePrivate } from "../files";
import { recordKey } from "../ownership/store";
import { restoreEngineAttachmentPaths, scrubEngine } from "./sqlite";
import { inside, outputPath, stat } from "./locations";

export function attachmentRecovery(saved: unknown, recovery: string, unavailable: { entry: string; reason: string }[]) {
  const names = ["attachments", "macAttachments"] as const;
  const sources = new Map(names.flatMap(name => {
    const folder = isObject(saved) ? saved[name] : null;
    return typeof folder === "string" && path.isAbsolute(folder) ? [[name, path.resolve(folder)] as const] : [];
  }));
  const placed = new Map<string, string>();
  const reported = new Set(unavailable.map(item => item.entry));
  return {
    placed,
    add(root: string, parts: string[], target: string) {
      const source = sources.get(root as typeof names[number]);
      if (source) placed.set(path.join(source, ...parts), target);
    },
    rewrite(record: unknown, format: AttachmentFormat = "transcript"): unknown {
      if (!sources.size) return record;
      return visitAttachmentPaths(record, format, ({ path: file, stayed }) => {
        if (stayed || file.includes("\0")) return undefined;
        const target = placed.get(file);
        if (target) return target;
        for (const [root, source] of sources) {
          if (file === source || !inside(source, file)) continue;
          const entry = root + "/" + path.relative(source, file);
          if (!reported.has(entry)) {
            reported.add(entry);
            unavailable.push({ entry, reason: "referenced session attachment has no restored copy (missing, expired or moved)" });
          }
          // The old absolute address can belong to unrelated replacement data.
          const missing = outputPath(recovery, ["unavailable-attachments", recordKey(entry), path.basename(file)]);
          if (stat(missing)) throw new Error("An unavailable attachment address is occupied.");
          return missing;
        }
        return undefined;
      });
    },
  };
}

/** Rewrite the staged engine database and return any newly missing attachments. */
export async function restoreEngineDatabase(file: string, fences: string[], saved: unknown, recovery: string, placed: Map<string, string>,
  unavailable: { entry: string; reason: string }[]) {
  scrubEngine(file, fences, true);
  if (saved) {
    const attachments = attachmentRecovery(saved, recovery, unavailable);
    for (const [from, to] of placed) attachments.placed.set(from, to);
    await restoreEngineAttachmentPaths(file, attachments.rewrite);
  }
  return unavailable;
}

/** Resolve V1 file addresses before allowing them into the engine's live storage. */
export function legacyEngineSession(name: string, file: string): string | undefined {
  const match = /^engine\/storage\/(session|message|part)\/([^/]+)\/([^/]+)\.json$/.exec(name);
  if (!match) return undefined;
  const [, kind, parent, id] = match;
  const record = parsed(fs.readFileSync(file));
  const session = kind === "session" ? id : kind === "message" ? parent : isObject(record) && record.sessionID;
  if (typeof session !== "string" || !/^ses_[A-Za-z0-9_-]{1,95}$/.test(session) || !isObject(record)) return undefined;
  if (typeof record.id === "string" && record.id !== id) return undefined;
  if (kind === "message" && record.sessionID !== undefined && record.sessionID !== session) return undefined;
  if (kind === "part" && record.messageID !== parent) return undefined;
  return session;
}

/** V1 JSON stores parts by message ID and parent records by session and message ID. */
export function restoreLegacyEngineParts(entries: ReadonlyMap<string, string>, rewrite: ReturnType<typeof attachmentRecovery>["rewrite"]): void {
  const read = (file: string) => parsed(fs.readFileSync(file));
  const save = (file: string, before: unknown, after: unknown) => {
    if (after !== before) writeDurablePrivate(file, JSON.stringify(after));
  };
  for (const [name, file] of entries) {
    if (!name.startsWith("engine/storage/part/") || !name.endsWith(".json")) continue;
    const value = read(file);
    let mapped: unknown;
    if (isObject(value) && value.type === "text" && typeof value.sessionID === "string" && typeof value.messageID === "string" &&
        /^ses_[A-Za-z0-9_-]+$/.test(value.sessionID) && /^msg_[A-Za-z0-9_-]+$/.test(value.messageID) &&
        name.split("/")[3] === value.messageID) {
      const parentFile = entries.get(`engine/storage/message/${value.sessionID}/${value.messageID}.json`);
      const parent = parentFile && read(parentFile);
      if (parentFile && isObject(parent) && parent.role === "user") {
        const envelope = { info: parent, parts: [value] };
        const rewritten = rewrite(envelope, "transcript") as typeof envelope;
        save(parentFile, parent, rewritten.info);
        mapped = rewritten.parts[0];
      }
    } else mapped = rewrite(value, "part");
    if (mapped !== undefined) save(file, value, mapped);
  }
}
