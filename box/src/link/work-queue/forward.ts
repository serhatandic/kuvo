import { setupSaveReceipt } from "../../shared/setup-local";
import { sharedSaveReply, waitShared } from "../setup/shared";
import { fingerprint } from "../ownership/merge";
import { SCOPED_FORWARD, readForwardGrant, type ForwardGrant } from "./forward-grant";
import { SUBMITTER_REVOKED, mutationSubmitter } from "../ownership/authority";
import { runMutation } from "../ownership/mutation";
import { keepRecord, readRecord } from "../ownership/store";
import { forwardDecision, forwardDecisionKey, forwardOriginKey, MUTATION_LOCK, readForwardEvidence, type ForwardEvidence } from "../ownership/forward-evidence";
import type { DataReference } from "../ownership/resolve";
import { ITEM_ID, matching, allItems } from "./store";
import { ownKey } from "../peers/known";
import { computerUpdateRequired, legacyPeerUpdateRequired } from "../updates/client";
import { sendOfflineEdits } from "../ownership/edits";
import { peerPlaces } from "../engine/client";
import { sendOwnerMessages } from "../ownership/messages";
// What you do to an item another of your computers keeps, done there (PROTOCOL.md, "The Work
// Queue", "Across your computers", Done where it's kept): a client sends it to any box, and that box,
// which knows which computer keeps the item (as that one told it), hands it to that computer's box
// by mail and answers with what it said. So no client works out where an item is.
//
// The asking box leaves it in `run/work-queue/forward/<id>.json` and waits for `<id>.answer.json`
// beside it: a machine's routes from a Mac over SSH are answered by a process of their own (`box
// route`) with no relay connection, so its link sends what's left there, and leaves the answer, as
// this Mac's box does in one process. The computer that keeps it does it as its own client's, once
// for each id.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { additiveObjectFormat, UnsupportedFormatVersion, supportsLegacyObjectVersion } from "../../shared/format-version";
import { isObject, sortedJSON } from "../../shared/json";
import { LABEL_RESTORE_ROUTE } from "../../shared/routes/work-queue";
import { locked } from "../../shared/files";
import { listOf, writeDurablePrivate, writePrivate } from "../files";
import { watchFolders } from "../follow";
import { ownName } from "../letters/start";
import { readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { awaySince } from "../peers/reach";
import { offRelay } from "../letters/leave";
import { IS_MAC } from "../home";
import { liveLink, readLink } from "../state";
import { computerName } from "../peers/computer-name";
import { mailsFor, sendMemoryMail } from "./parts";
import { OWNER, refNames, WORK_QUEUE, type ItemRef } from "./store";
import { toldItemOf, type ToldItem } from "./told";
import type { MailSender, Peer } from "../peers/known";
import type { LinkHost } from "../relay/host";
import type { LinkState } from "../state";
import type { Computer } from "./view";
import { acceptsMutation } from "../ownership/admission";

/** What a box's agents' mail says when it takes what you do to an item from your other computers'
 * boxes (`kuvo/info`'s `features` too). */
export const FORWARD_FEATURE = "work-queue.forward";
export const MEMORY_READ_FEATURE = "work-queue.forward.memory";
/** Peer mutations that carry the verified agent's identity. */
export const AGENT_FORWARD = "work-queue.forward.agent";
export interface QueueAgent {
  tool: "queue_update" | "queue_remove" | "queue_hand";
  session: string;
  owner: string;
  name: string;
}

/** What a computer says as it keeps the files a client attaches to an item (PROTOCOL.md). */
export const ATTACHMENTS = "work-queue.attachments";

/** Whether the computer `key` takes what you do to its items from another box (`feature`: what it
 * says it takes, for another area's: renaming and removing your machines, `computers.manage`). */
export const takesForwarded = (key: string, feature = FORWARD_FEATURE): boolean => (readPeerAgents()[key]?.features ?? []).includes(feature);

/** What a box says it takes of each other area's asks. */
const AREA_FEATURES = {
  backup: "box.backups.relay",
  "queue-agent": AGENT_FORWARD, ownership: "data.ownership", computers: "computers.manage",
  addComputer: "computers.add.from-any-client", devices: "devices.pair", setup: "setup.handover",
} as const;

export const FORWARDS = path.join(WORK_QUEUE, "forward");
// What a keeper did for the asks it took, by asker and id (changes only: a read is done again).
const forwardFormat = additiveObjectFormat("Work Queue forwarded request");
const answerFormat = additiveObjectFormat("Work Queue forwarded answer");
const doneFormat = additiveObjectFormat("Work Queue forwarded action receipts");
const DONE_FILE = path.join(WORK_QUEUE, "acted.json");
const DONE_LOCK = path.join(WORK_QUEUE, ".acted.lock");
const ID = /^wqa_[A-Za-z0-9_-]{16,40}$/;
export const FORWARD_FILE = /^(wqa_[A-Za-z0-9_-]{16,40})\.json$/;

/** How long a box waits for the computer that keeps an item to answer; a file longer, as its
 * answer comes in parts (a test's shorter). */
export const forwardTiming = { wait: 20_000, file: 60_000 };
const waitFor = (act: { area?: unknown; method?: unknown; action?: unknown }) =>
  (act.action === "file" || act.area === "backup" ? forwardTiming.file : (act.area === "setup" || act.area === "addComputer") && act.method !== "GET" ? 2 * forwardTiming.wait + AGAIN_AFTER : forwardTiming.wait);
/** How often what's waiting goes again while it's waited for (the keeper takes each id once). */
const AGAIN_AFTER = 5_000;

function forwardExpired(act: Act, now: number): boolean {
  return now - act.at > waitFor(act) + AGAIN_AFTER;
}

function knownSpool(value: Record<string, unknown> | null | undefined): boolean {
  if (!value) return true;
  const scoped = Object.hasOwn(value, "scopedAct");
  if (scoped && (Object.hasOwn(value, "act") || !isObject(value.scopedAct))) return false;
  return [value, scoped ? value.scopedAct : value.act, scoped ? value.grant : undefined]
    .every(record => !isObject(record) || supportsLegacyObjectVersion(record));
}

export function pendingForward(value: Record<string, unknown> | undefined, now = Date.now()): boolean {
  if (!knownSpool(value)) return true;
  const act = actOf(value && Object.hasOwn(value, "scopedAct") ? value.scopedAct : value?.act);
  return Boolean(act && !forwardExpired(act, now));
}
/** How soon a keeper answers an ask sent again: a file's not while it's waited for (it can be big). */
const answerAgainAfter = (act: Act) => (act.action === "file" ? 60_000 : 4_000);
/** How long a keeper remembers what it did for each id (from when it was asked, or its `at` if
 * that's later), and how long ago an ask's `at` may be; and how far ahead of this clock, a minute. */
export const ACT_MEMORY = 600_000;
const AHEAD = 60_000;
/** The most asks a keeper remembers at once (the oldest go first past it). */
const MOST_REMEMBERED = 2_000;

/** What a client did, as it goes to the box that keeps the item: a whole read (`GET`) or one of its
 * files (`action` `file`, `name`), a delete, a change (`PATCH`, with what it said, `body`), or a
 * bringing back (`action` `restore`). `area` `computers`: one of your machines (`item`, its key)
 * renamed (`PATCH {name}`) or removed (`DELETE`) on the box that writes its record
 * (`peers/manage.ts`); `area` `devices` forwards pairing and revocation to the primary
 * (`peers/devices.ts`). The default area is the Work Queue. */
export interface Act {
  id: string;
  area?: keyof typeof AREA_FEATURES;
  method: "GET" | "DELETE" | "POST" | "PATCH";
  item: string;
  action?: "restore" | "file" | "removal-plan" | "name-save";
  name?: string;
  body?: Record<string, unknown>;
  at: number;
  operation?: string;
  agent?: QueueAgent;
  /** Recheck after adapter preparation, immediately before admitting the effect. */
  admit?: () => Acted | null;
  /** Setup-writer operation fence supplied after scoped admission. */
  decisionKey?: string;
}

/** What the box that keeps it answered: as its own route would have (a file: `{type, data}`, its
 * bytes in base64). */
export interface Acted { status: number; value: unknown }

/** How it went, for the box that asked: that box's answer, or why there's none. */
export type Forwarded = Acted | { status: 409 | 503; refused: string };

/** How what's left goes at once, where this process sends it (this Mac's box, a machine's link). */
export const forwardsSoon: { send: () => void } = { send: () => {} };

const left = (id: string) => path.join(FORWARDS, `${id}.json`);
const SCOPE_LOCK = path.join(FORWARDS, ".scope.lock");

function pinForward(grant: ForwardGrant, owner: string, listedBy: string | null, intent: Omit<Act, "id" | "at">): boolean {
  if (!grant.operation) return true; // Older clients can retry transport delivery within this request.
  if (!listedBy) return false;
  const key = forwardOriginKey(grant.setup, grant.operation);
  const digest = fingerprint({ owner, intent, submitter: grant.submitter });
  return locked(SCOPE_LOCK, () => {
    const shadow = readRecord<{ digest: string; forward?: ForwardEvidence }>("mutations", key);
    const previous = readRecord<{ digest: string; forward?: ForwardEvidence }>("forward-origins", key);
    if ((shadow && shadow.digest !== digest) || (previous && previous.digest !== digest)) return false;
    readForwardEvidence(shadow?.forward, key);
    readForwardEvidence(previous?.forward, key);
    const forward: ForwardEvidence = { kind: "forwardedOperationOrigins", setup: grant.setup, operation: grant.operation!,
      owner, listedBy, submitter: grant.submitter, area: intent.area ?? "queue", record: intent.item };
    // Previous keepers still accept this wire contract and read the compatibility fence.
    if (!shadow?.forward) locked(MUTATION_LOCK, () => keepRecord("mutations", key, { digest, forward }), { required: true });
    if (!previous?.forward) keepRecord("forward-origins", key, { digest, forward });
    return true;
  }, { required: true });
}

const answerOf = (id: string) => path.join(FORWARDS, `${id}.answer.json`);
const read = (format: ReturnType<typeof additiveObjectFormat>, file: string): Record<string, unknown> | null => {
  try {
    const value: unknown = JSON.parse(fs.readFileSync(file, "utf8"));
    return isObject(value) ? format.read(value) : null;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
    return null;
  }
};

// A detached backup worker obtains the Box identity from its authenticated host. Bind its
// spool to that identity and to the exact recipient and request before the live link sends it.
function spoolProof(link: LinkState | null, domain: string, to: string, act: unknown): Buffer | null {
  if (!link?.key || Buffer.from(link.key, "base64url").length !== 32) return null;
  return crypto.createHmac("sha256", Buffer.from(link.key, "base64url"))
    .update(domain + "\0").update(sortedJSON([to, act])).digest();
}

const backupProof = (link: LinkState | null, to: string, act: unknown) => spoolProof(link, "kuvo-relay-backup-spool-v1", to, act);
const scopedProof = (link: LinkState | null, to: string, act: unknown, grant: unknown) => spoolProof(link, "kuvo-relay-scoped-spool-v1", to, [act, grant]);

function authorizedBackup(link: LinkState, kept: Record<string, unknown>): boolean {
  if (typeof kept.to !== "string" || typeof kept.backupProof !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(kept.backupProof)) return false;
  const want = backupProof(link, kept.to, kept.act);
  const given = Buffer.from(kept.backupProof, "base64url");
  return Boolean(want && given.length === want.length && crypto.timingSafeEqual(want, given));
}

const normalMutation = (act: Pick<Act, "method" | "area" | "item">) => act.method !== "GET"
  && (act.area === undefined || act.area === "queue-agent" || (act.area === "ownership" && act.item !== "transfer-ownership"));

/** `act` done on the computer `computer` (one of your other computers, by what it told of its
 * items), and what it answered: or, when it can't be asked, why, in words a client shows (`named`,
 * the item as they name it). */
export async function forwardAct(computer: Computer, named: string, act: Omit<Act, "id" | "at">, signal?: AbortSignal | null, link: LinkState | null = readLink(), now = Date.now(), { backgroundRelay = false, memory = false, grant, deferSend = false }: { backgroundRelay?: boolean; memory?: boolean; grant?: ForwardGrant | null; deferSend?: boolean } = {}): Promise<Forwarded> {
  const peer = peersOf(link).find(each => each.key === computer.key);
  // (One it doesn't know any more, or that doesn't take it yet: changed there, as before.)
  if (!peer || !takesForwarded(peer.key, act.area ? AREA_FEATURES[act.area] : FORWARD_FEATURE)) return { status: 409, refused: `${named} is on ${computer.name}: change it there.` };
  if (grant === null) return { status: 409, refused: "Use a current device grant and a valid operation id." };
  if (grant && !takesForwarded(peer.key, SCOPED_FORWARD))
    return { status: 409, refused: `Update ${computer.name} before forwarding this operation from a paired device.` };
  const behind = normalMutation(act) && legacyPeerUpdateRequired(peer.key);
  if (behind) return { status: 503, value: behind };
  const id = `wqa_${crypto.randomBytes(16).toString("base64url")}`;
  if (memory) {
    if (act.method !== "GET" || !takesForwarded(peer.key, MEMORY_READ_FEATURE)) {
      return { status: 409, refused: `Update the box on ${computer.name} before requesting this read.` };
    }
    return forwardMemoryRead(peer, { ...act, id, at: now }, signal, link);
  }
  if ((!act.area || act.area === "queue-agent") && Array.isArray(act.body?.files) && act.body.files.length && !takesForwarded(peer.key, ATTACHMENTS)) return { status: 409, refused: `${computer.name} needs an update before it can keep attached files. Update it and try again.` };
  const here = ownName();
  if (awaySince(peer.key) !== null) return { status: 503, refused: `${computer.name} isn't answering now (it may be asleep, off or offline), so nothing was changed there. Try again once it's back.` };
  if (!backgroundRelay && offRelay()) return { status: 503, refused: `${here} isn't connected to your other computers now, so nothing was changed on ${computer.name}. Try again in a moment.` };
  try {
    if (grant && !pinForward(grant, peer.key, link && ownKey(link), act)) return { status: 409, refused: "That operation id is already bound to another change or owner." };
    const request = { ...act, id, at: now };
    const proof = act.area === "backup" ? backupProof(link, peer.key, request) : null;
    if (act.area === "backup" && !proof) throw new Error("The backup worker has no Box identity.");
    const scopedGrant = grant && { ...grant, operation: grant.operation ?? id };
    const seal = scopedGrant && scopedProof(link, peer.key, request, scopedGrant);
    if (scopedGrant && !seal) throw new Error("The forwarding Box has no identity.");
    writePrivate(left(id), JSON.stringify(forwardFormat.write({
      to: peer.key, ...(scopedGrant ? { scopedAct: request, grant: scopedGrant, seal: seal!.toString("base64url") } : { act: request }), ...(proof ? { backupProof: proof.toString("base64url") } : {}),
    })));
  } catch {
    return { status: 503, refused: `${here} couldn't send it to ${computer.name}. Try again in a moment.` };
  }
  madeHere.add(id);
  if (!deferSend) forwardsSoon.send();
  try {
    const answer = await answered(id, peer.key, waitFor(act), signal);
    // (It may have got there before that computer stopped answering, or its answer was lost.)
    if (answer === "away") return { status: 503, refused: `${computer.name} stopped answering, so ${here} can't tell whether it was done there. Look again once it's back.` };
    if (answer === "late") return { status: 503, refused: `${computer.name} didn't answer in time, so ${here} can't tell whether it was done there. Look again in a moment.` };
    return answer;
  } finally {
    madeHere.delete(id);
    if (knownSpool(read(forwardFormat, left(id)))) {
      read(answerFormat, answerOf(id));
      fs.rmSync(left(id), { force: true });
      fs.rmSync(answerOf(id), { force: true });
    }
  }
}

const memoryReads = new Map<string, { peer: string; answer: (reply: Forwarded) => void }>();

function forwardMemoryRead(peer: Peer, act: Act, signal: AbortSignal | null | undefined, link: LinkState | null): Promise<Forwarded> {
  const unavailable = { status: 503 as const, refused: `${peer.name} didn't answer the read. Try again in a moment.` };
  const host = liveLink();
  const mails = link && host?.welcomed && mailsFor(peer, { act }, link, act.at, true);
  if (!mails || !host || signal?.aborted) return Promise.resolve(unavailable);
  return new Promise(resolve => {
    const finish = (reply: Forwarded) => {
      if (!memoryReads.delete(act.id)) return;
      clearTimeout(timer);
      signal?.removeEventListener("abort", stop);
      resolve(reply);
    };
    const stop = () => finish(unavailable);
    const unreachable = () => finish({ status: 503, refused: `${peer.name} isn't reachable. Try again once it's back.` });
    const timer = setTimeout(stop, waitFor(act));
    memoryReads.set(act.id, { peer: peer.key, answer: finish });
    signal?.addEventListener("abort", stop, { once: true });
    try { if (!sendMemoryMail(host, peer.address, mails, unreachable)) stop(); }
    catch { stop(); }
  });
}

/** What asking your other computers for an item this box knows nothing of came to: the one that
 * keeps it, with the item whole as its box answered (`told`: as this box reads any item another
 * computer tells); that none has it (`missing`: every one answered); or that some couldn't say, so
 * it may be there (`unsure`, with why in words a client shows). */
export type Sought = { computer: Computer; item: Record<string, unknown>; told: ToldItem } | { missing: true } | { unsure: Computer[]; message: string };

const listed = (names: string[]) => (names.length < 3 ? names.join(" and ") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`);

/** An item no computer told this box of (an old Done one, a deleted one), named as a client named
 * it (`named`, which is `ref`: its number, id or issue), asked of each of your other computers'
 * boxes at once, for a client (PROTOCOL.md, "The Work Queue", "Across your computers", Found where
 * it's kept): so a client asks one box, whichever keeps the item. A live one answers at once; a
 * deleted one only once every computer answered, as another may keep it live (an issue is one item,
 * and of two the one changed last). */
export async function seekElsewhere(named: string, ref: ItemRef, signal?: AbortSignal | null, link: LinkState | null = readLink()): Promise<Sought> {
  const peers = peersOf(link);
  // (A name the box that keeps it wouldn't take, or an asker that's gone: no one is asked.)
  if (!peers.length || named.length > 300 || signal?.aborted) return { missing: true };
  const here = ownName();
  const computerOf = (peer: (typeof peers)[number]): Computer => ({ key: peer.key, name: computerName(peer), kind: peer.kind, ...(peer.id ? { id: peer.id } : {}) });
  if (offRelay()) return { unsure: peers.map(computerOf), message: `It isn't on ${here}, and ${here} isn't connected to your other computers now, so it can't tell whether it's on one of them. Try again in a moment.` };
  type Found = { computer: Computer; item: Record<string, unknown>; told: ToldItem };
  // (A live one answers for all: the others aren't waited for.)
  const rest = new AbortController();
  const gone = () => rest.abort();
  signal?.addEventListener("abort", gone, { once: true });
  // Why each that couldn't say couldn't: it isn't answering, it's a box from before this, or it
  // refused (in its own words).
  const away: Computer[] = [];
  const older: Computer[] = [];
  const refused: { computer: Computer; said: string }[] = [];
  let deleted: Found | null = null;
  try {
    const found = await new Promise<Found | null>(resolve => {
      let left = peers.length;
      const done = () => { if (--left === 0) resolve(deleted); };
      for (const peer of peers) {
        const computer = computerOf(peer);
        // (One that doesn't take it from another box, an older one: it can't be asked.)
        if (!takesForwarded(peer.key)) {
          older.push(computer);
          done();
          continue;
        }
        forwardAct(computer, "", { method: "GET", item: named }, rest.signal, link).then(there => {
          if ("refused" in there) away.push(computer);
          else if (there.status === 200) {
            const item = isObject(there.value) && isObject(there.value.item) ? there.value.item : null;
            const told = item ? toldItemOf(item) : null;
            // (One this box can't read, or that isn't the one asked for, is no answer.)
            if (!item || !told || !refNames(ref, told)) refused.push({ computer, said: "" });
            else if (!isObject(item.removed)) return resolve({ computer, item, told });
            else if (!deleted || told.updated.at > deleted.told.updated.at) deleted = { computer, item, told };
          } else if (there.status !== 404) {
            const said = isObject(there.value) && typeof there.value.message === "string" ? there.value.message.slice(0, 300) : "";
            refused.push({ computer, said });
          }
          done();
        }, () => {
          refused.push({ computer, said: "" });
          done();
        });
      }
    });
    // (A deleted one stands only when no computer that may keep it live couldn't say.)
    if (found && (!isObject(found.item.removed) || (!away.length && !older.length && !refused.length))) return found;
  } finally {
    rest.abort();
    signal?.removeEventListener("abort", gone);
  }
  if (!away.length && !older.length && !refused.length) return { missing: true };
  const names = (computers: Computer[]) => listed(computers.map(each => each.name));
  const why = [
    ...(away.length ? [`${names(away)} ${away.length === 1 ? "isn't" : "aren't"} answering now (${away.length === 1 ? "it" : "they"} may be asleep, off or offline)`] : []),
    ...(older.length ? [`${names(older)} ${older.length === 1 ? "needs" : "need"} a newer Kuvo Code to be asked from ${here}`] : []),
    ...refused.map(each => `${each.computer.name} couldn't say${each.said ? ` ("${each.said}")` : ""}`),
  ].join(", and ");
  // (Trying again helps only for one that isn't answering.)
  const again = away.length ? ` Try again once ${away.length === 1 ? `${away[0]!.name} answers` : "they answer"}.` : "";
  return { unsure: [...away, ...older, ...refused.map(each => each.computer)], message: `It isn't on ${here}, and ${why}, so it may be there.${again}` };
}

