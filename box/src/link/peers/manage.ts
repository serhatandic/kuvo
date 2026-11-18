// Renaming and removing your machines (PROTOCOL.md, "Computers", Managing them), from any box,
// with no client and no Mac's app open. A machine's record (its name, that it's one of your
// computers) is in the peers document of the Mac that set it up, which your other computers take
// it from: so one box writes it, as one numbers the Work Queue. That's this Mac's box for its own
// machines (a Mac that isn't a member of another's setup); every other box hands what you do to
// the computer that writes it (its setup's main computer, or the one Mac that set this machine
// up), by mail (`work-queue/forward.ts`), and answers with what it said, or why it can't.
// Connect is every box's own (`reach-again.ts`). What each computer offers is said on it
// (`actions` in `kuvo/computers`), with the words for its confirm, so no client works it out.
// Letting a computer add Kuvo Code's app to your devices again (`allow-devices`) is the main
// computer's own, for its own clients: never handed on from another box (areas/computers.ts).

import { updateAccess } from "../setup/access";
import { PENDING_SAVE, stageShared, waitShared, type SharedSave } from "../setup/shared";
import { isObject } from "../../shared/json";
import { LEADERSHIP_WAIT, primaryFenced } from "../setup/lease";
import { plainName } from "../../shared/text";
import type { ComputerAction, ListedComputer } from "../../shared/routes/box";
import { ownDoc, primaryPeersBody } from "../documents/work";
import { macDocsChanged } from "../documents/exchange";
import { IS_MAC } from "../home";
import { movedFromOldApp } from "../rehome/moved";
import { tellGone } from "../mac/link";
import { LIST_FILE, localDestination, peerAtDestination, savedMachines } from "../mac/machines";
import { setMacPeers } from "../mac/routes";
import { standbyHere } from "../setup/handover";
import { leadershipOf, PRIMARY_UNLINK, STANDBY_UNLINK } from "../setup/leadership";
import { followedSetup, requiresStandby } from "../setup/record";
import { macName, mainElsewhere } from "../setup/main-only";
import { takeOffVouched, vouchedWords } from "../phones";
import { macLink, readLink, updateLink } from "../state";
import { writePrivate } from "../files";
import { ownName } from "../letters/start";
import type { LinkState } from "../state";
import { takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { readPeerAgents } from "./directory";
import { macsOf, namedBoxes, peersOf } from "./known";
import { awaySince } from "./reach";
import type { MailSender, Peer } from "./known";

/** What a box's agents' mail says when it renames and removes the machines it writes for others. */
export const MANAGE_FEATURE = "computers.manage";

/** The longest name a computer is given. */
export const MOST_NAME = 100;

/** Who writes your machines' records, as this box knows: itself, the computer it hands what you do
 * to (by its key and name), or none (no Mac's). */
export type Writer = { here: true } | { here: false; key: string; name: string } | null;

export function writerOf(link: LinkState | null | undefined): Writer {
  if (!link) return null;
  // (Named as its own record names it, else as this box lists it.)
  const named = (key: string, name: string, or: string) => name || peersOf(link).find(peer => peer.key === key)?.name || or;
  const main = mainElsewhere(link);
  if (main) return { here: false, key: main.key, name: named(main.key, main.name, "your main computer") };
  if (IS_MAC || followedSetup(link)?.role === "primary") return { here: true };
  const macs = macsOf(link);
  return macs.length === 1 ? { here: false, key: macs[0]!, name: named(macs[0]!, macName(link, macs[0]!), "the Mac that set it up") } : null;
}

/** The machines whose records the writer keeps, by key, as this box knows them: a Mac's own, or
 * those the writer's own document names (this machine among them; never another Mac's machines,
 * which that Mac keeps). */
function writtenMachines(link: LinkState | null | undefined, writer: Writer): Set<string> {
  if (!writer) return new Set();
  const boxes = writer.here ? peersOf(link) : namedBoxes(link, writer.key);
  return new Set(boxes.filter(box => box.kind === "machine" || link?.setup?.managed).map(box => box.key));
}

/** What this box knows of the writer, worked out once for a whole list: who it is, the machines
 * whose records it keeps, whether it takes what you do from here, and whether this box can hand it
 * on at all (a Mac's box reaches only its own machines, never another Mac). */
export interface WriterFacts {
  writer: Writer;
  machines: Set<string>;
  takes: boolean;
  reachable: boolean;
  /** The old Kuvo Code app (before 1.0) is attached to the writer's box: it keeps its own list of
   * machines and writes it back, so a removal there wouldn't hold. */
  oldApp: boolean;
}

/** How long after the old app last said its link's config (`kuvo/link/config`, the first thing it
 * does as it attaches) it counts as attached, with no follow yet. */
export const OLD_APP_GRACE = 60_000;

/** Whether the old Kuvo Code app (the Mac app before 1.0) is attached to this Mac's box now: it,
 * and only it, follows the box's link (`kuvo/link/status/events`) for as long as it's open, and
 * says its link's config first, as it attaches or attaches again. */
export const oldAppAttached = (now = Date.now()): boolean => IS_MAC && !movedFromOldApp() && (macLink.statusFollowers.size > 0 || now - (macLink.appSaid ?? -Infinity) < OLD_APP_GRACE);


/** Why a removal isn't taken while the old app is attached to the Mac `mac` (its name). */
export const oldAppWords = (mac: string, name: string): string => `Quit the old Kuvo Code app on ${mac} first: it keeps its own list of machines, and would add ${name} back.`;

export function writerFacts(link: LinkState | null = readLink()): WriterFacts {
  const writer = writerOf(link);
  const reachable = Boolean(writer && (writer.here || peersOf(link).some(peer => peer.key === writer.key)));
  const told = writer && !writer.here ? readPeerAgents()[writer.key] : undefined;
  return { writer, machines: writtenMachines(link, writer), takes: Boolean(writer && (writer.here || (told?.features ?? []).includes(MANAGE_FEATURE))), reachable,
    // (A writer that's away is said to be away: what it last said of its app may be long over.)
    oldApp: Boolean(writer && (writer.here ? oldAppAttached() : told?.computer?.oldApp === true && awaySince(writer.key) === null)) };
}

/** Why `computer` can't be renamed (`rename`) or removed (`remove`) from this box, in words a client
 * shows as they are; null when it can. `here`: this box's own name. */
export function whyNot(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "via">, what: "rename" | "remove", here: string, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): string | null {
  const name = computer.name || "This computer";
  const { writer } = facts;
  if (what === "remove" && link?.setup && computer.key === link.setup.primary) return PRIMARY_UNLINK;
  if (computer.kind === "mac" && (what === "rename" || !link?.setup?.managed)) return what === "rename" ? `${name} is a Mac: it goes by the name it has there.` : `A Mac can't be removed from here.`;
  if (what === "remove" && computer.here) return `${here} can't remove itself: remove it from another of your computers.`;
  if (!writer) return macsOf(link).length > 1 ? `${what === "rename" ? "Rename" : "Remove"} ${name} from the Mac that added it.` : `No Mac keeps ${name}'s record, so it can't be ${what === "rename" ? "renamed" : "removed"} from here.`;
  if (!facts.reachable) return `${here} can't reach ${(writer as { name: string }).name}, which keeps your machines' records: ${what} ${name} from there.`;
  if (!computer.key || !facts.machines.has(computer.key)) return `${name} isn't one of ${writer.here ? here : writer.name}’s machines, so it can't be ${what === "rename" ? "renamed" : "removed"} from here.`;
  if (!facts.takes) return `${(writer as { name: string }).name} needs a newer Kuvo Code to ${what} your machines. Update it, then try again.`;
  return null;
}

/** Why removing `computer` waits now, where it would be offered otherwise: the old app attached to
 * the writer's box. */
function waitsFor(computer: Pick<ListedComputer, "name">, here: string, facts: WriterFacts): string | null {
  if (!facts.oldApp || !facts.writer) return null;
  return oldAppWords(facts.writer.here ? here : facts.writer.name, computer.name || "it");
}

/** Why `computer` can't be removed (or renamed) from this box now, all of it: what `whyNot` says,
 * or that it waits for the old app to be quit. */
export function refusal(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "via">, what: "rename" | "remove", here: string, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): string | null {
  return whyNot(computer, what, here, link, facts) ?? (what === "remove" ? waitsFor(computer, here, facts) : null);
}

/** What can be done with `computer` from this box now, in the order to offer them. `trying`: it's
 * being reached again now (Connect was asked). */
export function actionsOf(computer: ListedComputer, here: string, trying: boolean, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): ComputerAction[] {
  const actions: ComputerAction[] = [];
  const name = computer.name || "this machine";
  if (!whyNot(computer, "rename", here, link, facts)) actions.push({ id: "rename", label: "Rename" });
  if (computer.via === "relay" && computer.reach === "away" && !trying) actions.push({ id: "connect", label: "Connect" });
  if (!whyNot(computer, "remove", here, link, facts)) {
    // (Offered, and while it waits for the old app to be quit, said why rather than left out.)
    const waits = waitsFor(computer, here, facts);
    actions.push({ id: "remove", label: "Remove", ...(waits ? { why: waits } : { confirm: { title: `Remove ${name}?`, message: `Your other computers stop reaching ${name}.${vouchedWords(link, computer.key, name)} It keeps running, with its sessions, and you can add it again later.`, label: "Remove" } }) });
  }
  // (Kept from adding Kuvo Code's app to your devices, as this box, the main computer, says of it:
  // `vouching`, areas/computers.ts.)
  if (computer.vouching?.blocked) {
    actions.push({ id: "allow-devices", label: "Allow Its App Again", confirm: { title: `Let ${name} add Kuvo Code's app again?`, message: `Kuvo Code on ${name} can add itself to your devices again, and reach every computer of yours. What you removed stays removed.`, label: "Allow" } });
  }
  return actions;
}

/** A name as given for a computer, cleaned: null for none, or one too long. */
export function givenName(name: unknown): string | null {
  const clean = plainName(name);
  return clean && clean.length <= MOST_NAME ? clean : null;
}

const answer = (status: number, value: unknown): Acted => ({ status, value });

/** Renames the machine `key` here, on the box that writes its record: its peers document names it so
 * from now on, and your other computers take it from there. */
export async function renameHere(key: string, name: string): Promise<Acted> {
  let found = false;
  let waitingForStandby = false;
  let save: SharedSave | null = null;
  const changed = updateLink(link => {
    if (!link || mainElsewhere(link) || !peersOf(link).some(peer => peer.key === key)) return null;
    found = true;
    if (requiresStandby(link.setup) && !link.setup?.standby) {
      waitingForStandby = true;
      return null;
    }
    if (link.setup?.standby || link.setup?.checkpointed) {
      const before = link.shared?.setupNames?.value;
      save = stageShared(link, "setupNames", { ...(isObject(before) ? before : link.peerNames), [key]: name });
      return save ? link : null;
    }
    if (link.peerNames?.[key] === name) return null;
    link.peerNames = { ...(link.peerNames ?? {}), [key]: name };
    return link;
  }, { durable: true });
  if (waitingForStandby) return answer(503, { message: LEADERSHIP_WAIT });
  if (!found) return answer(404, { message: `That isn't one of ${ownName()}'s machines.` });
  if (save && await waitShared(save) !== "saved") {
    return answer(202, { pending: true, message: PENDING_SAVE });
  }
  if (!changed && (readLink()?.setup?.standby || readLink()?.setup?.checkpointed)) return answer(409, { message: "A computer name is already waiting to be saved. Try again when it has saved." });
  macDocsChanged();
  return answer(200, { name });
}

/** Retire the old standby, install a replacement, then save the access change. */
export async function removeComputerHere(key: string, pendingOnly = false): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  if (!pendingOnly && setup?.role === "primary" && (setup.standby?.key === key
    || (!setup.standby && leadershipOf(link)?.body.retiredStandby === key))) {
    const candidates = peersOf(link).filter(peer => peer.key !== key && takesForwarded(peer.key, "setup.shared-saves"));
    if (!candidates.length) return answer(409, { message: STANDBY_UNLINK });
    let outcome: Acted = answer(409, { message: STANDBY_UNLINK });
    for (const candidate of candidates) {
      outcome = await standbyHere(candidate.key, { setup: setup.id, epoch: setup.epoch });
      const current = readLink();
      if (outcome.status === 202) break;
      if (current?.leadershipPending || current?.setup?.id !== setup.id || current.setup.epoch !== setup.epoch) return outcome;
    }
    if (outcome.status !== 202) return outcome;
  }
  return finishRemoval(prepareRemoval(key, pendingOnly));
}

