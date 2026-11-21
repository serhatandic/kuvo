import { forwardDecisionKey } from "../ownership/forward-evidence";
import { submitterAuthority } from "../ownership/authority";
import { forwardGrant } from "../work-queue/forward-grant";
import { clientTransportsLayoutReady, deviceGrantsLayoutDeferred } from "../../shared/folder-version";
import { ownComputerName } from "../computer-name";
import { serviceTitle } from "../../shared/account-names";
// Your computers, as this box knows them (PROTOCOL.md, "Computers", `kuvo/computers`): itself, its
// peers (the boxes its Macs name). What each is,
// whether it's heard from, where a client of your own reaches it (its address and relay), what it
// runs and answers, which of its sessions are at work, which
// services' plan limits it reads, and what can be done with it from here: renamed, removed
// (`peers/manage.ts`, handed to the box that writes its record) or reached again
// (`peers/reach-again.ts`); and, on your setup's main computer, which computer is kept from adding
// Every box answers alike, with no client
// there: each tells its peers its own facts with its agents (`peers/agents.ts`), so none is a hub.
// Platform: the system's name is read from macOS's own record of its version, or Linux's
// `os-release`; the rest is the same on both.

import fs from "node:fs";
import os from "node:os";
import { FEATURES } from "../../shared/features";
import { isObject, parsed, type Body } from "../../shared/json";
import type { ListedComputer } from "../../shared/routes/box";
import { macFeatures } from "../../shared/routes/mac";
import { computerCompatibility, editFeatures } from "./compatibility";
import { fleetFiles, fleetSummary, localFleetUpdate, peerFleetUpdate } from "../updates/progress";
import { followedSetup } from "../setup/record";
import { fleetFacts, fleetTarget } from "../updates/fleet";
import { appBehind } from "../mac/app";
import type { AskingSession, AtWorkSession, Target, ToldComputer } from "../../shared/records";
import { call, readEngineFile } from "../engine/client";
import { followedLive, watchFolders } from "../follow";
import { IS_MAC, K, LINK_FILE, RUN, MAC_APP_VERSION, MAC_ENGINE_VERSION } from "../home";
import { keptList, listFollowers, listNow, type Kept } from "../lists/sessions";
import { MOST_ASKING, MOST_AT_WORK, PEER_AGENTS_FILE, readPeerAgents } from "../peers/directory";
import { addressOf } from "../crypto/keys";
import { ADDRESS, ownKey, peersOf } from "../peers/known";
import { KEY } from "../peers/known";
import { WIPE_PREPARATION_FOLDER, pendingPreparations, planForComputer } from "../setup/before-wipe";
import { PREPARATION_FEATURE, REMOVAL_FEATURE, pendingWipes, removalsOf } from "../setup/removal-record";
import { takesForwarded } from "../work-queue/forward";
import { lonePrimary, readRemovalRequest, actionsOf, givenName, MOST_NAME, oldAppAttached, refusal, removeComputerHere, renameHere, writerFacts, writerOf } from "../peers/manage";
import { offRelay } from "../letters/leave";
import { plainName } from "../../shared/text";
import { peerIsAway, PEER_REACH, readReach } from "../peers/reach";
import { askReachAgain, REACH_AGAIN, readTries, tryingSince } from "../peers/reach-again";
import { ownName } from "../letters/start";
import { forwardAct, runForwardDecision } from "../work-queue/forward";
import { sshActions, sshAddressOf } from "../add/flow";
import { addressProblem } from "../add/address";
import { KEEP_UP_FILE, keepingUp } from "../add/keep-up";
import { tryFollowers } from "../add/tries";
import { updateAccess } from "../setup/access";
import { unlinkMain } from "../setup/handover";
import { mainElsewhere } from "../setup/main-only";
import { acceptedMembership, acceptedName } from "../setup/membership";
import { fromLocalApp } from "../server/context";
import { macLink, readLink } from "../state";
import type { LinkState } from "../state";
import { boxVersion } from "../updates/update";
import type { AnswerPiece, RouteContext } from "../server/context";
import { agentsState } from "./agents-down";
import { KUVO_FOLDER_LAYOUT } from "../../shared/folder-version";
import { pairedSubmitter } from "../ownership/authority";

