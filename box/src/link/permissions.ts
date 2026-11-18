// `kuvo/box/permissions` (PROTOCOL.md, "This Mac's box: its helper", "What macOS allows"): what
// macOS allows the program this Mac's box runs under, Kuvo Helper, as facts for a client to draw.
// Only the helper can ask macOS that about itself, so it notes it in `run/permissions.json` and the
// box reads it here. Darwin machine boxes report Files once they use the signed keeper.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../shared/json";
import type { BoxPermissions } from "../shared/routes/box";
import { textOf, writePrivate } from "./files";
import { HELPER_NAME, helperBundle, machineHelperAvailable, machineKeeperIdentity, machineMark, machinePermissionWatcher } from "./helper";
import { APP_FOLDER, K, recordedKind } from "./home";
import { appHost, askHost } from "./host-interface";
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

/** Each permission macOS can be asked about, as System Settings names it, what agents need it for
 * (`line`), and the address that opens its list there. Both are lists macOS has you add a program to yourself (`byHand`): it asks
 * nothing for a program that never asks, and the helper doesn't. */
const FILES = { id: "fullDiskAccess", name: "Files", line: "For agents that work with your files anywhere on this Mac.", settings: "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles", byHand: true } as const;
const PERMISSIONS = [
  { id: "screenRecording", name: "Screen Recording", line: "For agents that look at the screen.", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture", byHand: true },
  { id: "accessibility", name: "Accessibility", line: "For agents that click and type in other apps.", settings: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility", byHand: true },
  FILES,
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

function permissionNote(maxAge: number, helper?: number) {
  let noted: unknown;
  try { noted = parsed(textOf(path.join(K, "run", "permissions.json")) ?? ""); } catch { return null; }
  if (!isObject(noted) || noted.v !== 1 || typeof noted.pid !== "number" || !Number.isInteger(noted.pid) || noted.pid <= 1) return null;
  if (helper !== undefined && noted.pid !== helper) return null;
  const age = Date.now() - Number(noted.observedAt);
  if (typeof noted.observedAt !== "number" || !Number.isFinite(age) || age < 0 || age > maxAge) return null;
  if (typeof noted.started !== "number" || !Number.isFinite(noted.started) || !wroteIt(noted.pid, noted.started)) return null;
  return noted;
}

/** A recent grant from the exact Helper that currently supervises this machine. */
function machinePermissionNote() {
  const keeper = machineKeeperIdentity();
  if (keeper) return permissionNote(12_000, keeper.helper);
  const noted = permissionNote(12_000);
  return noted && typeof noted.pid === "number" && machinePermissionWatcher(noted.pid) ? noted : null;
}

export function machineFilesAllowed(helper?: number): boolean {
  return (helper === undefined ? machinePermissionNote() : permissionNote(12_000, helper))?.fullDiskAccess === true;
}

/** What macOS allows this box, as its helper last noted it. `applies`: whether this box runs under
 * a program macOS asks about as one (a Mac's own box in a home of its own). `known`: whether its
 * helper runs and has said (a note left by one that's gone says nothing: `wroteIt`). Each permission true or
 * false, or null where that isn't known. `bundle`: that program on this Mac, what you drag into a
 * list in System Settings. `list`: each permission in words, with whether a client brings it up
 * now (`asks`: known not to be allowed, and not put off). One that's allowed is no longer put off. */
export function boxPermissions(): BoxPermissions {
  const darwin = process.platform === "darwin" && !APP_FOLDER;
  const ownMac = darwin && recordedKind() === "mac";
  const machine = darwin && !ownMac && (fs.existsSync(machineMark) || machineHelperAvailable());
  const applies = ownMac || machine;
  const permissions = ownMac ? PERMISSIONS : machine ? [FILES] : [];
  const later = applies ? putOff() : new Set<string>();
  const said = (allowed: Partial<Record<PermissionID, boolean>> = {}): BoxPermissions => ({
    applies,
    name: applies ? HELPER_NAME : null,
    known: Object.keys(allowed).length > 0,
    screenRecording: allowed.screenRecording ?? null,
    accessibility: allowed.accessibility ?? null,
    bundle: applies ? helperBundle() : null,
    list: permissions.map(each => ({ ...each,
      byHandLine: `Drag ${HELPER_NAME} into the list in System Settings, then switch it on.`,
      unknownLine: `${HELPER_NAME} hasn’t said yet.`,
      allowed: allowed[each.id] ?? null, asks: allowed[each.id] === false && !later.has(each.id) })),
  });
  const unknown = said();
  if (!applies) return unknown;
  const noted = ownMac ? permissionNote(75_000) : machinePermissionNote();
  if (!noted) return unknown;
  if (ownMac && (typeof noted.screenRecording !== "boolean" || typeof noted.accessibility !== "boolean")) return unknown;
  const allowed: Partial<Record<PermissionID, boolean>> = {};
  for (const { id } of permissions) {
    if (typeof noted[id] === "boolean") allowed[id] = noted[id];
  }
  // (Allowed since it was put off: asked about again if it's ever taken away.)
  const still = new Set([...later].filter(id => PERMISSIONS.some(each => each.id === id && allowed[each.id] !== true)));
  if (still.size !== later.size) {
    try { keepPutOff(still); } catch { /* said again at the next ask */ }
    later.clear();
    for (const id of still) later.add(id);
  }
  return said(allowed);
}

/** How long a client's ask waits for the helper to look again: past that, the note as it is. */
const FRESH_WITHIN = 2_000;

/** As `boxPermissions`, the helper asked first to note what macOS allows it now (its host's
 * `permissions`): its own look is only every minute, and a client asks as you come back from
 * System Settings. Where it can't be asked (no helper answering, one from before it could), or
 * doesn't answer within two seconds, the note as it is. */
export async function freshBoxPermissions(): Promise<BoxPermissions> {
  const host = process.platform === "darwin" && !APP_FOLDER && recordedKind() === "mac" ? appHost() : null;
  if (host) {
    let waited: ReturnType<typeof setTimeout> | undefined;
    await Promise.race([
      askHost(host, "permissions", {}, true).catch(() => null),
      new Promise(resolve => { waited = setTimeout(resolve, FRESH_WITHIN); }),
    ]);
    clearTimeout(waited);
  }
  return boxPermissions();
}

/** `GET`: the facts, as the helper sees them now. `POST {later: "<id>"}`: that one put off ("Not
 * Now"), then the facts. Nothing here grants anything: only you do, in System Settings. */
export async function* permissionsRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  if (method === "GET") return yield* json(200, await freshBoxPermissions());
  if (method !== "POST") return yield* json(405, {});
  let asked: unknown;
  try { asked = parsed((request.body ?? Buffer.alloc(0)).toString("utf8")); } catch { asked = null; }
  const id = isObject(asked) ? asked.later : null;
  if (typeof id !== "string" || !PERMISSIONS.some(each => each.id === id)) return yield* json(400, { message: "Say which permission to ask about later." });
  const now = boxPermissions();
  if (!now.applies) return yield* json(409, { message: "Nothing on this computer needs macOS's permission." });
  if (!now.list.some(each => each.id === id)) return yield* json(409, { message: "This permission doesn’t apply to Kuvo on this computer." });
  try {
    keepPutOff(new Set([...putOff(), id]));
  } catch {
    return yield* json(500, { message: "Kuvo couldn't keep that on this Mac. Try again." });
  }
  return yield* json(200, boxPermissions());
}
