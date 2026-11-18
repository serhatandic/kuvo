// Open the saved file itself. A path check alone would let a replacement race the read.
import fs from "node:fs";
import path from "node:path";
import { FILE_NAME, FILES, ITEM_ID, WORK_QUEUE } from "./store";

const readFlags = fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK;
const same = (a: fs.BigIntStats, b: fs.BigIntStats) => a.dev === b.dev && a.ino === b.ino;
const entry = (file: string) => fs.lstatSync(file, { bigint: true });

/** Read only a regular file directly under this item's real attachment directory. */
export function readAttachment(item: string, name: string): Buffer | null {
  if (!ITEM_ID.test(item) || !FILE_NAME.test(name)) return null;
  try {
    const run = fs.realpathSync(path.dirname(WORK_QUEUE));
    const queue = path.join(run, path.basename(WORK_QUEUE));
    const root = path.join(queue, path.basename(FILES));
    const folder = path.join(root, item);
    const parents = [run, queue, root, folder];
    const before = parents.map(entry);
    if (before.some(stat => !stat.isDirectory())) return null;
    const dir = fs.openSync(folder, readFlags | fs.constants.O_DIRECTORY);
    try {
      if (!same(fs.fstatSync(dir, { bigint: true }), before[before.length - 1])) return null;
      const file = path.join(folder, name);
      const fd = fs.openSync(file, readFlags);
      try {
        const opened = fs.fstatSync(fd, { bigint: true });
        if (!opened.isFile() || !same(opened, entry(file))) return null;
        // Check every parent after the file identity check. A directory temporarily replaced
        // by a symlink and then put back also changes its parent's change time.
        if (parents.some((parent, at) => {
          const after = entry(parent);
          return !same(before[at], after) || before[at].ctimeNs !== after.ctimeNs;
        })) return null;
        return fs.readFileSync(fd);
      } finally { fs.closeSync(fd); }
    } finally { fs.closeSync(dir); }
  } catch { return null; }
}
