import { computerUpdateRequired, legacyPeerUpdateRequired } from "../updates/client";
import path from "node:path";
import { locked } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { listedGroups } from "../areas/groups";
import { mailsFor } from "../work-queue/parts";
import { workQueueClient } from "../areas/work-queue";
import { botHere, isMoving } from "../bots/here";
import { botEditView, editBot } from "../bots/offline";
import { bridgeTarget, readBridgeFile, readEngineFile, type PeerPlaces } from "../engine/client";
import { RUN } from "../home";
import { placeOfSession } from "../letters/letter";
import { awaySessions, frozen } from "../moves/records";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import type { LinkHost } from "../relay/host";
import type { RouteContext } from "../server/context";
import { editSetup, setupEditView } from "../setup/offline";
import { followedSetup, usesAcceptedSaves } from "../setup/record";
import { SharedCapacity } from "../setup/shared";
import { readLink, type LinkState } from "../state";
import { itemElsewhere } from "../work-queue/across";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { editQueue, queueEditView } from "../work-queue/offline";
import { allItems, deletionFor, departureFor, ITEM_ID, MOST_FILES, MOST_NOTE, OWNER, Refusal, YOURS } from "../work-queue/store";
import { editStatus, type EditReceipt, type EditTarget, type OfflineEdit } from "./edit-types";
import { fingerprint } from "./merge";
import { MUTATION_FEATURE, MUTATION_ID, mutationKey, mutationNonce } from "./mutation";
import { mayActivate, ownerOf } from "./resolve";
import { keepRecord, offlineEditsReady, readRecord, recordKey, records } from "./store";

const LOCK = path.join(RUN, "owned-data", ".edits.lock");
const ID = MUTATION_ID;
const MAX_BYTES = 512 * 1024;
const MAX_PENDING = 200;
const FEATURE = "data.offline-edits";
const kinds = new Set(["queueItems", "botSettings", "setupTimeZone", "setupUsageDisplay", "setupNames", "setupPersonalities", "queueLabels"]);

interface Envelope { setup: string; nonce: string; edit: OfflineEdit }
interface Outgoing {
  setup: string;
  digest: string;
  owner: string;
  ref: EditTarget;
  receipt: EditReceipt;
  envelope?: Envelope;
  recovery?: Record<string, string>;
  attachments?: OfflineEdit["attachments"];
}

function refOf(value: unknown): EditTarget | undefined {
  if (!isObject(value) || typeof value.kind !== "string" || !kinds.has(value.kind)
    || (value.computer !== undefined && (typeof value.computer !== "string" || !KEY.test(value.computer)))) return;
  const computer = typeof value.computer === "string" ? { computer: value.computer } : {};
  if (value.kind === "queueItems") {
    if (typeof value.id !== "string" || !ITEM_ID.test(value.id) || typeof value.queue !== "string" || !OWNER.test(value.queue)) return;
    return { kind: value.kind, id: value.id, queue: value.queue, ...computer };
  }
  if (value.kind === "botSettings") {
    if (typeof value.id !== "string" || !/^bot_[A-Za-z0-9]{1,95}$/.test(value.id)) return;
    return { kind: value.kind, id: value.id, ...computer };
  }
  return { kind: value.kind as EditTarget["kind"] };
}

