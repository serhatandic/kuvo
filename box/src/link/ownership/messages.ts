import { keepReceipt, removeReceipt } from "./receipt-worker";
import { supportsLegacyObjectVersion } from "../../shared/format-version";
import type { QueueItem } from "../../shared/routes/sessions";
import type { MessageReceipt } from "../../shared/routes/ownership";
import path from "node:path";
import { lockedWhile } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { alive } from "../../shared/platform";
import { takesForwarded } from "../work-queue/forward";
import { isWaitingID } from "../../shared/queues";
import { inline } from "../server/attached";
import { bridgeTarget, readBridgeFile, readEngineFile } from "../engine/client";
import { acquireSessionEngine } from "../engine/lifetime";
import { messageReached, queueOf } from "../engine/queue";
import { targetsOf } from "../letters/tools";
import { placeOfSessionPatiently } from "../letters/letter";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { peerSetupLists } from "../peers/lists";
import { pairedPhone } from "../phones";
import type { LinkHost } from "../relay/host";
import type { RouteContext } from "../server/context";
import { collected } from "../server/sent-once";
import { followedSetup } from "../setup/record";
import { K } from "../home";
import { liveLink, readLink, type LinkState } from "../state";
import { mailsFor } from "../work-queue/parts";
import { ownerOf, type Owner } from "./resolve";
import { readSubmitter, mutationSubmitter, submitterOf, SUBMITTER_FEATURE, SUBMITTER_REVOKED, SUBMITTER_UNKNOWN, type Submitter } from "./authority";
import { fingerprint } from "./merge";
import { MUTATION_FEATURE, mutationKey, mutationOutcome, runMutation } from "./mutation";
import { ownershipReady, pruneActiveOutbox, readRecord, readRecordAsync, recordKey, records, recordsAsync } from "./store";
import { readSubmissionID, sameReceiptAuthor, SUBMISSION_FEATURE, submissionIntent, submissionKey, SUBMIT_ROUTE } from "./submissions";
import { acceptsMutation } from "./admission";

const MAX_BODY = 128 * 1024;
const MAX_PENDING = 200;
// The peer mail envelope has its own base64 layer; leave headroom for forwarding metadata.
const MAX_PEER_DRAFT = 5 * 1024 * 1024;
export const DELIVERY_FEATURE = "typed.delivery";
export const DELIVERY_ROUTE = /^kuvo\/sessions\/(ses_[A-Za-z0-9_-]{1,95})\/delivery(?:\/(events|msg_[A-Za-z0-9_-]+)(?:\/(steer|now|ack))?)?$/;
const EVERY = 5_000;
const LOCK = path.join(K, "run", "owned-data", ".messages.lock");

interface Operation {
  setup: string;
  record: string;
  operation: string;
  owner: string;
  body: Record<string, unknown> & { id: string };
  draft?: true;
  delivery?: true;
  deliveryMode?: "steer" | "now";
  scoped?: true;
  submitter?: Submitter;
}
interface Outgoing {
  record: string;
  setup: string;
  digest: string;
  receipt: MessageReceipt;
  at: number;
  draft?: true;
  scoped?: true;
  submission?: string;
  queuedDelivery?: Operation;
  delivery?: true;
  removedDelivery?: { id: string; words: string; draft: string; files: unknown[]; receipt: string };
  removalToken?: string;
  removed?: true;
  /** Old keepers only read operation. They must not dispatch new device-aware work. */
  queuedMessage?: Operation;
  /** Distinct from prompts, so an older keeper can't send only the draft's text. */
  queuedDraft?: Operation;
  queuedSubmission?: Operation;
  /** Recovery bytes only. Neither current nor older keepers deliver this field. */
  parked?: Operation;
  operation?: Operation;
  submitter?: Submitter;
}

const sending = new Map<string, number>();
export const deliveryFollowers = new Set<() => void>();
let nudgeDelivery: (() => void) | undefined;
const changedDelivery = () => {
  for (const tell of deliveryFollowers) tell();
  nudgeDelivery?.();
};
const deliveringHere = new Set<string>();

const groupID = /^grp_[a-z0-9]{1,60}$/;
const sessionID = /^ses_[A-Za-z0-9_-]{1,95}$/;
const messageRecord = (id: string, computer?: string) => ({
  kind: groupID.test(id) ? "groupTranscript" as const : "conversations" as const, id, computer,
});

function prompt(value: unknown, group: boolean): Operation["body"] | undefined {
  if (!isObject(value) || !isWaitingID(value.id) || value.command !== undefined || value.delivery === "now") return;
  if (group ? typeof value.text !== "string" || value.files !== undefined
    : typeof value.words !== "string" && typeof value.text !== "string") return;
  if (Buffer.byteLength(JSON.stringify(value)) > MAX_BODY) return;
  if (value.files !== undefined && (!Array.isArray(value.files) || value.files.some(file => !isObject(file) || !inline(file)))) return;
  return { ...value, id: value.id };
}