/** Removes the machine `key` here, on the box that writes its record: its peers document names it
 * among those you removed (`left`), so it lets this Mac go and your other computers drop it, and
 * this Mac's list of machines no longer has it. Kuvo Code's app on it goes from your devices too:
 * every device its box vouched for (`setMacPeers`; `takeOffVouched`, phones.ts), as this box's
 * phones document then says to your other computers. It keeps running there, as a box of its own. */
// Legacy in-process callers can complete immediately. The routed removal waits for a receipt.
export function removeHere(key: string, pendingOnly = false): Acted {
  const removal = prepareRemoval(key, pendingOnly);
  return removal.save ? answer(202, { pending: true, message: PENDING_SAVE }) : completeRemoval(removal);
}

async function finishRemoval(removal: ReturnType<typeof prepareRemoval>): Promise<Acted> {
  const { save } = removal;
  if (save && await waitShared(save) !== "saved") {
    return answer(202, { pending: true, message: PENDING_SAVE });
  }
  return completeRemoval(removal);
}

function prepareRemoval(key: string, pendingOnly: boolean) {
  let blocked: string | null = null;
  let gone: Peer[] = [];
  let already = false;
  let needsCleanup = false;
  const result = updateAccess(link => {
    if (!link) return null;
    if (pendingOnly && primaryFenced(link)) { blocked = LEADERSHIP_WAIT; return null; }
    // A new removal needs a receipt, while cleanup on retry only changes the local address list.
    if (link.setup?.standby?.key === key) { blocked = STANDBY_UNLINK; return null; }
    if (link.setup?.primary === key || mainElsewhere(link)) {
      blocked = link.setup?.primary === key ? PRIMARY_UNLINK : "Only the primary can remove a computer.";
      return null;
    }
    const peer = peersOf(link).find(each => each.key === key);
    if (pendingOnly && (peer || !link.pendingMachineCleanup?.[key])) return null;
    already = Boolean(link.removedHere?.includes(key));
    const destination = peer && localDestination(link, key);
    needsCleanup = Boolean(destination || link.pendingMachineCleanup?.[key]);
    if (!peer) {
      // One removed before its devices went with it loses them when asked again.
      return !pendingOnly && already && takeOffVouched(link, key) ? link : null;
    }
    const made = setMacPeers(link, { peers: (Array.isArray(link.macPeers) ? link.macPeers : []).filter(each => each?.key !== key), moved: link.macMoved, removed: [key] }, undefined, Boolean(link.setup?.standby));
    if (!made.link) return null;
    gone = made.gone;
    if (made.link.peerNames) delete made.link.peerNames[key];
    if (made.link.setup?.managed) {
      // Keep the removal at this epoch for a member that misses it before standby rotation.
      // Build on a copy so the normal publisher still advances its own document counter.
      const roster = ownDoc({ ...made.link }, "peers", primaryPeersBody(made.link));
      if (roster) {
        made.link.untold = [...(made.link.untold ?? []).filter(each => each.key !== key), ...gone].slice(-64);
        made.link.untoldMembership = {
          ...Object.fromEntries(Object.entries(made.link.untoldMembership ?? {}).filter(([key]) => made.link!.untold!.some(peer => peer.key === key))),
          [key]: roster.envelope,
        };
      }
    }
    if (destination) made.link.pendingMachineCleanup = { ...made.link.pendingMachineCleanup, [key]: destination };
    return made.link;
  }, { localOnly: pendingOnly });
  return { ...result, blocked, gone, already, needsCleanup, key };
}

