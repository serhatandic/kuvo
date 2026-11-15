// `kuvo/box/permissions` (PROTOCOL.md, "This Mac's box: its helper", "What macOS allows"): what
// macOS allows the program this Mac's box runs under, Kuvo Helper, as facts for a client to draw.
// Only the helper can ask macOS that about itself, so it notes it in `run/permissions.json` and the
// box reads it here. A machine's box, on Linux or a Mac, runs under no such program: `applies` says so.

import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../shared/json";
import type { BoxPermissions } from "../shared/routes/box";
import { HELPER_NAME } from "./helper";
import { APP_FOLDER, K, recordedKind } from "./home";
import type { AnswerPiece, RouteContext } from "./server/context";

/** Whether a process with that number runs now (one of another user's counts: it's there). */
function runs(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
}

/** What macOS allows this box, as its helper last noted it. `applies`: whether this box runs under
 * a program macOS asks about as one (a Mac's own box in a home of its own). `known`: whether its
 * helper runs and has said (a note left by one that's gone says nothing). Each permission true or
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
  if (!isObject(noted) || noted.v !== 1 || typeof noted.pid !== "number" || !Number.isInteger(noted.pid) || noted.pid <= 1 || !runs(noted.pid)) return unknown;
  if (typeof noted.screenRecording !== "boolean" || typeof noted.accessibility !== "boolean") return unknown;
  return { ...unknown, known: true, screenRecording: noted.screenRecording, accessibility: noted.accessibility };
}

export async function* permissionsRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  return yield* json(200, boxPermissions());
}
