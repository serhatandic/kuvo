// A file on this box that an agent's reply or tool names (PROTOCOL.md, `kuvo/file`): its bytes, by its
// `file://` address, for a client to show, a machine's with the Mac closed too; and its text, where
// it's text, for a viewer (`kuvo/file/text`); or its metadata (`kuvo/file/info`).

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { FileInfo, FileKind, FileText } from "../../shared/routes/box";
import type { AnswerPiece, RouteContext } from "../server/context";

// The biggest file `kuvo/file` sends.
export const FILE_LIMIT = 25 * 1024 * 1024;
// The biggest `kuvo/file/text` reads into one answer (as much as one file's diff, `kuvo/git/changes`).
export const TEXT_LIMIT = 4 * 1024 * 1024;

const TYPES: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", heic: "image/heic",
  heif: "image/heif", bmp: "image/bmp", tif: "image/tiff", tiff: "image/tiff", ico: "image/vnd.microsoft.icon",
  svg: "image/svg+xml", pdf: "application/pdf", mp4: "video/mp4", mov: "video/quicktime", mp3: "audio/mpeg",
  wav: "audio/wav", txt: "text/plain", md: "text/markdown", csv: "text/csv", json: "application/json",
  html: "text/html", css: "text/css", js: "text/javascript", xml: "application/xml",
  ts: "text/plain", tsx: "text/plain", jsx: "text/plain", rs: "text/plain", py: "text/plain",
  sh: "text/plain", zsh: "text/plain", bash: "text/plain", log: "text/plain",
  yaml: "text/yaml", yml: "text/yaml", toml: "text/plain",
};

function kindOf(mime: string): FileKind {
  if (mime.startsWith("image/")) return "picture";
  if (mime === "application/pdf") return "pdf";
  if (mime.startsWith("text/") || mime === "application/json" || mime === "application/xml") return "text";
  return "other";
}

// `kuvo/file?uri=<file:// URI>[&max=<bytes>]`: a file on this machine, as an agent's reply or tool
// names it: its bytes with its type; 404 with a message for one that isn't there (or isn't a
// file), 413 over FILE_LIMIT (as text, over TEXT_LIMIT), or over `max` (all a client would show of it).
export async function* fileAt(requestPath: string, json: RouteContext["json"], as: "bytes" | "text" | "info" = "bytes"): AsyncGenerator<AnswerPiece> {
  const query = requestPath.includes("?") ? requestPath.slice(requestPath.indexOf("?") + 1) : "";
  const asked = new URLSearchParams(query);
  const uri = asked.get("uri");
  const most = as === "text" ? TEXT_LIMIT : FILE_LIMIT;
  // (A whole number of bytes, whatever was sent.)
  const given = Math.floor(Number(asked.get("max") ?? most));
  const max = Number.isFinite(given) ? given : most;
  let file;
  try {
    file = uri ? fileURLToPath(uri) : undefined;
  } catch {}
  if (!file) return yield* json(400, { message: "That isn’t a file:// address." });
  const name = path.basename(file);
  const missing = () => as === "info"
    ? json(200, { exists: false, name, size: null, kind: null, mime: null } satisfies FileInfo)
    : json(404, { message: "That file isn’t there anymore.", reason: "fileNotFound" });
  const inaccessible = (error: unknown) => {
    const code = (error as NodeJS.ErrnoException).code;
    if (code === "ENOENT" || code === "ENOTDIR") return missing();
    return json(403, { message: "That file can’t be opened here." });
  };
  try {
    // Avoid opening known directories or special files. Check the opened handle again below.
    if (!(await fs.promises.stat(file)).isFile()) return yield* missing();
  } catch (error) {
    return yield* inaccessible(error);
  }
  const extension = path.extname(file).slice(1).toLowerCase();
  const type = Object.hasOwn(TYPES, extension) ? TYPES[extension] : "application/octet-stream";
  // Opened before it's answered: one that can't be read says so, with no path in it.
  let handle;
  try {
    handle = await fs.promises.open(file, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK);
  } catch (error) {
    return yield* inaccessible(error);
  }
  try {
    let opened;
    try {
      opened = await handle.stat();
    } catch (error) {
      return yield* inaccessible(error);
    }
    if (!opened.isFile()) return yield* missing();
    if (as === "info") return yield* json(200, { exists: true, name, size: opened.size, kind: kindOf(type), mime: type } satisfies FileInfo);
    if (opened.size > most) return yield* json(413, { message: as === "text" ? "That file is too big to show here." : "That file is too big to open here." });
    if (opened.size > max) return yield* json(413, { message: "That file is too big to show here." });
    if (as === "text") {
      // Read one extra byte to detect a file that grew or reports no size (as /proc's do).
      const end = Math.min(max, most);
      const pieces: Buffer[] = [];
      let read = 0;
      try {
        for await (const piece of handle.createReadStream({ end, highWaterMark: 64 * 1024 })) {
          pieces.push(piece as Buffer);
          read += (piece as Buffer).length;
        }
      } catch {
        return yield* json(403, { message: "That file can’t be opened here." });
      }
      if (read > end) return yield* json(413, { message: "That file is too big to show here." });
      return yield* json(200, textOf(Buffer.concat(pieces)));
    }
    yield { head: { status: 200, headers: { "content-type": type, "content-length": String(opened.size) } } };
    if (opened.size === 0) return;
    // Stream the advertised length even if the file grows, and break the response if it shrinks.
    let read = 0;
    for await (const piece of handle.createReadStream({ end: opened.size - 1, highWaterMark: 32 * 1024 })) {
      read += (piece as Buffer).length;
      yield { body: piece };
    }
    if (read !== opened.size) throw new Error("The file changed while it was being read.");
  } finally {
    await handle.close().catch(() => {});
  }
}
/** Whether a file is text a viewer shows, and its text (`kuvo/file/text`, `file.text`): UTF-8 with
 * no NUL byte in it, the only encoding read here (so a UTF-16 file, a picture or a program isn't);
 * a mark at its start saying so (a BOM) isn't part of the text. */
export function textOf(bytes: Buffer): FileText {
  if (bytes.includes(0)) return { text: false };
  try {
    return { text: true, encoding: "utf-8", content: new TextDecoder("utf-8", { fatal: true }).decode(bytes) };
  } catch {
    return { text: false };
  }
}
