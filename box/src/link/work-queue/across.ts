import { numberDisk } from "../server/disk-worker";
import { supportsLegacyObjectVersion } from "../../shared/format-version";
import type { Act } from "./forward";
import { readEditForPeer, takeOfflineEdit, takeOfflineEditReceipt } from "../ownership/edits";
import { takeMessageReceipt, takeOwnerMessage } from "../ownership/messages";
// The Work Queue across your computers (PROTOCOL.md, "The Work Queue", "Across your computers"):
// one list for everything. Each box tells every other its items (summaries, the owners they name,
// its labels and how it numbers) as they change, unasked, so the All view and a lead's
// view on any computer have them, as of when they were told, while that one sleeps; each goes again
// each minute until that computer says it has it. Mail too big for one goes in parts. Numbers' asks
// and blocks, and items handed to another computer's conversations (`hand.ts`), come through here.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { IS_MAC } from "../home";
import { computerName } from "../peers/computer-name";
import { readPeerAgents } from "../peers/directory";
import { ownKey, peersOf, removedBoxes } from "../peers/known";
import { awaySince, readReach } from "../peers/reach";
import { usesAcceptedSaves } from "../setup/record";
import { sharedValue } from "../setup/shared";
import { readLink } from "../state";
import { labels as labelsHere, LABELS_FILE, setupLabels, takePrimaryLabels, type Label } from "./labels";
import { isClosed } from "./statuses";
import { stripLabel } from "./ops";
import { handArrived, handedBack, sendHandings } from "./hand";
import { queueToolForPeer } from "./tools";
import { actForPeer } from "./acts";
import { takeAct, takeScopedAct, takeActed } from "./forward";
import { deviceAct } from "../peers/devices";
import { setupAct } from "../setup/handover";
import { writerAct } from "../peers/manage";
import { mailsFor, sendMemoryMail, takePart } from "./parts";
import { takeSetupCopy } from "../setup/seed";
import { ownSince, primaryOf, takeNumbersMail, topOf } from "./numbers";
import { noteSetupNow } from "../setup/lead";
import { allItems, changeQueuesLater, OWNER, queueFollowers, refNames, WORK_QUEUE, type Item, type ItemRef } from "./store";
import { doneByRead, SEEN_FILE, seenOf, withSeen } from "./github";
import { forgetTold, keepTold, MOST_TOLD, MOST_TOLD_DONE, patchTold, readTold, TOLD, toldItemOf, toldKeys, toldOf, type Numbering, type ToldItem } from "./told";
import { locked, writePrivate } from "../files";
import { factsOf, ownersOf, summaryOf, visibleItem, type Computer, type OwnerFacts } from "./view";
import { rowsOf } from "../work/elsewhere";
import type { PeerPlaces } from "../engine/client";
import type { MailSender, Peer } from "../peers/known";
import type { LinkHost } from "../relay/host";
import type { LinkState } from "../state";
import type { Target } from "../../shared/records";

/** What a box's agents' mail says when it takes the Work Queue from its peers (`kuvo/info`'s
 * `features` too). */
export const QUEUES_FEATURE = "work-queue.elsewhere";

/** Whether the computer `key` takes the Work Queue's mail. */
export const takesQueues = (key: string): boolean => (readPeerAgents()[key]?.features ?? []).includes(QUEUES_FEATURE);

// ---- Telling ----

/** What this box tells its peers of its Work Queue now: its id (the same while it is), and what. */
interface Telling {
  id: string;
  payload: Record<string, unknown>;
}

/** How this box numbers, as it tells its peers: its kind, when it was set up, the highest number it
 * used or holds, and the primary it numbers from. */
