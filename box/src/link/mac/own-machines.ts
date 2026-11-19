// A Mac's machines for a phone, answered by its box alone (PROTOCOL.md, "Machines", In a home of
// its own). A Mac's box in a home of its own adds, removes and uninstalls computers itself
// (`add/flow.ts`, `peers/manage.ts`) and writes their list (`machines.json`, `phone-link.json`), so
// what a phone asks of `kuvo/machines…` and `kuvo/hosts…`, as it asked the Mac's app, is answered
// here with no app: the same routes and shapes, said from this box's own facts. It holds no
// connection to a machine (each runs by itself, and is met through the relay), so what stood for one
// (Connect, Disconnect) is refused in words. All of this file is for the phone’s old
// Machines page; it goes once the phone draws `kuvo/computers` and `kuvo/computers/add`.

import fs from "node:fs";
import { parsed, type Body } from "../../shared/json";
import type { MacMachines } from "../../shared/routes/mac";
import { addressProblem, cleanAddress, suggestedHosts } from "../add/address";
import { beginAdd, beginUninstall, canAdd, takenAddresses } from "../add/flow";
import { allTries, type Running, stopTry, tryFollowers, underWay } from "../add/tries";
import { COMPUTERS_ROUTE, computersRoute, peerReach } from "../areas/computers";
import { ONCE } from "../areas/computers-add";
import { writePrivate } from "../files";
import { followedLive } from "../follow";
import { LINK_FILE } from "../home";
import { ownName } from "../letters/start";
import { PEER_AGENTS_FILE, readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { forgetSaved, oldAppAttached, oldAppWords } from "../peers/manage";
import { readTries } from "../peers/reach-again";
import { PEER_REACH, readReach } from "../peers/reach";
import type { AnswerPiece, AnyPiece, RouteContext } from "../server/context";
import { acceptedMembership } from "../setup/membership";
import { addElsewhere, mainElsewhere, setupHosts } from "../setup/main-only";
import { readLink } from "../state";
import { changeKeptHosts, hostsWithoutApp, keptHosts, LIST_FILE, localDestination, peerAtDestination, savedMachines, UNREADABLE } from "./machines";

type Machine = MacMachines["machines"][number];

const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };

/** `kuvo/machines` as this box says it by itself: each machine on its list by the name your
 * computers know it by, standing as `kuvo/computers` says it stands (`peerReach`), and being set up
 * or uninstalled while that's under way here; then those being added for the first time, which
 * aren't on the list until they've joined; with the hosts in `~/.ssh/config` it could add. Null
 * when the list can't be read, which isn't none. */