/** Only ordinary draft prompts may use the conversation outbox. The owner still decides
 * current model readiness and performs the send through the full draft adapter. */
function draftPrompt(value: unknown, delivery = false, local = false): Operation["body"] | undefined {
  if (!isObject(value) || !isWaitingID(value.id) || typeof value.text !== "string" || !delivery && /^[\s]*[!/]/.test(value.text)
    || value.side === true || !isObject(value.message) || !delivery && value.message.delivery === "now"
    || (value.message.files !== undefined && (!Array.isArray(value.message.files)
      || value.message.files.some(file => !isObject(file) || !(local && typeof file.uri === "string" && file.uri.startsWith("file:")) && !inline(file))))
    || Buffer.byteLength(JSON.stringify(value)) > (local ? 140 * 1024 * 1024 : delivery ? MAX_PEER_DRAFT : MAX_BODY)) return undefined;
  return { ...value, id: value.id };
}

const legacyOperation = (setup: string, sender: string, record: string, id: string) => recordKey(setup, sender, record, id);
const operationOf = (setup: string, record: string, id: string) => mutationKey(setup, recordKey(record, id));

const deliveryOperation = (setup: string, record: string, id: string) => recordKey("delivery-v1", setup, record, id);
const draftDigest = (body: unknown, submitter: Submitter) => fingerprint({ draft: body, submitter });
const scopedDigest = (body: unknown, submitter: Submitter, through: string) => fingerprint(submissionIntent(body, submitter, through));
const outgoing = (operation: string) => readRecord<Outgoing>("message-receipts", operation) ?? readRecord<Outgoing>("outbox", operation);

const outgoingAsync = async (operation: string) => await readRecordAsync<Outgoing>("message-receipts", operation) ??
  await readRecordAsync<Outgoing>("outbox", operation);

function replyToDraft(json: RouteContext["json"], receipt: MessageReceipt, body: Record<string, unknown>) {
  if (receipt.status === "interrupted") return json(409, { message: receipt.message, ambiguous: true });
  if (receipt.status === "committed")
    return json(200, { answer: { send: { text: body.text, quoted: true } }, message: body.id, acceptedAt: receipt.acceptedAt });
  return json(202, receipt);
}

/** A retry through the same listing Box keeps its original receipt and destination, even
 * if the conversation is now on a different Box. */
export function queuedDraftOutcome(context: RouteContext, record: string, body: unknown) {
  if (!isObject(body) || !isWaitingID(body.id)) return;
  const link = readLink();
  const setup = followedSetup(link);
  if (!setup) return;
  const scoped = SUBMIT_ROUTE.test(context.route) && readSubmissionID(body.submission);
  if (scoped && (scoped.id !== body.id || scoped.setup !== setup.id)) return;
  const id = scoped ? submissionKey(setup.id, record, scoped.submission) : operationOf(setup.id, record, body.id);
  const prior = outgoing(id);
  if (!prior?.draft || prior.setup !== setup.id) return;
  const submitter = submitterOf(context, link, true);
  if (!submitter) return context.json(403, { message: "Open this from a paired Kuvo Code client." });
  if (prior.digest !== (scoped ? scopedDigest(body, submitter, ownKey(link ?? {}) ?? "") : draftDigest(body, submitter))) return context.json(409, { message: "That message id is already used." });
  return replyToDraft(context.json, prior.receipt, body);
}

