// Renaming and removing your machines (PROTOCOL.md, "Computers", Managing them), from any box,
// with no client and no Mac's app open. A machine's record (its name, that it's one of your
// computers) is in the peers document of the Mac that set it up, which your other computers take
// it from: so one box writes it, as one numbers the Work Queue. That's this Mac's box for its own
// machines (a Mac that isn't a member of another's setup); every other box hands what you do to
// the computer that writes it (its setup's main computer, or the one Mac that set this machine
// up), by mail (`work-queue/forward.ts`), and answers with what it said, or why it can't.
// Connect is every box's own (`reach-again.ts`). What each computer offers is said on it
// (`actions` in `kuvo/computers`), with the words for its confirm, so no client works it out.

import fs from "node:fs";
import { plainName } from "../../shared/text";
import type { ComputerAction, ListedComputer } from "../../shared/routes/box";
import { macDocsChanged } from "../documents/exchange";
import { IS_MAC } from "../home";
import { tellGone } from "../mac/link";
import { LIST_FILE } from "../mac/machines";
import { setMacPeers } from "../mac/routes";
import { macName, mainElsewhere } from "../setup/main-only";
import { sharedMachine } from "../setup/settle";
import { macLink, readLink, updateLink } from "../state";
import { writePrivate } from "../files";
import { ownName } from "../letters/start";
import type { LinkState } from "../state";
import type { Act, Acted } from "../work-queue/forward";
import { isApart } from "./apart";
import { readPeerAgents } from "./directory";
import { macsOf, namedBoxes, peersOf } from "./known";
import { awaySince } from "./reach";
import type { MailSender, Peer } from "./known";

/** What a box's agents' mail says when it renames and removes the machines it writes for others. */
export const MANAGE_FEATURE = "computers.manage";

/** The longest name a computer is given. */
export const MOST_NAME = 100;

/** Who writes your machines' records, as this box knows: itself, the computer it hands what you do
 * to (by its key and name), or none (a machine Macs of different setups share, or no Mac's). */
export type Writer = { here: true } | { here: false; key: string; name: string } | null;

export function writerOf(link: LinkState | null | undefined): Writer {
  if (!link || sharedMachine(link)) return null;
  // (Named as its own record names it, else as this box lists it.)
  const named = (key: string, name: string, or: string) => name || peersOf(link).find(peer => peer.key === key)?.name || or;
  const main = mainElsewhere(link);
  if (main) return { here: false, key: main.key, name: named(main.key, main.name, "your main computer") };
  if (IS_MAC) return { here: true };
  const macs = macsOf(link);
  return macs.length === 1 ? { here: false, key: macs[0]!, name: named(macs[0]!, macName(link, macs[0]!), "the Mac that set it up") } : null;
}

/** The machines whose records the writer keeps, by key, as this box knows them: a Mac's own, or
 * those its Macs' documents name (this machine among them). */
