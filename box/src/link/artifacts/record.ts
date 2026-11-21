// The record of what agents made, started as the bridge runs it: its recorder for tool calls, the
// watch of bots' folders, and the look at each turn's end for files commands made.

import { sep } from "node:path"
import { BotFolders } from "./folders"
import { Recorder } from "./recorder"
import type { ArtifactStore } from "./store"
import { changedSince } from "./turnfiles"
import type { Handover } from "../library/store"

/**
 * `bots`: the box's bots' folder. `watch`: watch it (a test leaves it to each turn's end);
 * `settle`: how long a change in it settles before it's recorded. `handed`: the Library, told what
 * was handed over.
 */
export function record(store: ArtifactStore, { bots, watch = true, settle = 1000, handed }: { bots?: string; watch?: boolean; settle?: number; handed?: (handover: Handover) => void } = {}) {
  let folders: BotFolders | undefined
  const recorder = new Recorder(store, {
    bots,
    handed,
    turnEnded: ({ bot, folder, since, made }) => {
      if (bot && folders) folders.turnEnded(bot, since, made)
      // A project's files a command changed (a bot's own folder is watched, or looked at above).
      if (folder && !(bots && (folder + sep).startsWith(bots + sep))) {
        void store.changeAll(changedSince(folder, since), made("shell"))
      }
    },
  })
  if (bots) {
    folders = new BotFolders(store, recorder, bots, settle)
    folders.start({ watch })
  }
  return { recorder, folders }
}