export async function queueOwnerMessage(context: RouteContext, record: string, owner: Owner) {
  const { request, json } = context;
  const link = readLink();
  const submitter = submitterOf(context, link, true);
  if (!submitter) return json(403, { message: "Open this from a paired Kuvo Code client." });
  const setup = followedSetup(link);
  const sender = link && ownKey(link);
  if (!ownershipReady() || !setup || !sender || !owner.key || !peersOf(link).some(peer => peer.key === owner.key))
    return json(503, { message: `Send this message on ${owner.name}.` });
  const scoped = SUBMIT_ROUTE.test(context.route);
  const draft = scoped || context.route.endsWith("/draft");
  if (draft && !takesForwarded(owner.key, scoped ? SUBMISSION_FEATURE : "typed.draft"))
    return json(409, { message: `Open the conversation on ${owner.name} to submit this draft.` });
  const body = draft ? draftPrompt(parsed(request.body)) : prompt(parsed(request.body), groupID.test(record));
  if (!body) return json(400, { message: "Send a message with a stable id and inline attachments under 128 KiB total. Send commands and Send Now on the session’s computer." });
  const pin = scoped && readSubmissionID(body.submission);
  if (scoped && (!pin || pin.setup !== setup.id || pin.origin !== owner.key || pin.id !== body.id)) return json(409, { ambiguous: true, message: "Look up that submission on its original computer." });
  const legacy = legacyOperation(setup.id, sender, record, body.id);
  const current = operationOf(setup.id, record, body.id);
  const operation = pin ? submissionKey(setup.id, record, pin.submission) : draft || outgoing(current) ? current
    : readRecord<Outgoing>("outbox", legacy) || !takesForwarded(owner.key, MUTATION_FEATURE) ? legacy : current;
  const digest = scoped ? scopedDigest(body, submitter, sender) : draft ? draftDigest(body, submitter) : operation === legacy ? recordKey(JSON.stringify(body)) : fingerprint(body);
  const version = draft ? { draft: true as const, ...(scoped ? { scoped: true as const } : {}) } : {};
  const receipt = await lockedWhile(LOCK, async () => {
    const existing = outgoing(operation);
    if (existing) return existing.digest === digest ? existing.receipt : null;
    if (records("outbox").filter(key => outgoing(key)?.receipt.status === "pending").length >= MAX_PENDING) return;
    const receipt: MessageReceipt = { status: "pending", operation, owner: owner.key!, message: `Unsent. Delivered when ${owner.name} accepts it.` };
    const queued = { setup: setup.id, record, operation, owner: owner.key!, body, submitter, ...version };
    await keepReceipt("outbox", operation, { record, setup: setup.id, digest, receipt, at: Date.now(), submitter, ...version, ...(pin ? { submission: pin.submission } : {}),
      ...(scoped ? { queuedSubmission: queued } : draft ? { queuedDraft: queued } : { queuedMessage: queued }) });
    return receipt;
  }, { required: true });
  if (receipt === null) return json(409, { message: "That message id is already used." });
  if (!receipt) return json(503, { message: "Too many messages are waiting for their computers." });
  if (draft) return replyToDraft(json, receipt, body);
  return json(receipt.status === "committed" ? 200 : receipt.status === "pending" ? 202 : 409, receipt);
}

/** The client hands a stable draft to its listing box. The target need not be online. */
export async function saveDelivery(context: RouteContext, record: string, computer: string) {
  const { request, json } = context;
  const link = readLink();
  const setup = followedSetup(link)?.id ?? "";
  const submitter = submitterOf(context, link, true);
  if (!submitter) return json(403, { message: "Open this from a paired Kuvo Code client." });
  if (!ownershipReady()) return json(503, { message: "Wait for Kuvo Code to finish updating." });
  const local = computer === "here" || Boolean(link && computer === ownKey(link));
  const peer = local ? undefined : peersOf(link).find(each => each.key === computer);
  if (!local && (!setup || !peer)) return json(404, { message: "There’s no such computer in this setup." });
  const body = draftPrompt(parsed(request.body), true, local);
  if (!body) return json(400, { message: "Send a draft with its stable id, text and message fields. Attachments for another computer need to fit in a 5 MiB draft." });
  const owner = peer?.key ?? "here";
  const operation = deliveryOperation(setup, record, body.id);
  const digest = fingerprint({ owner, body, submitter });
  const prior = await outgoingAsync(operation);
  if (prior) return prior.digest === digest ? replyToDraft(json, prior.receipt, body) :
    json(409, { message: "That message id is already used." });
  if (peer) {
    const owner = ownerOf(messageRecord(record), { link });
    if (owner && owner.key !== peer.key) return json(409, { message: "The conversation’s owner changed." });
    // A just-started conversation may precede the peer's next list announcement. The addressed
    // box verifies the session before admitting the draft, so this box can save it immediately.
  }
  const receipt = await lockedWhile(LOCK, async () => {
    const saved = await outgoingAsync(operation);
    if (saved) return saved.digest === digest ? saved.receipt : null;
    const pending = await deliveries();
    if (pending.length >= MAX_PENDING) return;
    const at = Math.max(Date.now(), (pending.at(-1)?.record.at ?? 0) + 1);
    const receipt: MessageReceipt = { status: "pending", operation, owner, message: `Unsent. Delivered when ${peer?.name ?? context.info.name} accepts it.` };
    const queuedDelivery: Operation = { setup, record, operation, owner, body, submitter, draft: true, delivery: true };
    await keepReceipt("outbox", operation, { record, setup, at, digest, receipt, draft: true, delivery: true, submitter, queuedDelivery } satisfies Outgoing);
    changedDelivery();
    return receipt;
  }, { required: true });
  if (receipt === null) return json(409, { message: "That message id is already used." });
  if (!receipt) return json(503, { message: "Too many messages are waiting for their computers." });
  return replyToDraft(json, receipt, body);
}

async function outbox() {
  const keys = new Set(await recordsAsync("outbox"));
  pruneActiveOutbox(keys);
  const entries: Outgoing[] = [];
  for (const key of keys) {
    const record = await readRecordAsync<Outgoing>("outbox", key);
    if (record) entries.push(record);
  }
  return entries;
}