function completeRemoval({ changed, blocked, gone, already, needsCleanup, key, busy, save }: ReturnType<typeof prepareRemoval>): Acted {
  if (busy) return answer(409, { message: "An access change is already waiting to be saved." });
  if (blocked) return answer(409, { message: blocked });
  if (!gone.length && !already) return answer(404, { message: `That isn't one of ${ownName()}'s machines.` });
  macDocsChanged({ closeRemoved: save ? readLink() : changed });
  tellGone(gone);
  // Once the removal is durable, clean up its saved address. Leave a retry if the list fails.
  if (needsCleanup && !cleanRemovalList(key)) return answer(503, { message: "The computer’s access was removed, but the saved machines list couldn’t be updated." });
  return answer(200, {});
}

function cleanRemovalList(key: string): boolean {
  let saved = true;
  try {
    updateLink(link => {
      const pending = link?.pendingMachineCleanup;
      const destination = pending?.[key];
      if (!link || !pending || !destination) return null;
      // A fresh Add may have kept the destination again since revocation.
      saved = Boolean(peerAtDestination(link, destination)) || forgetSaved(destination);
      if (!saved) return null;
      delete pending[key];
      if (!Object.keys(pending).length) delete link.pendingMachineCleanup;
      return link;
    });
  } catch (error) {
    console.error("Removal machines-list cleanup failed:", error);
    return false;
  }
  return saved;
}