function editOf(value: unknown): OfflineEdit | undefined {
  if (!isObject(value) || typeof value.operation !== "string" || !ID.test(value.operation)) return;
  const ref = refOf(value.ref);
  if (!ref || (value.create === true ? value.baseRevision !== null
    : typeof value.baseRevision !== "string" || !KEY.test(value.baseRevision))) return;
  if (value.fields !== undefined && (!isObject(value.fields) || Object.keys(value.fields).length > 64)) return;
  if (value.resolve !== undefined && (!isObject(value.resolve) || typeof value.resolve.id !== "string" || !KEY.test(value.resolve.id)
    || !["accepted", "proposed"].includes(String(value.resolve.use)))) return;
  const queue = ref.kind === "queueItems";
  if (value.create !== undefined && (value.create !== true || !queue)) return;
  if (value.delete !== undefined && (value.delete !== true || !queue)) return;
  if ((value.delete === true || value.resolve !== undefined) &&
    (value.create || value.fields !== undefined || value.notes !== undefined || value.attachments !== undefined || (value.delete && value.resolve))) return;
  const notes = value.notes;
  if (notes !== undefined && (!queue || !Array.isArray(notes) || notes.length > 40 || notes.some(note =>
    !isObject(note) || typeof note.id !== "string" || !ID.test(note.id) || typeof note.text !== "string" || !note.text.trim() || note.text.length > MOST_NOTE))) return;
  const attachments = value.attachments;
  if (attachments !== undefined && (!queue || !Array.isArray(attachments) || attachments.length > MOST_FILES || attachments.some(part =>
    !isObject(part) || typeof part.id !== "string" || !ID.test(part.id) || !isObject(part.file)
    || typeof part.file.uri !== "string" || !part.file.uri.startsWith("data:")))) return;
  if (!value.delete && !value.resolve && !value.fields && !notes?.length && !attachments?.length) return;
  return {
    ref, operation: value.operation, baseRevision: value.baseRevision as string | null,
    ...(isObject(value.fields) ? { fields: value.fields } : {}),
    ...(value.create === true ? { create: true } : {}),
    ...(value.delete === true ? { delete: true } : {}),
    ...(value.resolve ? { resolve: value.resolve as OfflineEdit["resolve"] } : {}),
    ...(Array.isArray(notes) ? { notes: notes.map(note => ({ id: note.id, text: note.text })) } : {}),
    ...(Array.isArray(attachments) ? { attachments: attachments.map(part => ({ id: part.id, file: part.file })) } : {}),
  };
}

const operationOf = (setup: string, sender: string, nonce: string) => recordKey(setup, sender, nonce);

function location(ref: EditTarget, link: LinkState | null) {
  if (ref.kind === "queueItems") {
    const item = allItems().find(item => item.id === ref.id);
    if (item) return ownerOf({ ...ref, queue: item.owner, computer: undefined }, { link, localRecord: true });
    const remote = itemElsewhere({ id: ref.id! }, link, { deleted: true });
    if (remote) return ownerOf({ ...ref, queue: remote.owner, computer: remote.computer.key }, { link });
    if (ref.queue === YOURS && ref.computer) return ownerOf({ kind: "ownerReadSnapshots", computer: ref.computer }, { link });
    const fence = deletionFor(ref.id!);
    if (fence) return ownerOf({ ...ref, queue: fence.queue }, { link, localRecord: true });
    const transferred = departureFor(ref.id!);
    if (transferred) return ownerOf({ ...ref, ...transferred }, { link });
  }
  return ownerOf(ref, { link, localRecord: ref.kind === "botSettings" && Boolean(botHere(ref.id)) });
}

function member(link: LinkState | null, setup: string, sender: string) {
  return followedSetup(link)?.id === setup && (ownKey(link!) === sender || peersOf(link).some(peer => peer.key === sender));
}

function stillOwns(ref: EditTarget, link: LinkState, checked?: string) {
  if (ref.kind === "queueItems") {
    if (ref.queue !== YOURS && ref.queue !== checked) return false;
    if (ref.queue === YOURS && ref.computer && ref.computer !== ownKey(link)) {
      // After a handoff the actual stored queue, supplied by the adapter, decides.
      if (!allItems().some(item => item.id === ref.id && item.owner === YOURS)) return false;
    }
    if (ref.queue !== YOURS && (frozen().has(ref.queue!) || awaySessions().includes(ref.queue!))) return false;
  }
  if (ref.kind === "botSettings") {
    const bot = botHere(ref.id);
    if (!bot || isMoving(bot.folder) || frozen().has(bot.session) || awaySessions().includes(bot.session)) return false;
  }
  return mayActivate(ref, { link });
}