async function deliveries() {
  return (await outbox()).flatMap(record => record.receipt.status === "pending" && record.queuedDelivery
    ? [{ record, op: record.queuedDelivery }] : []).sort((a, b) => a.record.at - b.record.at);
}

function draftFiles(op: Operation) {
  const message = op.body.message;
  return isObject(message) && Array.isArray(message.files) ? message.files : [];
}

export async function deliveryItems(context: RouteContext, session: string, computer: string): Promise<QueueItem[]> {
  const link = readLink();
  const setup = followedSetup(link)?.id ?? "";
  const viewer = submitterOf(context, link, true);
  const owner = computer === "here" || link && ownKey(link) === computer ? "here" : computer;
  return (await deliveries()).filter(({ op, record }) => op.record === session && op.owner === owner && op.setup === setup &&
    viewer && sameReceiptAuthor(record.submitter, viewer)).map(({ op }) => ({
    id: op.body.id, words: typeof op.body.quotedText === "string" ? op.body.quotedText : String(op.body.text),
    files: draftFiles(op).flatMap(file => isObject(file) && typeof file.name === "string" ? [file.name] : []),
    from: null, on: "box", delivery: "queue", held: false,
  }));
}

export async function changeDelivery(context: RouteContext, session: string, computer: string, id: string, mode?: "steer" | "now" | "ack") {
  const link = readLink();
  const setup = followedSetup(link)?.id ?? "";
  const viewer = submitterOf(context, link, true);
  const owner = computer === "here" || link && ownKey(link) === computer ? "here" : computer;
  const key = deliveryOperation(setup, session, id);
  const query = new URLSearchParams(context.request.path.split("?")[1]);
  const token = query.get("receipt") ?? undefined;
  return lockedWhile(LOCK, async () => {
    const saved = await outgoingAsync(key);
    if (!saved?.delivery || saved.receipt.owner !== owner || !viewer || !sameReceiptAuthor(saved.submitter, viewer))
      return context.json(404, { message: "That message isn’t waiting here." });
    if (mode === "ack") {
      if (!token || token !== saved.removalToken) return context.json(404, { message: "There’s no such removal here." });
      const { removedDelivery: _, ...kept } = saved;
      if (saved.removedDelivery) await keepReceipt("message-receipts", key, kept);
      return context.json(200, {});
    }
    if (!mode && saved.removed && saved.removalToken === token)
      return context.json(200, { removed: saved.removedDelivery ?? null });
    if (saved.removed) return context.json(404, { message: "That message isn’t waiting here." });
    const op = saved.queuedDelivery;
    if (!op) return context.json(404, { message: "That message isn’t waiting here." });
    // A remote acceptance may already be on its way back. Removing still stops further attempts.
    if (deliveringHere.has(key)) return context.json(409, { message: "The message is being handed to the agent." });
    if (mode) {
      await keepReceipt("outbox", key, { ...saved, queuedDelivery: { ...op, deliveryMode: mode } });
      changedDelivery();
      return context.json(200, {});
    }
    const words = typeof op.body.quotedText === "string" ? op.body.quotedText : String(op.body.text);
    const removed = { id, draft: words, words, files: draftFiles(op), ...(token ? { receipt: token } : {}) };
    const { queuedDelivery: _, ...kept } = saved;
    await keepReceipt("message-receipts", key, { ...kept, removed: true, removalToken: token,
      ...(token ? { removedDelivery: removed } : {}),
      receipt: { status: "interrupted", operation: key, owner: op.owner, message: "The message was removed." } });
    await removeReceipt("outbox", key);
    changedDelivery();
    return context.json(200, { removed });
  }, { required: true });
}

export async function sendAddressedMessage(context: RouteContext) {
  const query = new URLSearchParams(context.request.path.split("?")[1]);
  const session = query.get("session");
  const group = query.get("group");
  const id = session && !group && sessionID.test(session) ? session : group && !session && groupID.test(group) ? group : null;
  const computer = query.get("computer");
  if (!id || !computer || !KEY.test(computer))
    return context.json(400, { message: "Name the conversation and its computer." });
  let owner = ownerOf(messageRecord(id, computer), { name: context.info.name });
  if (!owner && session) {
    // The merged list can name a session outside the bounded ownership announcement.
    const listed = (await peerSetupLists()).find(({ saved, origin }) =>
      origin.computer.id === computer && saved.sessions.some(row => row.id === id));
    if (listed) owner = { key: computer, name: listed.origin.computer.name, scope: "session computer", local: false, reachable: !listed.origin.away };
  }
  if (!owner || owner.local || owner.key !== computer)
    return context.json(409, { message: "Open the conversation on its current computer to send this message." });
  return queueOwnerMessage(context, id, owner);
}