export function numberingNow(link: LinkState | null): Numbering {
  const primary = primaryOf(link);
  const own = link ? ownKey(link) : null;
  const kind = IS_MAC ? "mac" : "machine";
  const since = ownSince();
  let named: Numbering["primary"];
  if ("self" in primary) named = own ? { key: own, kind, since } : undefined;
  else if ("peer" in primary) {
    const told = primary.via ? readTold(primary.via.key)?.primary?.since : readTold(primary.peer.key)?.since;
    named = { key: primary.peer.key, kind: primary.peer.kind, ...(told ? { since: told } : {}) };
  }
  return { kind, since, top: topOf(), ...(named ? { primary: named } : {}) };
}

/** An item as this box tells it: its summary, with who handed it on; an issue's with when this
 * box last asked GitHub of it (`seen.checked`), so another computer reads it only once that's old. */
const toldItem = (item: Item, working: Set<string>, titles: Map<string, { title: string }>): ToldItem => {
  const summary = summaryOf(item, working, titles);
  const from = [...new Set((item.handed ?? []).map(each => each.from))];
  const read = summary.seen ? seenOf(item.source) : undefined;
  const checked = read ? read.checked ?? read.at : undefined;
  // (A bot's look is this box's to draw with: another computer has its name.)
  if (summary.assignee?.look !== undefined) summary.assignee = { kind: summary.assignee.kind, id: summary.assignee.id, name: summary.assignee.name };
  return { ...summary, ...(item.mentionIndex?.targets.length ? { mentionTargets: item.mentionIndex.targets } : {}), ...(summary.lastNote ? { lastNote: { ...summary.lastNote, text: summary.lastNote.text.slice(0, 500) } } : {}), ...(from.length ? { handedFrom: from } : {}),
    ...(summary.seen && checked !== undefined ? { seen: { ...summary.seen, checked } } : {}) };
};

// (A box that numbers from another takes that one's labels, and changes none of them itself.)

/** What this box tells of its Work Queue now: its items (not deleted ones; the latest Done ones
 * only), retained deleted items, the owners they name, its labels, and how it numbers. */
export async function tellingNow(targets: Target[], link: LinkState | null, signal?: AbortSignal, cache?: Map<string, unknown>): Promise<Telling> {
  const facts = await factsOf(targets, signal);
  const own = allItems().filter(item => OWNER.test(item.owner));
  const all = own.filter(item => !item.removed);
  const removed = own.filter(item => item.removed).sort((a, b) => b.removed!.at - a.removed!.at).slice(0, MOST_TOLD);
  // (An issue closed on GitHub is Done here too: among the latest Done, not the open ones.)
  const isDone = (item: Item) => isClosed(item.status) || Boolean(item.source && doneByRead(seenOf(item.source)));
  // (When it was Done: an issue Done on GitHub, when it was closed there or, gone, since when, as
  // clients show it; it has no Done time of its own here.)
  const doneTime = (item: Item) => {
    const read = item.source ? seenOf(item.source) : undefined;
    return read?.value && doneByRead(read) ? read.value.closedAt ?? read.at : item.doneAt ?? 0;
  };
  const done = all.filter(isDone).sort((a, b) => doneTime(b) - doneTime(a)).slice(0, MOST_TOLD_DONE);
  const items = [...all.filter(item => !isDone(item)).sort((a, b) => b.updated.at - a.updated.at).slice(0, MOST_TOLD - done.length), ...done];
  const { owners, titles } = await ownersOf("all", [...items, ...removed], facts, targets, cache);
  const told = items.map(item => toldItem(item, facts.working, titles));
  const content = { items: told, ...(removed.length ? { removed: removed.map(item => toldItem(item, facts.working, titles)) } : {}),
    owners, labels: labelsHere(), ...numberingNow(link) };
  // (When GitHub was last asked of an issue goes with it, so the others don't ask again meanwhile,
  // but isn't a change: what's told goes again only once something else changes.)
  const unchecked = { ...content, items: told.map(item => (item.seen?.checked !== undefined ? { ...item, seen: { ...item.seen, checked: undefined } } : item)) };
  const id = "wqt_" + crypto.createHash("sha256").update(JSON.stringify(unchecked)).digest("base64url").slice(0, 22);
  return { id, payload: { told: { id, at: Date.now(), ...content } } };
}

