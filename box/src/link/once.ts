import fs from "node:fs";

// Whether this process answers once and ends (`box route`, a client's request to a machine over
// SSH, whose answer is read when the process exits): what would outlive the answer goes in a
// process of its own instead.

let answersOnce = false;

/** This process answers once. */
export function answerOnce(): void {
  answersOnce = true;
}

export const answeringOnce = (): boolean => answersOnce;

/** `text` on stdout whole, before anything else happens: for a command whose answer another
 * reads to its end (`$(…)` in the keeper's start), which an exit right after a write that hadn't
 * gone out yet would cut short (a pipe takes it in pieces, and may say "again" meanwhile). */
export function sayWhole(text: string): void {
  const bytes = Buffer.from(text);
  const pause = new Int32Array(new SharedArrayBuffer(4));
  for (let at = 0; at < bytes.length;) {
    try {
      at += fs.writeSync(1, bytes, at, bytes.length - at);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "EAGAIN") throw error;
      Atomics.wait(pause, 0, 0, 5);
    }
  }
}