export async function readMessageReceipt(context: RouteContext, id: string) {
  const link = readLink();
  const record = KEY.test(id) ? await outgoingAsync(id) : undefined;
  const viewer = submitterOf(context, link, true);
  if (!record || record.setup !== (followedSetup(link)?.id ?? "") ||
      (record.scoped || record.delivery) && (!viewer || !sameReceiptAuthor(record.submitter, viewer))) return;
  return { ...record.receipt, ...(record.submission ? { submission: record.submission, session: record.record, digest: record.digest } : {}) };
}

export async function messageReceipt(context: RouteContext, id: string) {
  const receipt = await readMessageReceipt(context, id);
  return receipt ? context.json(200, receipt) : context.json(404, { message: "There’s no such message operation here." });
}

export function listingSubmissions(setup: string, session: string, viewer: Submitter, submission: string | undefined, after: string,
  candidates: readonly string[], local = true) {
  if (!local && viewer.kind === "computer") return [];
  const keys = submission ? [submissionKey(setup, session, submission)] : candidates;
  return Array.from(keys).flatMap(key => {
    const record = outgoing(key);
    const id = readSubmissionID(record?.submission);
    if (!record?.scoped || !id || record.setup !== setup || record.record !== session || id.submission <= after
        || !sameReceiptAuthor(record.submitter, viewer)) return [];
    const receipt = record.receipt;
    const draft = record.queuedSubmission?.body ?? record.parked?.body;
    return [{ submission: id.submission, originalOwner: id.origin, digest: record.digest, submitter: record.submitter, evidence: "listing",
      status: receipt.status === "interrupted" ? "unknown" : receipt.status,
      ...(receipt.status === "committed" ? { acceptedAt: receipt.acceptedAt, effect: { message: id.id } } : {}),
      ...(draft ? { draftAvailable: true, ...(submission ? { draft } : {}) } : {}) }];
  });
}

/** An authenticated read from the original authority is also sufficient to settle the matching
 * local receipt. A listing report cannot manufacture an owner acceptance. */
export async function settleListingOutcome(setup: string, session: string, outcome: Record<string, unknown>) {
  const handle = readSubmissionID(outcome.submission);
  if (!handle || outcome.evidence !== "owner" || outcome.status !== "committed" || outcome.conflict === true
    || typeof outcome.acceptedAt !== "number" || !Number.isSafeInteger(outcome.acceptedAt)) return;
  const operation = submissionKey(setup, session, handle.submission);
  const record = outgoing(operation);
  if (!record?.scoped || record.digest !== outcome.digest || record.receipt.owner !== handle.origin) return;
  await updateReceipt(record, { status: "committed", operation, owner: handle.origin, acceptedAt: outcome.acceptedAt });
}

async function updateReceipt(record: Outgoing, receipt: MessageReceipt) {
  const key = receipt.operation;
  await lockedWhile(LOCK, async () => {
    const current = await outgoingAsync(key);
    if (!current || current.setup !== record.setup || current.digest !== record.digest || current.receipt.status === "committed"
      || current.removed || (current.receipt.status === "interrupted" && receipt.status !== "committed")) return;
    const { queuedSubmission, queuedDraft, queuedMessage, queuedDelivery, operation, parked, ...saved } = current;
    const recovery = receipt.status === "interrupted" ? queuedSubmission ?? queuedDraft ?? parked : undefined;
    await keepReceipt(current.scoped || current.delivery ? "message-receipts" : "outbox", key, { ...saved, receipt, ...(recovery ? { parked: recovery } : {}) });
    // Publish the terminal receipt before dropping the active copy. The sender consults both
    // files after a cut, so a leftover active file cannot cause a replay.
    if (current.scoped || current.delivery) await removeReceipt("outbox", key);
    if (queuedDelivery) changedDelivery();
  }, { required: true });
}

