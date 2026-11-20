import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { DATA_DECLARATIONS } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { remote } from "../add/ssh";
import { exclusiveWhile, syncDurableFile } from "../files";
import { APP_FOLDER, K, located } from "../home";
import { KEY } from "../peers/known";
import { inside } from "./locations";
import { relayBackupAvailable, relayStore } from "./relay-store";

export const SOURCE = /^b_[A-Za-z0-9_-]{22}$/;
export const POINT = /^\d{4}-\d{2}-\d{2}T\d{6}\.\d{3}Z-(daily|move|wipe|update|vault)-[a-f0-9]{16}$/;
export type Destination = { kind: "disk"; path: string } | { kind: "computer"; computer: string; ssh?: string };

export const backupSshAddress = (value: unknown): value is string => typeof value === "string" && /^[A-Za-z0-9_][A-Za-z0-9_.@:\[\]-]{0,254}$/.test(value);

export function destinationOf(value: unknown, { resolveDisk = true } = {}): Destination {
  if (isObject(value) && value.kind === "disk" && typeof value.path === "string" && path.isAbsolute(value.path) && !value.path.includes("\0")) {
    return { kind: "disk", path: resolveDisk ? located(value.path) : value.path };
  }
  if (isObject(value) && value.kind === "computer" && typeof value.computer === "string" && KEY.test(value.computer) &&
      (value.ssh === undefined || backupSshAddress(value.ssh))) {
    return { kind: "computer", computer: value.computer, ...(value.ssh ? { ssh: value.ssh } : {}) };
  }
  throw new Error("Choose an enrolled backup computer or a folder on a connected disk.");
}

export interface DestinationStore {
  /** The accepted setup this store is scoped to, for Relay destinations. */
  readonly setup?: string;
  list(): Promise<string[]>;
  put(id: string, archive: string): Promise<void>;
  get(id: string, scratch: string): Promise<string>;
  erase(id: string): Promise<void>;
}

export function separateDisk(folder: string): boolean {
  return fs.statSync(folder).dev !== fs.statSync(K).dev;
}

function disk(folder: string, source: string): DestinationStore {
  const storage = DATA_DECLARATIONS.diskBackupArchives.externalStorage;
  const selected = fs.realpathSync(folder);
  const data = located(APP_FOLDER ?? K);
  if (inside(data, selected) || inside(selected, data)) throw new Error("Choose a backup folder separate from the Kuvo folder.");
  const root = path.join(selected, storage.root, source);
  // Do not follow a substituted backup subfolder.
  for (const part of [path.dirname(root), root]) {
    if (!fs.lstatSync(part, { throwIfNoEntry: false })) {
      try { fs.mkdirSync(part, { mode: 0o700 }); } catch (error) {
        if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error;
      }
    }
    if (!fs.lstatSync(part).isDirectory() || fs.realpathSync(part) !== part) throw new Error("The backup folder contains a link.");
  }
  const at = (id: string) => {
    if (!POINT.test(id)) throw new Error("Choose a dated backup.");
    return path.join(root, id + storage.archiveSuffix);
  };
  return {
    async list() { return pointNames(fs.readdirSync(root), storage.archiveSuffix); },
    async put(id, archive) {
      const file = at(id);
      await exclusiveWhile(file + ".publishing", async () => {
        if (fs.lstatSync(file, { throwIfNoEntry: false })) throw new Error("That backup point already exists.");
        const temporary = file + "." + crypto.randomBytes(8).toString("hex") + ".partial";
        try {
          fs.copyFileSync(archive, temporary, fs.constants.COPYFILE_EXCL);
          syncDurableFile(temporary);
          fs.renameSync(temporary, file);
          syncDurableFile(root, root);
        } finally { fs.rmSync(temporary, { force: true }); }
      });
    },
    async get(id) { return at(id); },
    async erase(id) {
      fs.rmSync(at(id), { force: true });
      syncDurableFile(root, root);
    },
  };
}

function pointNames(names: string[], suffix: string): string[] {
  return names.filter(name => name.endsWith(suffix)).map(name => name.slice(0, -suffix.length)).filter(name => POINT.test(name)).sort().reverse();
}

