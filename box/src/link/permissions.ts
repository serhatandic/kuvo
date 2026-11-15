// `kuvo/box/permissions` (PROTOCOL.md, "This Mac's box: its helper", "What macOS allows"): what
// macOS allows the program this Mac's box runs under, Kuvo Helper, as facts for a client to draw.
// Only the helper can ask macOS that about itself, so it notes it in `run/permissions.json` and the
// box reads it here. A machine's box, on Linux or a Mac, runs under no such program: `applies` says so.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../shared/json";
import type { BoxPermissions } from "../shared/routes/box";
import { writePrivate } from "./files";
import { HELPER_NAME, helperBundle } from "./helper";
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

/** Each permission macOS can be asked about, as System Settings names it, and the address that
 * opens its list there. Both are lists macOS has you add a program to yourself (`byHand`): it asks
 * nothing for a program that never asks, and the helper doesn't. */
const PERMISSIONS = [
  { id: "screenRecording", name: "Screen Recording", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture", byHand: true },
  { id: "accessibility", name: "Accessibility", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility", byHand: true },
] as const;
type PermissionID = (typeof PERMISSIONS)[number]["id"];

/** What you've put off ("Not Now"), kept in the box so every client agrees and none asks at each
 * open: `box/permissions-later.json`, `{"later": [<id>…]}`. */
const LATER = () => path.join(K, "box", "permissions-later.json");
function putOff(): Set<string> {
  try {
    const kept = parsed(fs.readFileSync(LATER(), "utf8"));
    return new Set(isObject(kept) && Array.isArray(kept.later) ? kept.later.filter((id): id is string => typeof id === "string") : []);
  } catch {
    return new Set();
  }
}
function keepPutOff(later: Set<string>): void {
  if (!later.size) return fs.rmSync(LATER(), { force: true });
  fs.mkdirSync(path.dirname(LATER()), { recursive: true, mode: 0o700 });
  writePrivate(LATER(), JSON.stringify({ later: [...later].sort() }));
}

/** What macOS allows this box, as its helper last noted it. `applies`: whether this box runs under
 * a program macOS asks about as one (a Mac's own box in a home of its own). `known`: whether its
 * helper runs and has said (a note left by one that's gone says nothing: `wroteIt`). Each permission true or
 * false, or null where that isn't known. `bundle`: that program on this Mac, what you drag into a
 * list in System Settings. `list`: each permission in words, with whether a client brings it up
 * now (`asks`: known not to be allowed, and not put off). One that's allowed is no longer put off. */
export function boxPermissions(): BoxPermissions {
  const applies = process.platform === "darwin" && !APP_FOLDER && recordedKind() === "mac";
  const later = applies ? putOff() : new Set<string>();
  const said = (allowed: Record<PermissionID, boolean> | null): BoxPermissions => ({
    applies,
    name: applies ? HELPER_NAME : null,
    known: allowed !== null,
    screenRecording: allowed ? allowed.screenRecording : null,
    accessibility: allowed ? allowed.accessibility : null,
    bundle: applies ? helperBundle() : null,
    list: applies ? PERMISSIONS.map(each => ({ ...each, allowed: allowed ? allowed[each.id] : null, asks: allowed !== null && !allowed[each.id] && !later.has(each.id) })) : [],
  });
  const unknown = said(null);
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
  const allowed = { screenRecording: noted.screenRecording, accessibility: noted.accessibility };
  // (Allowed since it was put off: asked about again if it's ever taken away.)
  const still = new Set([...later].filter(id => PERMISSIONS.some(each => each.id === id && !allowed[each.id])));
  if (still.size !== later.size) {
    try { keepPutOff(still); } catch { /* said again at the next ask */ }
    later.clear();
    for (const id of still) later.add(id);
  }
  return said(allowed);
}

/** `GET`: the facts. `POST {later: "<id>"}`: that one put off ("Not Now"), then the facts. Nothing
 * here grants anything: only you do, in System Settings. */
export async function* permissionsRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  if (method === "GET") return yield* json(200, boxPermissions());
  if (method !== "POST") return yield* json(405, {});
  let asked: unknown;
  try { asked = parsed((request.body ?? Buffer.alloc(0)).toString("utf8")); } catch { asked = null; }
  const id = isObject(asked) ? asked.later : null;
  if (typeof id !== "string" || !PERMISSIONS.some(each => each.id === id)) return yield* json(400, { message: "Say which permission to ask about later." });
  const now = boxPermissions();
  if (!now.applies) return yield* json(409, { message: "Nothing on this computer needs macOS's permission." });
  keepPutOff(new Set([...putOff(), id]));
  return yield* json(200, boxPermissions());
}
