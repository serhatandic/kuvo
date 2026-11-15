// What this box gives a computer it sets up (PROTOCOL.md, "Computers", Adding one): the files of
// its own release, as they are in its own folder (every box has the same: "Updates"), packed as one
// tar for the other side to unpack and check; and the engine, its own binary where the two
// computers are the same kind, else the package its release pins for the other's, fetched and
// checked here. The computer being set up downloads no code.
// Platform: the same on macOS and Linux; the engine's builds are named by `uname -sm`.

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import pin from "../../shared/engine-pin.json";
import { fetched } from "../engine/client";
import { K } from "../home";

/** A release's files that every box has, by their paths in its folder, with who may read and run
 * each (`release/files.ts` writes them; a test holds the two lists the same). In the order they're
 * put in place: the keeper's mark and the version last, so one cut short is seen as not done. */
export const RELEASE_FILES: readonly (readonly [path: string, mode: number])[] = [
  ["bin/bot-words.json", 0o644],
  ["bin/integrations-mac.json", 0o644],
  ["bin/integrations.json", 0o644],
  ["bin/kuvo-agents.js", 0o644],
  ["bin/kuvo-browser.js", 0o644],
  ["bin/kuvo-launch", 0o700],
  ["bin/kuvo-link.js", 0o600],
  ["bin/kuvo-npx", 0o755],
  ["bin/kuvo-setsid", 0o700],
  ["bin/kuvo-start", 0o700],
  ["bin/kuvo-wake", 0o755],
  ["bin/mcp-catalog.json", 0o644],
  ["bin/runtime/node", 0o755],
  ["config/opencode/plugins/kuvo-code.js", 0o644],
  ["bin/.kuvo-boot", 0o644],
  ["bin/box-version", 0o644],
];

/** The engine this box's release runs. */
export const ENGINE_VERSION: string = pin.version;

export interface OwnFile {
  path: string;
  mode: number;
  data: Buffer;
  sum: string;
}

const sha256 = (data: Buffer): string => crypto.createHash("sha256").update(data).digest("hex");

/** This box's own release files, read from its folder; null where it hasn't them all (a box an
 * app runs from its own folder, with no release of its own to give). */
export function ownFiles(home = K): OwnFile[] | null {
  const files: OwnFile[] = [];
  for (const [name, mode] of RELEASE_FILES) {
    const file = path.join(home, name);
    try {
      const seen = fs.lstatSync(file);
      if (!seen.isFile() || seen.size > 60_000_000) return null;
      const data = fs.readFileSync(file);
      files.push({ path: name, mode, data, sum: sha256(data) });
    } catch {
      return null;
    }
  }
  return files;
}

// One tar header (ustar): a regular file, with no owner and no time, as a release's pack has them.
function header(name: string, mode: number, size: number): Buffer {
  const block = Buffer.alloc(512);
  block.write(name, 0, 100, "utf8");
  block.write(mode.toString(8).padStart(7, "0") + "\0", 100, 8, "ascii");
  block.write("0000000\0", 108, 8, "ascii");
  block.write("0000000\0", 116, 8, "ascii");
  block.write(size.toString(8).padStart(11, "0") + "\0", 124, 12, "ascii");
  block.write("00000000000\0", 136, 12, "ascii");
  block.write("        ", 148, 8, "ascii");
  block.write("0", 156, 1, "ascii");
  block.write("ustar\0" + "00", 257, 8, "ascii");
  let sum = 0;
  for (const byte of block) sum += byte;
  block.write(sum.toString(8).padStart(6, "0") + "\0 ", 148, 8, "ascii");
  return block;
}

/** `files` as one tar, each by its path (shorter than a header's 100 bytes: a release's all are). */
export function tarOf(files: readonly Pick<OwnFile, "path" | "mode" | "data">[]): Buffer {
  const parts: Buffer[] = [];
  for (const file of files) {
    if (Buffer.byteLength(file.path) >= 100) throw new Error(`${file.path} is too long a name for the pack.`);
    parts.push(header(file.path, file.mode, file.data.length), file.data, Buffer.alloc((512 - (file.data.length % 512)) % 512));
  }
  parts.push(Buffer.alloc(1024));
  return Buffer.concat(parts);
}

/** A computer's kind, as `uname -sm` says it there, as the engine's builds are named. */
export interface Kind {
  system: "linux" | "darwin";
  machine: "x86_64" | "aarch64";
}

/** This computer's own kind; null for one the engine has no build for. */
export function ownKind(): Kind | null {
  const system = process.platform === "darwin" ? "darwin" : process.platform === "linux" ? "linux" : null;
  const machine = ({ x64: "x86_64", arm64: "aarch64" } as const)[os.arch() as "x64" | "arm64"];
  return system && machine ? { system, machine } : null;
}

/** The engine for a computer of `kind`, as a file to send: this box's own binary where the two are
 * the same kind (`packed: false`), else the pinned package for it (`packed: true`, the binary at
 * `package/bin/opencode` in it), fetched once and kept in this box's cache, checked against the
 * release's own sum each time it's used. `step`: told the fraction fetched isn't known, only that
 * it's being fetched. Throws with words for what couldn't be had. */
export async function engineFor(kind: Kind, fetching: () => void = () => {}, home = K): Promise<{ file: string; packed: boolean; sum: string; size: number }> {
  const own = ownKind();
  const binary = path.join(home, "engine", ENGINE_VERSION, "opencode");
  if (own && own.system === kind.system && own.machine === kind.machine && fs.existsSync(binary)) {
    return { file: binary, packed: false, sum: await sumOfFile(binary), size: fs.statSync(binary).size };
  }
  const build = (pin as unknown as Record<string, Record<string, { url: string; integrity: string } | undefined> | undefined>)[kind.system]?.[kind.machine];
  if (!build?.url || !build.integrity.startsWith("sha512-")) throw new Error("This Kuvo Code has no engine for that kind of computer.");
  const folder = path.join(home, "cache", "engine-packs");
  const file = path.join(folder, path.basename(new URL(build.url).pathname));
  const whole = (data: Buffer): boolean => crypto.createHash("sha512").update(data).digest("base64") === build.integrity.slice(7);
  let data: Buffer | null = null;
  try { data = fs.readFileSync(file); } catch {}
  if (!data || !whole(data)) {
    fetching();
    try {
      data = await fetched(build.url, 400_000_000, 900_000);
    } catch (error) {
      throw new Error(`The engine for it couldn't be downloaded here (${error instanceof Error ? error.message : String(error)}) Check this computer's connection, then try again.`);
    }
    if (!whole(data)) throw new Error("The engine that was downloaded isn't what this Kuvo Code's release says. Try again.");
    fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
    // (Older versions' packages go: one is kept.)
    for (const old of fs.readdirSync(folder)) if (old !== path.basename(file)) fs.rmSync(path.join(folder, old), { force: true });
    fs.writeFileSync(`${file}.new`, data, { mode: 0o600 });
    fs.renameSync(`${file}.new`, file);
  }
  return { file, packed: true, sum: sha256(data), size: data.length };
}

function sumOfFile(file: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash("sha256");
    fs.createReadStream(file).on("data", chunk => hash.update(chunk)).on("error", reject).on("end", () => resolve(hash.digest("hex")));
  });
}
