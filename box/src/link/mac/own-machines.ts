// A Mac's machines for a phone, answered by its box alone (PROTOCOL.md, "Machines", In a home of
// its own). A Mac's box in a home of its own adds, removes and uninstalls computers itself
// (`add/flow.ts`, `peers/manage.ts`) and writes their list (`machines.json`, `phone-link.json`), so
// what a phone asks of `kuvo/machines…` and `kuvo/hosts…`, as it asked the Mac's app, is answered
// here with no app: the same routes and shapes, said from this box's own facts. It holds no
// connection to a machine (each runs by itself, and is met through the relay), so what stood for one
// (Connect, Disconnect, Make This Mac Its Home) is refused in words, and never offered.

import fs from "node:fs";
import { isObject, parsed, type Body } from "../../shared/json";
import type { MacMachines } from "../../shared/routes/mac";
import { addressProblem, cleanAddress, suggestedHosts } from "../add/address";
import { beginAdd, beginUninstall, canAdd, takenAddresses } from "../add/flow";
import { type Running, stopTry, tryFollowers, underWay } from "../add/tries";
import { COMPUTERS_ROUTE, computersRoute } from "../areas/computers";
import { writePrivate } from "../files";
import { followedLive } from "../follow";
import { LINK_FILE } from "../home";
import { ownName } from "../letters/start";
import { PEER_AGENTS_FILE, readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { forgetSaved, removeHere } from "../peers/manage";
import { awaySince, PEER_REACH, readReach } from "../peers/reach";
import type { AnswerPiece, RouteContext } from "../server/context";
import { addElsewhere, mainElsewhere } from "../setup/main-only";
import { readLink } from "../state";
import { hostsWithoutApp, keptHosts, LIST_FILE, PHONE_LINK_FILE, savedMachines, UNREADABLE } from "./machines";

/** How long since a machine's box said its agents (every ten minutes, while it's on the relay) it
 * still counts as heard from: as `kuvo/computers` counts it. */
const HEARD_LATELY = 30 * 60_000;

type Machine = MacMachines["machines"][number];

const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };

/** `kuvo/machines` as this box says it by itself: each machine on its list by the name your
 * computers know it by, connected while this box hears from it, and being set up or uninstalled
 * while that's under way here; with the hosts in `~/.ssh/config` it could add. Null when the list
 * can't be read, which isn't none. */
export function ownMachines(now = Date.now()): MacMachines | null {
  const saved = savedMachines();
  if (!saved) return null;
  const peers = peersOf(readLink());
  const told = readPeerAgents();
  const reach = readReach();
  const machines = saved.map(({ destination, otherAddresses }): Machine => {
    const peer = peers.find(each => each.id === destination);
    const said = peer ? told[peer.key] : undefined;
    const busy = underWay(destination);
    const heard = said?.heard ?? said?.at;
    const away = !peer || awaySince(peer.key, reach) !== null || (heard !== undefined && now - heard > HEARD_LATELY);
    const [connection, status] = busy ? ["installing", busy.seen.title]
      : !peer ? ["notConnected", "Not connected"]
      : away ? ["notConnected", "Not heard from lately"]
      : heard === undefined ? ["connecting", "Connecting…"] : ["connected", "Connected"];
    return { id: destination, name: peer?.name || destination, otherAddresses: Array.isArray(otherAddresses) ? otherAddresses : [], connection: connection!, status: status! };
  });
  return { machines, hosts: canAdd().can ? suggestedHosts(takenAddresses()) : [] };
}

/** Once `it` is over, or asks you something. */
function settled(it: Running): Promise<void> {
  return new Promise(resolve => {
    const done = () => {
      tryFollowers.delete(look);
      resolve();
    };
    const look = () => { if (it.seen.state === "asking") done(); };
    tryFollowers.add(look);
    void it.over.then(done);
    look();
  });
}

const failureOf = (it: Running, otherwise: string): string => it.seen.failure?.message || otherwise;

/** What a phone asks of this Mac's machines and of the machines it reaches directly, answered by
 * this box itself. */
