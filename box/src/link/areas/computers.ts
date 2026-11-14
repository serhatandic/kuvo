// Your computers, as this box knows them (PROTOCOL.md, "Computers", `kuvo/computers`): itself, its
// peers (the boxes its Macs name), and the boxes those vouch for (plan limits only). What each is,
// whether it's heard from, what it runs, how many of its sessions are at work, and which services'
// plan limits it reads. Every box answers it alike, a Mac's and a machine's, with no client there:
// each tells its peers its own facts with its agents (`peers/agents.ts`), so none is a hub.
// Platform: the system's name is read from macOS's own record of its version, or Linux's
// `os-release`; the rest is the same on both.

import fs from "node:fs";
import os from "node:os";
import { isObject } from "../../shared/json";
import type { ListedComputer } from "../../shared/routes/box";
import type { Target, ToldComputer } from "../../shared/records";
import { call, readEngineFile } from "../engine/client";
import { followedLive } from "../follow";
import { IS_MAC, LINK_FILE, MAC_APP_VERSION, MAC_ENGINE_VERSION } from "../home";
import { keptList, listFollowers, listNow } from "../lists/sessions";
import { PEER_AGENTS_FILE, readPeerAgents, vouchedOf } from "../peers/directory";
import { ownKey, peersOf } from "../peers/known";
import { awaySince, PEER_REACH, readReach } from "../peers/reach";
import { sharedMachine } from "../setup/settle";
import { macLink, readLink } from "../state";
import { boxVersion } from "../updates/update";
import type { AnswerPiece, RouteContext } from "../server/context";

export const COMPUTERS_ROUTE = /^kuvo\/computers(\/events)?$/;

/** How long since a peer's last word (its agents, every ten minutes while it's on the relay) it
 * still counts as heard from. */
const HEARD_LATELY = 30 * 60_000;

/** The system this box runs on, as people know it ("macOS 15.1", "Ubuntu 24.04.1 LTS"); none where it can't be read. */
export function systemName(platform: NodeJS.Platform = process.platform, read: (file: string) => string = file => fs.readFileSync(file, "utf8")): string | undefined {
  try {
    if (platform === "darwin") {
      const version = read("/System/Library/CoreServices/SystemVersion.plist").match(/<key>ProductVersion<\/key>\s*<string>([^<]{1,40})<\/string>/)?.[1];
      return version ? `macOS ${version}` : "macOS";
    }
    if (platform === "linux") {
      const name = read("/etc/os-release").match(/^PRETTY_NAME="?([^"\n]{1,80})"?$/m)?.[1];
      return name || "Linux";
    }
  } catch {
    return platform === "darwin" ? "macOS" : platform === "linux" ? "Linux" : undefined;
  }
  return undefined;
}

let system: { name: string | undefined } | null = null;

/** How many of this box's sessions are at work now, as its session list has them: conversations
 * (not a subagent's or a run kept out of sight), not archived, this box's own copy, and never one
 * that waits on you. */
export function workingHere(): number {
  return Object.values(keptList()?.sessions ?? {}).filter(({ facts }) => facts.running && facts.asked === undefined && !facts.archived && !facts.away && facts.kind !== "subagent" && facts.kind !== "hidden").length;
}

/** What this box says of itself, here and to its peers (with its agents' mail): Kuvo Code's version
 * and its engine's, its system, and how many of its sessions are at work. */
export function ownComputer(): ToldComputer {
  system ??= { name: systemName() };
  let engine: string | undefined;
  try { engine = readEngineFile()?.version || undefined; } catch {}
  const version = IS_MAC ? macLink.config?.app || MAC_APP_VERSION || undefined : boxVersion() || undefined;
  return {
    ...(version ? { version } : {}),
    ...(engine ?? (IS_MAC ? MAC_ENGINE_VERSION : null) ? { engine: (engine ?? MAC_ENGINE_VERSION)! } : {}),
    ...(system.name ? { system: system.name } : {}),
    working: workingHere(),
  };
}

/** The services whose plan limits each computer reads, by its key (this box's under `here`), as the
 * coding agents' bridge lists them (`rows`' `boxes`); none while it doesn't run. */
