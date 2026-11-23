import type { verifyHeldPack } from "../updates/fleet";
import type { packForTransfer, unpackForTransfer } from "../moves/bundle";
import type { checkMoveQueueFiles, clearQueuePlacement, forgetWorkQueues, packQueuesForMove, placeMoveWorkQueues, verifyQueuePayloads } from "../work-queue/moves";
// These filesystem operations have synchronous transactions and can take a long time.
// Each domain has its own worker so an archive never holds up permission reads.
import { isMainThread, parentPort, Worker, workerData, type WorkerOptions } from "node:worker_threads";
import type { setStarred } from "../areas/models-starred";
import type { setSpeed } from "../areas/models-speed";
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
import type { forgetHistory, readHistory } from "../browser/history";
import type { browserSites, removeSiteData, siteSignInsHere } from "../browser/sites";
import type { sweepWakeFolders } from "../fence/state";
import type { readBacklogFiles } from "../work-queue/import";
import type { allocateNumbers, claimNumbers, movePast, numbersToSend, receiveNumbers } from "../work-queue/numbers";
import type { keepDownloadedRelease, receiveRelease } from "../updates/update";
import type { readPermissionSettings } from "../permissions";
import { macLink } from "../state";

type Operations = {
  applySettings: typeof applySettings;
  applyResult: typeof applyResult;
  confirmConfig: typeof confirmConfig;
  previewConfig: typeof previewConfig;
  readConfigPlan: typeof readConfigPlan;
  saveChangeSet: typeof saveChangeSet;
  installConfigCatalog: typeof installConfigCatalog;
  patchConfig: typeof patchConfig;
  setStarred: typeof setStarred;
  setSpeed: typeof setSpeed;
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
  receiveRelease: typeof receiveRelease;
  keepDownloadedRelease: typeof keepDownloadedRelease;
  verifyHeldPack: typeof verifyHeldPack;
  allocateNumbers: typeof allocateNumbers;
  claimNumbers: typeof claimNumbers;
  movePast: typeof movePast;
  numbersToSend: typeof numbersToSend;
  receiveNumbers: typeof receiveNumbers;
  readBacklogFiles: typeof readBacklogFiles;
  checkMoveQueueFiles: typeof checkMoveQueueFiles;
  clearQueuePlacement: typeof clearQueuePlacement;
  forgetWorkQueues: typeof forgetWorkQueues;
  packForTransfer: typeof packForTransfer;
  unpackForTransfer: typeof unpackForTransfer;
  packQueuesForMove: typeof packQueuesForMove;
  placeMoveWorkQueues: typeof placeMoveWorkQueues;
  verifyQueuePayloads: typeof verifyQueuePayloads;
  browserSites: typeof browserSites;
  readHistory: typeof readHistory;
  forgetHistory: typeof forgetHistory;
  removeSiteData: typeof removeSiteData;
  siteSignInsHere: typeof siteSignInsHere;
  sweepWakeFolders: typeof sweepWakeFolders;
};
type Operation = keyof Operations;
type Request = { [Key in Operation]: { id: number; operation: Key; args: Parameters<Operations[Key]> } }[Operation] & {
  macIdentity?: typeof macLink.overlay;
};
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
        case "setStarred": value = (await import("../areas/models-starred")).setStarred(...args); break;
        case "setSpeed": value = (await import("../areas/models-speed")).setSpeed(...args); break;
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
        case "readHistory": value = (await import("../browser/history")).readHistory(...args); break;
        case "forgetHistory": value = (await import("../browser/history")).forgetHistory(...args); break;
        case "browserSites": value = await (await import("../browser/sites")).browserSites(...args); break;
        case "removeSiteData": value = await (await import("../browser/sites")).removeSiteData(...args); break;
        case "siteSignInsHere": value = await (await import("../browser/sites")).siteSignInsHere(...args); break;
        case "sweepWakeFolders": value = (await import("../fence/state")).sweepWakeFolders(...args); break;
        case "readBacklogFiles": value = (await import("../work-queue/import")).readBacklogFiles(...args); break;
        case "checkMoveQueueFiles": value = (await import("../work-queue/moves")).checkMoveQueueFiles(...args); break;
        case "clearQueuePlacement": value = (await import("../work-queue/moves")).clearQueuePlacement(...args); break;
        case "forgetWorkQueues": value = (await import("../work-queue/moves")).forgetWorkQueues(...args); break;
        case "placeMoveWorkQueues": value = (await import("../work-queue/moves")).placeMoveWorkQueues(...args); break;
        case "packForTransfer": value = await (await import("../moves/bundle")).packForTransfer(...args); break;
        case "unpackForTransfer": value = await (await import("../moves/bundle")).unpackForTransfer(...args); break;
        case "packQueuesForMove": value = (await import("../work-queue/moves")).packQueuesForMove(...args); break;
        case "verifyQueuePayloads": value = (await import("../work-queue/moves")).verifyQueuePayloads(...args); break;
        case "claimNumbers":
        case "movePast":
        case "numbersToSend":
        case "receiveNumbers":
        case "allocateNumbers":
        {
          const numbers = await import("../work-queue/numbers");
          // Number operations are synchronous. They reread membership under the ledger
          // lock and use the dispatcher's Mac identity only for the duration of that read.
          const overlay = macLink.overlay;
          macLink.overlay = request.macIdentity ?? null;
          try {
            switch (request.operation) {
              case "claimNumbers": value = numbers.claimNumbers(...request.args); break;
              case "movePast": value = numbers.movePast(...request.args); break;
              case "numbersToSend": value = numbers.numbersToSend(...request.args); break;
              case "receiveNumbers": value = numbers.receiveNumbers(...request.args); break;
              case "allocateNumbers": value = numbers.allocateNumbers(...request.args); break;
            }
          } finally {
            macLink.overlay = overlay;
          }
          break;
        }
        case "verifyHeldPack": value = await (await import("../updates/fleet")).verifyHeldPack(...args); break;
        case "keepDownloadedRelease": value = (await import("../updates/update")).keepDownloadedRelease(...args); break;
        case "receiveRelease": value = (await import("../updates/update")).receiveRelease(...args); break;
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
  private parking?: ReturnType<typeof setTimeout>;

  constructor(private readonly usesMacIdentity = false) {}
  private readonly idleMs = 5_000;
  private next = 0;
  private pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void }>();

  run<Key extends Operation>(operation: Key, ...args: Parameters<Operations[Key]>): Promise<Awaited<ReturnType<Operations[Key]>>> {
    const worker = this.worker ?? this.start();
    clearTimeout(this.parking);
    worker.ref();
    return new Promise((resolve, reject) => {
      const id = ++this.next;
      this.pending.set(id, { resolve: value => resolve(value as Awaited<ReturnType<Operations[Key]>>), reject });
      const macIdentity = this.usesMacIdentity ? macLink.overlay : undefined;
      try { worker.postMessage({ id, operation, args, macIdentity }); }
      catch (error) {
        this.pending.delete(id);
        if (!this.pending.size) worker.unref();
        reject(error);
      }
    });
  }

  private start(): Worker {
    const worker = this.worker = new Worker(new URL(import.meta.url), { workerData: role, smol: true } as WorkerOptions);
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
      if (!this.pending.size) {
        worker.unref();
        if (this.idleMs) this.parking = setTimeout(() => {
          if (this.worker !== worker || this.pending.size) return;
          this.worker = undefined;
          void worker.terminate();
        }, this.idleMs).unref();
      }
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
export const releaseDisk = new DiskWorker();
export const numberDisk = new DiskWorker(true);
export const browserDisk = new DiskWorker();
export const wakeDisk = new DiskWorker();
export const backlogDisk = new DiskWorker();
export const queueMoveDisk = new DiskWorker();
export const numberUpkeepDisk = new DiskWorker(true);
export const moveBundleDisk = new DiskWorker();