// What this box last told each computer: the id, whether it said it has it, and when it last went.
const pushed = new Map<string, { id: string; done: boolean; sent: number; soon?: boolean }>();
let telling: Telling | null = null;
let builtAt = -Infinity;
let builtMarks = "";
let building = false;
/** The least time between two tellings to one computer, however busy. */
export const TELL_SPACING = 15_000;
const marks = () => [path.join(WORK_QUEUE, "queues"), LABELS_FILE, path.join(WORK_QUEUE, "numbers.json"), TOLD, SEEN_FILE].map(file => {
  try { return String(fs.statSync(file).mtimeMs); } catch { return "-"; }
}).join(" ");
// Titles of sessions the list doesn't have, looked up once in ten minutes at most.
let titles = new Map<string, unknown>();
let titlesAt = 0;

let labelRevision = "";
async function reconcileSetupLabels(link: LinkState) {
  const selected = setupLabels(sharedValue(link, "queueLabels"));
  if (!selected) return;
  const revision = `${link.sharedSetup}:${link.shared?.queueLabels?.sequence}`;
  if (labelRevision === revision) return;
  const removed = new Set(selected.removed);
  if (removed.size) await changeQueuesLater(book => {
    for (const item of book.items()) if (item.labels.some(id => removed.has(id))) {
      item.labels = item.labels.filter(id => !removed.has(id));
    }
  }, undefined, { onItsOwn: true });
  labelRevision = revision;
  telling = null;
  for (const tell of queueFollowers) tell();
}

/** This box's Work Queue told to each computer that takes it, on its rounds: worked out again when
 * a queue changes, and each minute (who's at work changes it too); to one computer once in 15
 * seconds at most, again each minute until it says it has it, and nothing while it's the same and
 * had; nothing while that computer isn't answering. The items it's handing to another computer's
 * conversations go first (`hand.ts`), whatever's told. */
export async function pushQueues(host: Pick<LinkHost, "send">, link: LinkState | null = readLink(), targets: Target[], now = Date.now()): Promise<void> {
  if (!link?.key) return;
  await reconcileSetupLabels(link);
  const peers = peersOf(link);
  // What's kept of computers you removed goes.
  const removed = removedBoxes(link);
  for (const key of toldKeys()) if (removed.has(key)) forgetTold(key);
  await sendHandings(host, link, targets, now);
  // (Nothing to work out while no computer takes it.)
  if (building || !peers.some(peer => takesQueues(peer.key))) return;
  const changed = marks();
  if (!telling || changed !== builtMarks || now - builtAt > 60_000) {
    building = true;
    try {
      if (now - titlesAt > 600_000) {
        titles = new Map();
        titlesAt = now;
      }
      telling = await tellingNow(targets, link, undefined, titles);
      builtAt = now;
      builtMarks = changed;
    } catch {
      // (Its engine can't say now: as it was, until it can.)
      return;
    } finally {
      building = false;
    }
  }
  for (const peer of peers) {
    if (!takesQueues(peer.key) || awaySince(peer.key) !== null) continue;
    const before = pushed.get(peer.key);
    // (Had: nothing while it's the same. Never two to one computer within 15 seconds; the same one
    // again after a minute, or sooner when this box's queue was changed from a client.)
    if (before?.id === telling.id && before.done) continue;
    if (before && now - before.sent < TELL_SPACING) continue;
    if (before?.id === telling.id && !before.soon && now - before.sent < 60_000) continue;
    const mails = fittedMails(peer, telling, link, now);
    if (!mails) continue;
    try {
      for (const bytes of mails) host.send(bytes);
      pushed.set(peer.key, { id: telling.id, done: false, sent: now });
    } catch {}
  }
}

