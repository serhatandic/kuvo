// These filesystem operations have synchronous transactions and can take a long time.
// Each domain has its own worker so an archive never holds up permission reads.
import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";
import type { applySettings } from "../areas/apply";
import type { applyResult, confirmConfig, previewConfig, readConfigPlan, saveChangeSet } from "../areas/apply-from";
import type { installConfigCatalog, patchConfig } from "../areas/settings";
import type { listFolders, makeFolder } from "../areas/folders";
import type { captureBackup } from "../backup/engine";
import type { authenticate, openArchive } from "../backup/archive";
import type { noWorkers, placeRecovered, sameFile } from "../backup/restore";
import type { restoreProjects } from "../backup/projects";
import type { restoreEngineDatabase } from "../backup/session-files";
import type { mergeInsightsDatabase, prepareEngineBootstrap } from "../backup/sqlite";
import type { scanSourceGeneration } from "../setup/removal-generation";
import type { readPermissionSettings } from "../permissions";

type Operations = {
  applySettings: typeof applySettings;
  applyResult: typeof applyResult;
  confirmConfig: typeof confirmConfig;
  previewConfig: typeof previewConfig;
  readConfigPlan: typeof readConfigPlan;
  saveChangeSet: typeof saveChangeSet;
  installConfigCatalog: typeof installConfigCatalog;
  patchConfig: typeof patchConfig;
  listFolders: typeof listFolders;
  makeFolder: typeof makeFolder;
  captureBackup: typeof captureBackup;
  scanSourceGeneration: typeof scanSourceGeneration;
  authenticate: (file: string, key: string, context: Buffer) => ReturnType<typeof authenticate>;
  openArchive: typeof openArchive;
  noWorkers: typeof noWorkers;
  sameFile: typeof sameFile;
  placeRecovered: typeof placeRecovered;
  restoreProjects: typeof restoreProjects;
  restoreEngineDatabase: typeof restoreEngineDatabase;
  readPermissionSettings: typeof readPermissionSettings;
  mergeInsightsDatabase: typeof mergeInsightsDatabase;
  prepareEngineBootstrap: typeof prepareEngineBootstrap;
};
type Operation = keyof Operations;
type Request = { [Key in Operation]: { id: number; operation: Key; args: Parameters<Operations[Key]> } }[Operation];
type Reply = { id: number; value?: unknown; error?: string };
const role = "kuvo-disk-worker";

export const isDiskWorker = !isMainThread && workerData === role;
if (isDiskWorker) {
  const port = parentPort!;
  port.on("message", async (request: Request) => {
    const { id, operation, args } = request;
    try {
      let value: unknown;
      switch (operation) {
        case "applySettings": value = (await import("../areas/apply")).applySettings(...args); break;
        case "applyResult": value = (await import("../areas/apply-from")).applyResult(...args); break;
        case "confirmConfig": value = (await import("../areas/apply-from")).confirmConfig(...args); break;
        case "previewConfig": value = (await import("../areas/apply-from")).previewConfig(...args); break;
        case "readConfigPlan": value = (await import("../areas/apply-from")).readConfigPlan(...args); break;
        case "saveChangeSet": value = (await import("../areas/apply-from")).saveChangeSet(...args); break;
        case "installConfigCatalog": value = (await import("../areas/settings")).installConfigCatalog(...args); break;
        case "patchConfig": value = (await import("../areas/settings")).patchConfig(...args); break;
        case "listFolders": value = (await import("../areas/folders")).listFolders(...args); break;
        case "makeFolder": value = (await import("../areas/folders")).makeFolder(...args); break;
        case "scanSourceGeneration": value = (await import("../setup/removal-generation")).scanSourceGeneration(...args); break;
        case "captureBackup": value = (await import("../backup/engine")).captureBackup(...args); break;
        case "authenticate": {
          const [file, key, context] = args;
          value = (await import("../backup/archive")).authenticate(file, key, Buffer.from(context));
          break;
        }
        case "openArchive": {
          const [file, key, context, scratch] = args;
          value = (await import("../backup/archive")).openArchive(file, key, Buffer.from(context), scratch);
          break;
        }
        case "noWorkers": value = (await import("../backup/restore")).noWorkers(); break;
        case "sameFile": value = (await import("../backup/restore")).sameFile(...args); break;
        case "placeRecovered": value = (await import("../backup/restore")).placeRecovered(...args); break;
        case "restoreProjects": value = (await import("../backup/projects")).restoreProjects(...args); break;
        case "restoreEngineDatabase": value = await (await import("../backup/session-files")).restoreEngineDatabase(...args); break;
        case "mergeInsightsDatabase": value = (await import("../backup/sqlite")).mergeInsightsDatabase(...args); break;
        case "prepareEngineBootstrap": value = (await import("../backup/sqlite")).prepareEngineBootstrap(...args); break;
        case "readPermissionSettings": value = (await import("../permissions")).readPermissionSettings(...args); break;
      }
      port.postMessage({ id, value } satisfies Reply);
    } catch (error) {
      port.postMessage({ id, error: error instanceof Error ? error.message : String(error) } satisfies Reply);
    }
  });
}

class DiskWorker {
  private worker: Worker | undefined;
  private next = 0;
  private pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();

  run<Key extends Operation>(operation: Key, ...args: Parameters<Operations[Key]>): Promise<Awaited<ReturnType<Operations[Key]>>> {
    const worker = this.worker ?? this.start();
    worker.ref();
    return new Promise((resolve, reject) => {
      const id = ++this.next;
      this.pending.set(id, { resolve: value => resolve(value as Awaited<ReturnType<Operations[Key]>>), reject });
      try { worker.postMessage({ id, operation, args }); }
      catch (error) {
        this.pending.delete(id);
        if (!this.pending.size) worker.unref();
        reject(error);
      }
    });
  }

  private start(): Worker {
    const worker = this.worker = new Worker(new URL(import.meta.url), { workerData: role });
    const fail = (error: Error) => {
      if (this.worker !== worker) return;
      this.worker = undefined;
      for (const task of this.pending.values()) task.reject(error);
      this.pending.clear();
    };
    worker.on("error", fail);
    worker.on("exit", code => fail(new Error(`The disk worker exited (${code}).`)));
    worker.on("message", ({ id, value, error }: Reply) => {
      const task = this.pending.get(id);
      if (!task) return;
      this.pending.delete(id);
      if (!this.pending.size) worker.unref();
      if (error === undefined) task.resolve(value);
      else task.reject(new Error(error));
    });
    return worker;
  }
}

export const backupDisk = new DiskWorker();
export const permissionDisk = new DiskWorker();

export const removalDisk = new DiskWorker();
export const folderDisk = new DiskWorker();
export const settingsDisk = new DiskWorker();