// The machine `id` gone from this Mac's list of machines (`machines.json`), as its app would take
// it off: what `kuvo/machines` and whether the link meets machines go by. One that won't read stays.
export function forgetSaved(id: string): boolean {
  const saved = savedMachines();
  if (!saved) return false;
  if (!saved.some(each => each.destination === id)) return true;
  try {
    writePrivate(LIST_FILE, JSON.stringify(saved.filter(each => each.destination !== id), null, 2));
    return true;
  } catch {
    return false;
  }
}

/** What another of your computers' box handed here (`area: "computers"`): done as this box's own
 * client's, where this box writes that machine's record. */
export async function writerAct(sender: MailSender, act: Act): Promise<Acted> {
  const link = readLink();
  const writer = writerOf(link);
  if (act.method === "DELETE" && act.item === link?.setup?.primary) return answer(409, { message: PRIMARY_UNLINK });
  if (!writer?.here) return answer(409, { message: `${ownName()} doesn't keep your machines' records.` });
  // (The same rules as the asking box's, again here: it may be older, or not say true.)
  const target = peersOf(link).find(peer => peer.key === act.item);
  if (target && act.method === "DELETE" && oldAppAttached()) return answer(409, { message: oldAppWords(ownName(), target.name) });
  if (act.method === "PATCH") {
    const name = givenName(act.body?.name);
    if (!name) return answer(400, { message: `Give it a name of up to ${MOST_NAME} characters.` });
    return renameHere(act.item, name);
  }
  if (act.method === "DELETE") return removeComputerHere(act.item, act.body?.cleanup === true);
  return answer(405, { message: "Rename or remove." });
}
