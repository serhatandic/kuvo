import { ownComputerName } from "../computer-name";
import { serviceTitle } from "../../shared/account-names";
// Your computers, as this box knows them (PROTOCOL.md, "Computers", `kuvo/computers`): itself, its
// peers (the boxes its Macs name). What each is,
// whether it's heard from, where a client of your own reaches it (its address and relay), what it
// runs and answers, which of its sessions are at work, which
// services' plan limits it reads, and what can be done with it from here: renamed, removed
// (`peers/manage.ts`, handed to the box that writes its record) or reached again
// (`peers/reach-again.ts`); and, on your setup's main computer, which computer is kept from adding
// Kuvo Code's app to your devices (`vouching`), with letting it again (`…/allow-devices`). Every box answers it alike, a Mac's and a machine's, with no client
// there: each tells its peers its own facts with its agents (`peers/agents.ts`), so none is a hub.
// Platform: the system's name is read from macOS's own record of its version, or Linux's
// `os-release`; the rest is the same on both.

import fs from "node:fs";
import os from "node:os";
import { FEATURES } from "../../shared/features";
import { isObject, parsed, type Body } from "../../shared/json";
import type { ListedComputer } from "../../shared/routes/box";
import { macFeatures } from "../../shared/routes/mac";
import { fleetFacts } from "../updates/fleet";
import { appBehind } from "../mac/app";
import type { AskingSession, AtWorkSession, Target, ToldComputer } from "../../shared/records";
import { call, readEngineFile } from "../engine/client";
import { followedLive } from "../follow";
import { IS_MAC, LINK_FILE, MAC_APP_VERSION, MAC_ENGINE_VERSION } from "../home";
import { keptList, listFollowers, listNow, type Kept } from "../lists/sessions";
import { MOST_ASKING, MOST_AT_WORK, PEER_AGENTS_FILE, readPeerAgents } from "../peers/directory";
import { addressOf } from "../crypto/keys";
import { ADDRESS, ownKey, peersOf } from "../peers/known";
import { actionsOf, givenName, MOST_NAME, oldAppAttached, refusal, removeComputerHere, renameHere, writerFacts, writerOf } from "../peers/manage";
import { offRelay } from "../letters/leave";
import { plainName } from "../../shared/text";
import { peerIsAway, PEER_REACH, readReach } from "../peers/reach";
import { askReachAgain, REACH_AGAIN, readTries, tryingSince } from "../peers/reach-again";
import { ownName } from "../letters/start";
import { forwardAct } from "../work-queue/forward";
import { sshActions } from "../add/flow";
import { KEEP_UP_FILE, keepingUp } from "../add/keep-up";
import { tryFollowers } from "../add/tries";
import { allowVouching, vouchBlockOf } from "../phones";
import { blockedWords } from "../peers/devices";
import { PRIMARY_UNLINK } from "../setup/leadership";
import { mainElsewhere } from "../setup/main-only";
import { followedSetup } from "../setup/record";
import { fromLocalApp } from "../server/context";
import { updateLink } from "../state";
import { macLink, readLink } from "../state";
import type { LinkState } from "../state";
import { boxVersion } from "../updates/update";
import type { AnswerPiece, RouteContext } from "../server/context";
import { agentsState } from "./agents-down";

export const COMPUTERS_ROUTE = /^kuvo\/computers(?:(\/events)|\/([A-Za-z0-9_-]{1,60})(\/connect|\/allow-devices)?)?$/;

/** The most sessions at work this box names of itself (it counts them all). */
const MOST_HERE = 100;

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

/** This box's sessions at work now, as its session list has them, newest first: conversations
 * (not a subagent's or a run kept out of sight), not archived, this box's own copy, and never one
 * that waits on you. */
export function atWorkHere(): AtWorkSession[] {
  return Object.entries(keptList()?.sessions ?? {})
    .filter(([, { facts }]) => facts.running && facts.asked === undefined && !facts.archived && !facts.away && facts.kind !== "subagent" && facts.kind !== "hidden")
    .sort(([one, a], [other, b]) => b.facts.updated - a.facts.updated || (one < other ? -1 : 1))
    .map(([id, { facts }]) => ({ id, ...(plainName(facts.title) ? { title: plainName(facts.title).slice(0, 200) } : {}) }));
}

/** This box's sessions waiting on you, as `list` has them, the latest asking first: conversations
 * (not a subagent's or a run kept out of sight), not archived, this box's own copy; each with what
 * it asks, in the box's words, where the list says. */
