import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { syncDurableFile, syncDurableTree, writeDurablePrivate } from "../../shared/files";

const TARGET = /^versions\/\d+\.\d+\.\d+(?:-beta\.\d+)?-[0-9a-f]{12}\/bin$/;
export const previousBin = (root: string) => path.join(root, "run", "previous");
const pluginPath = "config/opencode/plugins";
const helper = "Kuvo Helper.app";

export function activeBin(root: string): string | null {
  try {
    const target = fs.readlinkSync(path.join(root, "bin"));
    if (TARGET.test(target)) return target;
    throw new Error("Kuvo’s bin link has an unexpected target.");
  } catch (error) {
    if (["ENOENT", "EINVAL"].includes((error as NodeJS.ErrnoException).code ?? "")) return null;
    throw error;
  }
}

export function versionBin(version: string, sha256: string): string {
  const target = `versions/${version}-${sha256.slice(0, 12)}/bin`;
  if (!TARGET.test(target)) throw new Error("Kuvo’s release has an invalid version or digest.");
  return target;
}

// The first conversion uses the OS's atomic exchange of two directory entries. Both names must
// be on the same filesystem. A failed exchange leaves the original directory at its launch path.
async function exchange(a: string, b: string): Promise<void> {
  const { dlopen, FFIType } = await import("bun:ffi");
  const mac = process.platform === "darwin";
  const name = mac ? "renamex_np" : "renameat2";
  const { symbols, close } = dlopen(mac ? "/usr/lib/libSystem.B.dylib" : "libc.so.6", {
    [name]: {
      args: mac ? [FFIType.ptr, FFIType.ptr, FFIType.u32]
        : [FFIType.i32, FFIType.ptr, FFIType.i32, FFIType.ptr, FFIType.u32],
      returns: FFIType.i32,
    },
  });
  try {
    const from = Buffer.from(a + "\0"), to = Buffer.from(b + "\0");
    if ((mac ? symbols[name]!(from, to, 2) : symbols[name]!(-100, from, -100, to, 2)) !== 0) {
      throw new Error("This filesystem couldn’t exchange Kuvo’s old directory with its link.");
    }
  } finally { close(); }
}

function freshLink(link: string, target: string): void {
  fs.rmSync(link, { force: true });
  fs.symlinkSync(target, link);
}

// LaunchAgents keep their exact bin/Kuvo Helper.app path. Keep the enrolled, signed
// supervisor's backing directory and inode fixed as release workers switch under it.
async function linkHelper(root: string, bin: string): Promise<void> {
  if (process.platform !== "darwin") return;
  const file = path.join(bin, helper);
  const stat = fs.lstatSync(file, { throwIfNoEntry: false });
  if (stat?.isSymbolicLink()) return;
  const fixed = path.join(root, "helper", helper);
  if (!fs.existsSync(fixed)) {
    if (!stat?.isDirectory()) return;
    fs.mkdirSync(path.dirname(fixed), { recursive: true, mode: 0o700 });
    freshLink(fixed, fixed);
    syncDurableTree(file, path.dirname(file));
    syncDurableFile(path.dirname(fixed), root);
    await exchange(file, fixed);
    syncDurableFile(path.dirname(fixed), root);
    syncDurableFile(bin, root);
  } else if (!stat) {
    fs.symlinkSync(fixed, file);
  } else {
    // The pack can carry a new copy. Launch this box's already enrolled supervisor.
    const next = file + ".new";
    freshLink(next, fixed);
    await exchange(file, next);
    fs.rmSync(next, { recursive: true, force: true });
  }
}

export async function convertBin(root: string, version: string, sha256?: string): Promise<string> {
  const linked = activeBin(root);
  if (linked) return linked;
  const bin = path.join(root, "bin");
  const digest = sha256 ?? crypto.createHash("sha256").update(fs.readFileSync(path.join(bin, "kuvo-start"))).digest("hex");
  const target = versionBin(version, digest);
  const saved = path.join(root, target);
  await linkHelper(root, bin);
  const plugins = path.join(root, pluginPath);
  const local = path.join(bin, "opencode-plugins");
  fs.mkdirSync(path.dirname(plugins), { recursive: true, mode: 0o700 });
  if (!fs.lstatSync(plugins, { throwIfNoEntry: false })?.isSymbolicLink()) {
    // Until plugins is exchanged, the engine still sees its original directory.
    fs.rmSync(local, { force: true, recursive: true });
    // The running engine uses the generated sandbox plug-in's mtime to tell whether it has loaded it.
    if (fs.existsSync(plugins)) fs.cpSync(plugins, local, { recursive: true, verbatimSymlinks: true, preserveTimestamps: true });
    else fs.mkdirSync(local, { mode: 0o700 });
    syncDurableTree(local, root);
    const pluginLink = "../../bin/opencode-plugins";
    if (fs.existsSync(plugins)) {
      const next = plugins + ".kuvo-new";
      freshLink(next, pluginLink);
      await exchange(plugins, next);
      syncDurableFile(path.dirname(plugins), root);
    } else fs.symlinkSync(pluginLink, plugins);
  }
  fs.rmSync(plugins + ".kuvo-new", { recursive: true, force: true });
  fs.mkdirSync(path.dirname(saved), { recursive: true, mode: 0o700 });
  // The link is relative to bin's final location. After exchange the original directory is
  // already at the link's target. At every point the unchanged @reboot entry can run.
  freshLink(saved, target);
  syncDurableTree(bin, root);
  syncDurableFile(path.dirname(saved), root);
  await exchange(bin, saved);
  syncDurableFile(path.dirname(saved), root);
  return target;
}

export async function stageVersion(root: string, unpacked: string, target: string, receipt: string, rust = false): Promise<void> {
  const bin = path.join(unpacked, "bin");
  const plugins = path.join(unpacked, pluginPath);
  const local = path.join(bin, "opencode-plugins");
  if (fs.existsSync(plugins)) fs.renameSync(plugins, local);
  else fs.mkdirSync(local, { mode: 0o700 });
  await linkHelper(root, bin);
  writeDurablePrivate(path.join(bin, ".installed.json"), receipt);
  // The legacy keeper notices the change by this token, including a rebuild under the same version.
  if (!fs.existsSync(path.join(bin, ".kuvo-boot"))) writeDurablePrivate(path.join(bin, ".kuvo-boot"), target + "\n");
  const saved = path.join(root, target);
  fs.mkdirSync(path.dirname(saved), { mode: 0o700, recursive: true });
  if (activeBin(root) === target) throw new Error("That release is already selected.");
  const from = rust ? unpacked : bin;
  const destination = rust ? path.dirname(saved) : saved;
  fs.rmSync(destination, { recursive: true, force: true });
  syncDurableTree(from, rust ? path.dirname(unpacked) : unpacked);
  fs.renameSync(from, destination);
  syncDurableFile(path.dirname(destination), root);
}

export function pruneVersions(root: string, keep: string[]): void {
  const versions = path.join(root, "versions");
  for (const name of fs.readdirSync(versions)) {
    const target = `versions/${name}/bin`;
    if (TARGET.test(target) && !keep.includes(target)) fs.rmSync(path.join(versions, name), { recursive: true, force: true });
  }
}
