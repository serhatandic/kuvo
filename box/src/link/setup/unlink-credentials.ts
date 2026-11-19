// Run inside the link lock with a durable unlink instruction already installed.
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { BOT_KEYS, BOX_KEYS, ENGINE_DATA, INTEGRATION_DATA, K, OWN_RUNTIME } from "../home";

const clearCredentialTable = `
const SQLite = process.versions.bun ? require("bun:sqlite").Database : require("node:sqlite").DatabaseSync;
const db = new SQLite(process.argv[1]);
try {
  db.exec("PRAGMA busy_timeout=5000; PRAGMA secure_delete=ON; PRAGMA synchronous=FULL;");
  if (db.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='credential'").get()) {
    db.exec("DELETE FROM credential");
    const checkpoint = db.prepare("PRAGMA wal_checkpoint(TRUNCATE)").get();
    if (checkpoint.busy) process.exitCode = 1;
  }
} finally { db.close(); }
`;

export function clearUnlinkCredentials(): void {
  const database = path.join(ENGINE_DATA, "opencode.db");
  const entry = fs.lstatSync(database, { throwIfNoEntry: false });
  if (entry) {
    if (!entry.isFile() || ["-wal", "-shm", "-journal"].some(suffix => fs.lstatSync(database + suffix, { throwIfNoEntry: false })?.isFile() === false)) {
      throw new Error("Could not open the Kuvo engine credential table.");
    }
    const result = spawnSync(process.execPath, [...OWN_RUNTIME, "-e", clearCredentialTable, database], {
      env: { ...process.env, BUN_BE_BUN: "1" }, timeout: 10_000,
    });
    if (result.status !== 0) throw new Error("Could not clear the Kuvo engine credential table.");
  }
  for (const folder of new Set([INTEGRATION_DATA, path.join(K, "keys"), BOX_KEYS, BOT_KEYS])) {
    fs.rmSync(folder, { recursive: true, force: true });
    const parent = path.dirname(folder);
    if (!fs.existsSync(parent)) continue;
    const dir = fs.openSync(parent, "r");
    try { fs.fsyncSync(dir); } finally { fs.closeSync(dir); }
  }
}