// What's told, as mails for `peer`: whole, or, too big even in parts, with its items cut (Done ones
// and the longest unchanged first: they're last in the list), said in the box's log.
function fittedMails(peer: Peer, telling: Telling, link: LinkState, now: number): Buffer[] | null {
  const told = telling.payload.told as { items: unknown[]; removed?: unknown[] } & Record<string, unknown>;
  const full = mailsFor(peer, telling.payload, link, now);
  if (full) return full;
  // Keep active items before deleted summaries if the combined snapshot exceeds mail limits.
  const { removed, ...active } = told;
  if (removed?.length) for (const most of [500, 100, 20, 0]) {
    if (most >= removed.length) continue;
    const mails = mailsFor(peer, { told: { ...active, ...(most ? { removed: removed.slice(0, most) } : {}) } }, link, now);
    if (mails) {
      console.error(`Work Queue: ${removed.length} deleted items were too many to tell ${computerName(peer)} alongside its active items. Told ${most}.`);
      return mails;
    }
  }
  for (const most of [1000, 500, 200, 50]) {
    if (most >= active.items.length) continue;
    const mails = mailsFor(peer, { told: { ...active, items: active.items.slice(0, most) } }, link, now);
    if (mails) {
      if (most < told.items.length) console.error(`Work Queue: ${told.items.length} items were too many to tell ${computerName(peer)} at once: told the ${most} latest.`);
      return mails;
    }
  }
  console.error(`Work Queue: what this box tells ${computerName(peer)} is too big to send, even cut to its 50 latest items.`);
  return null;
}

/** A computer answers again: what's changed goes at the next round. */
export function queuesBack(key: string): void {
  const before = pushed.get(key);
  if (before && !before.done) pushed.set(key, { ...before, sent: 0 });
}

/** Told again: to `key` from the start (a box that asks for agents: it may have missed it); to every
 * computer, what it hasn't said it has, at the next round rather than after a minute (a change from
 * a client), never sooner than 15 seconds after the last. */
export function tellQueuesAgain(key?: string): void {
  if (key) return void pushed.delete(key);
  for (const [each, before] of pushed) if (!before.done) pushed.set(each, { ...before, soon: true });
}

/** All that was told forgotten (a test's). */
export function forgetPushed(): void {
  pushed.clear();
}

/** What a computer told of its Work Queue, kept (unless what's kept is newer) and said to be had;
 * the primary moves its counter past the numbers it uses; a box takes the labels of the
 * primary it numbers from (from the peer it reaches it through, one that numbers from it too). */
async function takeTold(sender: MailSender, given: unknown, reply: (payload: Record<string, unknown>) => void): Promise<void> {
  const told = toldOf(given);
  if (!told) return;
  const link = readLink();
  // (Only from one of your computers that's still a peer: a Mac that set this one up is one.)
  if (!peersOf(link).some(peer => peer.key === sender.key)) return;
  // (What came late, older than what's kept, isn't had: had, it would never go again.)
  const before = readTold(sender.key);
  if (!keepTold(sender.key, told)) return;
  forgetDeletedThere(new Set(told.items.map(item => item.id)));
  reply({ had: told.id });
  // How it numbers changed: look at this box's setup now.
  if (before?.primary?.key !== told.primary?.key || before?.primary?.since !== told.primary?.since) noteSetupNow(Date.now(), { soon: 0 });
  const primary = primaryOf(link);
  if ("self" in primary) await numberDisk.run("movePast", Math.max(told.top ?? 0, ...told.items.map(item => item.number ?? 0)));
  else if (!usesAcceptedSaves(link?.setup) && "peer" in primary && (primary.via ? primary.via.key === sender.key && told.primary?.key === primary.peer.key : primary.peer.key === sender.key)) {
    // (Read only when a label it no longer has is one this box might keep.)
    let counts: Map<string, number> | null = null;
    takePrimaryLabels(told.labels, id => {
      if (!counts) {
        counts = new Map();
        for (const item of allItems()) for (const label of item.labels) counts.set(label, (counts.get(label) ?? 0) + 1);
      }
      return counts.get(id) ?? 0;
    }, stripLabel, {
      key: primary.peer.key, at: told.at,
      previousAt: before?.latestAt ?? before?.at,
      complete: isObject(given) && Array.isArray(given.labels) && given.labels.length === told.labels.length,
    });
  }
  for (const tell of queueFollowers) tell();
}