export function ownMachines(now = Date.now()): MacMachines | null {
  const link = readLink();
  const membership = acceptedMembership(link);
  const saved = savedMachines();
  if (!membership && (!saved || link?.setup?.checkpointed)) return null;
  const roster = membership
    ? peersOf(link).filter(peer => peer.kind === "machine").map(peer => {
      const destination = localDestination(link, peer.key);
      return { peer, destination: destination ?? peer.key, otherAddresses: saved?.find(each => each.destination === destination)?.otherAddresses };
    })
    : saved!.map(machine => ({ ...machine, peer: peerAtDestination(link, machine.destination) }));
  const told = readPeerAgents();
  const reach = readReach();
  const tries = readTries();
  const machines = roster.map(({ destination, otherAddresses, peer }): Machine => {
    const busy = underWay(destination);
    const stands = peer ? peerReach(peer.key, told, reach, tries, now).reach : null;
    const [connection, status] = busy ? ["installing", busy.seen.title]
      : stands === "connected" ? ["connected", "Connected"]
      : stands === "connecting" ? ["connecting", "Connecting…"]
      : stands === "away" ? ["notConnected", "Not heard from lately"] : ["notConnected", "Not connected"];
    return { id: destination, name: peer?.name || destination, otherAddresses: Array.isArray(otherAddresses) ? otherAddresses : [], connection: connection!, status: status! };
  });
  const listed = new Set(roster.flatMap(each => [each.destination, ...(Array.isArray(each.otherAddresses) ? each.otherAddresses : [])]));
  for (const seen of allTries().reverse()) {
    if (seen.kind !== "add" || seen.state === "done" || seen.state === "failed" || listed.has(seen.destination)) continue;
    listed.add(seen.destination);
    machines.push({ id: seen.destination, name: seen.name, otherAddresses: [], connection: "installing", status: seen.title });
  }
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

// The computer `key` removed as your computers' page removes it (`DELETE kuvo/computers/<key>`):
// its refusals, its words, and handed to the box that writes its record where that isn't this one.
async function removed(context: RouteContext, key: string): Promise<AnswerPiece[]> {
  const path = `kuvo/computers/${key}`;
  const pieces: AnswerPiece[] = [];
  for await (const piece of computersRoute({ ...context, route: path, request: { ...context.request, method: "DELETE", path } }, path.match(COMPUTERS_ROUTE)!)) pieces.push(piece);
  return pieces;
}
const statusOf = (pieces: AnswerPiece[]): number => (pieces as AnyPiece[]).find(piece => piece.head)?.head?.status ?? 500;

/** What a phone asks of this Mac's machines and of the machines it reaches directly, answered by
 * this box itself. */
export async function* ownMachinesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, signal, json } = context;
  const method = request.method.toUpperCase();
  const read = method === "GET" || method === "HEAD";
  // On a member of a setup, only its main computer adds and removes computers (PROTOCOL.md, "Your setup").
  const link = readLink();
  const main = mainElsewhere(link);
  if (method === "DELETE" && acceptedMembership(link)) {
    const item = /^kuvo\/(?:machines|hosts)\/([^/]+)$/.exec(route)?.[1];
    const key = item && peersOf(link).find(peer => peer.key === item || (!main && localDestination(link, peer.key) === item))?.key;
    if (key) return yield* computersRoute(context, COMPUTERS_ROUTE.exec(`kuvo/computers/${key}`)!);
  }
  if (main && !read) return yield* json(409, addElsewhere(main));
  if ((main || link?.setup?.managed) && read && route === "kuvo/hosts") {
    const hosts = setupHosts(link);
    return yield* json(hosts ? 200 : 503, hosts ?? { message: "This computer can't list your computers yet. Try again in a moment." });
  }
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
  const requested = parts[2]!;
  const known = link?.setup?.checkpointed && peersOf(link).find(peer => peer.key === requested);
  if (known && method === "DELETE" && parts.length === 3) return yield* await removed(context, known.key);
  const destination = known ? localDestination(link, known.key) : requested;
  const machine = saved.find(each => each.destination === destination);
  if (!machine) return yield* json(404, {});
  const peer = peerAtDestination(link, machine.destination);
  const name = peer?.name || machine.destination;
  const entry = () => ownMachines()?.machines.find(each => each.id === machine.destination);
  const action = parts[3];

  // (The old app, attached, keeps a list of its own and would write over what's changed here.)
  const waits = oldAppAttached() ? oldAppWords(here, name) : null;
  if (method === "DELETE" && parts.length === 3) {
    if (peer) return yield* await removed(context, peer.key);
    // One this box never signed in to your computers (an address kept from before): only off its list.
    if (waits) return yield* json(409, { message: waits });
    if (!forgetSaved(machine.destination)) return yield* json(503, { message: "The saved machines list couldn’t be updated." });
    return yield* json(200, {});
  }
  if (method === "POST" && action === "uninstall" && parts.length === 4) {
    // (As `kuvo/computers/<key>/uninstall`: never for `box route`, a single request anything here could run.)
    if (request.fromMac && !request.served) return yield* json(409, { message: ONCE });
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
    if (waits) return yield* json(409, { message: waits });
    writePrivate(LIST_FILE, JSON.stringify(saved.map(each => (each === machine ? { ...each, otherAddresses: machine.otherAddresses!.filter(address => address !== parts[4]) } : each)), null, 2));
    return yield* json(200, { machine: entry() });
  }
  if (method === "POST" && parts.length === 4) {
    // What stood for the old app's SSH connection to it, which no box holds.
    if (action === "connect" || action === "disconnect" || action === "start") {
      return yield* json(409, { message: `Kuvo Code on ${name} runs by itself now, and your devices reach it directly: ${here} keeps no connection to it to ${action === "disconnect" ? "close" : "open"}.` });
    }
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
  // (As `kuvo/computers/add`: never for `box route`, a single request anything here could run.)
  if (request.fromMac && !request.served) return yield* json(409, { message: ONCE });
  const can = canAdd();
  if (!can.can) return yield* json(can.status, { message: can.message, ...(can.primary ? { primary: can.primary } : {}) });
  const before = underWay(destination);
  // Here already, at that address or as another of a machine's, with nothing under way for it:
  // said so at once, as the Mac's app did. (Set Up Again is `kuvo/computers/<key>/setup`.)
  const had = savedMachines()?.find(each => each.destination === destination || each.otherAddresses?.includes(destination));
  if (had && !before && peerAtDestination(readLink(), had.destination)) {
    const machine = ownMachines()?.machines.find(each => each.id === had.destination);
    if (machine) return yield* json(200, { machine, known: true });
  }
  const begun = beginAdd(destination, context.phone);
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
  const id = (key ? localDestination(readLink(), key) : undefined)
    ?? savedMachines()?.find(each => each.otherAddresses?.includes(destination))?.destination ?? destination;
  const machine = ownMachines()?.machines.find(each => each.id === id);
  if (!machine) return yield* json(502, { message: `${destination} was set up, but ${here} couldn’t read its list of machines afterwards.` });
  return yield* json(200, { machine, known: Boolean(had) || begun.seen.known === true });
}

// `DELETE kuvo/hosts/<id>`: a machine this Mac removed, forgotten for your phones too. One it
// still has is removed first. One your computers still count among theirs (its list and their
// record disagree) is removed as a removal removes it, with its refusals, before it's forgotten.
async function* forgetHost(context: RouteContext, id: string, here: string): AsyncGenerator<AnswerPiece> {
  const { json } = context;
  const kept = keptHosts();
  const saved = savedMachines(kept);
  if (kept === null || !saved) return yield* json(503, { message: UNREADABLE });
  // (Its own entry: never what every object has, `constructor`.)
  const host = kept && Object.hasOwn(kept, id) ? kept[id] : undefined;
  if (!host) return yield* json(404, {});
  if (saved.some(each => each.destination === id)) return yield* json(409, { message: `It’s one of ${here}’s machines: remove it first.` });
  if (oldAppAttached()) return yield* json(409, { message: oldAppWords(here, typeof host.name === "string" ? host.name : id) });
  // (Unless it's still one of this Mac's under another name.)
  const stillNamed = typeof host.key === "string" && !Object.entries(kept ?? {}).some(([other, each]) => other !== id && each.key === host.key)
    && peersOf(readLink()).some(peer => peer.key === host.key);
  if (stillNamed) {
    const gone = await removed(context, host.key as string);
    if (statusOf(gone) !== 200 && statusOf(gone) !== 404) return yield* gone;
  }
  const done = changeKeptHosts(machines => {
    if (!Object.hasOwn(machines, id)) return null;
    const { [id]: _gone, ...rest } = machines;
    return rest;
  });
  return yield* done ? json(200, {}) : json(503, { message: UNREADABLE });
}
