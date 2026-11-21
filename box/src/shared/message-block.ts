/** A block at a message's end, found where it is (`open`), with its lines from its heading on. */
interface FoundBlock {
  open: number;
  lines: string[];
}

/** A kind of block at a message's end: its tag, and how to take it off and to escape one. */
export interface MessageBlock {
  start: string;
  end: string;
  split: (text: string) => { before: string; lines: string[] } | null;
  escaping: (text: string) => string;
  startOf: (text: string) => number | undefined;
  mapLines: (text: string, map: (lines: string[]) => string[]) => string;
}

// A block the app adds to the end of a message (MessageBlock.swift): its tag on a line of its own,
// its heading, what's in it, then its closing tag, with nothing after.
export function messageBlock(start: string, end: string, headings: string[]): MessageBlock {
  const find = (text: string): FoundBlock | null => {
    const body = text.replace(/\s+$/u, "");
    if (!body.endsWith(`\n${end}`)) return null;
    const close = body.length - end.length;
    const opening = `${start}\n`;
    let before = close;
    for (;;) {
      const open = before - opening.length < 0 ? -1 : text.lastIndexOf(opening, before - opening.length);
      if (open < 0) return null;
      before = open;
      if (open !== 0 && text[open - 1] !== "\n") continue;
      const lines = text.slice(open + opening.length, close).split("\n").slice(0, -1);
      if (lines.length && headings.includes(lines[0])) return { open, lines };
    }
  };
  return {
    start,
    end,
    startOf: text => find(text)?.open,
    mapLines: (text, map) => {
      const found = find(text);
      if (!found) return text;
      const at = found.open + start.length + 1;
      return text.slice(0, at) + map(found.lines).join("\n") + text.slice(at + found.lines.join("\n").length);
    },
    // The words before the block that ends `text` (trimmed), and its lines from its heading on.
    split: (text: string) => {
      const found = find(text);
      return found ? { before: text.slice(0, found.open).trim(), lines: found.lines } : null;
    },
    // `text` as it can't be read as ending with a block (one copied from a message): its tag gets
    // a space after it.
    escaping: (text: string) => {
      let found: FoundBlock | null;
      while ((found = find(text))) text = text.slice(0, found.open + start.length) + " " + text.slice(found.open + start.length);
      return text;
    },
  };
}

export const FROM_PHONE_BLOCK = messageBlock("<from-phone>", "</from-phone>", ["The user sent this from Kuvo Code on their iPhone."]);
export const ATTACHED_HEADINGS = [
  "The attached files are on disk here (what's in them is in this message already):",
  "Open these attached files with your tools if you need them:",
  "These attached files stayed on the user's Mac (they couldn't be read, or copied to this machine):",
] as const;
export const ATTACHED_BLOCK = messageBlock("<attached-files>", "</attached-files>", [...ATTACHED_HEADINGS]);

export const UNSENT_SUFFIX = " (on the user's Mac: it couldn't be copied to this machine)";