// The asks this process left. This Mac's box answers its routes in the process that sends, so it
// sends only these and backup spools authenticated with its identity. Other Mac processes cannot
// borrow its link (on a machine,
// a Mac's `box route` leaves them, and anything there could read the box's keys anyway).
const madeHere = new Set<string>();

// The answer to `id` once it's left beside it; "away" once the relay finds that computer not there
// (after it was sent), "late" past the wait or once its asker went.
function answered(id: string, key: string, wait: number, signal?: AbortSignal | null): Promise<Acted | "away" | "late"> {
  return new Promise((resolve, reject) => {
    const until = Date.now() + wait;
    let done = false;
    const finish = (value: Acted | "away" | "late" | Error) => {
      if (done) return;
      done = true;
      watch.close();
      clearInterval(timer);
      signal?.removeEventListener("abort", gone);
      if (value instanceof Error) reject(value);
      else resolve(value);
    };
    const check = () => {
      try {
        const answer = read(answerFormat, answerOf(id));
        if (answer && typeof answer.status === "number") return finish({ status: answer.status, value: answer.value });
        if (awaySince(key) !== null) return finish("away");
        if (Date.now() >= until || signal?.aborted) finish("late");
      } catch (error) {
        finish(error instanceof Error ? error : new Error(String(error)));
      }
    };
    const gone = () => finish("late");
    // (The system's watch, and a look every half second for what one missed.)
    const watch = watchFolders([FORWARDS], check, { recursive: false });
    const timer = setInterval(check, 500);
    signal?.addEventListener("abort", gone, { once: true });
    check();
  });
}

