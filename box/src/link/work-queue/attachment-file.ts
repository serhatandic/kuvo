// Open the saved file itself. A path check alone would let a replacement race the read.
import fs from "node:fs";
import path from "node:path";
import { FILE_NAME, FILES, ITEM_ID, WORK_QUEUE } from "./store";

const readFlags = fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK;
const same = (a: fs.BigIntStats, b: fs.BigIntStats) => a.dev === b.dev && a.ino === b.ino;
const entry = (file: string) => fs.promises.lstat(file, { bigint: true });

/** Read only a regular file directly under this item's real attachment directory. */
export async function readAttachment(item: string, name: string): Promise<Buffer | null> {
  if (!ITEM_ID.test(item) || !FILE_NAME.test(name)) return null;
  try {
    const run = await fs.promises.realpath(path.dirname(WORK_QUEUE));
    const queue = path.join(run, path.basename(WORK_QUEUE));
    const root = path.join(queue, path.basename(FILES));
    const folder = path.join(root, item);
    const parents = [run, queue, root, folder];
    const before = await Promise.all(parents.map(entry));
    if (before.some(stat => !stat.isDirectory())) return null;
    const dir = await fs.promises.open(folder, readFlags | fs.constants.O_DIRECTORY);
    try {
      if (!same(await dir.stat({ bigint: true }), before[before.length - 1])) return null;
      const file = path.join(folder, name);
      const fd = await fs.promises.open(file, readFlags);
      try {
        const opened = await fd.stat({ bigint: true });
        if (!opened.isFile() || !same(opened, await entry(file))) return null;
        // Check every parent after the file identity check. A directory temporarily replaced
        // by a symlink and then put back also changes its parent's change time.
        const after = await Promise.all(parents.map(entry));
        if (after.some((stat, at) => !same(before[at], stat) || before[at].ctimeNs !== stat.ctimeNs)) return null;
        return await fd.readFile();
      } finally { await fd.close(); }
    } finally { await dir.close(); }
  } catch { return null; }
}