async function confirmQueue(ref: EditTarget, places: PeerPlaces): Promise<string | undefined> {
  if (ref.kind !== "queueItems") return;
  const queue = allItems().find(item => item.id === ref.id)?.owner ?? deletionFor(ref.id!)?.queue ?? ref.queue;
  if (!queue || queue === YOURS || frozen().has(queue) || awaySessions().includes(queue)) return;
  if (queue.startsWith("grp_")) {
    const bridge = bridgeTarget();
    if (bridge && (await listedGroups(bridge, AbortSignal.timeout(3_000)))?.some(group => group.id === queue)) return queue;
  } else if (places.targets.length && await placeOfSession(queue, places.targets, 3_000).catch(() => null)) return queue;
  return undefined;
}

const engines = {
  engine: readEngineFile,
  bridge: () => bridgeTarget() ? readBridgeFile() : null,
};

async function apply(envelope: Envelope, sender: string, places: PeerPlaces): Promise<EditReceipt | undefined> {
  const { setup, edit } = envelope;
  const { ref, operation } = edit;
  const interrupted = (message: string): EditReceipt => ({ operation, status: "interrupted", message });
  if (!member(readLink(), setup, sender)) return;
  const checked = await confirmQueue(ref, places);
  if (computerUpdateRequired("kuvo/data/edits", "POST")) return;
  const owns = (target: EditTarget) => {
    const link = readLink();
    return member(link, setup, sender) && stillOwns(target, link!, checked);
  };
  try {
    const result = ref.kind === "queueItems" ? editQueue(edit, owns)
      : ref.kind === "botSettings" ? await editBot(edit, owns, engines.engine, engines.bridge)
      : editSetup(edit, link => member(link, setup, sender));
    return result === false ? interrupted("That operation id is already in use.") : result;
  } catch (error) {
    if (error instanceof SharedCapacity) return { operation, status: "capacity", message: error.message };
    if (error instanceof Refusal) return interrupted(error.message);
    throw error;
  }
}

const pending = (operation: string): EditReceipt => ({ operation, status: "pending", message: "Saved here. Waiting for the current owner to accept it." });

function finish(outgoing: Outgoing, owner: string, receipt: EditReceipt) {
  locked(LOCK, () => {
    const kept = readRecord<Outgoing>("edit-outbox", receipt.operation);
    if (!kept?.envelope || kept.setup !== outgoing.setup || kept.owner !== owner || kept.digest !== outgoing.digest) return;
    const edit = kept.envelope.edit;
    const recovery: Record<string, string> = {};
    const rejected = receipt.status === "refresh" || receipt.status === "deleted" || receipt.status === "interrupted" || receipt.status === "capacity";
    if (rejected) {
      for (const [field, value] of Object.entries(edit.fields ?? {})) if (typeof value === "string") recovery[field] = value;
      for (const note of edit.notes ?? []) recovery[`note:${note.id}`] = note.text;
    }
    keepRecord("edit-outbox", receipt.operation, { setup: kept.setup, owner, digest: kept.digest, ref: kept.ref, receipt, recovery,
      ...(rejected && edit.attachments?.length ? { attachments: edit.attachments } : {}) });
  }, { required: true });
}

