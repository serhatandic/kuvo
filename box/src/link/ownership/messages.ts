import type { MessageReceipt } from "../../shared/routes/ownership";
import path from "node:path";
import { locked } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { alive } from "../../shared/platform";
import { isWaitingID } from "../../shared/queues";
import { inline } from "../server/attached";
import { bridgeTarget, readBridgeFile, readEngineFile } from "../engine/client";
import { targetsOf } from "../letters/tools";
import { placeOfSessionPatiently } from "../letters/letter";
import { awaySessions, frozen } from "../moves/records";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import type { LinkHost } from "../relay/host";
import type { RouteContext } from "../server/context";
import { collected } from "../server/sent-once";
import { followedSetup } from "../setup/record";
import { K } from "../home";
import { readLink, type LinkState } from "../state";
import { mailsFor } from "../work-queue/parts";
import { ownerOf, type Owner } from "./resolve";
import { keepRecord, ownershipReady, readRecord, recordKey, records } from "./store";

const MAX_BODY = 128 * 1024;
const MAX_PENDING = 200;
const EVERY = 5_000;
const LOCK = path.join(K, "run", "owned-data", ".messages.lock");

interface Operation {
  setup: string;
  record: string;
  operation: string;
  owner: string;
  body: Record<string, unknown> & { id: string };
}
interface Outgoing {
  record: string;
  setup: string;
  digest: string;
  receipt: MessageReceipt;
  at: number;
  operation?: Operation;
}

const sending = new Map<string, number>();
const accepting = new Set<string>();

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

const operationOf = (setup: string, sender: string, record: string, id: string) => recordKey(setup, sender, record, id);

export function queueOwnerMessage(context: RouteContext, record: string, owner: Owner) {
  const { request, json } = context;
  const link = readLink();
  const setup = followedSetup(link);
  const sender = link && ownKey(link);
  if (!ownershipReady() || !setup || !sender || !owner.key || !peersOf(link).some(peer => peer.key === owner.key))
    return json(503, { message: `Send this message on ${owner.name}.` });
  const body = prompt(parsed(request.body), groupID.test(record));
  if (!body) return json(400, { message: "Send a message with a stable id and inline attachments under 128 KiB total. Send commands and Send Now on the session’s computer." });
  const operation = operationOf(setup.id, sender, record, body.id);
  const digest = recordKey(JSON.stringify(body));
  const receipt = locked(LOCK, () => {
    const existing = readRecord<Outgoing>("outbox", operation);
    if (existing) return existing.digest === digest ? existing.receipt : null;
    if (records("outbox").filter(key => readRecord<Outgoing>("outbox", key)?.receipt.status === "pending").length >= MAX_PENDING) return;
    const receipt: MessageReceipt = { status: "pending", operation, owner: owner.key!, message: `Unsent. Delivered when ${owner.name} accepts it.` };
    keepRecord("outbox", operation, { record, setup: setup.id, digest, receipt, at: Date.now(), operation: { setup: setup.id, record, operation, owner: owner.key!, body } });
    return receipt;
  }, { required: true });
  if (receipt === null) return json(409, { message: "That message id is already used." });
  if (!receipt) return json(503, { message: "Too many messages are waiting for their computers." });
  return json(receipt.status === "committed" ? 200 : receipt.status === "pending" ? 202 : 409, receipt);
}

export function sendAddressedMessage(context: RouteContext) {
  const query = new URLSearchParams(context.request.path.split("?")[1]);
  const session = query.get("session");
  const group = query.get("group");
  const id = session && !group && sessionID.test(session) ? session : group && !session && groupID.test(group) ? group : null;
  const computer = query.get("computer");
  if (!id || !computer || !KEY.test(computer))
    return context.json(400, { message: "Name the conversation and its computer." });
  const owner = ownerOf(messageRecord(id, computer), { name: context.info.name });
  if (!owner || owner.local || owner.key !== computer)
    return context.json(409, { message: "Open the conversation on its current computer to send this message." });
  return queueOwnerMessage(context, id, owner);
}

export function messageReceipt(context: RouteContext, id: string) {
  const link = readLink();
  const record = KEY.test(id) ? readRecord<Outgoing>("outbox", id) : undefined;
  return !record || record.setup !== followedSetup(link)?.id
    ? context.json(404, { message: "There’s no such message operation here." })
    : context.json(200, record.receipt);
}

function updateReceipt(record: Outgoing, receipt: MessageReceipt) {
  const key = receipt.operation;
  locked(LOCK, () => {
    const current = readRecord<Outgoing>("outbox", key);
    if (current?.receipt.status !== "pending" || current.setup !== record.setup) return;
    keepRecord("outbox", key, { record: record.record, setup: record.setup, digest: record.digest, at: record.at, receipt });
  }, { required: true });
}

export function sendOwnerMessages(host: Pick<LinkHost, "send" | "welcomed"> | null | undefined, link: LinkState | null, now: number): void {
  const setup = followedSetup(link);
  if (!host?.welcomed || !setup || !link) return;
  const pending = records("outbox").flatMap(key => {
    const record = readRecord<Outgoing>("outbox", key);
    return record?.operation && record.setup === setup.id ? [record] : [];
  }).sort((a, b) => a.at - b.at);
  const remaining = new Set(pending.map(record => record.receipt.operation));
  for (const key of sending.keys()) if (!remaining.has(key)) sending.delete(key);
  const tried = new Set<string>();
  for (const record of pending) {
    const op = record.operation!;
    const peer = peersOf(link).find(each => each.key === op.owner);
    const owner = ownerOf(messageRecord(op.record, op.owner), { link });
    if (!peer || (owner && owner.key !== op.owner)) {
      updateReceipt(record, { status: "interrupted", operation: op.operation, owner: op.owner, message: "The conversation’s owner changed. Check the conversation before sending again." });
      continue;
    }
    if (tried.has(op.record)) continue;
    tried.add(op.record);
    if (now - (sending.get(op.operation) ?? -Infinity) < EVERY) continue;
    const mails = mailsFor(peer, { ownerMessage: op }, link, now);
    if (!mails) continue;
    for (const bytes of mails) host.send(bytes);
    sending.set(op.operation, now);
  }
}