function writtenMachines(link: LinkState | null | undefined, writer: Writer): Set<string> {
  if (!writer) return new Set();
  const boxes = writer.here ? peersOf(link) : namedBoxes(link);
  return new Set(boxes.filter(box => box.kind === "machine").map(box => box.key));
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
export const oldAppAttached = (now = Date.now()): boolean => IS_MAC && (macLink.statusFollowers.size > 0 || now - (macLink.appSaid ?? -Infinity) < OLD_APP_GRACE);

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
export function whyNot(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "shared" | "apart" | "via">, what: "rename" | "remove", here: string, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): string | null {
  const name = computer.name || "This computer";
  const { writer } = facts;
  // (The Mac that set up a machine shared with another setup keeps its own record of it.)
  const mine = writer?.here === true && Boolean(computer.key) && facts.machines.has(computer.key);
  if ((computer.shared || computer.apart) && !mine) return `${name} is shared with another setup, so it isn't yours to ${what} from here.`;
  if (computer.kind === "mac") return what === "rename" ? `${name} is a Mac: it goes by the name it has there.` : `A Mac can't be removed from here.`;
  if (what === "remove" && computer.here) return `${here} can't remove itself: remove it from another of your computers.`;
  if (!writer) return macsOf(readLink()).length > 1 || sharedMachine(link) ? `${name} was set up by more than one Mac, so no one of them keeps its record: ${what} it from the Mac that added it.` : `No Mac keeps ${name}'s record, so it can't be ${what === "rename" ? "renamed" : "removed"} from here.`;
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
export function refusal(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "shared" | "apart" | "via">, what: "rename" | "remove", here: string, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): string | null {
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
    actions.push({ id: "remove", label: "Remove", ...(waits ? { why: waits } : { confirm: { title: `Remove ${name}?`, message: `Your other computers stop reaching ${name}. It keeps running, with its sessions, and you can add it again later.`, label: "Remove" } }) });
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
export function renameHere(key: string, name: string): Acted {
  let found = false;
  updateLink(link => {
    if (!link || !peersOf(link).some(peer => peer.key === key)) return null;
    found = true;
    if (link.peerNames?.[key] === name) return null;
    link.peerNames = { ...(link.peerNames ?? {}), [key]: name };
    return link;
  });
  if (!found) return answer(404, { message: `That isn't one of ${ownName()}'s machines.` });
  macDocsChanged();
  return answer(200, { name });
}

/** Removes the machine `key` here, on the box that writes its record: its peers document names it
 * among those you removed (`left`), so it lets this Mac go and your other computers drop it, and
 * this Mac's list of machines no longer has it. It keeps running there, as a box of its own. */
export function removeHere(key: string): Acted {
  let gone: Peer[] = [];
  let id: string | undefined;
  let already = false;
  updateLink(link => {
    const peer = link ? peersOf(link).find(each => each.key === key) : undefined;
    // (Removed already, from here: the same again, as when it's asked again after a restart.)
    already = Boolean(link?.removedHere?.includes(key));
    if (!link || !peer) return null;
    id = peer.id;
    link.removedHere = [...(link.removedHere ?? []).filter(each => each !== key), key].slice(-64);
    const made = setMacPeers(link, { peers: (Array.isArray(link.macPeers) ? link.macPeers : []).filter(each => each?.key !== key), moved: link.macMoved, removed: [key] });
    if (!made.link) return null;
    gone = made.gone;
    if (made.link.peerNames) delete made.link.peerNames[key];
    return made.link;
  });
  if (!gone.length) return already ? answer(200, {}) : answer(404, { message: `That isn't one of ${ownName()}'s machines.` });
  if (id) forgetSaved(id);
  macDocsChanged();
  tellGone(gone);
  return answer(200, {});
}

// The machine `id` gone from this Mac's list of machines (`machines.json`), as its app would take
// it off: what `kuvo/machines` and whether the link meets machines go by. One that won't read stays.
function forgetSaved(id: string): void {
  try {
    const saved: unknown = JSON.parse(fs.readFileSync(LIST_FILE, "utf8"));
    if (!Array.isArray(saved) || !saved.some(each => each?.destination === id)) return;
    writePrivate(LIST_FILE, JSON.stringify(saved.filter(each => each?.destination !== id), null, 2));
  } catch {}
}

/** What another of your computers' box handed here (`area: "computers"`): done as this box's own
 * client's, where this box writes that machine's record. */
export async function writerAct(sender: MailSender, act: Act): Promise<Acted> {
  const link = readLink();
  const writer = writerOf(link);
  if (!writer?.here) return answer(409, { message: `${ownName()} doesn't keep your machines' records.` });
  if (isApart(sender.key, link)) return answer(409, { message: "Only your own computers can rename and remove your machines." });
  // (The same rules as the asking box's, again here: it may be older, or not say true.)
  const target = peersOf(link).find(peer => peer.key === act.item);
  if (target && isApart(target.key, link)) return answer(409, { message: `${target.name} is shared with another setup: only ${ownName()} itself can ${act.method === "DELETE" ? "remove" : "rename"} it.` });
  if (target && act.method === "DELETE" && oldAppAttached()) return answer(409, { message: oldAppWords(ownName(), target.name) });
  if (act.method === "PATCH") {
    const name = givenName(act.body?.name);
    if (!name) return answer(400, { message: `Give it a name of up to ${MOST_NAME} characters.` });
    return renameHere(act.item, name);
  }
  if (act.method === "DELETE") return removeHere(act.item);
  return answer(405, { message: "Rename or remove." });
}
