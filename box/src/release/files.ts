import { boxFilesFormat } from "../shared/release-formats";
// What a box's release holds (PROTOCOL.md, "Updates"): every file a version puts in a box's
// `~/.kuvo-code` that's the same on every box, laid out as that folder is, and `box.json`, saying
// what's there and which engine goes with it. All of it from the box's own sources, with no app
// built or run: scripts/box-release.sh writes them (`bun box/src/release/files.ts <checkout>
// <version> <folder>`), then packs and signs them.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { integrationsCatalog, mcpCatalog } from "../shared/integrations/catalogs";

/** The engine a version of the box runs (`shared/engine-pin.json`): its version, and its builds
 * for Linux and for Macs, by `uname -m`'s kinds, each with npm's `dist.integrity` for it. */
export interface EnginePin {
  version: string;
  linux: Record<string, { url: string; integrity: string }>;
  darwin: Record<string, { url: string; integrity: string }>;
  /** The engine the release before ran (N-1), for the release check (scripts/engine-check.mjs):
   * moved here as the pin moves to a newer engine. */
  previous?: Omit<EnginePin, "previous">;
}

/** One file of a release: its path in `~/.kuvo-code`, who may read and run it, and its bytes. */
export interface BoxFile {
  path: string;
  mode: number;
  data: Buffer;
}

/** A version as a release has one: 1.2.3, or a beta's 1.2.3-beta.4. */
export const RELEASE_VERSION = /^\d+\.\d+\.\d+(-beta\.\d+)?$/;

/** Where the engine's version goes in the keeper's files (`box/src/keeper`). */
const ENGINE = "@ENGINE@";

// The keeper and what it starts with (`box/src/keeper`): the keeper itself, its launcher and what
// starts them in a session of their own (a Mac has no `setsid`), which only you may run; then
// `kuvo-wake`, `kuvo-npx` and the runtime it runs, which the engine's commands run too.
const BOOT = ["kuvo-launch", "kuvo-start", "kuvo-setsid"];
const TOOLS = ["kuvo-wake", "kuvo-npx", "runtime/node"];

const sha256 = (data: Buffer | string): string => crypto.createHash("sha256").update(data).digest("hex");

export function enginePin(root: string): EnginePin {
  const pin = JSON.parse(fs.readFileSync(path.join(root, "box/src/shared/engine-pin.json"), "utf8")) as EnginePin;
  for (const each of [pin, ...(pin.previous ? [pin.previous] : [])]) {
    if (!/^\d+\.\d+\.\d+$/.test(each.version)) throw new Error("box/src/shared/engine-pin.json names no engine version.");
    for (const system of ["linux", "darwin"] as const) {
      for (const kind of ["x86_64", "aarch64"]) {
        const build = each[system]?.[kind];
        if (!build?.url?.endsWith(`-${each.version}.tgz`) || !/^sha512-[A-Za-z0-9+/]{86}==$/.test(build.integrity ?? "")) {
          throw new Error(`box/src/shared/engine-pin.json has no ${system} ${kind} build of ${each.version}, with its integrity.`);
        }
      }
    }
  }
  return pin;
}

/** What the release check of its engines found (scripts/engine-check.mjs, its file named by
 * `ENGINE_CHECK`): the engines its link and bridge answered on, the pinned one first, then the one
 * before; and whether the one before opens what the pinned one wrote. Without a check, the pinned
 * engine alone (a box running another takes it with its engine, once nothing's at work). */
export function engineCheck(pin: EnginePin, file = process.env.ENGINE_CHECK): { engines: string[]; downgradeSafe?: true } {
  if (!file) return { engines: [pin.version] };
  const found = JSON.parse(fs.readFileSync(file, "utf8"));
  const engines: unknown = found?.engines;
  const expected = [pin.version, ...(pin.previous && pin.previous.version !== pin.version ? [pin.previous.version] : [])];
  // (The pinned one alone where its code doesn't run on the one before.)
  const alone = JSON.stringify([pin.version]);
  if (!Array.isArray(engines) || (JSON.stringify(engines) !== JSON.stringify(expected) && JSON.stringify(engines) !== alone)) throw new Error(`${file} isn't the check of engines ${expected.join(" and ")}: run scripts/engine-check.mjs again.`);
  return { engines: engines as string[], ...(found.downgradeSafe === true && engines.length > 1 ? { downgradeSafe: true as const } : {}) };
}