export async function saveOfflineEdit(context: RouteContext, places: PeerPlaces, proposal: unknown = parsed(context.request.body)) {
  const { request, json } = context;
  if ((request.body?.length ?? 0) > MAX_BYTES) return json(413, { message: "This draft is too large to send to the owner." });
  const given = editOf(proposal);
  if (!given) return json(400, { message: "Send a supported edit with its operation id, record address and base revision (inline attachments only)." });
  const link = readLink();
  const setup = followedSetup(link);
  const sender = link && ownKey(link);
  if (!setup || !sender) return json(409, { message: "Join a setup before saving an offline edit." });
  if (given.ref.kind !== "queueItems" && given.ref.kind !== "botSettings" && !usesAcceptedSaves(setup))
    return json(409, { message: "This setup has not enabled accepted preference saves." });
  const owner = location(given.ref, link) ?? (await confirmQueue(given.ref, places)
    ? ownerOf(given.ref, { link, localRecord: true }) : null);
  const legacy = operationOf(setup.id, sender, given.operation);
  const current = mutationKey(setup.id, given.operation);
  const legacyOwner = owner?.key && !owner.local && !takesForwarded(owner.key, MUTATION_FEATURE);
  const operation = readRecord<Outgoing>("edit-outbox", current) ? current
    : readRecord<Outgoing>("edit-outbox", legacy) || legacyOwner ? legacy : current;
  const envelope = { setup: setup.id, nonce: given.operation, edit: { ...given, operation } };
  const digest = fingerprint(envelope);
  const stored = locked(LOCK, () => {
    const existing = readRecord<Outgoing>("edit-outbox", operation);
    if (existing) return existing.digest === digest ? existing : null;
    if (!owner?.key) return;
    if (records("edit-outbox").filter(key => readRecord<Outgoing>("edit-outbox", key)?.envelope).length >= MAX_PENDING) return;
    const outgoing = { setup: setup.id, owner: owner.key, digest, envelope, ref: given.ref, receipt: pending(operation) };
    keepRecord("edit-outbox", operation, outgoing);
    return outgoing;
  }, { required: true });
  if (stored === null) return json(409, { message: "That operation id is already in use." });
  if (!stored) return json(503, { message: "The owner could not be identified or too many edits are waiting." });
  if (stored.envelope && owner?.local && owner.key === stored.owner && !applying.has(operation)) {
    applying.add(operation);
    try {
      const receipt = await apply(envelope, sender, places);
      if (receipt) finish(stored, stored.owner, receipt);
    } finally { applying.delete(operation); }
  }
  const kept = readRecord<Outgoing>("edit-outbox", operation)!;
  return json(editStatus(kept.receipt), kept.receipt);
}

/** An ordinary route carrying a base revision submits the same proposal as the edit outbox. */
export function ordinaryEdit(context: RouteContext, body: Record<string, unknown>): unknown {
  const { route, request } = context;
  const method = request.method.toUpperCase();
  const item = /^kuvo\/work-queue\/items\/(wqi_[A-Za-z0-9_-]{20})$/.exec(route);
  const bot = /^kuvo\/bots\/(bot_[A-Za-z0-9]{1,95})$/.exec(route);
  const nonce = mutationNonce(request, body);
  if (body.baseRevision === undefined) return;
  if (item && (method === "PATCH" || method === "DELETE"))
    return { ...body, operation: nonce, ref: { kind: "queueItems", id: item[1], queue: body.owner, computer: body.computer },
      ...(method === "DELETE" ? { delete: true } : {}) };
  if (route === "kuvo/work-queue/items" && method === "POST")
    return { ...body, operation: nonce, create: true, ref: { kind: "queueItems", id: body.id, queue: body.owner, computer: body.computer } };
  if (bot && method === "PATCH")
    return { ...body, operation: nonce, ref: { kind: "botSettings", id: bot[1], computer: body.computer } };
  if (route === "kuvo/settings/clock" && method === "PATCH")
    return { ...body, operation: nonce, ref: { kind: "setupTimeZone" }, fields: body.fields ?? { zone: body.value } };
  if (route === "kuvo/personalities" && method === "PATCH" || route === "kuvo/work-queue/labels" && method === "POST")
    return { ...body, operation: nonce, ref: { kind: route === "kuvo/personalities" ? "setupPersonalities" : "queueLabels" } };
  return undefined;
}

export function offlineEditReceipt(context: RouteContext, operation: string) {
  const kept = KEY.test(operation) && readRecord<Outgoing>("edit-outbox", operation);
  return kept && kept.setup === followedSetup(readLink())?.id
    ? context.json(200, { receipt: kept.receipt, recovery: kept.recovery ?? {}, ...(kept.attachments?.length ? { attachments: kept.attachments } : {}) })
    : context.json(404, { message: "There is no such edit on this computer." });
}