async function flushOwnerMessages(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, link: LinkState | null, now: number): Promise<void> {
  const setup = followedSetup(link);
  if (!ownershipReady()) return;
  const pending: { record: Outgoing; op: Operation }[] = [];
  for (const record of await outbox()) {
    const key = record.receipt?.operation;
    if (!key) continue;
    if ((record.scoped || record.delivery) && await readRecordAsync<Outgoing>("message-receipts", key)) {
      // Repair the interrupted compaction. The terminal fence is durable and never removed.
      await lockedWhile(LOCK, () => removeReceipt("outbox", key), { required: true });
      continue;
    }
    const op = record?.queuedDelivery ?? record?.queuedSubmission ?? record?.queuedDraft ?? record?.queuedMessage ?? record?.operation;
    if (record?.receipt.status === "pending" && op && record.setup === (setup?.id ?? "")) pending.push({ record, op });
  }
  pending.sort((a, b) => a.record.at - b.record.at);
  const remaining = new Set(pending.map(({ op }) => op.operation));
  for (const key of sending.keys()) if (!remaining.has(key)) sending.delete(key);
  const tried = new Set<string>();
  for (const { record, op } of pending) {
    if (tried.has(op.record)) continue;
    if (op.delivery && op.owner === "here") {
      tried.add(op.record);
      if (!deliveringHere.has(op.operation) && now - (sending.get(op.operation) ?? -Infinity) >= EVERY) {
        sending.set(op.operation, now);
        deliveringHere.add(op.operation);
        const run = lockedWhile(LOCK, async () => {
          const current = await outgoingAsync(op.operation);
          return current?.queuedDelivery;
        }, { required: true }).then(current => current && deliverLocal(record, current));
        void run.catch(error => console.error("Local draft delivery:", error))
          .finally(() => { deliveringHere.delete(op.operation); });
      }
      continue;
    }
    if (!host?.welcomed || !setup || !link) continue;
    const peer = peersOf(link).find(each => each.key === op.owner);
    const owner = ownerOf(messageRecord(op.record, op.owner), { link });
    if (!peer || (owner && owner.key !== op.owner)) {
      await updateReceipt(record, { status: "interrupted", operation: op.operation, owner: op.owner, message: "The conversation’s owner changed. Check the conversation before sending again." });
      continue;
    }
    const current = await outgoingAsync(op.operation);
    if (current?.receipt.status !== "pending") continue;
    if (op.delivery && !takesForwarded(peer.key, DELIVERY_FEATURE)) {
      tried.add(op.record);
      continue;
    }
    const authority = mutationSubmitter("queuedSessionWork", link, op.submitter, host);
    if (authority === "checking" || op.draft && !takesForwarded(peer.key, op.scoped ? SUBMISSION_FEATURE : "typed.draft") || tried.has(op.record)
      || !takesForwarded(peer.key, SUBMITTER_FEATURE)) continue;
    tried.add(op.record);
    if (now - (sending.get(op.operation) ?? -Infinity) < EVERY) continue;
    const field = op.scoped ? "scopedSubmission" : op.draft ? "scopedOwnerDraft" : "scopedOwnerMessage";
    const mails = mailsFor(peer, { [field]: { ...(current.queuedDelivery ?? op), ...(authority === "revoked" ? { replayOnly: true } : {}) } }, link, now);
    if (!mails) continue;
    for (const bytes of mails) host.send(bytes);
    sending.set(op.operation, now);
  }
}

let flushing: Promise<void> | undefined;
export function runLocalDelivery(signal?: AbortSignal) {
  const round = () => {
    if (!signal?.aborted) void sendOwnerMessages(liveLink(), readLink(), Date.now());
  };
  let queued = false;
  const nudge = () => {
    if (queued) return;
    queued = true;
    setImmediate(() => { queued = false; round(); }).unref();
  };
  nudgeDelivery = nudge;
  const timer = setInterval(round, EVERY);
  timer.unref?.();
  signal?.addEventListener("abort", () => {
    clearInterval(timer);
    if (nudgeDelivery === nudge) nudgeDelivery = undefined;
  }, { once: true });
  nudge();
}

export function sendOwnerMessages(...args: Parameters<typeof flushOwnerMessages>): Promise<void> {
  flushing ??= flushOwnerMessages(...args).catch(error => console.error("Owner messages:", error))
    .finally(() => { flushing = undefined; });
  return flushing;
}

export async function takeMessageReceipt(sender: MailSender, given: Record<string, unknown>, draft = false, scoped = false) {
  const { operation, owner, status, acceptedAt, message } = given;
  if (typeof operation !== "string" || !KEY.test(operation) || owner !== sender.key) return;
  const record = await outgoingAsync(operation);
  if (!record || Boolean(record.draft) !== draft || Boolean(record.scoped) !== scoped
    || scoped && given.digest !== record.digest || record.receipt.owner !== owner || record.setup !== followedSetup(readLink())?.id) return;
  if (status === "committed" && typeof acceptedAt === "number" && Number.isSafeInteger(acceptedAt))
    await updateReceipt(record, { status, operation, owner, acceptedAt });
  if (status === "interrupted" && typeof message === "string")
    await updateReceipt(record, { status, operation, owner, message: message.slice(0, 500) });
}

interface Accepted {
  digest: string;
  receipt: MessageReceipt;
  claimant?: number;
}

