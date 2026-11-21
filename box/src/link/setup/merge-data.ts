import { relayLease } from "./lease";
// A single graduating primary keeps the records it owns on the same disk. Its old setup's
// portable values join the retained primary in the accepted enrollment operation.
import fs from "node:fs";
import path from "node:path";
import type { SignedDoc, Doc } from "../../shared/documents";
import { KUVO_FOLDER_LAYOUT, setupMergeLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { writeDurablePrivate } from "../files";
import { ownSign, readDoc, SIGN, signDoc } from "../documents/format";
import { fingerprint } from "../ownership/merge";
import { K, RUN } from "../home";
import { cleanMoved, KEY, MOST_PEERS, ownKey, peersOf, removedBoxes, type Peer } from "../peers/known";
import { authorityAddress } from "../ownership/resolve";
import { computerHistory, historyFor, rememberEnrollment } from "./computer-history";
import { ownPhones } from "../phones";
import { primaryPeersBody } from "../documents/work";
import type { LinkState } from "../state";
import { changeNumbers, installNumberAuthority, MOST_NUMBER, topOf } from "../work-queue/numbers";
import { references } from "../work-queue/relations";
import { markdownRanges } from "../work-queue/markdown";
import { changeQueues } from "../work-queue/store";
import { followMergeValues, mergePreferenceChoices, mergeSetupPreferences, type MergePreferenceChoices, type MergePreferences } from "./offline";
import { acceptedMembership, membershipOf, type Membership } from "./membership";
import { followedSetup, SETUP_ID, type SetupWord } from "./record";
import { localRemovalPending, pendingWipes } from "./removal-record";
import { ownershipEntries, sharedValue, type CompanionChanges } from "./shared";
import { takesForwarded } from "../work-queue/forward";
import { primaryFenced } from "./lease";
import { readPeerAgents } from "../peers/directory";
import { readerLayout, type ReaderLayouts } from "../updates/choice";

export const MERGE_FEATURE = "setup.merge";

export interface MergeJournal {
  source: string;
  epoch: number;
  target: string;
  primary: string;
  operation: string;
  authority?: SignedDoc;
  snapshot?: SignedDoc;
}

export interface MergePlan {
  operation: string;
  source: string;
  digest: string;
  sourceHigh: number;
  sourceName: string;
  offset: number;
}

export interface FinishedMerge {
  operation: string;
  setup: string;
  primary: string;
}

export class MergeRefusal extends Error {}

type MergeInput = {
  source: string;
  digest: string;
  high: number;
  name: string;
  membership: Membership;
  authority: Doc;
  preferences: MergePreferences;
  choices: MergePreferenceChoices;
  owners: Record<string, unknown>;
  earlier: Record<string, unknown>;
  readers: ReaderLayouts;
};

const sessionAddress = (id: string) => authorityAddress({ kind: "conversations", id });
const sessionPrefix = sessionAddress("");

export function mergeSourceProblem(link: LinkState | null): string | undefined {
  const setup = followedSetup(link);
  if (!setupMergeLayoutReady(K)) return "Wait for the Kuvo Code update to finish before merging setups.";
  if (!link || setup?.role !== "primary" || !link.key || !link.sign || !link.secret) return "Choose the main computer of the setup you’re bringing over.";
  if (!computersSupportMerge(link)) return "Update the computers in this setup before merging it.";
  if (!link.setupMerge && primaryFenced(link)) return "Wait for this setup’s main computer to reconnect to Kuvo’s relay before merging.";
  if (link.localWipe || link.localUnlink || link.removalSource || link.leadershipPending || localRemovalPending(link)
    || pendingWipes(link).length)
    return "Wait for this setup’s removals to finish before merging.";
  return undefined;
}

export function computersSupportMerge(link: LinkState): boolean {
  const joined = acceptedMembership(link)?.mergedComputers;
  return peersOf(link).every(peer => peer.sign && (joined?.[peer.key] || takesForwarded(peer.key, MERGE_FEATURE)));
}

export interface MergeAuthority {
  source: string;
  sourceEpoch: number;
  sourcePrimary: string;
  target: string;
  primary: string;
  signer: string;
  operation: string;
  computers: string[];
}

export function mergeAuthority(document: Doc | null | undefined): MergeAuthority | null {
  const value = document?.body.mergeAuthority;
  if (document?.kind !== "setup-shared" || !isObject(value) || typeof value.source !== "string" || !SETUP_ID.test(value.source)
    || typeof value.target !== "string" || !SETUP_ID.test(value.target) || value.source === value.target || !Number.isSafeInteger(value.sourceEpoch)
    || value.sourceEpoch !== document.n || typeof value.sourcePrimary !== "string" || !KEY.test(value.sourcePrimary)
    || typeof value.primary !== "string" || !KEY.test(value.primary) || typeof value.signer !== "string" || !SIGN.test(value.signer)
    || typeof value.operation !== "string" || !KEY.test(value.operation) || !Array.isArray(value.computers)
    || !value.computers.every(key => typeof key === "string" && KEY.test(key)) || !value.computers.includes(value.sourcePrimary)) return null;
  return value as unknown as MergeAuthority;
}

export function authorizeMerge(link: LinkState, intention: MergeJournal, signer: string): SignedDoc {
  const mergeAuthority = { source: intention.source, sourceEpoch: intention.epoch, sourcePrimary: link.setup!.primary,
    target: intention.target, primary: intention.primary, signer, operation: intention.operation,
    computers: primaryPeersBody(link).boxes.map(peer => peer.key) };
  return signDoc(link.sign!, "setup-shared", intention.epoch, { mergeAuthority });
}

/** Runs after recording the source fence. Wait for any number allocation already under way. */
export function sealMergeSource(link: LinkState, intention: MergeJournal): SignedDoc {
  const membership = acceptedMembership(link);
  const own = ownKey(link);
  const devices = ownPhones(link);
  const preferences = followMergeValues(link);
  const owners = ownershipEntries(link);
  const retiredSigns = [...new Set([...(membership?.computerHistory ?? computerHistory(link))?.retiredSigns ?? [],
    ...(link.setupRevocations?.setup === intention.source ? link.setupRevocations.signs : [])])];
  const high = changeQueues(() => changeNumbers(numbers => {
    installNumberAuthority(numbers, link);
    return Math.max(topOf(numbers), relayLease(link)?.high ?? 0);
  }));
  const roster = primaryPeersBody(link).boxes;
  const facts = readPeerAgents();
  const readers = Object.fromEntries(roster.flatMap(({ key }) => key
    ? [[key, key === own ? KUVO_FOLDER_LAYOUT : readerLayout(facts[key]?.computer?.layout)]]
    : []));
  const envelope = signDoc(link.sign!, "setup-shared", intention.epoch, {
    merge: { source: intention.source, target: intention.target, primary: intention.primary, operation: intention.operation, computer: own },
    high, devices, preferences, choices: mergePreferenceChoices(link), owners: isObject(owners) ? owners : {},
    authority: intention.authority, roster, readers, left: membership?.left ?? [], retiredSigns, earlier: sharedValue(link, "setupMergeAuthorities"),
    moved: membership?.moved ?? cleanMoved(link.macMoved),
  });
  return envelope;
}

export function mergeInput(doc: Doc, peer: Peer, into: SetupWord & { sign: string }, operation: string, recovery = false, replacement?: Doc | null): MergeInput | null {
  const { merge, high, devices, preferences, choices, owners, moved, roster, left, earlier, retiredSigns, readers } = doc.body;
  if (doc.kind !== "setup-shared" || doc.origin !== peer.sign || !isObject(merge) || typeof merge.source !== "string" || !SETUP_ID.test(merge.source)
    || merge.source === into.id || merge.target !== into.id || typeof merge.primary !== "string" || !KEY.test(merge.primary)
    || !recovery && merge.primary !== into.primary || merge.operation !== operation || merge.computer !== peer.key
    || typeof high !== "number" || !Number.isSafeInteger(high) || high < 0 || high >= MOST_NUMBER || !isObject(devices)
    || !isObject(preferences) || !isObject(owners)) return null;
  const authorityDoc = replacement ?? readDoc(doc.body.authority);
  const authority = mergeAuthority(authorityDoc);
  const membership = membershipOf({ boxes: roster, devices, moved, left,
    computerHistory: { setup: merge.source, slots: [], retiredSigns } });
  if (!membership || !authorityDoc || !authority || authorityDoc.origin !== peer.sign || authority.source !== merge.source || authority.target !== into.id
    || authority.operation !== operation || authority.sourcePrimary !== peer.key || authority.primary !== into.primary || authority.signer !== into.sign || authority.sourceEpoch !== doc.n
    || membership.boxes.length !== authority.computers.length || membership.boxes.some(entry => !authority.computers.includes(entry.key))
    || !membership.boxes.some(entry => entry.key === peer.key && entry.sign === peer.sign && entry.address === peer.address)) return null;
  const name = membership.boxes.find(entry => entry.key === peer.key)!.name;
  return { source: merge.source, digest: fingerprint(doc.body), high, name, membership, authority: authorityDoc, preferences, choices: isObject(choices) ? choices : {},
    owners, earlier: isObject(earlier) ? earlier : {},
    readers: Object.fromEntries(membership.boxes.map(peer => [peer.key, readerLayout(isObject(readers) ? readers[peer.key] : undefined)])) };
}

/** Reserve above both number ledgers in the same accepted save as membership. */
export function planMerge(link: LinkState, source: MergeInput, operation: string, reservation?: { from: number; to: number }) {
  if (source.high && (!reservation || reservation.to - reservation.from + 1 !== source.high))
    throw new MergeRefusal("This setup needs enough Work Queue numbers to bring the other setup over.");
  const offset = reservation ? reservation.from - 1 : 0;
  const plan: MergePlan = { operation, source: source.source, digest: source.digest, sourceHigh: source.high, sourceName: source.name, offset };
  const kept = ownershipEntries(link);
  const local = sharedValue(link, "ownershipRecovery");
  const sourceKeys = new Set(source.membership.boxes.map(peer => peer.key));
  const retainedMoves = new Set(cleanMoved(link.macMoved).map(({ session }) => sessionAddress(session)));
  const owned = Object.fromEntries(Object.entries(source.owners).filter(([address, value]) =>
    !Object.hasOwn(kept, address) && !retainedMoves.has(address) && isObject(value) && typeof value.computer === "string" && sourceKeys.has(value.computer)));
  const together: CompanionChanges = {
    ...mergeSetupPreferences(link, source.preferences, operation, source.choices),
    ...(Object.keys(owned).length ? { ownershipRecovery: { value: { ...owned, ...(isObject(local) ? local : {}) } } } : {}),
  };
  return { plan, together, source };
}

export function bindMergeAcceptance(link: LinkState, roster: Membership, saved: ReturnType<typeof planMerge>): CompanionChanges {
  const position = followedSetup(link)!;
  const sequence = (link.shared?.membershipAndAccess?.sequence ?? 0) + 1;
  const replica = { membershipAndAccess: { sequence, value: roster } };
  const accepted = { ...link, sharedSetup: position.id, shared: { ...link.shared, ...replica } };
  const ticket = {
    authority: saved.source.authority.envelope,
    roster: signDoc(link.sign!, "peers", (link.own?.peers?.n ?? 0) + 1, primaryPeersBody(accepted)),
    replica: signDoc(link.sign!, "setup-shared", position.epoch, { id: position.id, epoch: position.epoch, replica }),
  };
  const authorities = sharedValue(link, "setupMergeAuthorities");
  return { ...saved.together, setupMergeAuthorities: { value: { ...saved.source.earlier, ...(isObject(authorities) ? authorities : {}), [saved.source.source]: ticket } } };
}

export function bringMergeAccess(draft: LinkState, saved: ReturnType<typeof planMerge>) {
  const { source, plan } = saved;
  const roster = Array.isArray(draft.macPeers) ? draft.macPeers as Peer[] : [];
  const retired = removedBoxes(draft);
  const history = historyFor(draft, roster)!;
  const incomingRetiredSigns = source.membership.computerHistory?.retiredSigns ?? [];
  if ([ownSign(draft), ...roster.map(peer => peer.sign)].some(sign => sign && incomingRetiredSigns.includes(sign))) {
    throw new MergeRefusal("A computer in this setup uses a signing key retired by the joining setup.");
  }
  history.retiredSigns = [...new Set([...history.retiredSigns, ...incomingRetiredSigns])];
  const keys = new Set(source.membership.boxes.map(peer => peer.key));
  for (const incoming of source.membership.boxes) {
    const known = roster.find(peer => peer.key === incoming.key);
    if (retired.has(incoming.key) || incoming.sign && history.retiredSigns.includes(incoming.sign)
      || known && known.sign !== incoming.sign || incoming.key === draft.setup?.primary) {
      throw new MergeRefusal("A joining computer conflicts with an existing or removed membership.");
    }
    if (!known) {
      roster.push(incoming);
      rememberEnrollment(history, incoming.key);
    }
  }
  if (roster.length > MOST_PEERS) throw new MergeRefusal("The combined setup would have too many computers.");
  draft.macPeers = roster;
  draft.computerHistory = history;
  const activeComputers = new Set([draft.setup?.primary, ...roster.map(peer => peer.key)]);
  const incomingRetired = source.membership.left.filter(key => !activeComputers.has(key));
  draft.removedHere = [...new Set([...draft.removedHere ?? [], ...incomingRetired])];
  draft.left = [...new Set([...draft.left ?? [], ...incomingRetired])];
  const { devices, moved } = source.membership;
  const kept = ownPhones(draft);
  const removed = [...new Map([...kept.removed, ...devices.removed].map(entry => [JSON.stringify([entry.key, entry.pairing]), entry])).values()];
  const active = [...kept.phones, ...devices.phones].filter(phone => !removed.some(entry => entry.key === phone.key && entry.pairing === (phone.pairing ?? "")));
  draft.phones = [...new Map(active.reverse().map(phone => [phone.key, { ...phone, from: "pair" as const }])).values()];
  draft.removed = removed;
  // Keep the accepted ownership of an already-known session. The incoming computer's working
  // copy and all of its stable IDs stay where they were.
  const prior = cleanMoved(draft.macMoved);
  const hints = new Map([...moved.filter(entry => keys.has(entry.key)), ...prior].map(entry => [entry.session, entry.key]));
  const imported = saved.together.ownershipRecovery?.value;
  const assignments = { ...ownershipEntries(draft), ...(isObject(imported) ? imported : {}) };
  if (isObject(assignments)) for (const [address, assignment] of Object.entries(assignments)) {
    if (!address.startsWith(sessionPrefix) || !isObject(assignment) || typeof assignment.computer !== "string") continue;
    hints.set(address.slice(sessionPrefix.length), assignment.computer);
  }
  draft.macMoved = cleanMoved([...hints].map(([session, key]) => ({ session, key })));
  draft.mergedComputers = { ...draft.mergedComputers, ...Object.fromEntries(source.membership.boxes.map(peer => [peer.key, plan])) };
}

function sourceArchive(operation: string) {
  if (!KEY.test(operation)) throw new Error("Invalid merge operation.");
  return path.join(RUN, "setup-merges", operation + ".json");
}

/** The receipt authorizes the source to give up its namespace. Replaying after either file
 * write leaves the same items and ledger. The archived number blocks preserve their provenance. */
export function settleMergedNumbers(journal: MergeJournal, plan: MergePlan, link: LinkState, primary: string) {
  if (plan.source !== journal.source || plan.operation !== journal.operation
    || journal.snapshot && fingerprint(readDoc(journal.snapshot)?.body ?? {}) !== plan.digest) throw new Error("The merge receipt doesn’t match this setup’s saved data.");
  changeQueues(book => {
    changeNumbers(numbers => {
      const archive = sourceArchive(plan.operation);
      fs.mkdirSync(path.dirname(archive), { recursive: true, mode: 0o700 });
      if (!fs.existsSync(archive)) writeDurablePrivate(archive, JSON.stringify({
        authority: journal.authority, snapshot: journal.snapshot, numbers, plan, shared: link.shared, outcomes: link.sharedOutcomes, leadership: link.leadership,
      }), RUN);
      if (numbers.primary === primary) return;
      numbers.primary = primary;
      delete numbers.asking;
      delete numbers.given;
      delete numbers.granted;
      delete numbers.used;
      delete numbers.allocation;
      delete numbers.leadership;
      numbers.leases = [];
    });
    for (const item of book.items()) {
      if (item.lastNumberMerge === plan.operation) continue;
      item.title = rewriteMergeReferences(item.title, plan);
      item.body = rewriteMergeReferences(item.body, plan);
      item.notes = item.notes.map(note => ({ ...note, text: rewriteMergeReferences(note.text, plan) }));
      item.links = item.links.map(link => ({ ...link, url: rewriteMergeLink(link.url, plan),
        ...(link.title && (link.kind === "work-queue" || rewriteMergeLink(link.url, plan) !== link.url) ? { title: rewriteMergeReferences(link.title, plan) } : {}),
        ...(link.kind === "work-queue" && link.ref ? { ref: rewriteMergeReferences(link.ref, plan) } : {}) }));
      if (item.number !== undefined) {
        const previous = item.number;
        if (previous > plan.sourceHigh) throw new Error("A Work Queue number is beyond the setup’s saved merge range.");
        item.number += plan.offset;
        item.numberAliases = [...item.numberAliases ?? [], { setup: plan.source, name: plan.sourceName, number: previous }];
      }
      delete item.wanted;
      delete item.backGranted;
      item.lastNumberMerge = plan.operation;
    }
  }, undefined, { durable: true });
}

const WORK_QUEUE_NUMBER_LINK = /^((?:kuvo:\/\/|\/?kuvo\/|\/?)work-queue\/items\/)(%23|#)?([1-9][0-9]{0,5})(?=$|[?])/i;

export function rewriteMergeReferences(text: string, plan: Pick<MergePlan, "sourceHigh" | "offset">): string {
  const renumber = ({ start, end, number }: { start: number; end: number; number: number }, offset = 0) =>
    number <= plan.sourceHigh ? [{ start: offset + start, end: offset + end, value: "#" + (number + plan.offset) }] : [];
  const labelEdits = (start: number, end: number, destinationNumber: number) =>
    markdownRanges(text.slice(start, end), node => node.type === "text").flatMap(range => {
      const label = text.slice(start + range.start, start + range.end);
      const ordinary = new Set(references(label).map(ref => ref.start));
      return [...label.matchAll(/#([1-9][0-9]{0,5})(?![\p{L}\p{N}_])/gu)].flatMap(match => {
        const number = Number(match[1]);
        return number === destinationNumber || ordinary.has(match.index!)
          ? renumber({ start: match.index!, end: match.index! + match[0].length, number }, start + range.start)
          : [];
      });
    });
  const edits = references(text).flatMap(ref => renumber(ref));
  const definitions = new Map<string, number>();
  const links = markdownRanges(text, node => node.type === "link" || node.type === "definition" || node.type === "linkReference");
  for (const { node, start, end } of links) {
    if (node.type !== "link" && node.type !== "definition") continue;
    if (rewriteMergeLink(node.url, plan) === node.url) continue;
    const afterLabel = node.type === "link" ? node.children.at(-1)?.position?.end.offset : start;
    if (afterLabel === undefined) continue;
    const destination = text.slice(afterLabel, end).match(node.type === "link"
      ? /^(\]\(\s*<?)([^\s)>]+)/
      : /^(\[[^\n]+?\]:\s*<?)([^\s>]+)/);
    if (!destination) continue;
    const value = rewriteMergeLink(destination[2]!, plan);
    if (value === destination[2]) continue;
    const destinationNumber = Number(destination[2]!.match(WORK_QUEUE_NUMBER_LINK)?.[3]);
    const urlStart = afterLabel + destination[1]!.length;
    edits.push({ start: urlStart, end: urlStart + destination[2]!.length, value });
    if (node.type === "link") edits.push(...labelEdits(start + 1, afterLabel, destinationNumber));
    else definitions.set(node.identifier, destinationNumber);
  }
  for (const { node, start, end } of links) {
    if (node.type !== "linkReference") continue;
    const destinationNumber = definitions.get(node.identifier);
    if (destinationNumber === undefined) continue;
    const labelEnd = node.children.at(-1)?.position?.end.offset;
    if (labelEnd === undefined) continue;
    const label = labelEdits(start + 1, labelEnd, destinationNumber);
    if (!label.length) continue;
    edits.push(...label);
    // A short reference derives its identifier from the label. Keep that identifier explicitly.
    if (node.referenceType !== "full") {
      const suffix = node.referenceType === "collapsed" ? end - 2 : end;
      edits.push({ start: suffix, end, value: "[" + text.slice(start + 1, labelEnd) + "]" });
    }
  }
  for (const { start, end, value } of edits.sort((a, b) => b.start - a.start)) {
    text = text.slice(0, start) + value + text.slice(end);
  }
  return text;
}

function rewriteMergeLink(link: string, plan: Pick<MergePlan, "sourceHigh" | "offset">): string {
  return link.replace(WORK_QUEUE_NUMBER_LINK,
    (old, prefix: string, hash: string | undefined, number: string) => Number(number) <= plan.sourceHigh ? prefix + (hash ?? "") + (Number(number) + plan.offset) : old);
}

export function sameMerge(link: LinkState | null, journal: MergeJournal) {
  const setup = followedSetup(link);
  return !!link?.setupMerge && !link.localUnlink && !link.localWipe && !link.removalSource
    && link.setupMerge.operation === journal.operation && setup?.id === journal.source && setup.epoch === journal.epoch
    && !!ownKey(link) && (setup.role === "member" || ownKey(link) === setup.primary && ownSign(link) === setup.sign);
}