// When each one left was last sent, in this process.
const sent = new Map<string, number>();

/** What's left to forward, sent by this process's relay connection (each round, and as it's left):
 * each again every few seconds while it's waited for. What's past its wait is cleared. `ownOnly`
 * (this Mac's box): only what this process left. */
export function sendForwards(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, link: LinkState | null = readLink(), now = Date.now(), { ownOnly = IS_MAC }: { ownOnly?: boolean } = {}): void {
  sendOwnerMessages(host, link, now);
  sendOfflineEdits(host, link, now, peerPlaces);
  const names = listOf(FORWARDS);
  const here = new Set<string>();
  for (const name of names) {
    const id = FORWARD_FILE.exec(name)?.[1];
    if (!id) continue;
    here.add(id);
    let kept: Record<string, unknown> | null;
    try { kept = read(forwardFormat, path.join(FORWARDS, name)); }
    catch (error) {
      if (error instanceof UnsupportedFormatVersion) continue;
      throw error;
    }
    if (!knownSpool(kept)) continue;
    const scoped = kept !== null && Object.hasOwn(kept, "scopedAct");
    const act = actOf(scoped ? kept!.scopedAct : kept?.act);
    // (Its asker is gone: a process that stopped mid-wait leaves it.)
    if (!act && scoped) continue;
    if (!act || forwardExpired(act, now)) {
      try { read(answerFormat, answerOf(id)); }
      catch (error) {
        if (error instanceof UnsupportedFormatVersion) continue;
        throw error;
      }
      fs.rmSync(path.join(FORWARDS, name), { force: true });
      fs.rmSync(answerOf(id), { force: true });
      here.delete(id);
      continue;
    }
    if ((ownOnly && act.area !== "backup" && !madeHere.has(id)) || !host?.welcomed || !link || fs.existsSync(answerOf(id)) || now - (sent.get(id) ?? -Infinity) < AGAIN_AFTER) continue;
    if (act.area === "backup" && !authorizedBackup(link, kept!)) continue;
    const peer = peersOf(link).find(each => each.key === kept!.to);
    const behind = peer && normalMutation(act) && legacyPeerUpdateRequired(peer.key);
    if (behind) {
      if (!sent.has(id)) keepAnswer(id, { status: 503, value: behind });
      continue;
    }
    const grant = scoped ? readForwardGrant(kept!.grant, link) : undefined;
    if (scoped && (!grant || !peer || !takesForwarded(peer.key, SCOPED_FORWARD))) continue;
    if (grant) {
      const want = scopedProof(link, peer!.key, kept!.scopedAct, kept!.grant);
      const found = typeof kept!.seal === "string" ? Buffer.from(kept!.seal, "base64url") : Buffer.alloc(0);
      if (!want || want.length !== found.length || !crypto.timingSafeEqual(want, found)) continue;
    }
    const authority = grant && mutationSubmitter("ownerWrite", link, grant.submitter, host);
    if (authority === "checking") continue;
    const payload = grant ? { scopedAct: { act, grant, owner: peer!.key, replayOnly: authority === "revoked" } } : { act };
    const mails = peer ? mailsFor(peer, payload, link, now) : null;
    if (!mails) continue;
    try {
      for (const bytes of mails) host.send(bytes);
      sent.set(id, now);
    } catch {}
  }
  for (const id of sent.keys()) if (!here.has(id)) sent.delete(id);
  // (An answer whose ask is gone.)
  for (const name of names) {
    const id = /^(wqa_[A-Za-z0-9_-]{16,40})\.answer\.json$/.exec(name)?.[1];
    if (id && !here.has(id)) {
      try { read(answerFormat, answerOf(id)); }
      catch (error) {
        if (error instanceof UnsupportedFormatVersion) continue;
        throw error;
      }
      fs.rmSync(answerOf(id), { force: true });
    }
  }
}

