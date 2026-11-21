import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isObject } from "./json";
import { ATTACHED_BLOCK, ATTACHED_HEADINGS, FROM_PHONE_BLOCK, UNSENT_SUFFIX } from "./message-block";

export type AttachmentFormat = "transcript" | "queues" | "prompt" | "part";
export type AttachmentReference = { path: string; stayed: boolean };
type Resolve = (reference: AttachmentReference) => string | undefined;
type RecordValue = Record<string, unknown>;

function mapNote(text: string, resolve: Resolve): string {
  const phone = FROM_PHONE_BLOCK.startOf(text) ?? text.length;
  return ATTACHED_BLOCK.mapLines(text.slice(0, phone), lines => {
    let stayed = false;
    return lines.map(line => {
      if (ATTACHED_HEADINGS.some(heading => heading === line)) {
        stayed = line === ATTACHED_HEADINGS[2];
        return line;
      }
      const unsent = line.endsWith(UNSENT_SUFFIX);
      const file = unsent ? line.slice(0, -UNSENT_SUFFIX.length) : line;
      if (!path.isAbsolute(file)) return line;
      const target = resolve({ path: file, stayed: stayed || unsent });
      return !stayed && !unsent && target ? target : line;
    });
  }) + text.slice(phone);
}

export function attachmentNote(text: string): AttachmentReference[] {
  const files: AttachmentReference[] = [];
  mapNote(text, file => { files.push(file); return undefined; });
  return files;
}

function mapFields(record: RecordValue, fields: RecordValue): RecordValue {
  return Object.entries(fields).some(([key, value]) => value !== record[key]) ? { ...record, ...fields } : record;
}

function mapArray(value: unknown, map: (entry: unknown) => unknown): unknown {
  if (!Array.isArray(value)) return value;
  const mapped = value.map(map);
  return mapped.some((entry, index) => entry !== value[index]) ? mapped : value;
}

function mapFile(value: unknown, resolve: Resolve, invalid?: () => void): unknown {
  if (!isObject(value)) return value;
  const address = isObject(value.source) && typeof value.source.uri === "string" ? value.source : value;
  const key = typeof address.uri === "string" ? "uri" : "url";
  const given = address[key];
  if (typeof given !== "string") return value;
  const uri = /^file:/i.test(given);
  let file = given;
  if (uri) {
    try { file = fileURLToPath(given); } catch { invalid?.(); return value; }
  }
  // Older native records store the absolute filename directly in source.uri.
  if (!path.isAbsolute(file)) return value;
  if (/[\0\r\n]/.test(file)) { invalid?.(); return value; }
  const target = resolve({ path: file, stayed: false });
  if (!target || target === file) return value;
  let replacement = target;
  if (uri) {
    const original = new URL(given);
    const updated = pathToFileURL(target);
    updated.search = original.search;
    updated.hash = original.hash;
    replacement = updated.href;
  }
  const mapped = { ...address, [key]: replacement };
  return address === value ? mapped : { ...value, source: mapped };
}

/** Visit attachment-bearing fields in the particular storage format. No text search or
 * recursion through unknown fields, tool input, command output or quoted evidence. */
export function visitAttachmentPaths<T>(value: T, format: AttachmentFormat, resolve: Resolve, invalid?: () => void): T {
  const file = (part: unknown) => mapFile(part, resolve, invalid);
  const keptParts = (given: unknown) => isObject(given) && isObject(given.kuvo) && isObject(given.kuvo.parts)
    ? given.kuvo.parts : undefined;
  const note = (text: unknown, notes: Map<string, string>, words: unknown): unknown => {
    if (typeof text !== "string") return text;
    const beforePhone = FROM_PHONE_BLOCK.startOf(text);
    if (typeof words === "string" && (text === words || text.slice(0, beforePhone ?? text.length).trim() === words.trim())) return text;
    return mapNote(text, ref => {
      const to = resolve(ref);
      if (!ref.stayed && to) notes.set(ref.path, to);
      return to;
    });
  };
  const metadata = (given: unknown, notes: Map<string, string>) => {
    if (!isObject(given) || !isObject(given.kuvo)) return given;
    const parts = keptParts(given);
    if (!parts) return given;
    const updated = mapFields(parts, { paths: mapArray(parts.paths, entry => typeof entry === "string" ? notes.get(entry) ?? entry : entry) });
    return updated === parts ? given : { ...given, kuvo: { ...given.kuvo, parts: updated } };
  };
  const part = (entry: unknown, user: boolean, notes = new Map<string, string>(), words?: unknown): unknown => {
    if (!isObject(entry)) return entry;
    if (entry.type === "file") return file(entry);
    if (entry.type === "tool" && isObject(entry.state)) {
      return mapFields(entry, { state: mapFields(entry.state, {
        content: mapArray(entry.state.content, item => isObject(item) && item.type === "file" ? file(item) : item),
      }) });
    }
    return user && entry.type === "text" ? mapFields(entry, { text: note(entry.text, notes, words) }) : entry;
  };
  const message = (entry: unknown, prompt = false): unknown => {
    if (!isObject(entry)) return entry;
    if (!prompt && (entry.type === "file" || entry.type === "tool")) return part(entry, false);
    const info = !prompt && isObject(entry.info) ? entry.info : undefined;
    if (info && Array.isArray(entry.messages)) return mapFields(entry, { messages: mapArray(entry.messages, one => message(one)) });
    const role = info?.role ?? entry.role ?? entry.type;
    const user = prompt || role === "user" || (role === undefined && typeof entry.id === "string" && /^msg_/.test(entry.id) && typeof entry.text === "string");
    const notes = new Map<string, string>();
    const words = keptParts(entry.metadata)?.words ?? keptParts(info?.metadata)?.words;
    const fields = {
      text: user ? note(entry.text, notes, words) : entry.text,
      files: user ? mapArray(entry.files, file) : entry.files,
      parts: prompt ? entry.parts : mapArray(entry.parts, one => part(one, user, notes, words)),
      content: prompt ? entry.content : mapArray(entry.content, one => part(one, user, notes, words)),
      metadata: user ? metadata(entry.metadata, notes) : entry.metadata,
    };
    return mapFields(entry, info && user ? { ...fields, info: mapFields(info, { metadata: metadata(info.metadata, notes) }) } : fields);
  };
  if (format === "part") return part(value, false) as T;
  if (format === "prompt") return message(value, true) as T;
  if (format === "queues" && isObject(value)) {
    return mapFields(value, Object.fromEntries(Object.entries(value).map(([id, queue]) => [id, isObject(queue)
      ? mapFields(queue, { waiting: mapArray(queue.waiting, entry => isObject(entry)
        ? mapFields(entry, { message: message(entry.message, true) }) : entry) }) : queue]))) as T;
  }
  return (Array.isArray(value) ? mapArray(value, entry => Array.isArray(entry)
    ? visitAttachmentPaths(entry, "transcript", resolve, invalid) : message(entry)) : message(value)) as T;
}

export function mapAttachmentPaths<T>(value: T, paths: ReadonlyMap<string, string>, format: AttachmentFormat): T {
  return paths.size ? visitAttachmentPaths(value, format, ref => ref.stayed ? undefined : paths.get(ref.path)) : value;
}
