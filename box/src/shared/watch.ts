// Folder watches shared by the process. The worker owns every native open and close. On macOS
// either can wait on FSEvents, and rebuilding the stream can drop changes for existing watches too.

import type fs from "node:fs";
import { resolve, sep } from "node:path";
import threads from "node:worker_threads";
import { runWatchWorker, type WatchCommand, type WatchNotice } from "./watch-worker";

export const WATCH_LOOKS_AGAIN = [250, 1000];
const CLOSE_GRACE = 500;

type Changed = (event?: fs.WatchEventType, name?: string | null) => void;
interface Subscriber {
  changed: Changed;
  missed: () => void;
  failed: (error: string) => void;
  recursive: boolean;
}
interface Entry {
  folder: string;
  id: number;
  recursive: boolean;
  create: boolean;
  ready: boolean;
  subscribers: Set<Subscriber>;
  expiry?: ReturnType<typeof setTimeout>;
}

export interface SharedWatch {
  readonly whole: boolean;
  close(): void;
}

const watching = new Map<string, Entry>();
const looks = new Map<number, ReturnType<typeof setTimeout>>();
let worker: threads.Worker | undefined;
let sequence = 0;

function notify(entry: Entry, tell: (subscriber: Subscriber) => void) {
  for (const subscriber of [...entry.subscribers]) {
    if (!entry.subscribers.has(subscriber)) continue;
    try { tell(subscriber); } catch (error) { console.error("Folder watch callback failed:", error); }
  }
}

function startedOrStopped() {
  for (const after of WATCH_LOOKS_AGAIN) {
    const at = Math.ceil((performance.now() + after) / 100) * 100;
    if (looks.has(at)) continue;
    const timer = setTimeout(() => {
      looks.delete(at);
      for (const entry of watching.values()) notify(entry, subscriber => subscriber.missed());
    }, at - performance.now());
    timer.unref?.();
    looks.set(at, timer);
  }
}

function failed(entry: Entry, error: string) {
  watching.delete(entry.folder);
  clearTimeout(entry.expiry);
  entry.ready = false;
  notify(entry, subscriber => subscriber.failed(error));
  notify(entry, subscriber => subscriber.missed());
}

function heard(notice: WatchNotice) {
  const mac = process.platform === "darwin";
  if ("stopped" in notice) {
    if (mac) startedOrStopped();
    return;
  }
  const entry = watching.get(notice.folder);
  if (!entry || entry.id !== notice.id) return;
  if ("event" in notice) {
    notify(entry, subscriber => {
      if (subscriber.recursive || !notice.name?.includes(sep)) subscriber.changed(notice.event, notice.name);
    });
  } else {
    if ("error" in notice) failed(entry, notice.error);
    else {
      entry.ready = true;
      notify(entry, subscriber => subscriber.missed());
    }
    if (mac) startedOrStopped();
  }
}

function send(command: WatchCommand) {
  if (!worker) {
    const started = new threads.Worker(
      `(${runWatchWorker.toString()})(require("node:fs"), require("node:worker_threads").parentPort)`,
      { eval: true },
    );
    worker = started;
    const lost = (error: string) => {
      if (worker !== started) return;
      worker = undefined;
      for (const entry of [...watching.values()]) failed(entry, error);
    };
    started.on("message", (notice: WatchNotice) => { if (worker === started) heard(notice); });
    started.on("error", error => lost(error.message));
    started.on("exit", code => lost(`Folder watch worker exited (${code})`));
    started.unref();
  }
  worker.postMessage(command);
}

/** Subscribe to one folder. A recursive subscription also covers its shallow subscribers.
 * `missed` looks again after native watches start or stop. `whole` is true once the watch is up. */
export function watchFolder(
  folder: string,
  changed: Changed,
  { recursive = false, create = false, missed = () => changed(), failed: onError = () => {} }:
  { recursive?: boolean; create?: boolean; missed?: () => void; failed?: (error: string) => void } = {},
): SharedWatch {
  folder = resolve(folder);
  let entry = watching.get(folder);
  if (!entry) {
    entry = { folder, id: 0, recursive, create, ready: false, subscribers: new Set() };
    watching.set(folder, entry);
  }
  const watch = entry;
  const subscriber = { changed, missed, failed: onError, recursive };
  watch.subscribers.add(subscriber);
  clearTimeout(watch.expiry);
  if (!watch.id || (recursive && !watch.recursive) || (create && !watch.create && !watch.ready)) {
    watch.recursive ||= recursive;
    watch.create ||= create;
    watch.ready = false;
    const id = watch.id = ++sequence;
    setImmediate(() => {
      if (watching.get(folder) !== watch || watch.id !== id) return;
      try { send({ folder, id, recursive: watch.recursive, create: watch.create }); }
      catch (error) { failed(watch, String(error)); }
    });
  }
  return {
    get whole() { return watch.ready && watch.subscribers.has(subscriber); },
    close() {
      if (!watch.subscribers.delete(subscriber) || watch.subscribers.size || watching.get(folder) !== watch) return;
      watch.expiry = setTimeout(() => {
        watching.delete(folder);
        const command: WatchCommand = { folder, id: watch.id, close: true };
        worker?.postMessage(command);
      }, CLOSE_GRACE);
      watch.expiry.unref?.();
    },
  };
}
