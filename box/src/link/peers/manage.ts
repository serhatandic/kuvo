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
import { readLink, updateLink } from "../state";
import { writePrivate } from "../files";
import type { LinkState } from "../state";
import type { Act, Acted } from "../work-queue/forward";
import { isApart } from "./apart";
import { readPeerAgents } from "./directory";
import { macsOf, namedBoxes, peersOf } from "./known";
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

/** Whether the writer takes what you do from this box: itself, or one whose agents said it does. */
const writerTakes = (writer: Writer): boolean => Boolean(writer && (writer.here || (readPeerAgents()[writer.key]?.features ?? []).includes(MANAGE_FEATURE)));

/** Why `computer` can't be renamed (`rename`) or removed (`remove`) from this box, in words a client
 * shows as they are; null when it can. `here`: this box's own name. */
export function whyNot(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "shared" | "apart" | "via">, what: "rename" | "remove", here: string, link: LinkState | null = readLink()): string | null {
  const name = computer.name || "This computer";
  if (computer.shared || computer.apart) return `${name} is shared with another setup, so it isn't yours to ${what} from here.`;
  if (computer.kind === "mac") return what === "rename" ? `${name} is a Mac: it goes by the name it has there.` : `A Mac can't be removed from here.`;
  if (what === "remove" && computer.here) return `${here} can't remove itself: remove it from another of your computers.`;
  const writer = writerOf(link);
  if (!writer) return `${name} was set up by more than one Mac, so no one of them keeps its record: ${what} it from the Mac that added it.`;
  if (!computer.key || !writtenMachines(link, writer).has(computer.key)) return `${name} isn't one of ${writer.here ? here : writer.name}’s machines, so it can't be ${what === "rename" ? "renamed" : "removed"} from here.`;
  if (!writerTakes(writer)) return `${(writer as { name: string }).name} needs a newer Kuvo Code to ${what} your machines. Update it, then try again.`;
  return null;
}

/** What can be done with `computer` from this box now, in the order to offer them. `trying`: it's
 * being reached again now (Connect was asked). */
export function actionsOf(computer: ListedComputer, here: string, trying: boolean, link: LinkState | null = readLink()): ComputerAction[] {
  const actions: ComputerAction[] = [];
  const name = computer.name || "this machine";
  if (!whyNot(computer, "rename", here, link)) actions.push({ id: "rename", label: "Rename" });
  if (computer.via === "relay" && computer.reach === "away" && !trying) actions.push({ id: "connect", label: "Connect" });
  if (!whyNot(computer, "remove", here, link)) {
    actions.push({ id: "remove", label: "Remove", confirm: { title: `Remove ${name}?`, message: `Your other computers stop reaching ${name}. It keeps running, with its sessions, and you can add it again later.`, label: "Remove" } });
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
  if (!found) return answer(404, { message: "That isn't one of this Mac's machines." });
  macDocsChanged();
  return answer(200, { name });
}

/** Removes the machine `key` here, on the box that writes its record: its peers document names it
 * among those you removed (`left`), so it lets this Mac go and your other computers drop it, and
 * this Mac's list of machines no longer has it. It keeps running there, as a box of its own. */
export function removeHere(key: string): Acted {
  let gone: Peer[] = [];
  let id: string | undefined;
  updateLink(link => {
    const peer = link ? peersOf(link).find(each => each.key === key) : undefined;
    if (!link || !peer) return null;
    id = peer.id;
    const made = setMacPeers(link, { peers: (Array.isArray(link.macPeers) ? link.macPeers : []).filter(each => each?.key !== key), moved: link.macMoved, removed: [key] });
    if (!made.link) return null;
    gone = made.gone;
    if (made.link.peerNames) delete made.link.peerNames[key];
    return made.link;
  });
  if (!gone.length) return answer(404, { message: "That isn't one of this Mac's machines." });
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
  if (!writer?.here) return answer(409, { message: "This computer doesn't keep your machines' records." });
  if (isApart(sender.key, link)) return answer(409, { message: "Only your own computers can rename and remove your machines." });
  if (act.method === "PATCH") {
    const name = givenName(act.body?.name);
    if (!name) return answer(400, { message: `Give it a name of up to ${MOST_NAME} characters.` });
    return renameHere(act.item, name);
  }
  if (act.method === "DELETE") return removeHere(act.item);
  return answer(405, { message: "Rename or remove." });
}