/** The Work Queue's mail from another computer (`{workQueue: …}`): a part of one too big for one
 * mail, what it tells of its queue and that it has this box's, numbers' asks and blocks, items
 * handed between them, and what a client did to an item one keeps, handed on by the box it asked
 * (`forward.ts`), and its answer. */
export async function takeWorkQueueMail(sender: MailSender, given: unknown, host: Pick<LinkHost, "send"> | null | undefined, places: () => PeerPlaces): Promise<void> {
  if (!isObject(given)) return;
  const memory = given.memory === true;
  const whole = isObject(given.part) ? takePart(sender, given.part, memory) : given;
  if (!whole || !supportsLegacyObjectVersion(whole)) return;
  if (memory && (Object.keys(whole).some(key => !["act", "acted", "memory"].includes(key))
    || (whole.act !== undefined && (!isObject(whole.act) || whole.act.method !== "GET")))) return;
  const reply = (payload: Record<string, unknown>) => {
    const link = readLink();
    const asked = isObject(whole.scopedAct) ? whole.scopedAct.act : whole.act;
    const ownRemoval = isObject(asked) && asked.area === "computers" && asked.method === "DELETE"
      && asked.item === sender.key && Object.hasOwn(payload, "acted");
    const to = peersOf(link).find(peer => peer.key === sender.key) ?? (ownRemoval && "address" in sender ? sender : undefined);
    const mails = link && to ? mailsFor(to, payload, link, Date.now(), memory) : null;
    if (mails && host) try {
      if (memory && to?.address) sendMemoryMail(host, to.address, mails);
      else for (const bytes of mails) host.send(bytes);
    } catch {}
  };
  if (isObject(whole.ask) || isObject(whole.give) || isObject(whole.numberProposal) || isObject(whole.numberVote)) await takeNumbersMail(sender, whole, host);
  if (isObject(whole.told)) await takeTold(sender, whole.told, reply);
  if (isObject(whole.setupCopy)) await takeSetupCopy(sender.key, whole.setupCopy);
  if (typeof whole.had === "string") {
    const before = pushed.get(sender.key);
    if (before?.id === whole.had) pushed.set(sender.key, { ...before, done: true });
  }
  if (isObject(whole.handed)) await handedBack(sender, whole.handed);
  if (Object.hasOwn(whole, "evidenceHand")) {
    if (isObject(whole.evidenceHand) && !Object.hasOwn(whole, "hand"))
      await handArrived(sender, whole.evidenceHand, reply, places, undefined, true).catch(() => {});
  } else if (isObject(whole.hand)) await handArrived(sender, whole.hand, reply, places).catch(() => {});
  if (isObject(whole.offlineEditReceipt)) await takeOfflineEditReceipt(sender, whole.offlineEditReceipt);
  if (Object.hasOwn(whole, "scopedOfflineEdit")) {
    if (isObject(whole.scopedOfflineEdit) && !Object.hasOwn(whole, "offlineEdit"))
      await takeOfflineEdit(sender, whole.scopedOfflineEdit, reply, places(), host, true).catch(error => console.error("Offline edit:", error));
  } else if (isObject(whole.offlineEdit)) await takeOfflineEdit(sender, whole.offlineEdit, reply, places(), host).catch(error => console.error("Offline edit:", error));
  if (isObject(whole.ownerMessageReceipt)) await takeMessageReceipt(sender, whole.ownerMessageReceipt);
  if (isObject(whole.ownerDraftReceipt)) await takeMessageReceipt(sender, whole.ownerDraftReceipt, true);
  if (isObject(whole.submissionReceipt)) await takeMessageReceipt(sender, whole.submissionReceipt, true, true);
  const messages = { scopedSubmission: "submission", scopedOwnerDraft: "draft", scopedOwnerMessage: "scoped", ownerMessage: "legacy" } as const;
  const lanes = (Object.keys(messages) as (keyof typeof messages)[]).filter(key => Object.hasOwn(whole, key));
  if (lanes.length === 1) {
    const key = lanes[0]!;
    const message = whole[key];
    if (isObject(message)) await takeOwnerMessage(sender, message, reply, host, messages[key]);
  }
  if (isObject(whole.acted)) takeActed(sender, whole.acted, memory);
  const run = async (act: Act) => {
    switch (act.area) {
      case "ownership": {
        if (act.method === "GET" && act.item === "backup-restore") return (await import("../backup/restore")).restoreForPeer(sender, act);
        if (act.method === "GET" && act.item === "offline-edit") return readEditForPeer(sender, act, places());
        if (act.method === "GET" && act.item === "draft-submissions") return (await import("../ownership/submissions")).readSubmissionForPeer(sender, act, host);
        if (act.method === "GET") return (await import("../ownership/read")).readForPeer(sender, act, places());
        return (await import("../ownership/write")).writeForPeer(sender, act, places());
      }
      case "addComputer": return (await import("../areas/computers-add")).addComputerAct(sender, act);
      case "computers": return writerAct(sender, act);
      case "devices": return deviceAct(sender, act);
      case "setup": return setupAct(sender, act);
      case "queue-agent": return queueToolForPeer(act, places().targets);
      case "marketplace": {
        const { marketplaceForPeer } = await import("../areas/marketplace");
        const { readEngineFile, bridgeTarget } = await import("../engine/client");
        return marketplaceForPeer(act, { readEngine: readEngineFile, bridge: bridgeTarget });
      }
      case "library": return (await import("../areas/library")).libraryForPeer(act);
      default: return actForPeer(act, places().targets);
    }
  };
  if (Object.hasOwn(whole, "scopedAct")) {
    if (isObject(whole.scopedAct) && !Object.hasOwn(whole, "act"))
      await takeScopedAct(sender, whole.scopedAct, reply, run, host).catch(() => {});
  } else if (isObject(whole.act)) await takeAct(sender, whole.act, reply, run, undefined, {
    // Re-authorize every chunk and let the store resolve replay against its own durable state.
    configApply: async act => (await import("../areas/apply-from")).configApplyAct(sender, act),
    backup: async act => (await import("../backup/relay-store")).relayBackupAct(sender, act),
  }).catch(() => {});
}