export function askingOf(list: Kept | null): AskingSession[] {
  const asks = new Map((list?.needsYou ?? []).map(each => [each.session, each.asks.summary]));
  return Object.entries(list?.sessions ?? {})
    .filter(([, { facts }]) => facts.asked !== undefined && !facts.archived && !facts.away && facts.kind !== "subagent" && facts.kind !== "hidden")
    .sort(([one, a], [other, b]) => b.facts.asked! - a.facts.asked! || (one < other ? -1 : 1))
    .map(([id, { facts }]) => {
      const title = plainName(facts.title).slice(0, 200);
      const said = asks.get(id);
      return { id, ...(title ? { title } : {}), ...(said ? { asks: said } : {}) };
    });
}

/** How many of this box's sessions are at work now (`atWorkHere`). */
export const workingHere = (): number => atWorkHere().length;

/** How its coding agents stand, as it tells its peers (`ToldComputer.agents`). */
function agentsFact(): NonNullable<ToldComputer["agents"]> {
  const state = agentsState();
  return state.state === "down" ? { state: "down", why: state.why } : { state: state.state };
}

/** What this box says of itself, here and to its peers (with its agents' mail): Kuvo Code's version
 * and its engine's, its system, how many of its sessions are at work and how many wait on you, and
 * the first few of each. */
export function ownComputer(): ToldComputer {
  system ??= { name: systemName() };
  const atWork = atWorkHere();
  const asking = askingOf(keptList());
  let engine: string | undefined;
  try { engine = readEngineFile()?.version || undefined; } catch {}
  const version = IS_MAC ? macLink.config?.app || MAC_APP_VERSION || undefined : boxVersion() || undefined;
  return {
    ...(version ? { version } : {}),
    ...(engine ?? (IS_MAC ? MAC_ENGINE_VERSION : null) ? { engine: (engine ?? MAC_ENGINE_VERSION)! } : {}),
    ...(system.name ? { system: system.name } : {}),
    working: atWork.length,
    atWork: atWork.slice(0, MOST_AT_WORK),
    waiting: asking.length,
    asking: asking.slice(0, MOST_ASKING),
    ...(oldAppAttached() ? { oldApp: true as const } : {}),
    ...(IS_MAC ? {} : { addresses: ownAddresses() }),
    // The Kuvo it runs, and the signed release it hands to your other computers that are behind.
    ...fleetFacts(),
    agents: agentsFact(),
  };
}

/** Where this machine's Macs may reach it over SSH besides the address they set it up at, as it
 * knows itself (`ToldComputer.addresses`): its tailnet's addresses (100.64.0.0/10) first, its name,
 * then its other networks' IPv4 addresses. Never a loopback or link-local one, nor a container's
 * or VM's bridge. */
export function ownAddresses(interfaces = os.networkInterfaces(), name = os.hostname()): string[] {
  // (Containers' and VMs' own bridges reach nothing from another computer: left out.)
  const virtual = /^(docker|br-|cni|veth|virbr|vmnet|vboxnet|flannel|lxc|lxd|podman|kube|cali|weave|tun|utun|bridge)/;
  const found = Object.entries(interfaces).filter(([name]) => !virtual.test(name)).flatMap(([, each]) => each ?? []).filter(each => each.family === "IPv4" && !each.internal && !each.address.startsWith("169.254.")).map(each => each.address);
  const tailnet = (address: string) => { const [a, b] = address.split(".").map(Number); return a === 100 && b! >= 64 && b! < 128; };
  const named = /^[A-Za-z0-9][A-Za-z0-9.-]{0,99}$/.test(name) && name !== "localhost" ? [name] : [];
  return [...new Set([...found.filter(tailnet), ...named, ...found.filter(address => !tailnet(address))])].slice(0, 8);
}

/** What this box answers, as its `kuvo/info` says. */
const ownFeatures = (): string[] => (IS_MAC ? macFeatures() : [...FEATURES]);

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

/** This box's own address on the relay, from its relay secret; none without one. */
function ownAddress(link: LinkState | null): string | undefined {
  try { return link?.secret ? addressOf(Buffer.from(link.secret, "base64url")).toString("base64url") : undefined; } catch { return undefined; }
}

/** Where a client reaches one of your own computers' boxes itself (`computers.reach`): its address
 * on the relay, and the relay this box reaches it through. Nothing while this box has no relay or
 * doesn't know the address. */
function reachOf(link: LinkState | null, address: string | undefined): { address?: string; relay?: string } {
  if (!link?.relay || !address || !ADDRESS.test(address)) return {};
  return { address, relay: link.relay };
}