async function deliverMessage(context: RouteContext, record: string, setup: string, body: Operation["body"]) {
  const { groupsRoute } = await import("../areas/groups");
  const { sendRoute, SEND_ROUTE } = await import("../areas/messages");
  const group = groupID.test(record);
  const recovering = group ? undefined : async () => {
    if (queueOf(record, true).waiting?.some(item => item.id === body.id))
      return { status: 200, value: { waits: true } };
    const release = await acquireSessionEngine(record);
    try {
      const place = await placeOfSessionPatiently(record, targetsOf(context));
      if (!place) return null;
      return await messageReached(place.target, record, body.id)
        ? { status: 200, value: {} } : "resume" as const;
    } finally {
      release?.();
    }
  };
  const stillChecking = group ? undefined : () => ({
    status: 503, value: { message: "The engine isn't answering. Try again in a moment." }, notAdmitted: true as const,
  });
  return runMutation(operationOf(setup, record, body.id), { record, body }, async () => {
    const veto = context.admitQueued?.();
    if (veto) return veto;
    const answer = await collected(group ? groupsRoute(context) : sendRoute(context, context.route.match(SEND_ROUTE)!));
    return { status: answer.status, value: parsed(answer.body) ?? {}, notAdmitted: answer.notAdmitted };
  }, () => context.admitQueued?.() ?? null, stillChecking, undefined, undefined, undefined, recovering);
}

/** Direct submission and relayed owner mail use the same acceptance record. */
export async function ownerMessageHere(context: RouteContext, record: string) {
  const setup = followedSetup(readLink());
  const body = prompt(parsed(context.request.body), groupID.test(record));
  if (!setup || !body || !acceptsMutation("queuedSessionWork", messageRecord(record))) return null;
  const answer = await deliverMessage(context, record, setup.id, body);
  return context.json(answer.status, answer.value);
}

export async function takeOwnerMessage(sender: MailSender, given: Record<string, unknown>, reply: (answer: Record<string, unknown>) => void, host?: Pick<LinkHost, "send"> | null, format: "legacy" | "scoped" | "draft" | "submission" = "legacy"): Promise<void> {
  if (!supportsLegacyObjectVersion(given)) return;
  if (!ownershipReady()) return;
  const link = readLink();
  const setup = followedSetup(link);
  const key = link && ownKey(link);
  const { record, operation, owner } = given;
  if (typeof record !== "string" || (!sessionID.test(record) && !groupID.test(record))) return;
  const group = groupID.test(record);
  const scoped = format !== "legacy";
  const submission = format === "submission";
  const draft = format === "draft" || submission;
  if (draft !== (given.draft === true) || submission !== (given.scoped === true) || draft && group) return;
  const durable = draft && given.delivery === true;
  const body = draft ? draftPrompt(given.body, durable) : prompt(given.body, group);
  const submitter = scoped ? readSubmitter(given.submitter) : undefined;
  if (scoped && (given.submitter !== undefined || draft) && !submitter) return;
  if (!setup || setup.id !== given.setup || !key || owner !== key || !peersOf(link).some(peer => peer.key === sender.key) || !body) return;
  const legacy = legacyOperation(setup.id, sender.key, record, body.id);
  const pin = submission && readSubmissionID(body.submission);
  if (submission ? !pin || pin.setup !== setup.id || pin.id !== body.id || pin.origin !== key || operation !== submissionKey(setup.id, record, pin.submission)
    : durable ? operation !== deliveryOperation(setup.id, record, body.id)
    : operation !== operationOf(setup.id, record, body.id) && operation !== legacy) return;
  const id = operation as string;
  const sendReceipt = (receipt: MessageReceipt) => reply(submission && submitter ? { submissionReceipt: { ...receipt, digest: scopedDigest(body, submitter, sender.key) } }
    : draft ? { ownerDraftReceipt: receipt } : { ownerMessageReceipt: receipt });
  const refused = (message: string): MessageReceipt => ({ status: "interrupted", operation: id, owner: key, message });
  const received = (value: unknown, status: number, acceptedAt?: number) => status >= 200 && status < 300
    ? { status: "committed" as const, operation: id, owner: key, acceptedAt: acceptedAt ?? Date.now() }
    : refused(isObject(value) && typeof value.message === "string" ? value.message : "The owner couldn’t confirm acceptance. Check the conversation before sending again.");
  const earlier = !draft && readRecord<Accepted>("accepted", legacy);
  if (earlier) {
    if (earlier.digest !== recordKey(JSON.stringify(body))) return;
    if (earlier.receipt.status === "pending" && earlier.claimant && alive(earlier.claimant)) return;
    return sendReceipt(earlier.receipt.status === "pending"
      ? refused("The owner restarted before recording acceptance. Check the conversation before sending again.")
      : { ...earlier.receipt, operation: id });
  }
  const decided = !draft && mutationOutcome(operationOf(setup.id, record, body.id), { record, body });
  if (decided) return sendReceipt(received(decided.value, decided.status, decided.acceptedAt));
  const bridge = () => bridgeTarget() ? readBridgeFile() : null;
  const admitQueued: NonNullable<RouteContext["admitQueued"]> = () => {
    const current = readLink();
    if (followedSetup(current)?.id !== setup.id || !peersOf(current).some(peer => peer.key === sender.key)
      || !acceptsMutation("queuedSessionWork", messageRecord(record), current))
      return { status: 409, value: { message: "The owner or the submitting computer changed." } };
    const authority = given.replayOnly === true ? "revoked" : mutationSubmitter("queuedSessionWork", current, submitter, host);
    if (authority === "checking") return { status: 503, value: { message: "Checking the submitting device’s access." }, notAdmitted: true };
    if (authority !== "allowed") return { status: 409, value: { message: authority === "unknown" ? SUBMITTER_UNKNOWN : SUBMITTER_REVOKED } };
    return null;
  };
  const request = { method: "POST", ...(draft ? { fromOwnerPeer: true as const } : {}), path: group ? `kuvo/groups/${record}` : `kuvo/sessions/${record}/${submission ? "submit" : draft ? "draft" : "send"}`, headers: {}, body: Buffer.from(JSON.stringify(body)), desktop: submitter?.kind !== "paired" || pairedPhone(link, submitter.key, submitter.pairing)?.kind === "desktop" };
  const json: RouteContext["json"] = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const context: RouteContext = { request, route: request.path, phone: undefined, signal: undefined, bridge, readBridge: readBridgeFile,
    readEngine: readEngineFile, info: { name: "" }, hold: null, phonesChanged: () => {}, json, text: json, admitQueued,
    durableDelivery: durable, deliveryMode: durable && (given.deliveryMode === "steer" || given.deliveryMode === "now") ? given.deliveryMode : undefined };
  if (draft && submitter) {
    // The draft ledger itself is the claim. A second enclosing claim would hide a
    // committed draft if the keeper stopped before persisting the enclosing answer.
    const { draftRoute, DRAFT_ROUTE } = await import("../areas/typed");
    const answer = await collected(draftRoute(context, context.route.match(submission ? SUBMIT_ROUTE : DRAFT_ROUTE)!, submitter, sender.key));
    if (answer.notAdmitted) return;
    const value = parsed(answer.body);
    if (durable && (answer.status >= 500 || isObject(value) && (value.ambiguous === true || isObject(value.answer) && value.answer.ambiguous === true))) return;
    const delivered = answer.status === 200 && isObject(value) && value.message === body.id;
    const decision = isObject(value) && isObject(value.answer) ? value.answer : value;
    return sendReceipt(delivered ? received(value, 200, isObject(value) && typeof value.acceptedAt === "number" ? value.acceptedAt : undefined) : refused(isObject(decision) && typeof (decision.error ?? decision.message) === "string"
      ? String(decision.error ?? decision.message) : "The owner did not send this draft. Check the conversation before sending again."));
  }
  const { listedGroups } = await import("../areas/groups");
  const target = group ? bridge() : null;
  const present = group ? target && (await listedGroups(target, AbortSignal.timeout(5_000)))?.some(chat => chat.id === record)
    : await placeOfSessionPatiently(record, targetsOf(context));
  const current = readLink();
  if (!present || followedSetup(current)?.id !== setup.id || !peersOf(current).some(peer => peer.key === sender.key)
    || !acceptsMutation("queuedSessionWork", messageRecord(record), current)) return;
  const answer = await deliverMessage(context, record, setup.id, body);
  if (answer.notAdmitted) return; // The sender still has this operation and will deliver it again.
  sendReceipt(received(answer.value, answer.status, answer.acceptedAt));
}

