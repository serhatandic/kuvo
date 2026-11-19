// Run inside the link lock with a durable unlink instruction already installed.
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { APP_FOLDER, APP_KEY_FILES, BOT_KEYS, BOX_KEYS, ENGINE_DATA, INTEGRATION_DATA, K, OWN_RUNTIME, REAL_BOX, RUN } from "../home";
import { SSH_FRESH_ADD } from "../marks";
import { SEEDED_FILE } from "../add/seed";
import { claudeKeychainService } from "../rehome/paths";

const clearCredentialTable = `
const SQLite = process.versions.bun ? require("bun:sqlite").Database : require("node:sqlite").DatabaseSync;
const db = process.versions.bun
  ? new SQLite(process.argv[1], { readwrite: true })
  : new SQLite(process.argv[1], { allowExtension: false });
try {
  db.exec("PRAGMA busy_timeout=5000; PRAGMA secure_delete=ON; PRAGMA synchronous=FULL; BEGIN IMMEDIATE;");
  if (db.prepare("SELECT 1 FROM sqlite_schema WHERE type='table' AND name='credential'").get()) {
    db.exec("DELETE FROM credential");
    if (db.prepare("SELECT 1 FROM credential LIMIT 1").get()) throw new Error("Credentials remain.");
  }
  db.exec("COMMIT;");
  if (db.prepare("PRAGMA wal_checkpoint(TRUNCATE)").get().busy) throw new Error("Credential WAL is busy.");
  if (db.prepare("SELECT 1 FROM sqlite_schema WHERE type='table' AND name='credential'").get()
    && db.prepare("SELECT 1 FROM credential LIMIT 1").get()) throw new Error("Credentials remain.");
} finally { db.close(); }
`;

function checkPrivatePath(file: string): void {
  for (const root of new Set([K, APP_FOLDER])) {
    if (!root) continue;
    const relative = path.relative(root, file);
    if (!relative || relative === ".." || relative.startsWith(".." + path.sep) || path.isAbsolute(relative)) continue;
    const parent = path.dirname(file);
    const actual = fs.realpathSync(parent);
    const expected = path.resolve(fs.realpathSync(root), path.dirname(relative));
    if (actual !== expected) throw new Error("A Kuvo credential folder was redirected.");
  }
}

function entries(folder: string): fs.Dirent[] {
  const info = fs.lstatSync(folder, { throwIfNoEntry: false });
  if (!info) return [];
  checkPrivatePath(folder);
  if (!info.isDirectory()) throw new Error("A Kuvo credential folder could not be opened.");
  return fs.readdirSync(folder, { withFileTypes: true });
}

function clear(file: string, recursive = false): void {
  if (!fs.lstatSync(file, { throwIfNoEntry: false })) return;
  checkPrivatePath(file);
  fs.rmSync(file, { recursive, force: true });
  if (fs.lstatSync(file, { throwIfNoEntry: false })) throw new Error("A Kuvo credential store was not removed.");
  const dir = fs.openSync(path.dirname(file), "r");
  try { fs.fsyncSync(dir); } finally { fs.closeSync(dir); }
}

function engineCredentials(folder: string): void {
  for (const entry of entries(folder)) {
    const file = path.join(folder, entry.name);
    if (/^auth\.json(?:\..*)?$/.test(entry.name)) clear(file);
    if (!/^opencode.*\.db$/.test(entry.name)) continue;
    if (!entry.isFile() || ["-wal", "-shm", "-journal"].some(suffix => fs.lstatSync(file + suffix, { throwIfNoEntry: false })?.isFile() === false)) {
      throw new Error("Could not open a Kuvo engine credential database.");
    }
    const result = spawnSync(process.execPath, [...OWN_RUNTIME, "-e", clearCredentialTable, file], {
      env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 10_000,
    });
    if (result.status !== 0) throw new Error("Could not clear a Kuvo engine credential database.");
  }
}

function clearClaudeKeychain(home: string): void {
  if (process.platform !== "darwin" || !REAL_BOX) return;
  const service = claudeKeychainService(home);
  const run = (command: string) => spawnSync("/usr/bin/security", [command, "-s", service], {
    stdio: "ignore", timeout: 5_000,
  }).status;
  // security returns the low byte of errSecItemNotFound (-25300).
  const missing = 44;
  const status = run("find-generic-password");
  if (status === missing) return;
  if (status !== 0 || run("delete-generic-password") !== 0 || run("find-generic-password") !== missing) {
    throw new Error("Could not verify that the Kuvo Claude sign-in was cleared.");
  }
}

function agentCredentials(root: string): void {
  const agentKinds = new Set(["claude", "codex", "grok", "cursor", "antigravity"]);
  function walk(folder: string) {
    for (const entry of entries(folder)) {
      const file = path.join(folder, entry.name);
      if (file === path.join(root, "claude", "tokens")) clear(file, true);
      else if (entry.isDirectory()) walk(file);
      else if (/^(?:\.credentials\.json|auth\.json|acp_token\.json|oauth-token\.json)(?:\..*)?$|^\.auth-/.test(entry.name)) clear(file);
    }
  }
  for (const entry of entries(root)) {
    if (!entry.isDirectory() || !agentKinds.has(entry.name)) continue;
    const folder = path.join(root, entry.name);
    if (entry.name === "claude") {
      clearClaudeKeychain(path.join(folder, "home"));
      clearClaudeKeychain(path.join(folder, "console"));
      for (const account of entries(path.join(folder, "accounts"))) {
        if (account.isDirectory()) clearClaudeKeychain(path.join(folder, "accounts", account.name));
      }
    }
    walk(folder);
  }
}

export function clearUnlinkCredentials(): void {
  for (const root of new Set([ENGINE_DATA, path.join(K, "data", "opencode"), process.env.KUVO_ENGINE_DB_FOLDER])) {
    if (root) engineCredentials(root);
  }
  for (const root of new Set([path.join(K, "agents"), path.join(APP_FOLDER ?? K, "agents"), process.env.KUVO_AGENTS_HOME])) {
    if (root) agentCredentials(root);
  }
  for (const folder of new Set([INTEGRATION_DATA, path.join(K, "keys"), BOX_KEYS, BOT_KEYS, APP_KEY_FILES])) {
    if (folder) clear(folder, true);
  }
  clear(SEEDED_FILE);
  clear(path.join(RUN, SSH_FRESH_ADD));
}
