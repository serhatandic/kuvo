// Canonical receipt I/O and its SQLite index use synchronous transactions. Run them together
// outside the link loop so the index stamp and the canonical file still change in the same order.
import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";
import { syncDurableFile } from "../../shared/files";
import path from "node:path";
import { RUN } from "../home";
import { keepRecord, removeRecord } from "./store";
import { receiptPageKeys, refreshReceiptIndex } from "./receipt-index";

type Work =
  | ["keep", Parameters<typeof keepRecord>]
  | ["remove", Parameters<typeof removeRecord>]
  | ["page", Parameters<typeof receiptPageKeys>]
  | ["refresh", []];
type Reply = { id: number; result?: unknown; error?: string };
const ROLE = "kuvo-receipt-worker";

function serveReceiptWorker(): void {
  const port = parentPort!;
  port.on("message", async ({ id, command }: { id: number; command: Work }) => {
    try {
      const [method, args] = command;
      let result: unknown;
      switch (method) {
        case "keep": keepRecord(...args); break;
        case "remove":
          removeRecord(...args);
          syncDurableFile(path.join(RUN, "owned-data", args[0]), RUN);
          break;
        case "page": result = receiptPageKeys(...args); break;
        case "refresh": await refreshReceiptIndex(); break;
      }
      port.postMessage({ id, result } satisfies Reply);
    } catch (error) {
      port.postMessage({ id, error: error instanceof Error ? error.message : String(error) } satisfies Reply);
    }
  });
}

export const isReceiptWorker = !isMainThread && workerData === ROLE;
if (isReceiptWorker) serveReceiptWorker();

let worker: Worker | undefined;
let next = 0;
const pending = new Map<number, { done: (result: unknown) => void; failed: (error: Error) => void }>();

function receiptWork<T>(command: Work): Promise<T> {
  if (!worker) {
    // In a built link import.meta.url is the bundle, which imports this module during startup.
    const started = new Worker(new URL(import.meta.url), { workerData: ROLE });
    worker = started;
    const fail = (error: Error) => {
      if (worker !== started) return;
      worker = undefined;
      for (const wait of pending.values()) wait.failed(error);
      pending.clear();
    };
    started.on("error", fail);
    started.on("exit", code => fail(new Error(`The receipt worker exited (${code}).`)));
    started.on("message", ({ id, result, error }: Reply) => {
      const wait = pending.get(id);
      if (!wait) return;
      pending.delete(id);
      if (!pending.size) started.unref();
      if (error === undefined) wait.done(result);
      else wait.failed(new Error(error));
    });
  }
  const current = worker;
  current.ref();
  return new Promise((resolve, reject) => {
    const id = ++next;
    pending.set(id, { done: value => resolve(value as T), failed: reject });
    try { current.postMessage({ id, command }); }
    catch (error) {
      pending.delete(id);
      if (!pending.size) current.unref();
      reject(error);
    }
  });
}

export const keepReceipt = (...args: Parameters<typeof keepRecord>): Promise<void> => receiptWork(["keep", args]);
export const removeReceipt = (...args: Parameters<typeof removeRecord>): Promise<void> => receiptWork(["remove", args]);
export const pageReceiptKeys = (...args: Parameters<typeof receiptPageKeys>): Promise<ReturnType<typeof receiptPageKeys>> => receiptWork(["page", args]);
export const refreshReceipts = (): Promise<void> => receiptWork(["refresh", []]);