// ---- Reading what others told ----

/** An item another computer told of, with which one. */
export type ItemElsewhere = ToldItem & { computer: Computer };

function shadowedHere(item: ToldItem, live: ReadonlySet<string>, removed: ReadonlySet<string>): boolean {
  return live.has(item.id) || (removed.has(item.id) && Boolean(item.handing));
}

/** Your other computers' items in the named view (`all`: every queue), as last told.
 * Local live copies take precedence. A deleted local copy also takes precedence over a peer's
 * stale copy on its way here. */
export function itemsElsewhere(owner: string, link: LinkState | null = readLink(), here?: Set<string>, deleted?: Set<string>, { asTold = false }: { asTold?: boolean } = {}): {
  items: ItemElsewhere[];
  removed: ItemElsewhere[];
  owners: OwnerFacts[];
  labels: Label[];
  computers: (Computer & { at: number; away?: number })[];
} {
  if (!here || !deleted) {
    const all = allItems();
    here ??= new Set(all.filter(item => !item.removed).map(item => item.id));
    deleted ??= new Set(all.filter(item => item.removed).map(item => item.id));
  }
  const overseen = owner === "all" ? null : new Set(rowsOf(owner).flatMap(each => each.rows.map(row => row.session)));
  const owners = new Map<string, OwnerFacts>();
  const custom = new Map<string, Label>();
  const removedLabels = new Set(setupLabels(sharedValue(link, "queueLabels"))?.removed);
  const computers: (Computer & { at: number; away?: number })[] = [];
  // (An item two computers told of, one handed between them and the first not told again yet: the
  // newer word, so a stale copy never hides the live one.)
  const best = new Map<string, { item: ItemElsewhere; at: number }>();
  const reach = readReach();
  for (const peer of peersOf(link)) {
    const told = readTold(peer.key);
    if (!told) continue;
    const computer: Computer = { key: peer.key, name: computerName(peer), kind: peer.kind, ...(peer.id ? { id: peer.id } : {}) };
    const away = awaySince(peer.key, reach);
    computers.push({ ...computer, at: told.at, ...(away !== null ? { away } : {}) });
    for (const item of [...told.items, ...(told.removed ?? [])]) {
      if (item.removed && deleted.has(item.id)) continue;
      // (Not one here; nor one deleted here that this one told of on its way here: it got here.)
      if (shadowedHere(item, here, deleted)) continue;
      const other = best.get(item.id);
      if (!other || newer(item, told.at, other.item, other.at)) best.set(item.id, { item: asTold ? { ...item, computer } : withSeen({ ...asAway(item, away), computer }), at: told.at });
    }
    for (const each of told.owners) if (!owners.has(each.owner)) owners.set(each.owner, each);
    for (const each of told.labels) if (!removedLabels.has(each.id) && !custom.has(each.id)) custom.set(each.id, each);
  }
  const visible = [...best.values()].map(each => each.item)
    .filter(item => visibleItem(item, { owner, overseen: overseen ?? undefined, deleted: true }));
  const items = visible.filter(item => !item.removed);
  const removed = visible.filter(item => item.removed);
  const named = new Set(visible.flatMap(item => [item.owner, ...(item.handedBy ? [item.handedBy] : [])]));
  return { items, removed, owners: [...owners.values()].filter(each => named.has(each.owner)), labels: [...custom.values()], computers };
}