/** Those of the keeper's files that name the engine's version, each once: never a version written
 * in one by hand, which the next engine would leave behind. */
const NAMES_ENGINE = new Set(["kuvo-launch", "kuvo-start", "kuvo-npx", "runtime/node"]);

/** A file of the keeper's, for the pinned engine: its one variable written in. */
function keeperFile(root: string, name: string, engine: string): Buffer {
  const parts = fs.readFileSync(path.join(root, "box/src/keeper", name), "utf8").split(ENGINE);
  if (parts.length !== (NAMES_ENGINE.has(name) ? 2 : 1)) throw new Error(`box/src/keeper/${name} ${NAMES_ENGINE.has(name) ? `says ${ENGINE} ${parts.length - 1} times, not once` : `says ${ENGINE}, which it has no use for`}.`);
  return Buffer.from(parts.join(engine));
}

/** The browser as built (scripts/build-browser.ts): its first line names the rest
 * (`// kuvo-browser <16 hex digits of its SHA-256>`), which a box tells copies apart by. One whose
 * line doesn't (edited by hand, or cut short) goes into no release. */
function browser(root: string, file: string): Buffer {
  const data = built(root, file);
  const end = data.indexOf("\n");
  const mark = /^\/\/ kuvo-browser ([0-9a-f]{16})$/.exec(data.subarray(0, Math.max(end, 0)).toString("utf8"));
  if (!mark || mark[1] !== sha256(data.subarray(end + 1)).slice(0, 16)) throw new Error(`${file} isn't what its first line says: build it again (scripts/build-browser.sh).`);
  return data;
}

/** Read a bundle prepared by scripts/build-built.sh. */
function built(root: string, file: string): Buffer {
  const data = fs.readFileSync(path.join(root, file));
  if (!data.length) throw new Error(`${file} is empty.`);
  return data;
}

/** The agents' bridge as it goes to a box: the bundle, with a last line naming its contents
 * (`// kuvo-agents <16 hex digits of its SHA-256>`), as every release has had it. A Mac from before
 * boxes updated themselves tells by that line whether a machine has this one, and would write its
 * own over one without it. (The box reads the line before it, `shared/bundle.ts`.) */
const marked = (bundle: Buffer): Buffer => Buffer.concat([bundle, Buffer.from(`\n// kuvo-agents ${sha256(bundle).slice(0, 16)}\n`)]);

