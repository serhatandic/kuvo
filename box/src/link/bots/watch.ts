// Bot and wake-up metadata shares a recursive FSEvents watch on a Mac. On Linux a recursive
// watch consumes an inotify descriptor for each directory, including the bot's workspace.
// Descend only into schedule directories there, sharing shallow watches with the session list.

import { readdir } from "node:fs/promises";
import { join, sep } from "node:path";
import { watchFolder, type SharedWatch } from "../../shared/watch";

type Tell = (name?: string | null) => void;
const scheduleDirs = new Set(["wakes", "watches", "summaries"]);
const trees = new Map<string, { listeners: Set<Tell>; watch: SharedWatch }>();

export function watchBotSchedules(root: string, tell: Tell): SharedWatch {
  if (process.platform !== "linux") {
    return watchFolder(root, (_, name) => tell(name), { recursive: true, create: true });
  }
  let tree = trees.get(root);
  if (!tree) {
    const listeners = new Set<Tell>();
    tree = { listeners, watch: watchMetadata(root, name => {
      for (const listener of [...listeners]) if (listeners.has(listener)) listener(name);
    }) };
    trees.set(root, tree);
  }
  const { listeners, watch } = tree;
  const listener: Tell = name => tell(name);
  listeners.add(listener);
  return {
    get whole() { return watch.whole; },
    close() {
      if (!listeners.delete(listener) || listeners.size) return;
      trees.delete(root);
      watch.close();
    },
  };
}

function watchMetadata(root: string, tell: Tell): SharedWatch {
  const folders = (at: string, prefix: string, accepts: (name: string) => boolean,
    child: (path: string, prefix: string) => SharedWatch): SharedWatch => {
    const children = new Map<string, SharedWatch>();
    let closed = false, reading = false, again = false;
    const scan = async () => {
      if (closed) return;
      if (reading) { again = true; return; }
      reading = true;
      try {
        do {
          again = false;
          const entries = await readdir(at, { withFileTypes: true }).catch(() => []);
          if (closed) return;
          const present = new Set(entries.filter(entry => entry.isDirectory() && accepts(entry.name)).map(entry => entry.name));
          for (const [name, watch] of children) {
            if (present.has(name)) continue;
            watch.close();
            children.delete(name);
          }
          for (const name of present) {
            if (!children.has(name)) children.set(name, child(join(at, name), prefix + name + sep));
          }
        } while (again);
      } finally { reading = false; }
    };
    const changed = (name?: string | null) => tell(name ? prefix + name : prefix.slice(0, -1));
    const watch = watchFolder(at, (event, name) => {
      if (event === "rename" && (!name || accepts(name))) void scan();
      changed(name);
    }, { create: !prefix, missed: () => { void scan(); changed(); } });
    void scan();
    return {
      get whole() { return watch.whole && !reading && [...children.values()].every(child => child.whole); },
      close() {
        closed = true;
        watch.close();
        for (const child of children.values()) child.close();
        children.clear();
      },
    };
  };
  return folders(root, "", name => /^[A-Za-z0-9_-]+$/.test(name), (at, prefix) =>
    folders(at, prefix, name => scheduleDirs.has(name), (directory, inside) =>
      // A watch's checks and notes live in a small directory per watch. Wakes and summaries are flat.
      watchFolder(directory, (_, name) => tell(inside + (name ?? "")), { recursive: directory.endsWith(sep + "watches") })));
}
