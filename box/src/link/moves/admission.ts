// In-flight prompt writes are visible to the move worker even when a separate process serves them.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { sleep } from "../../shared/wait";
import { MOVING } from "../bots/here";
import { RUN } from "../home";
import { frozen, liveMoves, movedAway, readMoves } from "./records";

const folder = path.join(RUN, "move-prompt-writes");
const prefixOf = (session: string) => crypto.createHash("sha256").update(session).digest("hex") + "-";
const frozenHere = (session: string) => frozen(liveMoves(readMoves(true))).has(session) || movedAway().some(move => move.session === session);

export class MoveAdmissionClosed extends Error {
  constructor() { super(MOVING); }
}

export async function admitPrompt<T>(session: string, write: () => Promise<T>): Promise<T> {
  fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
  const marker = path.join(folder, `${prefixOf(session)}${process.pid}-${crypto.randomUUID()}`);
  fs.closeSync(fs.openSync(marker, "wx", 0o600));
  try {
    // Register before checking the freeze. A snapshot either sees this write or it refuses here.
    if (frozenHere(session)) throw new MoveAdmissionClosed();
    return await write();
  } finally {
    fs.rmSync(marker, { force: true });
  }
}

function activeWrites(session: string): boolean {
  let entries: string[];
  try { entries = fs.readdirSync(folder); } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
    throw error;
  }
  const prefix = prefixOf(session);
  return entries.some(name => {
    if (!name.startsWith(prefix)) return false;
    const pid = /^([1-9][0-9]*)-/.exec(name.slice(prefix.length));
    if (!pid) return false;
    try { process.kill(Number(pid[1]), 0); } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== "ESRCH") throw error;
      fs.rmSync(path.join(folder, name), { force: true });
      return false;
    }
    return true;
  });
}

// Called after the durable freeze covers these sessions. No new prompt writes can be admitted.
export async function waitForPromptWrites(sessions: readonly string[]): Promise<void> {
  const deadline = performance.now() + 125_000;
  while (sessions.some(activeWrites)) {
    if (performance.now() >= deadline) throw new Error("A prompt write has not finished");
    await sleep(25);
  }
}
