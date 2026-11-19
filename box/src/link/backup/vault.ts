import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { addItem, generations, isLocked, isMade, readItems, valuesOf, vaultState } from "../vault/store";
import { archiveContext, authenticate, newRecoveryKey, openArchive, seal } from "./archive";
import { destinationStore, POINT, type Destination } from "./destination";
import { backupLock, bytes, pointId, scratchFolder } from "./engine";

function requireVault() {
  if (!isMade() || isLocked() || vaultState().tampered) throw new Error("Open an unlocked, healthy vault first.");
}

export function exportVault(dest: Destination, source: string) {
  return backupLock(async () => {
    requireVault();
    const original = JSON.stringify(generations());
    const items = await Promise.all(readItems().map(async item => ({ ...item, values: await valuesOf(item.id) })));
    if (original !== JSON.stringify(generations()) || vaultState().tampered || items.some(item => item.fields.some(field => typeof item.values[field] !== "string"))) {
      throw new Error("The vault changed or a value was unavailable. Try exporting again.");
    }
    const key = newRecoveryKey();
    const id = pointId("vault", Date.now());
    const scratch = scratchFolder();
    try {
      const file = path.join(scratch, "archive");
      seal(file, key, [bytes("vault.json", { version: 1, id, source, items })], archiveContext(source, id));
      const store = destinationStore(dest, source);
      await store.put(id, file);
      try { authenticate(await store.get(id, scratch), key, archiveContext(source, id)); }
      catch (error) { await store.erase(id).catch(() => {}); throw error; }
      return { id, source, recoveryKey: key };
    } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
  });
}

/** Import as new items. A value needs fresh grants before any agent can use it. */
export function importVault(dest: Destination, source: string, id: string, key: string) {
  return backupLock(async () => {
    requireVault();
    if (!POINT.test(id) || !id.includes("-vault-")) throw new Error("Choose a vault export.");
    const scratch = scratchFolder();
    try {
      const entries = openArchive(await destinationStore(dest, source).get(id, scratch), key, archiveContext(source, id), scratch);
      if (entries.length !== 1 || entries[0]!.name !== "vault.json") throw new Error("Choose a vault export.");
      const value: unknown = JSON.parse(fs.readFileSync(entries[0]!.file, "utf8"));
      if (!isObject(value) || value.version !== 1 || value.id !== id || value.source !== source || !Array.isArray(value.items) ||
        !value.items.every(item => isObject(item) && typeof item.name === "string" && isObject(item.values))) throw new Error("The export does not match that vault.");
      const existing = new Set(readItems().map(item => item.name.toLowerCase()));
      const skipped: string[] = [];
      let imported = 0;
      for (const item of value.items) {
        const name = item.name.toLowerCase();
        if (existing.has(name)) { skipped.push(item.name); continue; }
        await addItem(item);
        existing.add(name);
        imported++;
      }
      return { imported, skipped, grants: "approval required" };
    } finally { fs.rmSync(scratch, { recursive: true, force: true }); }
  });
}