/** Display words belong to the answering box; reach remains a machine-readable state. */
export function computerWords(computer: Pick<ListedComputer, "reach" | "name" | "working">, here: string): Pick<ListedComputer, "title" | "detail" | "notice" | "settingsNotice" | "workingTitle" | "reachTitles"> {
  const { reach, name, working } = computer;
  const title = ({ connected: "Connected", connecting: "Connecting", away: "Can’t be reached" })[reach];
  const notice = reach === "away" ? { title: `${name} can’t be reached`, detail: `${here} hasn’t heard from it for a while: it may be asleep, off, or offline. It shows here again by itself once it’s back, with what it runs.` }
    : reach === "connecting" ? { title: `Connecting to ${name}…`, detail: `${here} hasn’t heard from it yet.` } : undefined;
  // The same standing where its settings would show: what can't be read or changed, not what it runs.
  const settingsNotice = reach === "away" ? { title: `${name} can’t be reached`, detail: `Its settings are kept on ${name}, so they can’t be read or changed until ${here} hears from it again. They show here by themselves once it’s back.` }
    : reach === "connecting" ? { title: `Connecting to ${name}…`, detail: `Its settings show here once ${here} hears from it.` } : undefined;
  return { title, reachTitles: {
    connecting: "Connecting…", held: "Needs an update", away: "Can’t be reached",
    notPaired: "Hasn’t taken this app yet",
    unreached: "Can’t be reached from this app",
  }, ...(notice ? { detail: notice.detail, notice } : {}), ...(settingsNotice ? { settingsNotice } : {}), ...(working !== undefined ? { workingTitle: `${working} ${working === 1 ? "session" : "sessions"} at work` } : {}) };
}

/** How the peer `key` stands from here: `away` when it was found away or not heard from lately;
 * `reach` says it as `kuvo/computers` does: being reached (Connect asked a moment ago and not heard
 * from since, or never heard from since this box knew of it), away, or connected. */
export function peerReach(key: string, told = readPeerAgents(), reach = readReach(), tries = readTries(), now = Date.now()): { away: boolean; reach: "connected" | "connecting" | "away" } {
  const said = told[key];
  const away = peerIsAway(key, said?.heard, reach, now);
  // (Connect asked a moment ago, and not heard from since: being reached.)
  const trying = tryingSince(key, now, tries);
  const reaching = trying !== null && !(said?.heard !== undefined && said.heard >= trying);
  return { away, reach: reaching ? "connecting" : away ? "away" : said?.heard === undefined ? "connecting" : "connected" };
}

/** Your computers: this box first, then its peers by name, each
 * with what can be done with it from here. */
export async function listComputers(name: string, bridge: Target | null, signal?: AbortSignal, now = Date.now()): Promise<ListedComputer[]> {
  let link = null as ReturnType<typeof readLink>;
  try { link = readLink(); } catch {}
  const limits = await limitsRead(bridge, signal);
  const own = link ? ownKey(link) : null;
  const told = readPeerAgents();
  const tries = readTries();
  const byName = <T extends { name: string; key: string }>(boxes: T[]) => [...boxes].sort((a, b) => a.name.localeCompare(b.name) || a.key.localeCompare(b.key));
  const { version, kuvo, engine, system: runs, agents } = ownComputer();
  const atWork = atWorkHere();
  const computers: Omit<ListedComputer, "actions" | "limitServices" | "title" | "reachTitles">[] = [{
    key: own ?? "",
    name: ownComputerName(),
    kind: IS_MAC ? "mac" : "machine",
    here: true,
    reach: "connected",
    via: "here",
    ...reachOf(link, ownAddress(link)),
    ...(version ? { version } : {}),
    ...(kuvo ? { kuvo } : {}),
    ...(engine ? { engine } : {}),
    ...(runs ? { system: runs } : {}),
    ...(agents ? { agents } : {}),
    features: ownFeatures(),
    working: atWork.length,
    atWork: atWork.slice(0, MOST_HERE),
    limits: limits.get("here") ?? (own ? limits.get(own) : undefined) ?? [],
    // (Kuvo on this Mac ahead of its desktop app: the app checks for its update.)
    ...(IS_MAC && appBehind(boxVersion()) ? { appCheck: appBehind(boxVersion())! } : {}),
  }];
  const withActions = (): ListedComputer[] => {
    const here = computers[0]!.name;
    const facts = writerFacts(link);
    return computers.map(computer => {
      // (Brought up to date by this box, where it set it up and it's older: how that stands, and Update Now.)
      const keeping = keepingUp(computer, here, now);
      return { ...computer, ...computerWords(computer, here), limitServices: computer.limits.map(id => ({ id, title: serviceTitle(id) })), ...(keeping.update ? { update: keeping.update } : {}), actions: [...actionsOf(computer as ListedComputer, here, tryingSince(computer.key, now, tries) !== null, link, facts), ...keeping.actions, ...sshActions(computer as ListedComputer, here)] };
    });
  };
  const peers = peersOf(link);
  const reach = readReach();
  const leads = followedSetup(link)?.role === "primary";
  for (const peer of byName(peers)) {
    const said = told[peer.key];
    const { away, reach: standing } = peerReach(peer.key, told, reach, tries, now);
    const mine = said?.computer;
    // What it said it answers, whole (a list longer than peers keep can't say what's missing).
    const features = said?.features && !said.featuresCut ? said.features : undefined;
    // (Kept from vouching, as only the main computer knows: a device it added was removed.)
    const blocked = vouchBlockOf(link, peer.key);
    computers.push({
      key: peer.key,
      name: peer.name,
      kind: peer.kind,
      ...(peer.id ? { id: peer.id } : {}),
      // (Not heard from since this box knew of it, and not found away: still being reached.)
      reach: standing,
      via: "relay",
      // (Where a client of yours reaches it itself.)
      ...reachOf(link, peer.address),
      ...(mine ? { ...(mine.version ? { version: mine.version } : {}), ...(mine.kuvo ? { kuvo: mine.kuvo } : {}), ...(mine.engine ? { engine: mine.engine } : {}), ...(mine.system ? { system: mine.system } : {}), ...(mine.agents && !away ? { agents: mine.agents } : {}) } : {}),
      ...(features ? { features } : {}),
      // What's at work there, as it last said: nothing seen at work while it's away; nothing said
      // by one that never said it (an older box).
      ...(mine?.working !== undefined ? (away ? { working: 0, atWork: [] } : { working: mine.working, ...(mine.atWork ? { atWork: mine.atWork } : {}) }) : {}),
      limits: limits.get(peer.key) ?? [],
      ...(leads && blocked ? { vouching: { blocked: true as const, words: blockedWords(blocked.device, peer.name) } } : {}),
    });
  }
  return withActions();
}