export const COMPUTERS_ROUTE = /^kuvo\/computers(?:(\/events)|\/([A-Za-z0-9_-]{1,60})(\/connect|\/allow-devices|\/unlink|\/unlink-and-wipe|\/removal-plan)?)?$/;

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
export function ownComputer(target = fleetTarget(), now = Date.now()): ToldComputer {
  const facts = fleetFacts();
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
    ...(!IS_MAC || clientTransportsLayoutReady(K) ? { addresses: ownAddresses() } : {}),
    // The Kuvo it runs, and the signed release it hands to your other computers that are behind.
    ...facts,
    layout: KUVO_FOLDER_LAYOUT,
    ...(target ? { fleetUpdate: localFleetUpdate(target, facts) } : {}),
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
const ownFeatures = (): string[] => editFeatures(IS_MAC ? macFeatures() : FEATURES);

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
export async function listComputers(name: string, bridge: Target | null, signal?: AbortSignal, now = Date.now(), target = fleetTarget(now)): Promise<ListedComputer[]> {
  let link = null as ReturnType<typeof readLink>;
  try { link = readLink(); } catch {}
  const limits = await limitsRead(bridge, signal);
  const own = link ? ownKey(link) : null;
  const told = readPeerAgents();
  const tries = readTries();
  const byName = <T extends { name: string; key: string }>(boxes: T[]) => [...boxes].sort((a, b) => a.name.localeCompare(b.name) || a.key.localeCompare(b.key));
  const { version, kuvo, engine, system: runs, agents, addresses, fleetUpdate } = ownComputer(target, now);
  const direct = clientTransportsLayoutReady(K);
  const atWork = atWorkHere();
  const features = ownFeatures();
  const localName = acceptedMembership(link)?.boxes.find(box => box.key === own)?.name ?? ownComputerName();
  const computers: Omit<ListedComputer, "actions" | "limitServices" | "title" | "reachTitles">[] = [{
    key: own ?? "",
    name: link ? acceptedName(link, own, localName) : localName,
    kind: IS_MAC ? "mac" : "machine",
    here: true,
    reach: "connected",
    via: "here",
    ...reachOf(link, ownAddress(link)),
    ...(direct && addresses?.length ? { ssh: addresses } : {}),
    ...(version ? { version } : {}),
    ...(kuvo ? { kuvo } : {}),
    ...(engine ? { engine } : {}),
    ...(runs ? { system: runs } : {}),
    ...(agents ? { agents } : {}),
    ...(fleetUpdate ? { fleetUpdate } : {}),
    features,
    compatibility: computerCompatibility(features),
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
      const keeping = followedSetup(link) ? { actions: [] } : keepingUp(computer, here, now);
      const action = keeping.actions[0];
      const update = keeping.update && { ...keeping.update, ...(action ? {
        action: { ...action, ...(action.id === "add" && computer.id ? { address: computer.id } : {}) },
      } : {}) };
      return { ...computer, ...computerWords(computer, here), limitServices: computer.limits.map(id => ({ id, title: serviceTitle(id) })), ...(update ? { update } : {}), actions: [...actionsOf(computer as ListedComputer, here, tryingSince(computer.key, now, tries) !== null, link, facts), ...keeping.actions, ...sshActions(computer as ListedComputer, here)] };
    });
  };
  const peers = peersOf(link);
  const reach = readReach();
  for (const peer of byName(peers)) {
    const said = told[peer.key];
    const { away, reach: standing } = peerReach(peer.key, told, reach, tries, now);
    const mine = said?.computer;
    // What it said it answers, whole (a list longer than peers keep can't say what's missing).
    const features = said?.features && !said.featuresCut ? said.features : undefined;
    const saved = direct ? sshAddressOf(peer.key)?.destination : undefined;
    const ssh = direct ? [...new Set([...(saved ? [saved] : []), ...(mine?.addresses ?? [])])].filter(address => !addressProblem(address)).slice(0, 8) : [];
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
      ...(ssh.length ? { ssh } : {}),
      ...(mine ? { ...(mine.version ? { version: mine.version } : {}), ...(mine.kuvo ? { kuvo: mine.kuvo } : {}), ...(mine.engine ? { engine: mine.engine } : {}), ...(mine.system ? { system: mine.system } : {}), ...(mine.agents && !away ? { agents: mine.agents } : {}) } : {}),
      ...(features ? { features, compatibility: computerCompatibility(features) } : {}),
      ...(target ? { fleetUpdate: peerFleetUpdate(target, mine, away) } : {}),
      // What's at work there, as it last said: nothing seen at work while it's away; nothing said
      // by one that never said it (an older box).
      ...(mine?.working !== undefined ? (away ? { working: 0, atWork: [] } : { working: mine.working, ...(mine.atWork ? { atWork: mine.atWork } : {}) }) : {}),
      limits: limits.get(peer.key) ?? [],
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
  if (action === "/removal-plan") {
    if (method !== "POST") return yield* json(405, { message: "Only POST." });
    if (!fromLocalApp(request)) return yield* json(403, { message: "Open the removal plan from Kuvo Code." });
    const body = parsed(request.body);
    const plan = await planForComputer(key, isObject(body) ? body.projects : undefined, signal);
    return yield* json(plan.status, plan.value);
  }
  if (action === "/allow-devices") {
    if (deviceGrantsLayoutDeferred(K)) return yield* json(503, { message: "Wait for the Kuvo Code update to finish before changing device access." });
    return yield* json(404, { message: "Pair devices with your setup." });
  }
  const connect = action === "/connect";
  const explicit = action === "/unlink" || action === "/unlink-and-wipe";
  const what = explicit ? (method === "POST" ? "remove" : null) : connect ? (method === "POST" ? "connect" : null) : method === "PATCH" ? "rename" : method === "DELETE" ? "remove" : null;
  if (!what) return yield* json(405, { message: connect || explicit ? "Only POST." : "Rename (PATCH) or remove (DELETE)." });
  const before = readLink();
  const removal = what === "remove" ? readRemovalRequest(request.body?.length ? parsed(request.body) ?? false : undefined) : {};
  if (!removal) return yield* json(400, { message: "Send a valid removal request." });
  if (what === "remove" && !KEY.test(key)) return yield* json(404, { message: "That isn’t one of your computers." });
  if (explicit) removal.wipe = action === "/unlink-and-wipe";
  if ((removal.prepare || removal.retryPreparation || removal.resolve || removal.supersedes) && (!fromLocalApp(request) || action !== "/unlink-and-wipe")) {
    return yield* json(403, { message: "Prepare the wipe from Kuvo Code." });
  }
  if (what === "remove" && before?.setup?.primary === key && !lonePrimary(before, key)) {
    // Unlinking the main: hand its role over first (the computer picked, else an always-on one), and the new main unlinks it.
    if (removal.prepare || removal.retryPreparation || removal.resolve || removal.supersedes) return yield* json(409, { message: "Change the main computer before preparing its wipe." });
    const body = parsed(request.body);
    const picked = isObject(body) && typeof body.next === "string" && KEY.test(body.next) ? body.next : undefined;
    const done = await unlinkMain(key, removal.wipe === true, picked, signal, forwardGrant(context, before));
    return yield* json(done.status, done.value);
  }
  const removalWriter = what === "remove" ? writerOf(before) : null;
  const modern = explicit || removal.operation !== undefined || removal.wipe !== undefined;
  if (modern && removalWriter && !removalWriter.here && !takesForwarded(removalWriter.key, REMOVAL_FEATURE)) {
    return yield* json(409, { message: "Update the main computer before removing computers from this setup." });
  }
  if (removal.prepare && removalWriter && !removalWriter.here && !takesForwarded(removalWriter.key, PREPARATION_FEATURE)) {
    return yield* json(409, { message: "Update the main computer before preparing a wipe." });
  }
  const removalGrant = forwardGrant(context, before);
  // Polling a removal receipt has a new transport intent. Its signed domain operation is still
  // resolved by the removal adapter, including after the writer changes.
  const forwardRemoval = (writer: Extract<NonNullable<ReturnType<typeof writerOf>>, { here: false }>, cleanup = false) =>
    forwardAct({ key: writer.key, name: writer.name, kind: "mac" }, "This computer",
      { area: "computers", item: key, method: "DELETE", body: { ...removal, ...(cleanup ? { cleanup: true } : {}) } }, signal, before, Date.now(), { grant: (removal.retryPreparation || removal.resolve || removal.supersedes) && removalGrant ? { ...removalGrant, operation: undefined } : removalGrant });
  const computers = await listComputers(info.name ?? "", null, signal ?? undefined);
  const computer = computers.find(each => each.key === key);
  if (!computer) {
    // A failed list write may outlive the peer in the directory. Only retry cleanup in this case.
    const writer = removalWriter;
    if (!writer) return yield* json(404, { message: "That isn't one of your computers." });
    if (writer.here) {
      const done = await removeComputerHere(key, !(modern || removalsOf(before)[key]), removal);
      return yield* json(done.status, done.value);
    }
    const there = await forwardRemoval(writer, !(modern || removalsOf(before)[key]));
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
    const nameIntent = parsed(request.body);
    const stable = request.headers["x-kuvo-operation"] !== undefined || request.headers["idempotency-key"] !== undefined
      || (isObject(nameIntent) && nameIntent.operation !== undefined);
    const grant = what === "rename" && stable ? forwardGrant(context, link) : undefined;
    const owner = link && ownKey(link);
    if (grant === null) return yield* json(409, { message: "Use a current device grant and a valid operation id." });
    const admit = () => submitterAuthority(readLink(), grant?.submitter) === "allowed" ? null
      : { status: 403, value: { message: "The submitting device no longer has access." }, notAdmitted: true as const };
    const done = what === "remove" ? await removeComputerHere(key, false, removal)
      : owner && grant?.operation
        ? await runForwardDecision({ area: "computers", item: key, method: "PATCH", body: { name } },
          { ...grant, operation: grant.operation }, owner, { kind: "setupNames", id: key }, admit,
          () => renameHere(key, name!, admit, forwardDecisionKey(grant.setup, grant.operation!)))
          .catch(() => ({ status: 503, value: { message: "This computer couldn't confirm the name change. Check its result before retrying." } }))
        : await renameHere(key, name!);
    return yield* json(done.status, done.value);
  }
  // (Handed to the box that writes it: it does it as its own client's, and says how it went.)
  const there = what === "remove" ? await forwardRemoval(writer) : await forwardAct(
    { key: writer.key, name: writer.name, kind: "mac" }, computer.name, { area: "computers", item: key, method: "PATCH", body: { name } }, signal, link, Date.now(), { grant: forwardGrant(context, link) });
  if ("refused" in there) return yield* json(there.status, { message: there.refused });
  return yield* json(there.status, isObject(there.value) ? there.value : {});
}

export async function* computersRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json, info, bridge } = context;
  if (match[2]) return yield* manage(context, match[2], match[3]);
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  // (Its own sessions at work, from its list: read first where this process hasn't yet.)
  await listNow(context, signal ?? undefined).catch(() => null);
  const read = async () => {
    const target = fleetTarget();
    const computers = await listComputers(info.name ?? "", bridge(), signal, Date.now(), target);
    const pending = [...pendingWipes(readLink()), ...pendingPreparations()];
    const fleetUpdate = fleetSummary(computers, target);
    return { computers, ...(fleetUpdate ? { fleetUpdate } : {}), ...(pending.length ? { wipePending: pending } : {}) };
  };
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
    const watched = watchFolders([RUN], told, { recursive: false });
    return () => { listFollowers.delete(listTold); tryFollowers.delete(told); watched.close(); };
  };
  return yield* followedLive(read, listen, signal,
    { look: () => `${mtime(PEER_AGENTS_FILE)} ${mtime(PEER_REACH)} ${mtime(LINK_FILE)} ${mtime(WIPE_PREPARATION_FOLDER)} ${mtime(REACH_AGAIN)} ${mtime(KEEP_UP_FILE)} ${oldAppAttached()} ${fleetFiles().map(mtime).join(" ")}` });
}