export function takeMessageReceipt(sender: MailSender, given: Record<string, unknown>) {
  const { operation, owner, status, acceptedAt, message } = given;
  if (typeof operation !== "string" || !KEY.test(operation) || owner !== sender.key) return;
  const record = readRecord<Outgoing>("outbox", operation);
  if (!record || record.receipt.owner !== owner || record.setup !== followedSetup(readLink())?.id) return;
  if (status === "committed" && typeof acceptedAt === "number" && Number.isSafeInteger(acceptedAt))
    updateReceipt(record, { status, operation, owner, acceptedAt });
  if (status === "interrupted" && typeof message === "string")
    updateReceipt(record, { status, operation, owner, message: message.slice(0, 500) });
}

interface Accepted {
  setup: string;
  record: string;
  digest: string;
  receipt: MessageReceipt;
  claimant?: number;
}

export async function takeOwnerMessage(sender: MailSender, given: Record<string, unknown>, reply: (answer: Record<string, unknown>) => void): Promise<void> {
  if (!ownershipReady()) return;
  const link = readLink();
  const setup = followedSetup(link);
  const key = link && ownKey(link);
  const { record, operation, owner } = given;
  if (typeof record !== "string" || (!sessionID.test(record) && !groupID.test(record))) return;
  const group = groupID.test(record);
  const body = prompt(given.body, group);
  if (!setup || setup.id !== given.setup || !key || owner !== key || !peersOf(link).some(peer => peer.key === sender.key)
    || !body || operation !== operationOf(setup.id, sender.key, record, body.id)) return;
  const id = operation as string;
  const digest = recordKey(JSON.stringify(body));
  const sendReceipt = (receipt: MessageReceipt) => reply({ ownerMessageReceipt: receipt });
  const refused = (message: string): MessageReceipt => ({ status: "interrupted", operation: id, owner: key, message });
  if (accepting.has(id)) return;
  const replay = (earlier: Accepted) => {
    if (earlier.digest !== digest) return;
    if (earlier.receipt.status === "pending" && earlier.claimant && earlier.claimant !== process.pid && alive(earlier.claimant)) return;
    return sendReceipt(earlier.receipt.status === "pending"
      ? refused("The owner restarted before recording acceptance. Check the conversation before sending again.")
      : earlier.receipt);
  };
  const earlier = readRecord<Accepted>("accepted", id);
  if (earlier) return replay(earlier);
  if (frozen().has(record) || awaySessions().includes(record)) return;
  accepting.add(id);
  try {
    const bridge = () => {
      const target = bridgeTarget();
      return target ? readBridgeFile() : null;
    };
    const request = { method: "POST", path: group ? `kuvo/groups/${record}` : `kuvo/sessions/${record}/send`, headers: {}, body: Buffer.from(JSON.stringify(body)), desktop: true };
    const json = (status: number, value: unknown) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
    const context: RouteContext = { request, route: request.path, phone: undefined, signal: undefined, bridge, readBridge: readBridgeFile,
      readEngine: readEngineFile, info: { name: "" }, hold: null, phonesChanged: () => {}, json, text: json };
    const { groupsRoute, listedGroups } = await import("../areas/groups");
    const { sendRoute, SEND_ROUTE } = await import("../areas/messages");
    const target = group ? bridge() : null;
    const present = group ? target && (await listedGroups(target, AbortSignal.timeout(5_000)))?.some(chat => chat.id === record)
      : await placeOfSessionPatiently(record, targetsOf(context));
    if (!present || frozen().has(record) || awaySessions().includes(record)) return;
    const current = readLink();
    if (followedSetup(current)?.id !== setup.id || !peersOf(current).some(peer => peer.key === sender.key)
      || !ownerOf(messageRecord(record), { link: current, localRecord: true })?.local) return;
    // A crash after handing the prompt to the engine can leave an unknown outcome. Keep
    // the claim first so retries cannot run it twice, even if the engine discarded its ID.
    const save = (receipt: MessageReceipt) => keepRecord("accepted", id, {
      setup: setup.id, record, digest, receipt, ...(receipt.status === "pending" ? { claimant: process.pid } : {}),
    });
    const claimed = locked(LOCK, () => {
      const previous = readRecord<Accepted>("accepted", id);
      if (!previous) save({ status: "pending", operation: id, owner: key, message: "Checking acceptance." });
      return previous;
    }, { required: true });
    if (claimed) return replay(claimed);
    const answer = await collected(group ? groupsRoute(context) : sendRoute(context, request.path.match(SEND_ROUTE)!));
    const error = parsed(answer.body);
    const receipt: MessageReceipt = answer.status >= 200 && answer.status < 300
      ? { status: "committed", operation: id, owner: key, acceptedAt: Date.now() }
      : refused(isObject(error) && typeof error.message === "string" ? error.message : "The owner couldn’t confirm acceptance. Check the conversation before sending again.");
    save(receipt);
    sendReceipt(receipt);
  } finally {
    accepting.delete(id);
  }
}