async function deliverLocal(record: Outgoing, op: Operation) {
  const { draftRoute, DRAFT_ROUTE } = await import("../areas/typed");
  const path = `kuvo/sessions/${op.record}/draft`;
  const request = { method: "POST", path, headers: {}, body: Buffer.from(JSON.stringify(op.body)), fromApp: true, fromOwnerPeer: true as const };
  const json: RouteContext["json"] = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const context: RouteContext = {
    request, route: path, phone: undefined, signal: undefined, bridge: () => bridgeTarget() ? readBridgeFile() : null,
    readBridge: readBridgeFile, readEngine: readEngineFile, info: { name: "" }, hold: null, phonesChanged: () => {},
    json, text: json, durableDelivery: true, deliveryMode: op.deliveryMode,
  };
  const answer = await collected(draftRoute(context, path.match(DRAFT_ROUTE)!, op.submitter));
  const value = parsed(answer.body);
  if (answer.notAdmitted || answer.status >= 500 || isObject(value) && value.ambiguous === true) return;
  const receipt = { operation: op.operation, owner: op.owner };
  if (answer.status === 200 && isObject(value) && value.message === op.body.id)
    return updateReceipt(record, { ...receipt, status: "committed", acceptedAt: typeof value.acceptedAt === "number" ? value.acceptedAt : Date.now() });
  const decision = isObject(value) && isObject(value.answer) ? value.answer : value;
  await updateReceipt(record, { ...receipt, status: "interrupted",
    message: isObject(decision) && typeof (decision.error ?? decision.message) === "string" ? String(decision.error ?? decision.message) : "The message could not be sent." });
}