export async function* ownMachinesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, signal, json } = context;
  const method = request.method.toUpperCase();
  const read = method === "GET" || method === "HEAD";
  // On a member of a setup, only its main computer adds and removes computers (PROTOCOL.md, "Your setup").
  const main = read ? null : mainElsewhere(readLink());
  if (main) return yield* json(409, addElsewhere(main));
  const parts = route.split("/").map(part => { try { return decodeURIComponent(part); } catch { return part; } });
  const here = ownName();

  if (parts[1] === "hosts") {
    if (parts.length === 2) {
      if (!read) return yield* json(405, {});
      const hosts = hostsWithoutApp();
      return yield* json(hosts ? 200 : 503, hosts ?? { message: UNREADABLE });
    }
    if (parts.length !== 3 || method !== "DELETE") return yield* json(404, {});
    return yield* forgetHost(context, parts[2]!, here);
  }

  if (route === "kuvo/machines/events") {
    if (method !== "GET") return yield* json(405, {});
    const first = ownMachines();
    if (!first) return yield* json(503, { message: UNREADABLE });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    // (A list that stops reading says nothing new: what it said last stands, as a read says why.)
    return yield* followedLive(async () => ownMachines() ?? undefined, told => {
      tryFollowers.add(told);
      return () => { tryFollowers.delete(told); };
    }, signal, { look: () => `${mtime(LIST_FILE)} ${mtime(PEER_AGENTS_FILE)} ${mtime(PEER_REACH)} ${mtime(LINK_FILE)}` });
  }

  if (parts.length === 2) {
    if (read) {
      const own = ownMachines();
      return yield* json(own ? 200 : 503, own ?? { message: UNREADABLE });
    }
    if (method !== "POST") return yield* json(405, {});
    return yield* add(context, here);
  }

  const saved = savedMachines();
  if (!saved) return yield* json(503, { message: UNREADABLE });
  const machine = saved.find(each => each.destination === parts[2]);
  if (!machine) return yield* json(404, {});
  const peer = peersOf(readLink()).find(each => each.id === machine.destination);
  const name = peer?.name || machine.destination;
  const entry = () => ownMachines()?.machines.find(each => each.id === machine.destination);
  const action = parts[3];

  if (method === "DELETE" && parts.length === 3) {
    // One this box never signed in to your computers (an address kept from before): only off its list.
    if (!peer) {
      forgetSaved(machine.destination);
      return yield* json(200, {});
    }
    // As your computers' page removes it (`DELETE kuvo/computers/<key>`): its refusals, its words.
    const path = `kuvo/computers/${peer.key}`;
    return yield* computersRoute({ ...context, route: path, request: { ...request, path } }, path.match(COMPUTERS_ROUTE)!);
  }
  if (method === "POST" && action === "uninstall" && parts.length === 4) {
    if (!peer) return yield* json(409, { message: `${here} never finished setting ${name} up, so there’s nothing of its own to uninstall there. Remove it instead.` });
    const can = canAdd();
    if (!can.can) return yield* json(can.status, { message: can.message });
    const begun = beginUninstall(peer.key);
    if (!begun) return yield* json(409, { message: `${here} didn’t set ${name} up over SSH, so it can’t uninstall Kuvo Code there. Remove it instead.` });
    await begun.over;
    if (begun.seen.state === "done") return yield* json(200, {});
    return yield* json(502, { message: failureOf(begun, `Kuvo Code couldn’t be uninstalled from ${name}.`) });
  }
  if (method === "DELETE" && action === "addresses" && parts.length === 5) {
    if (!machine.otherAddresses?.includes(parts[4]!)) return yield* json(404, {});
    writePrivate(LIST_FILE, JSON.stringify(saved.map(each => (each === machine ? { ...each, otherAddresses: machine.otherAddresses!.filter(address => address !== parts[4]) } : each)), null, 2));
    return yield* json(200, { machine: entry() });
  }
  if (method === "POST" && parts.length === 4) {
    // What stood for the old app's SSH connection to it, which no box holds.
    if (action === "connect" || action === "disconnect" || action === "start") {
      return yield* json(409, { message: `Kuvo Code on ${name} runs by itself now, and your devices reach it directly: ${here} keeps no connection to it to ${action === "disconnect" ? "close" : "open"}.` });
    }
    if (action === "home") return yield* json(409, { message: `Kuvo Code on ${name} runs by itself now, with its own accounts and settings, so no Mac is its home.` });
    if (action === "same-as") return yield* json(409, { message: "Kuvo Code tells by itself when an address you add is a computer it already has. To make these two one, remove one of them, then add its address again." });
  }
  return yield* json(404, {});
}