const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };

/** What a client does with one of your computers (`PATCH`/`DELETE kuvo/computers/<key>`, `POST
 * …/connect`): done here, or handed to the box that writes its record, and answered with what that
 * one said, or why it can't be, in words a client shows as they are. */
async function* manage(context: RouteContext, key: string, action: string | undefined): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, info } = context;
  const method = request.method.toUpperCase();
  if (action === "/allow-devices") return yield* allowDevices(context, key);
  const connect = action === "/connect";
  const what = connect ? (method === "POST" ? "connect" : null) : method === "PATCH" ? "rename" : method === "DELETE" ? "remove" : null;
  if (!what) return yield* json(405, { message: connect ? "Only POST." : "Rename (PATCH) or remove (DELETE)." });
  if (what === "remove" && readLink()?.setup?.primary === key) return yield* json(409, { message: PRIMARY_UNLINK });
  const computers = await listComputers(info.name ?? "", null, signal ?? undefined);
  const computer = computers.find(each => each.key === key);
  if (!computer) {
    // A failed list write may outlive the peer in the directory. Only retry cleanup in this case.
    const writer = what === "remove" ? writerOf(readLink()) : null;
    if (!writer) return yield* json(404, { message: "That isn't one of your computers." });
    if (writer.here) {
      const done = await removeComputerHere(key, true);
      return yield* json(done.status, done.value);
    }
    const there = await forwardAct({ key: writer.key, name: writer.name, kind: "mac" }, "This computer", { area: "computers", item: key, method: "DELETE", body: { cleanup: true } }, signal);
    return yield* json(there.status, "refused" in there ? { message: there.refused } : isObject(there.value) ? there.value : {});
  }
  const here = computers[0]!.name;
  if (what === "connect") {
    if (computer.here) return yield* json(409, { message: `${here} is the computer you're asking.` });
    if (computer.via !== "relay") return yield* json(409, { message: `${here} doesn't reach ${computer.name} itself, so it can't connect to it.` });
    if (computer.reach === "connecting") return yield* json(409, { message: `${here} is reaching ${computer.name} now.` });
    if (computer.reach !== "away") return yield* json(409, { message: `${computer.name} is connected.` });
    if (offRelay()) return yield* json(503, { message: `${here} isn't connected to your other computers now, so it can't reach ${computer.name}. Try again in a moment.` });
    if (!askReachAgain(key)) return yield* json(503, { message: `${here} couldn't try now. Try again in a moment.` });
    return yield* json(202, {});
  }
  const name = what === "rename" ? givenName((parsed(request.body) as Body<"name">)?.name) : null;
  if (what === "rename" && !name) return yield* json(400, { message: `Give it a name of up to ${MOST_NAME} characters.` });
  const link = readLink();
  const why = refusal(computer, what, here, link);
  if (why) return yield* json(409, { message: why });
  const writer = writerOf(link)!;
  if (writer.here) {
    const done = what === "rename" ? renameHere(key, name!) : await removeComputerHere(key);
    return yield* json(done.status, done.value);
  }
  // (Handed to the box that writes it: it does it as its own client's, and says how it went.)
  const there = await forwardAct({ key: writer.key, name: writer.name, kind: "mac" }, `${computer.name}`, { area: "computers", item: key, ...(what === "rename" ? { method: "PATCH", body: { name } } : { method: "DELETE" }) }, signal);
  if ("refused" in there) return yield* json(there.status, { message: there.refused });
  return yield* json(there.status, isObject(there.value) ? there.value : {});
}