/** The item another computer told of that `ref` names (by id, number or source), or null: of two,
 * the newer word. With `deleted`, one you deleted there through this box that none tells of now, as
 * that box answered (`removed`): for bringing it back there (Undo) and reading it, only.
 * With `local`, suppress copies shadowed here as the cross-computer list does. */
export function itemElsewhere(ref: ItemRef, link: LinkState | null = readLink(), { asTold = false, deleted = false, local }: { asTold?: boolean; deleted?: boolean; local?: readonly Item[] } = {}): ItemElsewhere | null {
  let found: { item: ItemElsewhere; at: number } | null = null;
  const peers = peersOf(link);
  const liveHere = new Set(local?.filter(item => !item.removed).map(item => item.id));
  const deletedHere = new Set(local?.filter(item => item.removed).map(item => item.id));
  const computerOf = (peer: Peer): Computer => ({ key: peer.key, name: computerName(peer), kind: peer.kind, ...(peer.id ? { id: peer.id } : {}) });
  for (const peer of peers) {
    const told = readTold(peer.key);
    const items = told ? deleted ? [...told.items, ...(told.removed ?? [])] : told.items : [];
    const item = items.find(each => refNames(ref, each) && !shadowedHere(each, liveHere, deletedHere) && visibleItem(each, { owner: "all", deleted }));
    if (item && (!found || newer(item, told!.at, found.item, found.at))) found = { item: { ...(asTold ? item : asAway(item, awaySince(peer.key))), computer: computerOf(peer) }, at: told!.at };
  }
  if (!found && deleted) {
    for (const [, each] of Object.entries(readDeletedThere())) {
      const peer = peers.find(one => one.key === each.key);
      const item = toldItemOf(each.item);
      if (peer && item && refNames(ref, item)) {
        const removed = (each.item as Pick<Item, "removed">).removed ?? { at: each.at, by: { user: true } };
        return { ...item, removed: item.removed ?? removed, computer: computerOf(peer) };
      }
    }
  }
  return found ? (asTold ? found.item : withSeen(found.item)) : null;
}