// `POST kuvo/machines {destination}`: this box's own add (`kuvo/computers/add`), waited for, as
// the Mac's app answered once it had checked and added one.
async function* add(context: RouteContext, here: string): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  const destination = cleanAddress((parsed(request.body) as Body<"destination">)?.destination);
  if (!destination) return yield* json(400, { message: "Give the machine’s SSH host: a Host from ~/.ssh/config, or user@host." });
  const problem = addressProblem(destination);
  if (problem) return yield* json(400, { message: problem });
  const can = canAdd();
  if (!can.can) return yield* json(can.status, { message: can.message, ...(can.primary ? { primary: can.primary } : {}) });
  const before = underWay(destination);
  // (Here already, at that address: set up again where it stands, and said so.)
  const had = takenAddresses().includes(destination);
  const begun = beginAdd(destination);
  if ("busy" in begun) return yield* json(409, { message: begun.busy });
  await settled(begun);
  if (begun.seen.state === "asking") {
    // What it asks is answered where its facts are shown (a host key's fingerprint, whose computer
    // it is): the old route has no way to ask, and nothing is trusted or joined unasked.
    const asked = begun.seen.question?.title ?? "It needs an answer from you";
    // (One begun elsewhere, on the Mac itself, goes on there.)
    if (begun !== before) {
      stopTry(begun);
      await begun.over;
    }
    return yield* json(409, { message: `${asked}${/[.?!…]$/.test(asked) ? "" : "."} Add it from Kuvo Code on ${here}, which shows you what to check first.` });
  }
  if (begun.seen.state !== "done") {
    return yield* json(begun.seen.state === "failed" ? 502 : 409, { message: begun.seen.state === "failed" ? failureOf(begun, `${destination} couldn’t be added.`) : `Adding ${destination} was stopped.` });
  }
  // The one it came to: itself, or the computer already here that this address reaches too.
  const key = begun.seen.computer?.key;
  const id = (key ? peersOf(readLink()).find(each => each.key === key)?.id : undefined)
    ?? savedMachines()?.find(each => each.otherAddresses?.includes(destination))?.destination ?? destination;
  const machine = ownMachines()?.machines.find(each => each.id === id);
  if (!machine) return yield* json(502, { message: `${destination} was set up, but ${here} couldn’t read its list of machines afterwards.` });
  return yield* json(200, { machine, known: had || begun.seen.known === true });
}

// `DELETE kuvo/hosts/<id>`: a machine this Mac removed, forgotten for your phones too. One it
// still has is removed first. Your other computers stop counting it among theirs, if they still did.
async function* forgetHost(context: RouteContext, id: string, here: string): AsyncGenerator<AnswerPiece> {
  const { json } = context;
  const kept = keptHosts();
  const saved = savedMachines(kept);
  if (kept === null || !saved) return yield* json(503, { message: UNREADABLE });
  const host = kept?.[id];
  if (!host) return yield* json(404, {});
  if (saved.some(each => each.destination === id)) return yield* json(409, { message: `It’s one of ${here}’s machines: remove it first.` });
  let whole: Record<string, unknown>;
  try {
    const text: unknown = JSON.parse(fs.readFileSync(PHONE_LINK_FILE, "utf8"));
    if (!isObject(text) || !isObject(text.machines)) throw new Error("not a record");
    whole = text;
  } catch {
    return yield* json(503, { message: UNREADABLE });
  }
  const { [id]: _gone, ...rest } = whole.machines as Record<string, unknown>;
  writePrivate(PHONE_LINK_FILE, JSON.stringify({ ...whole, machines: rest }, null, 2));
  // (Removed, said so, unless it's still one of this Mac's under another name.)
  if (typeof host.key === "string" && !Object.values(rest).some(each => isObject(each) && each.key === host.key)) removeHere(host.key);
  return yield* json(200, {});
}
