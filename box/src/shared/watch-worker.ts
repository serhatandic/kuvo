import type fs from "node:fs";
import type { MessagePort } from "node:worker_threads";

export type WatchCommand =
  | { folder: string; id: number; recursive: boolean; create: boolean }
  | { folder: string; id: number; close: true };

type Change =
  | { ready: true }
  | { event: fs.WatchEventType; name: string | null }
  | { error: string }
  | { stopped: true };
export type WatchNotice = { folder: string; id: number } & Change;

/** Runs in a worker, including in the single-file box bundles. Keep its dependencies in the arguments. */
export function runWatchWorker(files: typeof fs, port: MessagePort) {
  const watches = new Map<string, { id: number; watcher: fs.FSWatcher }>();
  port.on("message", (command: WatchCommand) => {
    const { folder, id } = command;
    const prior = watches.get(folder);
    if ("close" in command && prior?.id !== id) return;
    if (prior) {
      prior.watcher.close();
      watches.delete(folder);
    }
    const send = (notice: Change) => port.postMessage({ folder, id, ...notice });
    if ("close" in command) return send({ stopped: true });
    try {
      if (command.create) files.mkdirSync(folder, { recursive: true });
      const watcher = files.watch(folder, { recursive: command.recursive }, (event, name) => {
        if (watches.get(folder)?.id === id) send({ event, name: name === null ? null : String(name) });
      });
      watches.set(folder, { id, watcher });
      watcher.on("error", (error: Error) => {
        if (watches.get(folder)?.id !== id) return;
        watcher.close();
        watches.delete(folder);
        send({ error: error.message });
      });
      send({ ready: true });
    } catch (error) {
      send({ error: String(error instanceof Error ? error.message : error) });
    }
  });
}