export function offlineEditOutbox(context: RouteContext) {
  const setup = followedSetup(readLink());
  const entries = !setup ? [] : records("edit-outbox").flatMap(key => {
    const kept = readRecord<Outgoing>("edit-outbox", key);
    if (kept?.setup !== setup.id || (!kept.envelope && !Object.keys(kept.recovery ?? {}).length && !kept.attachments?.length)) return [];
    return [{ ref: kept.ref ?? kept.envelope?.edit.ref, owner: kept.owner, receipt: kept.receipt }];
  });
  return context.json(200, { entries });
}

async function readHere(ref: EditTarget, sender: string, setup: string, places: PeerPlaces): Promise<Acted> {
  try {
    const checked = await confirmQueue(ref, places);
    const owns = (target: EditTarget) => {
      const link = readLink();
      return member(link, setup, sender) && stillOwns(target, link!, checked);
    };
    const value = ref.kind === "queueItems" ? queueEditView(ref, owns)
      : !owns(ref) ? null : ref.kind === "botSettings" ? botEditView(ref, owns) : setupEditView(ref);
    if (value) return { status: 200, value };
  } catch (error) {
    if (!(error instanceof Refusal)) throw error;
  }
  return { status: 409, value: { message: "This computer could not confirm the current record." } };
}

export async function readEditForPeer(sender: MailSender, act: Act, places: PeerPlaces): Promise<Acted> {
  const ref = refOf(act.body?.ref);
  const setup = act.body?.setup;
  if (offlineEditsReady() && ref && typeof setup === "string") return readHere(ref, sender.key, setup, places);
  return { status: 400, value: { message: "Name the record and its setup." } };
}

export async function readOfflineEdit(context: RouteContext, places: PeerPlaces) {
  const query = new URLSearchParams(context.request.path.split("?")[1]);
  const ref = refOf(Object.fromEntries(query));
  if (!ref) return context.json(400, { message: "Name the record to edit." });
  const link = readLink();
  const setup = followedSetup(link);
  const own = link && ownKey(link);
  const owner = location(ref, link) ?? (await confirmQueue(ref, places) ? ownerOf(ref, { link, localRecord: true }) : null);
  if (!setup || !link || !own || !owner?.key) return context.json(409, { message: "This computer could not identify the owner." });
  if (owner.local) {
    const answer = await readHere(ref, own, setup.id, places);
    return context.json(answer.status, answer.status === 200 ? { ...answer.value as object, owner: own, stale: false } : answer.value);
  }
  const key = recordKey(setup.id, owner.key, ref.kind, ref.id ?? "");
  const peer = peersOf(link).find(peer => peer.key === owner.key);
  if (peer && owner.reachable && takesForwarded(peer.key, FEATURE)) {
    const result = await forwardAct(peer, "This record", { method: "GET", item: "offline-edit", area: "ownership", body: { ref, setup: setup.id } }, context.signal, link);
    if (!("refused" in result) && result.status === 200 && isObject(result.value)
      && member(readLink(), setup.id, peer.key) && location(ref, readLink())?.key === peer.key) {
      keepRecord("edit-snapshots", key, { value: result.value });
      return context.json(200, { ...result.value, owner: peer.key, stale: false });
    }
  }
  const cached = peer && readRecord<{ value: object }>("edit-snapshots", key);
  return cached ? context.json(200, { ...cached.value, owner: owner.key, stale: true })
    : context.json(503, { message: "There is no saved revision here yet." });
}

export function editRouteAllowed(context: RouteContext) {
  return workQueueClient(context) && Boolean(context.request.fromApp || context.request.fromMac || context.phone?.kind === "desktop");
}

const retry = new Map<string, number>();
const applying = new Set<string>();