/** `POST kuvo/computers/<key>/allow-devices` (`devices.allow`): the computer `key`, kept from adding
 * Kuvo Code's app to your devices since one it added was removed, is let do it again. The main
 * computer's own, and only for its own clients: a device paired with it, over the relay, or Kuvo
 * Code's app beside it, over the box's own server. Never `box route`, which an agent's command
 * could run, never a device the computer `key` itself vouched for, and never handed on from another box (no act carries it, work-queue/forward.ts): the
 * computer that's kept out can't let itself back in. */
async function* allowDevices(context: RouteContext, key: string): AsyncGenerator<AnswerPiece> {
  const { request, phone, signal, json, info } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, { message: "Only POST." });
  const link = readLink();
  const asking = phone?.key?.toString("base64url");
  const device = asking === undefined ? undefined : (link?.phones ?? []).find(each => each.key === asking);
  // (Nor a device that same computer vouched for: whoever sits there would be allowing themselves.)
  if ((!device && !fromLocalApp(request)) || (device?.by !== undefined && device.by === key)) return yield* json(403, { message: "Only Kuvo Code on this computer, or one of your devices, allows a computer to add your devices again." });
  const computers = await listComputers(info.name ?? "", null, signal ?? undefined);
  const computer = computers.find(each => each.key === key);
  if (!computer) return yield* json(404, { message: "That isn't one of your computers." });
  const here = computers[0]!.name;
  // Only the main computer keeps a computer from it, so only it allows it again.
  const main = mainElsewhere(link);
  if (main) return yield* json(409, { message: `Allow it from ${main.name || peersOf(link).find(peer => peer.key === main.key)?.name || "your main computer"}.`, primary: main.key });
  if (followedSetup(link)?.role !== "primary") return yield* json(409, { message: `${here} isn't your setup's main computer, so no computer adds your devices through it.` });
  if (computer.here) return yield* json(409, { message: `${here} is the computer you're asking: it adds its own app itself.` });
  if (computer.via !== "relay") return yield* json(409, { message: `${computer.name} isn't one of ${here}'s own computers, so it adds none of your devices.` });
  // (Not kept from it: the same answer. What was removed stays removed: its app makes a new key.)
  updateLink(kept => (kept && followedSetup(kept)?.role === "primary" && allowVouching(kept, key) ? kept : null));
  return yield* json(200, {});
}

export async function* computersRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, info, bridge } = context;
  if (match[2]) return yield* manage(context, match[2], match[3]);
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  // (Its own sessions at work, from its list: read first where this process hasn't yet.)
  await listNow(context, signal ?? undefined).catch(() => null);
  const read = async () => ({ computers: await listComputers(info.name ?? "", bridge(), signal ?? undefined) });
  if (!match[1]) return yield* json(200, await read());
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // A line now, and one as it changes: its peers, what they told, which are away (their files),
  // and its own sessions starting and stopping work; and every 15 s for the rest (its limits).
  // (Of its list's changes, only which are at work: it changes with every step of a run.)
  const atWork = () => JSON.stringify(atWorkHere().slice(0, MOST_HERE));
  const listen = (told: () => void) => {
    let was = atWork();
    const listTold = () => {
      const now = atWork();
      if (now === was) return;
      was = now;
      told();
    };
    listFollowers.add(listTold);
    // (And what's under way with one over SSH, which changes what's offered for it.)
    tryFollowers.add(told);
    return () => { listFollowers.delete(listTold); tryFollowers.delete(told); };
  };
  return yield* followedLive(read, listen, signal,
    { look: () => `${mtime(PEER_AGENTS_FILE)} ${mtime(PEER_REACH)} ${mtime(LINK_FILE)} ${mtime(REACH_AGAIN)} ${mtime(KEEP_UP_FILE)} ${oldAppAttached()}` });
}