async function limitsRead(bridge: Target | null, signal?: AbortSignal): Promise<Map<string, string[]>> {
  const read = new Map<string, string[]>();
  if (!bridge) return read;
  const limits = await call(bridge, "GET", "/api/kuvo/limits", undefined, signal ? AbortSignal.any([signal, AbortSignal.timeout(5_000)]) : AbortSignal.timeout(5_000)).catch(() => null);
  for (const row of isObject(limits) && Array.isArray(limits.rows) ? limits.rows : []) {
    if (!isObject(row) || typeof row.service !== "string" || !Array.isArray(row.boxes)) continue;
    for (const box of row.boxes) {
      if (!isObject(box)) continue;
      const key = box.here === true ? "here" : typeof box.key === "string" ? box.key : "";
      const services = read.get(key) ?? [];
      if (!services.includes(row.service)) services.push(row.service);
      read.set(key, services);
    }
  }
  return read;
}

/** Your computers: this box first, then its peers by name, then the boxes they vouch for. */
export async function listComputers(name: string, bridge: Target | null, signal?: AbortSignal, now = Date.now()): Promise<ListedComputer[]> {
  let link = null as ReturnType<typeof readLink>;
  try { link = readLink(); } catch {}
  const limits = await limitsRead(bridge, signal);
  const own = link ? ownKey(link) : null;
  const told = readPeerAgents();
  const byName = <T extends { name: string; key: string }>(boxes: T[]) => [...boxes].sort((a, b) => a.name.localeCompare(b.name) || a.key.localeCompare(b.key));
  const computers: ListedComputer[] = [{
    key: own ?? "",
    name: (IS_MAC ? macLink.config?.name : name) || name || os.hostname(),
    kind: IS_MAC ? "mac" : "machine",
    here: true,
    shared: sharedMachine(link),
    reach: "connected",
    via: "here",
    ...ownComputer(),
    limits: limits.get("here") ?? (own ? limits.get(own) : undefined) ?? [],
  }];
  // A machine Macs of different setups share lists itself alone: the others there aren't all one
  // person's computers (as `kuvo/setup` refuses there).
  if (sharedMachine(link)) return computers;
  const peers = peersOf(link);
  const reach = readReach();
  for (const peer of byName(peers)) {
    const said = told[peer.key];
    const away = awaySince(peer.key, reach) !== null || (said?.heard !== undefined && now - said.heard > HEARD_LATELY);
    computers.push({
      key: peer.key,
      name: peer.name,
      kind: peer.kind,
      ...(peer.id ? { id: peer.id } : {}),
      shared: said?.shared === true,
      // (Not heard from since this box knew of it, and not found away: still being reached.)
      reach: away ? "away" : said?.heard === undefined ? "connecting" : "connected",
      via: "relay",
      ...(said?.computer ? { ...said.computer, working: away ? 0 : said.computer.working } : { working: 0 }),
      limits: limits.get(peer.key) ?? [],
    });
  }
  // The boxes a peer vouches for (another Mac's, beside a machine two Macs share): known by that
  // peer's word alone, for plan limits, so nothing is known of what they run or whether they answer.
  for (const box of byName(vouchedOf(link, told))) {
    const through = peers.find(peer => told[peer.key]?.boxes?.some(each => each.key === box.key));
    computers.push({ key: box.key, name: box.name, kind: box.kind, shared: false, reach: "unknown", via: "peer", ...(through ? { through: through.name } : {}), working: 0, limits: limits.get(box.key) ?? [] });
  }
  return computers;
}

const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };

export async function* computersRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, info, bridge } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  // (Its own sessions at work, from its list: read first where this process hasn't yet.)
  await listNow(context, signal ?? undefined).catch(() => null);
  const read = async () => ({ computers: await listComputers(info.name ?? "", bridge(), signal ?? undefined) });
  if (!match[1]) return yield* json(200, await read());
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // A line now, and one as it changes: its peers, what they told, which are away (their files),
  // and its own sessions starting and stopping work; and every 15 s for the rest (its limits).
  // (Of its list's changes, only how many are at work: it changes with every step of a run.)
  const listen = (told: () => void) => {
    let was = workingHere();
    const listTold = () => {
      const now = workingHere();
      if (now === was) return;
      was = now;
      told();
    };
    listFollowers.add(listTold);
    return () => { listFollowers.delete(listTold); };
  };
  return yield* followedLive(read, listen, signal,
    { look: () => `${mtime(PEER_AGENTS_FILE)} ${mtime(PEER_REACH)} ${mtime(LINK_FILE)}` });
}