/** The answer from the box an ask went to, left for whoever waits for it; only from that box. */
export function takeActed(sender: MailSender, given: unknown, memory = false): void {
  if (!isObject(given) || !supportsLegacyObjectVersion(given) || typeof given.id !== "string" || !ID.test(given.id)) return;
  const status = given.status;
  if (typeof status !== "number" || !Number.isInteger(status) || status < 200 || status > 599) return;
  if (memory) {
    const waiting = memoryReads.get(given.id);
    if (waiting?.peer === sender.key) waiting.answer({ status, value: given.value ?? null });
    return;
  }
  const kept = read(forwardFormat, left(given.id));
  if (kept?.to !== sender.key) return;
  keepAnswer(given.id, { status, value: given.value ?? null });
}

function keepAnswer(id: string, answer: Acted): void {
  const file = answerOf(id);
  read(answerFormat, file);
  try { writePrivate(file, JSON.stringify(answerFormat.write(answer))); } catch {}
}

// What this box did for each ask, by asker and id, and when it last answered it: so one sent again
// is answered the same (not again at once) and done once. A change's is kept on
// disk too, so it holds across a restart; one that was under way as the box stopped is answered as
// one that may have been done.
interface Done { until: number; answer: Acted | null; answered?: number }
const did = new Map<string, Done>();