export function sendOfflineEdits(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, link: LinkState | null, now: number, places: () => PeerPlaces) {
  const setup = followedSetup(link);
  const self = link && ownKey(link);
  if (!setup || !link || !self) return;
  const waiting = records("edit-outbox");
  const remaining = new Set<string>();
  for (const operation of waiting) {
    const kept = readRecord<Outgoing>("edit-outbox", operation);
    if (!kept?.envelope || kept.setup !== setup.id) continue;
    remaining.add(operation);
    if (now - (retry.get(operation) ?? -Infinity) < 5_000 || applying.has(operation)) continue;
    let owner = location(kept.envelope.edit.ref, link);
    if (!owner?.key && kept.owner === self) owner = { key: self, local: true, reachable: true, scope: "local computer", name: "" };
    if (!owner?.key) continue;
    if (owner.key !== kept.owner) {
      const changed = { ...kept, owner: owner.key };
      const saved = locked(LOCK, () => {
        const current = readRecord<Outgoing>("edit-outbox", operation);
        if (!current?.envelope || current.owner !== kept.owner) return false;
        keepRecord("edit-outbox", operation, changed);
        return true;
      }, { required: true });
      if (!saved) continue;
    }
    if (owner.local) {
      const envelope = kept.envelope;
      const key = owner.key;
      retry.set(operation, now);
      applying.add(operation);
      void apply(envelope, self, places()).then(result => { if (result) finish(kept, key, result); })
        .catch(error => console.error("Offline edit:", error)).finally(() => applying.delete(operation));
    } else if (host?.welcomed) {
      const peer = peersOf(link).find(peer => peer.key === owner!.key);
      if (!peer || !takesForwarded(peer.key, FEATURE) || legacyPeerUpdateRequired(peer.key)) continue;
      const mails = mailsFor(peer, { offlineEdit: kept.envelope }, link, now);
      if (!mails) continue;
      for (const bytes of mails) host.send(bytes);
      retry.set(operation, now);
    }
  }
  for (const operation of retry.keys()) if (!remaining.has(operation)) retry.delete(operation);
}

export async function takeOfflineEdit(sender: MailSender, given: Record<string, unknown>, reply: (value: Record<string, unknown>) => void, places: PeerPlaces) {
  if (!offlineEditsReady() || Buffer.byteLength(JSON.stringify(given)) > MAX_BYTES + 512) return;
  const { setup, nonce } = given;
  const edit = editOf(given.edit);
  if (typeof setup !== "string" || typeof nonce !== "string" || !ID.test(nonce) || !edit || (edit.operation !== mutationKey(setup, nonce) && edit.operation !== operationOf(setup, sender.key, nonce))) return;
  if (applying.has(edit.operation)) return;
  applying.add(edit.operation);
  try {
    const receipt = await apply({ setup, nonce, edit }, sender.key, places);
    if (receipt && member(readLink(), setup, sender.key)) reply({ offlineEditReceipt: { setup, receipt } });
  } finally { applying.delete(edit.operation); }
}

export function takeOfflineEditReceipt(sender: MailSender, given: Record<string, unknown>) {
  const receipt = given.receipt;
  if (!isObject(receipt) || typeof receipt.operation !== "string" || !KEY.test(receipt.operation)
    || typeof given.setup !== "string" || !member(readLink(), given.setup, sender.key)) return;
  if (receipt.status === "interrupted" || receipt.status === "capacity" ? typeof receipt.message !== "string"
    : !["committed", "choice", "deleted", "refresh"].includes(String(receipt.status)) || typeof receipt.revision !== "string" || !KEY.test(receipt.revision)
      || !Array.isArray(receipt.choices) || receipt.choices.some(id => typeof id !== "string" || !KEY.test(id))) return;
  const kept = readRecord<Outgoing>("edit-outbox", receipt.operation);
  if (kept?.setup === given.setup && kept.owner === sender.key) finish(kept, sender.key, receipt as EditReceipt);
}