function ssh(target: string, source: string): DestinationStore {
  const storage = DATA_DECLARATIONS.sshBackupArchives.externalStorage;
  // Fixed operations and validated identifiers only. The remote account holds ciphertext outside
  // its own Kuvo home, so removing Kuvo there leaves the backup directory intact.
  const run = async (command: string, options?: Parameters<typeof remote>[2], create = false) => {
    const prefix = `set -eu
umask 077
base="$HOME/${storage.root}"
root="$base/${source}"
[ ! -L "$base" ] && [ ! -L "$root" ] || exit 1
${create ? 'mkdir -p "$root"' : '[ -d "$root" ] || exit 0'}
`;
    const response = await remote(target, prefix + command, { most: 45 * 60_000, ...options });
    if (response.status !== 0) throw new Error("Could not reach or use the backup folder over SSH.");
    return response.stdout;
  };
  const name = (id: string) => {
    if (!POINT.test(id)) throw new Error("Choose a dated backup.");
    return `"$root/${id}${storage.archiveSuffix}"`;
  };
  return {
    async list() {
      const files = await run(`for file in "$root"/*${storage.archiveSuffix}; do [ -f "$file" ] && printf "%s\\n" "\${file##*/}"; done\nexit 0`);
      return pointNames(files.trim().split("\n"), storage.archiveSuffix);
    },
    async put(id, file) {
      await run(`dest=${name(id)}
tmp=$(mktemp "$root/.backup.XXXXXXXX")
trap 'rm -f "$tmp"' EXIT
cat > "$tmp"
sync
ln "$tmp" "$dest"
sync`, { input: { file } }, true);
    },
    async get(id, scratch) {
      const file = path.join(scratch, "received");
      const output = fs.openSync(file, "wx", 0o600);
      try { await run(`file=${name(id)}; [ -f "$file" ] && [ ! -L "$file" ] && cat "$file"`, { output }); }
      finally { fs.closeSync(output); }
      return file;
    },
    async erase(id) { await run(`rm -f ${name(id)}\nsync`); },
  };
}

/** SSH verifies the enrolled box as well as OpenSSH's host key. Restore uses its recovery key. */
export async function verifyBackupComputer(target: Extract<Destination, { kind: "computer" }>): Promise<void> {
  if (!target.ssh) {
    if (!relayBackupAvailable(target.computer)) throw new Error("Choose an enrolled computer that supports Relay backups.");
    return;
  }
  const answer = await remote(target.ssh, `set -eu
export KUVO_HOME="$HOME/.kuvo-code" BUN_BE_BUN=1
version=$(sed -n 's/^V=\\([a-zA-Z0-9._-]*\\)$/\\1/p' "$KUVO_HOME/bin/kuvo-start" | head -n 1)
[ -n "$version" ] && exec "$KUVO_HOME/engine/$version/opencode" --config=/dev/null --no-env-file "$KUVO_HOME/bin/kuvo-link.js" backup-identity`);
  if (answer.status !== 0 || answer.stdout.trim() !== target.computer) throw new Error("SSH did not reach the selected Kuvo computer.");
}

export function destinationStore(destination: Destination, source: string): DestinationStore {
  if (!SOURCE.test(source)) throw new Error("Choose the backup's computer.");
  if (destination.kind === "disk") return disk(destination.path, source);
  return destination.ssh ? ssh(destination.ssh, source) : relayStore(destination.computer, source);
}

export function expiredPoints(points: string[], now: number): string[] {
  const day = new Date(now).toISOString().slice(0, 10);
  const daily = points.filter(id => id.includes("-daily-")).sort().reverse();
  const days = [...new Set(daily.map(id => id.slice(0, 10)))];
  const cutoff = days[29] ?? new Date(now - 29 * 86_400_000).toISOString().slice(0, 10);
  return points.filter(id => {
    if (id.includes("-vault-")) return false;
    const date = id.slice(0, 10);
    return date <= day && (id.includes("-daily-") ? days.indexOf(date) >= 30 || daily.find(point => point.startsWith(date)) !== id : date < cutoff);
  });
}
