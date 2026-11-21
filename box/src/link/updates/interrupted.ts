import fs from "node:fs";
import path from "node:path";
import { exclusiveWhile, writeDurablePrivate } from "../../shared/files";
import { isObject } from "../../shared/json";
import type { Target } from "../../shared/records";
import { call, readBridgeFile, readEngineFile } from "../engine/client";
import { queueOf, resumeQueue, sendInto } from "../engine/queue";
import { messageID } from "../engine/sessions";
import { previousBin } from "./versions";

interface CutTurn {
  lane: "engine" | "bridge";
  session: string;
  message: string;
  pid: number;
}

const record = (root: string) => path.join(root, "run", "continue-after-update.json");
const readTarget = { engine: readEngineFile, bridge: readBridgeFile };

async function active(target: Target): Promise<string[]> {
  const sessions = await call(target, "GET", "/api/session/active", undefined, AbortSignal.timeout(3_000));
  return Array.isArray(sessions) ? sessions.filter((id): id is string => typeof id === "string")
    : isObject(sessions) ? Object.keys(sessions) : [];
}

export async function rememberCutTurns(root: string): Promise<void> {
  if (fs.existsSync(record(root))) await continueCutTurns(root);
  const turns = savedTurns(root);
  for (const lane of ["engine", "bridge"] as const) {
    let target;
    try { target = readTarget[lane](); } catch { continue; }
    if (!target?.pid) continue;
    for (const session of await active(target)) {
      if (!queueOf(session).paused && !turns.some(turn => turn.lane === lane && turn.session === session)) {
        turns.push({ lane, session, message: messageID(), pid: target.pid });
      }
    }
  }
  if (turns.length) writeDurablePrivate(record(root), JSON.stringify(turns));
}

export async function continueCutTurns(root: string): Promise<void> {
  if (fs.existsSync(previousBin(root)) || !fs.existsSync(record(root))) return;
  await exclusiveWhile(record(root) + ".lock", () => continueRecordedTurns(root));
}

function savedTurns(root: string): CutTurn[] {
  try { return JSON.parse(fs.readFileSync(record(root), "utf8")); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
    throw error;
  }
}

async function continueRecordedTurns(root: string): Promise<void> {
  const turns = savedTurns(root);
  for (const turn of [...turns]) {
    try {
      const target = readTarget[turn.lane]();
      if (!target?.pid) continue;
      const current = queueOf(turn.session);
      // The original process survived (the switch aborted), the bridge adopted the turn,
      // or the user has since stopped it. None needs another prompt.
      if (target.pid !== turn.pid && !current.userPaused && !current.limitStop
          && !(await active(target)).includes(turn.session)) {
        const route = `/api/session/${encodeURIComponent(turn.session)}/prompt-receipt/${encodeURIComponent(turn.message)}`;
        const exists = await call(target, "GET", route, undefined, AbortSignal.timeout(3_000)).catch(error => {
          if (error?.status === 404) return null;
          throw error;
        });
        if (!exists) {
          if (current.paused) await resumeQueue(target, turn.session);
          await sendInto(target, turn.session, { id: turn.message, text: "continue", delivery: "queue" });
        }
      }
      turns.splice(turns.indexOf(turn), 1);
      writeDurablePrivate(record(root), JSON.stringify(turns));
    } catch (error) {
      console.error("Couldn’t continue an interrupted turn:", error);
    }
  }
  if (!turns.length) fs.rmSync(record(root), { force: true });
}