/** A release's files for `version`, but for `box.json`, in the order they're installed by name. */
export function boxFiles(root: string, version: string): BoxFile[] {
  if (!RELEASE_VERSION.test(version)) throw new Error(`${version} isn't a version (1.2.3, or 1.2.3-beta.4).`);
  const pin = enginePin(root);
  const boot = BOOT.map(name => ({ name, data: keeperFile(root, name, pin.version) }));
  const files: BoxFile[] = [
    ...boot.map(({ name, data }) => ({ path: `bin/${name}`, mode: 0o700, data })),
    // What says which keeper files a box has: a hash over their names and bytes, in this order.
    { path: "bin/.kuvo-boot", mode: 0o644, data: Buffer.from(sha256(Buffer.concat(boot.flatMap(({ name, data }, index) => [Buffer.from(`${index ? "\n" : ""}${name}\n`), data])))) },
    ...TOOLS.map(name => ({ path: `bin/${name}`, mode: 0o755, data: keeperFile(root, name, pin.version) })),
    // The engine's plugin, which gives the commands an agent runs your own folders back.
    { path: "config/opencode/plugins/kuvo-code.js", mode: 0o644, data: fs.readFileSync(path.join(root, "box/src/plugins/kuvo-code.js")) },
    // The link (with the vault's and the phones' keys passing through it, yours alone to read),
    // the coding agents' bridge and the browser.
    { path: "bin/kuvo-link.js", mode: 0o600, data: built(root, "box/dist/kuvo-link.js") },
    { path: "bin/kuvo-agents.js", mode: 0o644, data: marked(built(root, "box/dist/kuvo-agents.js")) },
    { path: "bin/kuvo-browser.js", mode: 0o644, data: browser(root, "box/dist/kuvo-browser.js") },
    // Settings › Integrations and › MCP Servers: a machine's, a Mac's box's, and the popular servers.
    { path: "bin/integrations.json", mode: 0o644, data: Buffer.from(integrationsCatalog(false)) },
    { path: "bin/integrations-mac.json", mode: 0o644, data: Buffer.from(integrationsCatalog(true)) },
    { path: "bin/mcp-catalog.json", mode: 0o644, data: Buffer.from(mcpCatalog()) },
    // What a box makes its bots with and tells them.
    { path: "bin/bot-words.json", mode: 0o644, data: fs.readFileSync(path.join(root, "box/src/shared/bot-words.json")) },
    { path: "bin/box-version", mode: 0o644, data: Buffer.from(version) },
  ];
  return files.sort((a, b) => (a.path < b.path ? -1 : 1));
}

/** `box.json`: the version, its engine and where each system gets it, and each file by its path
 * in `~/.kuvo-code` with its SHA-256. (A Mac's bundles are added as they're packed.) */
export function boxManifest(root: string, version: string, files: BoxFile[]): string {
  const pin = enginePin(root);
  const checked = engineCheck(pin);
  const builds = (system: EnginePin["linux"]) =>
    Object.fromEntries(["aarch64", "x86_64"].map(kind => [kind, { integrity: system[kind]!.integrity, url: system[kind]!.url }]));
  return JSON.stringify({
    formatVersion: boxFilesFormat.version,
    darwin: builds(pin.darwin),
    ...(checked.downgradeSafe ? { downgradeSafe: true } : {}),
    engine: pin.version,
    engines: checked.engines,
    files: Object.fromEntries(files.map(file => [file.path, sha256(file.data)])),
    linux: builds(pin.linux),
    version,
  }, null, 2) + "\n";
}

/** Writes a release's files into `folder` (new, or empty), each with its own mode whatever the
 * umask, and `box.json` beside them. */
export function writeBoxFiles(root: string, version: string, given: string): void {
  const files = boxFiles(root, version);
  // (As the system names it: a path given with `//` or a last `/` in it is the same folder.)
  const folder = path.resolve(given);
  fs.mkdirSync(folder, { recursive: true });
  if (fs.readdirSync(folder).length) throw new Error(`${folder} isn't empty.`);
  // The release's own folders, by their paths in it ("" is `folder` itself): nothing above it is
  // ever touched.
  const folders = new Set<string>([""]);
  for (const file of [...files, { path: "box.json", mode: 0o644, data: Buffer.from(boxManifest(root, version, files)) }]) {
    const parts = file.path.split("/").slice(0, -1);
    parts.forEach((_, index) => folders.add(parts.slice(0, index + 1).join("/")));
    const to = path.join(folder, file.path);
    fs.mkdirSync(path.dirname(to), { recursive: true });
    fs.writeFileSync(to, file.data);
    fs.chmodSync(to, file.mode);
  }
  for (const each of folders) fs.chmodSync(path.join(folder, each), 0o755);
}

if (import.meta.main) {
  const [root, version, folder] = process.argv.slice(2);
  if (!root || !version || !folder) {
    console.error("Usage: bun box/src/release/files.ts <checkout> <version> <folder>");
    process.exit(2);
  }
  try {
    writeBoxFiles(root, version, folder);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
