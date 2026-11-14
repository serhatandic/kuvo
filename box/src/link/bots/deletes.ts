// What deleting a bot takes with it, in this box's words (`deletes` in `kuvo/bots`, PROTOCOL.md,
// `bots.deletes`): what `deleteBot` does (edit.ts), said before it's asked, so every client's
// confirm shows the same lines and none writes its own. For a bot here, as it is now; for one on
// another of your computers, as far as that computer's kind says (where its folder goes).

import type { BotDeletes } from "../../shared/routes/bots";

/** Where a deleted bot's folder ends up: the Finder's Trash (a Mac whose app takes it there), or
 * Kuvo Code's own trash folder on that computer (`folder`, as it's written for you, where known);
 * `unsaid` for a computer that hasn't told (one from before it did). */
export type FolderGoes = { to: "trash" } | { to: "kuvo"; folder?: string } | { to: "unsaid" };

const counted = (count: number, one: string, many: string): string => (count === 1 ? one : `${count} ${many}`);

/** The lines, in the order `deleteBot` does them, then what stays. `computer`: how that computer is
 * named in a sentence ("this Mac", "lab"). `wakes` and `watching`: how many wake-ups wait and how
 * many things it watches, where the box asked knows (a bot of its own); left out, it says so
 * without a count. */
export function whatDeleteTakes({ computer, folder, wakes, watching }: { computer: string; folder: FolderGoes; wakes?: number; watching?: number }): BotDeletes {
  const lines: BotDeletes[number][] = [{ what: "conversation", words: "Its conversation is deleted." }];
  lines.push({
    what: "folder",
    words: folder.to === "trash"
      ? `Its folder, with its memory and what it made there, goes to the Trash on ${computer}.`
      : folder.to === "kuvo"
        ? `Its folder, with its memory and what it made there, goes to Kuvo Code’s trash folder on ${computer}${folder.folder ? ` (${folder.folder})` : ""}.`
        : `Its folder, with its memory and what it made there, goes to the Trash on ${computer}, or to Kuvo Code’s trash folder there.`,
  });
  if (wakes === undefined || watching === undefined) lines.push({ what: "wakes", words: "Its wake-ups and watches are cancelled." });
  else if (wakes || watching) {
    const parts = [...(wakes ? [counted(wakes, "wake-up", "wake-ups")] : []), ...(watching ? [counted(watching, "watch", "watches")] : [])];
    lines.push({ what: "wakes", words: `Its ${parts.join(" and ")} ${wakes + watching === 1 ? "is" : "are"} cancelled.` });
  }
  lines.push({ what: "keys", words: "Any keys of its own are removed." });
  lines.push({ what: "groups", words: `It leaves the group chats on ${computer}.` });
  lines.push({ what: "sessions", words: "The sessions it started stay.", kept: true });
  return lines;
}