// Those of `asks` past their time gone (in place), and past MOST_REMEMBERED, reads answered first
// (one asked again is read again), then the oldest.
function prune<T extends { until: number; answer: Acted | null; answered?: number }>(asks: Map<string, T>, now: number): void {
  for (const [key, each] of asks) if (each.until < now) asks.delete(key);
  for (const [key, each] of asks) {
    if (asks.size <= MOST_REMEMBERED) return;
    if (!each.answer && each.answered !== undefined) asks.delete(key);
  }
  for (const key of asks.keys()) {
    if (asks.size <= MOST_REMEMBERED) return;
    asks.delete(key);
  }
}

function readDone(now: number): Map<string, { until: number; answer: Acted | null }> {
  let kept: Record<string, unknown> = {};
  try {
    const value = JSON.parse(fs.readFileSync(DONE_FILE, "utf8"));
    if (!isObject(value)) throw new Error("The forwarding receipts are unreadable.");
    kept = doneFormat.read(value);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const fresh = new Map<string, { until: number; answer: Acted | null }>();
  for (const [key, each] of Object.entries(kept)) {
    if (isObject(each) && typeof each.until === "number") fresh.set(key, { until: each.until, answer: isObject(each.answer) ? each.answer as unknown as Acted : null });
  }
  prune(fresh, now);
  return fresh;
}

function keepDone(key: string, done: Done, now: number, claim = false): Done | undefined {
  return locked(DONE_LOCK, () => {
    const kept = readDone(now);
    const existing = kept.get(key);
    if (claim && existing) return existing;
    kept.delete(key);
    kept.set(key, { until: done.until, answer: done.answer });
    prune(kept, now);
    writeDurablePrivate(DONE_FILE, JSON.stringify(doneFormat.write(Object.fromEntries(kept))), path.dirname(WORK_QUEUE));
    return undefined;
  }, { required: true });
}

const inFlightServices = new Set<string>();
type ReplayableServices = Partial<Record<NonNullable<Act["area"]>, (act: Act) => Promise<Acted>>>;

/** Areas that handle their own replay still use the same authenticated request envelope.
 * Throttle their concurrent retries without caching replies or persisting a mutation claim. */
export async function takeAct(sender: MailSender, given: unknown, reply: (payload: Record<string, unknown>) => void,
  run: (act: Act) => Promise<Acted>, now = Date.now(), replayable?: ReplayableServices): Promise<void> {
  const act = actOf(given);
  if (!act) return;
  const link = readLink();
  // (Only from one of your computers that's still a peer.)
  const peer = peersOf(link).find(each => each.key === sender.key);
  if (!peer) return;
  const here = ownName();
  const answer = (value: Acted) => reply({ acted: { id: act.id, ...value } });
  const key = `${sender.key} ${act.id}`;
  const service = act.area && replayable?.[act.area];
  const badClock = () => answer({ status: 409, value: { message: `${here}'s clock and ${computerName(peer)}'s are too far apart to take it: check both computers' time.` } });
  if (service) {
    if (now - act.at > ACT_MEMORY || act.at - now > AHEAD) return badClock();
    if (inFlightServices.has(key)) return;
    inFlightServices.add(key);
    try { return answer(await service(act)); }
    finally { inFlightServices.delete(key); }
  }
  prune(did, now);
  const changes = act.method !== "GET";
  const mine = did.get(key);
  // (Under way now, or answered a moment ago; a change as it went, a read done again.)
  if (mine && (mine.answered === undefined || now - mine.answered < answerAgainAfter(act))) return;
  if (mine?.answer) {
    mine.answered = now;
    return answer(mine.answer);
  }
  // A change this box took before it last started: as it went, or, cut short then, that it may
  // have been done.
  const unavailable = { status: 503, value: { message: "This computer couldn’t persist the forwarded operation." } };
  let kept: Done | undefined;
  try { kept = changes ? readDone(now).get(key) : undefined; }
  catch { return answer(unavailable); }
  // (Renaming or removing one of your machines, or pairing your app, is the same done twice: one
  // cut short is done again.)
  const redo = kept !== undefined && kept.answer === null && (act.area === "computers" || act.area === "devices");
  if (kept && !redo) {
    const value = kept.answer ?? { status: 409, value: { message: `${here} stopped while it was doing this, so it can't tell whether it was done. Look again in a moment.` } };
    did.set(key, { until: kept.until, answer: value, answered: now });
    return answer(value);
  }
  // (One from long ago, sent again, or from a computer whose clock is far off: not done. Each is
  // remembered until it would be too old to take, so none is ever taken twice.)
  if (!redo && (now - act.at > ACT_MEMORY || act.at - now > AHEAD)) return badClock();
  if (normalMutation(act)) {
    const behind = computerUpdateRequired(act.area === "ownership" ? act.item : "kuvo/queue", act.method);
    if (behind) return answer({ status: 503, value: behind });
  }
  const entry: Done = { until: kept?.until ?? Math.max(now, act.at) + ACT_MEMORY, answer: null };
  if (changes) {
    try {
      const previous = keepDone(key, entry, now, !redo);
      if (previous) return answer(previous.answer ?? { status: 409, value: { message: "This operation was already claimed." } });
    } catch { return answer(unavailable); }
  }
  did.set(key, entry);
  try {
    entry.answer = await run(act);
  } catch {
    entry.answer = { status: 500, value: { message: `Something went wrong on ${here}. Try again in a moment.` } };
  }
  if (changes) {
    try { keepDone(key, entry, now); }
    catch {
      entry.answered = Date.now();
      return answer({ status: 503, value: { message: "This computer couldn’t persist the operation’s outcome. Check the record before retrying." } });
    }
  }
  entry.answered = Date.now();
  answer(entry.answer);
  // (A read's answer isn't kept: a file's can be big, and one asked again is read again.)
  if (!changes) entry.answer = null;
}

/** All this box did for others forgotten, in this process (a test's: what's on disk stays, as for a
 * box that started again). */
export function forgetActs(): void {
  did.clear();
  sent.clear();
}

const actOf = (given: unknown): Act | null => {
  if (!isObject(given) || !supportsLegacyObjectVersion(given) || typeof given.id !== "string" || !ID.test(given.id) || typeof given.item !== "string" || !given.item || given.item.length > 300) return null;
  if (typeof given.at !== "number" || !Number.isFinite(given.at)) return null;
  const { method, action, name } = given;
  const fits = (method === "GET" && action === undefined) || (method === "DELETE" && action === undefined) || (method === "POST" && action === "restore") || (method === "PATCH" && action === undefined)
    || (method === "GET" && action === "file" && typeof name === "string" && name.length > 0 && name.length <= 300);
  const devices = given.area === "devices" && (method === "GET" || method === "POST" || method === "DELETE") && action === undefined;
  const setup = given.area === "setup" && (method === "GET" || method === "POST") && action === undefined;
  const ownership = given.area === "ownership" && (method === "GET" || method === "PATCH" || method === "POST" || method === "DELETE") && action === undefined;
  const nameSave = given.area === "computers" && method === "GET" && action === "name-save" && given.item === "name-save";
  const removalPlan = given.area === "computers" && action === "removal-plan" && (method === "GET" || method === "POST");
  const addComputer = given.area === "addComputer" && (method === "GET" || method === "POST" || method === "DELETE") && action === undefined;
  const backup = given.area === "backup" && (method === "GET" || method === "POST" || method === "DELETE") && action === undefined;
  if (!fits && !backup && !addComputer && !devices && !setup && !ownership && !removalPlan && !nameSave) return null;
  const queueAgent = given.area === "queue-agent" && fits;
  if (given.area !== undefined && !queueAgent && !backup && !addComputer && !devices && !setup && !ownership && !removalPlan && !nameSave
      && !(given.area === "computers" && (method === "PATCH" || method === "DELETE") && action === undefined)) return null;
  if (given.operation !== undefined && (typeof given.operation !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(given.operation))) return null;
  if (ownership && method === "POST" && given.item === LABEL_RESTORE_ROUTE && isObject(given.body)
    && isObject(given.body.value) && given.body.value.dryRun === true) return null;
  let agent: QueueAgent | undefined;
  if (queueAgent) {
    const who = given.agent;
    if (!isObject(who) || !["queue_update", "queue_remove", "queue_hand"].includes(String(who.tool)) ||
        (who.tool === "queue_remove" ? method !== "DELETE" && !(method === "POST" && action === "restore") : method !== "PATCH") ||
        typeof who.session !== "string" || !/^ses_[A-Za-z0-9]{1,60}$/.test(who.session) ||
        typeof who.owner !== "string" || !OWNER.test(who.owner) || (who.owner !== who.session && !who.owner.startsWith("grp_")) ||
        typeof who.name !== "string" || !who.name.trim() || who.name.length > 200) return null;
    agent = { tool: who.tool, session: who.session, owner: who.owner, name: who.name } as QueueAgent;
  } else if (given.agent !== undefined) return null;
  return { id: given.id, ...(agent ? { agent } : {}), ...(given.operation ? { operation: given.operation } : {}), ...(given.area === "computers" || addComputer || devices || setup || ownership || queueAgent || backup ? { area: given.area } : {}), method, item: given.item, ...(action ? { action } : {}), ...(action === "file" ? { name } : {}), ...(isObject(given.body) ? { body: given.body } : {}), at: given.at } as Act;
};

/** Each adapter must still enforce its own operation and owner rules. A scoped retry can
 * recover a finished decision independently of the sender's current device authority. */
export async function takeScopedAct(sender: MailSender, given: Record<string, unknown>, reply: (payload: Record<string, unknown>) => void,
  run: (act: Act) => Promise<Acted>, host?: Pick<LinkHost, "send"> | null, now = Date.now()) {
  if (!supportsLegacyObjectVersion(given)) return;
  const act = actOf(given.act);
  const link = readLink();
  const grant = readForwardGrant(given.grant, link);
  if (!act || !grant || !link || act.method === "GET" || act.area === "backup" || act.area === "queue-agent"
    || typeof given.owner !== "string" || given.owner !== ownKey(link) || !peersOf(link).some(peer => peer.key === sender.key)) return;
  const { id, at, ...intent } = act;
  const owner = given.owner;
  const key = forwardDecisionKey(grant.setup, grant.operation);
  let ref: DataReference | undefined;
  if (!act.area && ITEM_ID.test(act.item)) {
    const item = matching(allItems(), { id: act.item })[0];
    if (item) ref = { kind: "queueItems", id: item.id, queue: item.owner };
  } else if (act.area === "ownership") {
    ref = act.item === "transfer-ownership" ? { kind: "setupLeadership" }
      : (await import("../ownership/write")).ownerWrite(act.method, act.item, act.body?.value)?.ref;
  } else if (act.area === "computers") {
    ref = { kind: act.method === "PATCH" ? "setupNames" : "membershipAndAccess", id: act.item };
  } else if (act.area === "setup") {
    ref = { kind: "setupLeadership" };
  } else if (act.area === "devices") {
    ref = { kind: "membershipAndAccess" };
  }
  const admit = () => {
    const current = readLink();
    const authority = mutationSubmitter("ownerWrite", current, grant.submitter, host);
    if (!readForwardGrant(grant, current) || !peersOf(current).some(peer => peer.key === sender.key)
      || given.replayOnly === true || authority === "revoked")
      return { status: 409, value: { message: SUBMITTER_REVOKED } };
    if ((!act.area && !ref) || (ref && !acceptsMutation("ownerWrite", ref, current)))
      return { status: 409, value: { message: "The record is moving or owned by another computer." }, notAdmitted: true as const };
    if (authority === "checking")
      return { status: 503, value: { message: "Checking the submitting device's access." }, notAdmitted: true as const };
    if (now - at > ACT_MEMORY || at - now > AHEAD)
      return { status: 409, value: { message: "This forwarded operation expired." } };
    const behind = normalMutation(act) && computerUpdateRequired(act.area === "ownership" ? act.item : "kuvo/queue", act.method);
    return behind ? { status: 503, value: behind, notAdmitted: true as const } : null;
  };
  const answer = await runForwardDecision(intent, grant, owner, ref, admit, () => run({ ...act, admit, decisionKey: key })).catch(() => ({ status: 503, value: { message: "This computer couldn't confirm the forwarded operation. Check its result before retrying." }, notAdmitted: false }));
  if (answer.notAdmitted) return;
  const saved = act.area === "ownership" && answer.status === 202 ? setupSaveReceipt(answer.value) : null;
  const current = saved ? sharedSaveReply(await waitShared(saved, 0)) : answer;
  reply({ acted: { id, status: current.status, value: current.value } });
}

/** Both the local entry point and peer delivery consult the same owner decision. */
export function runForwardDecision(intent: Omit<Act, "id" | "at">, grant: ForwardGrant & { operation: string }, owner: string,
  ref: DataReference | undefined, admit: () => Acted | null, run: () => Promise<Acted>) {
  const evidence: ForwardEvidence = { kind: forwardDecision(ref), setup: grant.setup, operation: grant.operation,
    owner, submitter: grant.submitter, area: intent.area ?? "queue", record: intent.item, ref };
  const change = (owner: string) => ({ intent, submitter: grant.submitter, owner });
  return runMutation(forwardDecisionKey(grant.setup, grant.operation), change(owner), async () => {
    const veto = admit();
    return veto ?? await run();
  }, admit, undefined, undefined, undefined, { evidence, digestFor: owner => fingerprint(change(owner)) });
}