// Items you deleted on another computer through this box (`run/work-queue/deleted-there.json`, by
// id: the computer's key, when, and the item as its box answered), for Undo: a day, 200 at most,
// and only until that computer, or this box, has it again.
const deletedThereFormat = additiveObjectFormat("Work Queue remote undo receipts");
const DELETED_THERE = path.join(WORK_QUEUE, "deleted-there.json");
const DELETED_LOCK = path.join(WORK_QUEUE, ".deleted-there.lock");
const DELETED_FOR = 86_400_000;
type DeletedThere = Record<string, { key: string; at: number; item: unknown }>;

function readDeletedThere(now = Date.now()): DeletedThere {
  try {
    const kept = deletedThereFormat.read(JSON.parse(fs.readFileSync(DELETED_THERE, "utf8")));
    const here = new Set(allItems().map(item => item.id));
    return Object.fromEntries(Object.entries(kept).filter(([id, each]) => isObject(each) && typeof each.key === "string" && typeof each.at === "number" && now - each.at < DELETED_FOR && !here.has(id))) as DeletedThere;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return {};
  }
}

// The latest 200 kept, or the file gone when none are.
function keepDeletedThere(entries: [string, DeletedThere[string]][]): void {
  if (!entries.length) return void fs.rmSync(DELETED_THERE, { force: true });
  writePrivate(DELETED_THERE, JSON.stringify(deletedThereFormat.write(Object.fromEntries(entries.sort(([, a], [, b]) => b.at - a.at).slice(0, 200)))));
}

// What's kept of items deleted there, gone for those a computer tells of again (brought back).
// (Its ids looked at first, as they're written: most tellings name none of them.)
function forgetDeletedThere(told: Set<string>): void {
  let ids: string[];
  try {
    const kept = deletedThereFormat.read(JSON.parse(fs.readFileSync(DELETED_THERE, "utf8")));
    ids = Object.keys(kept);
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) console.error(error.message);
    return;
  }
  if (!ids.some(id => told.has(id))) return;
  try {
    locked(DELETED_LOCK, () => keepDeletedThere(Object.entries(readDeletedThere()).filter(([id]) => !told.has(id))), { required: true });
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) console.error(error.message);
  }
}

/** A change you made through this box to an item the computer `key` keeps, as its box answered
 * (`item`): what this box has of it now, so its views show it at once, not when that computer next
 * tells (one deleted is gone from them, and kept for bringing back there). */
export function changedThere(key: string, action: "delete" | "change" | "restore", item: Record<string, unknown>, now = Date.now()): void {
  const id = typeof item.id === "string" ? item.id : null;
  if (!id) return;
  patchTold(key, id, item, action === "delete");
  if (action !== "delete" && !fs.existsSync(DELETED_THERE)) return;
  try {
    locked(DELETED_LOCK, () => {
      const kept = readDeletedThere(now);
      if (action === "delete") kept[id] = { key, at: now, item };
      else delete kept[id];
      keepDeletedThere(Object.entries(kept));
    }, { required: true });
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) console.error(error.message);
  }
  for (const tell of queueFollowers) tell();
}

// An item told by a computer that isn't answering (`away`, since when): as it told it, but with
// none of its sessions said to be at work, as nothing there can be seen at work now.
const asAway = (item: ToldItem, away: number | null): ToldItem =>
  away === null || !item.sessions.some(each => each.working) ? item : { ...item, sessions: item.sessions.map(each => ({ ...each, working: false })) };

// Whether `a` (told at `aAt`) is newer word of an item than `b`: changed later, or the same and
// told later.
const changedAt = (item: ToldItem) => Math.max(item.updated.at, item.removed?.at ?? 0);
const newer = (a: ToldItem, aAt: number, b: ToldItem, bAt: number): boolean =>
  changedAt(a) > changedAt(b) || (changedAt(a) === changedAt(b) && aAt > bAt);
