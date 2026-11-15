// `kuvo/box/permissions` (PROTOCOL.md, "This Mac's box: its helper", "What macOS allows"): what
// macOS allows the program this Mac's box runs under, Kuvo Helper, as facts for a client to draw.
// Only the helper can ask macOS that about itself, so it notes it in `run/permissions.json` and the
// box reads it here. A machine's box, on Linux or a Mac, runs under no such program: `applies` says so.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../shared/json";
import type { BoxPermissions } from "../shared/routes/box";
import { HELPER_NAME } from "./helper";
import { APP_FOLDER, K, recordedKind } from "./home";
import type { AnswerPiece, RouteContext } from "./server/context";

/** Whether a process of yours with that number runs now. (The helper is yours: one this can't
 * signal is someone else's, which took the number of a helper that's gone.) */
function runs(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

/** When the process with that number started, in seconds, as `ps` says how long it has run (to the
 * second); null where that can't be told. */
function startedAt(pid: number): number | null {
  const asked = spawnSync("/bin/ps", ["-o", "etime=", "-p", String(pid)], { encoding: "utf8", timeout: 10_000, stdio: ["ignore", "pipe", "ignore"] });
  // `[[days-]hours:]minutes:seconds`.
  const ran = asked.status === 0 ? (asked.stdout ?? "").trim().match(/^(?:(?:(\d+)-)?(\d+):)?(\d+):(\d+)$/) : null;
  if (!ran) return null;
  const seconds = Number(ran[1] ?? 0) * 86_400 + Number(ran[2] ?? 0) * 3600 + Number(ran[3]) * 60 + Number(ran[4]);
  return Date.now() / 1000 - seconds;
}

/** Whether the process with that number now is the one that wrote the note: it started when the
 * note says its writer did (within what `ps` can tell). A helper killed where it stood leaves its
 * note behind, and its number goes to another process, of yours as likely as not. */
function wroteIt(pid: number, started: number): boolean {
  if (!runs(pid)) return false;
  const began = startedAt(pid);
  return began !== null && Math.abs(began - started) <= 3;
}

/** What macOS allows this box, as its helper last noted it. `applies`: whether this box runs under
 * a program macOS asks about as one (a Mac's own box in a home of its own). `known`: whether its
 * helper runs and has said (a note left by one that's gone says nothing: `wroteIt`). Each permission true or
 * false, or null where that isn't known. */
export function boxPermissions(): BoxPermissions {
  const applies = process.platform === "darwin" && !APP_FOLDER && recordedKind() === "mac";
  const unknown: BoxPermissions = { applies, name: applies ? HELPER_NAME : null, known: false, screenRecording: null, accessibility: null };
  if (!applies) return unknown;
  let noted: unknown;
  try {
    noted = parsed(fs.readFileSync(path.join(K, "run", "permissions.json"), "utf8"));
  } catch {
    return unknown;
  }
  if (!isObject(noted) || noted.v !== 1 || typeof noted.pid !== "number" || !Number.isInteger(noted.pid) || noted.pid <= 0) return unknown;
  if (typeof noted.started !== "number" || !Number.isFinite(noted.started) || !wroteIt(noted.pid, noted.started)) return unknown;
  if (typeof noted.screenRecording !== "boolean" || typeof noted.accessibility !== "boolean") return unknown;
  return { ...unknown, known: true, screenRecording: noted.screenRecording, accessibility: noted.accessibility };
}

export async function* permissionsRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  return yield* json(200, boxPermissions());
}
