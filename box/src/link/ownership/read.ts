import { dataKind, dataOwner, type DataKind } from "../../shared/data-declarations";
import { isObject, parsed } from "../../shared/json";
import { OWNER } from "../../shared/work-queue-owner";
import { forwardGroups } from "../areas/groups";
import { QUEUE_ROUTE, queueRoute } from "../areas/queue";
import { SESSION_ASKS_ROUTE, sessionAsksRoute } from "../areas/session-asks";
import { answerWakes } from "../areas/wakes";
import { listBots } from "../bots/list";
import { asked, bridgeTarget, readBridgeFile, readEngineFile, type PeerPlaces } from "../engine/client";
import { placeOfSession } from "../letters/letter";
import { targetsOf } from "../letters/tools";
import { awaySessions } from "../moves/records";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import type { RouteContext } from "../server/context";
import { collected } from "../server/sent-once";
import { followedSetup } from "../setup/record";
import { sharedValue } from "../setup/shared";
import { readLink } from "../state";
import { goalPlans } from "../work/goals";
import { actHere } from "../work-queue/acts";
import { itemElsewhere } from "../work-queue/across";
import { allItems, matching } from "../work-queue/store";
import { forwardAct, type Act, type Acted } from "../work-queue/forward";
import { DATA_ID, ownerOf, type DataReference, type Owner } from "./resolve";
import { keepRecord, readRecord, recordKey, ownershipReady } from "./store";

const MAX_READ = 512 * 1024;
const primaryReads = ["setupNames", "setupTimeZone", "setupPersonalities", "setupUsageDisplay", "queueLabels"] as const;
type PrimaryRead = typeof primaryReads[number];
const isPrimaryRead = (kind: DataKind): kind is PrimaryRead => (primaryReads as readonly DataKind[]).includes(kind);

interface Snapshot {
  at: number;
  value?: unknown;
  absent?: true;
  files: string[];
  messages: string[];
}
interface ReadRequest extends DataReference {
  offset?: string;
}

function requestOf(kind: unknown, id: unknown, offset: unknown): ReadRequest | null {
  if (!dataKind(kind) || typeof id !== "string" || (id !== "_" && !DATA_ID.test(id))) return null;
  if (offset !== undefined && (typeof offset !== "string" || !/^[A-Za-z0-9_:-]{1,200}$/.test(offset) || kind !== "conversations")) return null;
  if (isPrimaryRead(kind)) return id === "_" ? { kind, id } : null;
  switch (kind) {
    case "conversations": case "sessionFiles": case "sessionQueue": case "sessionSchedules": case "pendingSessionWork": case "sessionGoals":
      return /^ses_[A-Za-z0-9_-]+$/.test(id) ? { kind, id, ...(offset ? { offset: offset as string } : {}) } : null;
    case "groupTranscript": case "groupSettings":
      return /^grp_[a-z0-9]+$/.test(id) ? { kind, id } : null;
    case "queueItems": case "queueFiles": case "queueNotes": case "queueHistoryAndDelivery":
    case "botSettings":
      return id !== "_" ? { kind, id } : null;
    default:
      return null;
  }
}

function fileNames(value: unknown): string[] {
  const names = new Set<string>();
  const add = (file: unknown) => {
    if (typeof file === "string") names.add(file.slice(0, 500));
    else if (isObject(file)) {
      const label = file.name ?? file.filename ?? file.path ?? file.id;
      if (typeof label === "string" && label) names.add(label.slice(0, 500));
    }
  };
  for (const msg of Array.isArray(value) ? value : []) {
    if (!isObject(msg)) continue;
    if (Array.isArray(msg.files)) msg.files.forEach(add);
    for (const part of Array.isArray(msg.content) ? msg.content : []) {
      if (isObject(part) && ["file", "image", "attachment"].includes(String(part.type))) add(part);
    }
  }
  return [...names];
}

function snapshot(value: unknown, kind: DataKind): Pick<Snapshot, "value" | "files" | "messages"> {
  const rows = Array.isArray(value) ? value : isObject(value) && Array.isArray(value.messages) ? value.messages : [];
  return {
    value, files: kind === "sessionFiles" ? value as string[] : fileNames(isObject(value) && isObject(value.item) ? [value.item] : rows),
    messages: rows.flatMap(row => isObject(row) && typeof row.id === "string" ? [row.id] : []),
  };
}

async function localRead(ref: ReadRequest, context: RouteContext, places: PeerPlaces, computer: string | null): Promise<Acted> {
  const kind = ref.kind;
  const id = ref.id!;
  const link = readLink();
  const me = () => ownerOf({ ...ref, computer: computer ?? undefined }, { link: readLink(), localRecord: true })?.local;
  if (isPrimaryRead(kind)) {
    if (!me()) return { status: 409, value: null };
    const value = sharedValue(link, kind);
    return value === undefined ? { status: 404, value: null } : { status: 200, value };
  }
  if (dataOwner(kind) === undefined) {
    const item = matching(allItems(), { id })[0];
    if (!item || item.removed) return { status: 404, value: null };
    if (!ownerOf({ ...ref, queue: item.owner }, { link, localRecord: true })?.local)
      return { status: 409, value: null };
    return actHere({ method: "GET", ref: { id }, targets: places.targets, signal: context.signal });
  }
  if (awaySessions().includes(id) || !me()) return { status: 409, value: null };
  if (kind === "botSettings") {
    const bot = listBots().find(bot => bot.id === id);
    return bot ? { status: 200, value: bot } : { status: 404, value: null };
  }
  if (kind === "groupTranscript" || kind === "groupSettings") {
    const bridge = context.bridge();
    if (!bridge) return { status: 503, value: null };
    const route = `kuvo/groups/${id}`;
    const answer = await collected(forwardGroups(bridge, route, { method: "GET", path: route, headers: {} }, context.signal));
    const value = parsed(answer.body);
    if (answer.status === 200 && me() && !awaySessions().includes(id) && isObject(value)) return { status: 200, value };
    return { status: answer.status === 200 ? 409 : answer.status, value: null };
  }
  const place = await placeOfSession(id, places.targets, 4_000);
  if (!place) return { status: 404, value: null };
  if (kind === "sessionGoals") return { status: 200, value: goalPlans().filter(goal => goal.session === id) };
  if (kind === "conversations" || kind === "sessionFiles") {
    const after = ref.offset ? `&cursor=${encodeURIComponent(ref.offset)}` : "";
    const answer = await asked(place.target, "GET", `api/session/${id}/message?limit=40&order=desc${after}`, undefined, 8_000);
    if (!isObject(answer) || !Array.isArray(answer.data)) return { status: 503, value: null };
    if (!me() || awaySessions().includes(id)) return { status: 409, value: null };
    return { status: 200, value: kind === "sessionFiles" ? fileNames(answer.data) : { messages: answer.data, cursor: answer.cursor } };
  }
  const area = kind === "sessionQueue" ? "queue" : kind === "sessionSchedules" ? "wakes" : "asks";
  const route = `kuvo/sessions/${id}/${area}`;
  const request = { method: "GET", path: route, headers: {}, desktop: true };
  const local = { ...context, route, request };
  if (kind === "pendingSessionWork" && !context.request.fromApp && !context.request.fromMac && !context.request.desktop) return { status: 403, value: null };
  const answer = await collected(kind === "sessionQueue" ? queueRoute(local, route.match(QUEUE_ROUTE)!)
    : kind === "sessionSchedules" ? answerWakes(id, undefined, request, context.readEngine, context.bridge, context.signal)
      : sessionAsksRoute(local, route.match(SESSION_ASKS_ROUTE)!));
  if (!me() || awaySessions().includes(id)) return { status: 409, value: null };
  return { status: answer.status, value: parsed(answer.body) ?? null };
}

export async function readForPeer(sender: MailSender, act: Act, places: PeerPlaces): Promise<Acted> {
  const link = readLink();
  if (!ownershipReady() || !followedSetup(link) || !peersOf(link).some(peer => peer.key === sender.key)) return { status: 403, value: null };
  const ref = requestOf(act.body?.kind, act.item, act.body?.offset);
  // Asks can include locally unlockable permissions. They remain on the existing authenticated client route.
  if (!ref || ref.kind === "pendingSessionWork") return { status: 400, value: null };
  const json: RouteContext["json"] = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const answer = await localRead(ref, {
    request: { method: "GET", path: "", headers: {}, desktop: true },
    phone: undefined, signal: AbortSignal.timeout(10_000), route: "", info: { name: "" }, phonesChanged: () => {}, hold: null,
    readEngine: readEngineFile, readBridge: readBridgeFile, bridge: () => bridgeTarget() ? readBridgeFile() : null, json, text: json,
  }, places, null);
  return Buffer.byteLength(JSON.stringify(answer.value)) <= MAX_READ ? answer : { status: 413, value: null };
}

export async function readOwned(context: RouteContext) {
  const { request, json, signal, info } = context;
  const query = new URLSearchParams(request.path.split("?")[1]);
  const kind = query.get("kind");
  const id = query.get("id") ?? "_";
  const computer = query.get("computer") ?? undefined;
  const offset = query.get("offset") ?? undefined;
  const queue = query.get("queue") ?? undefined;
  if (!dataKind(kind) || !DATA_ID.test(id) || (computer && !KEY.test(computer)) || (queue && !OWNER.test(queue)))
    return json(400, { message: "Name a data kind and its record or computer." });
  const link = readLink();
  if (computer && !peersOf(link).some(peer => peer.key === computer) && computer !== (link && ownKey(link)))
    return json(404, { message: "There’s no such computer in this setup." });
  const ref = requestOf(kind, id, offset);
  const queueKind = dataOwner(kind) === undefined;
  const hereItem = queueKind ? matching(allItems(), { id })[0] : undefined;
  const farItem = queueKind && !hereItem ? itemElsewhere({ id }, link, { deleted: true }) : null;
  const address = { kind, id, computer: computer ?? farItem?.computer.key, queue: hereItem?.owner ?? farItem?.owner ?? queue,
    client: context.phone?.key?.toString("base64url") };
  const localBot = dataOwner(kind, queue) === "bot home" && listBots().some(bot => bot.id === id || bot.session === id);
  const owner = ownerOf(address, { link, name: info.name, localRecord: localBot || Boolean(hereItem) });
  // A group without a known address may still live on the box the client is asking.
  const local = !owner && ref && !computer && (kind === "groupTranscript" || kind === "groupSettings" || dataOwner(kind, queue) === "session computer")
    ? await localRead(ref, context, { targets: targetsOf(context), bridge: Boolean(context.bridge()) }, null).catch(() => null)
    : null;
  const resolved = owner ?? (local?.status === 200 ? ownerOf(address, { link, name: info.name, localRecord: true }) : null);
  if (!resolved) return json(404, { message: "This box doesn’t know which computer owns that record." });
  if (!ref || !ownershipReady()) return json(200, view(resolved, null, query, !resolved.reachable));
  if (!resolved.local && (!resolved.key || !peersOf(link).some(peer => peer.key === resolved.key)))
    return json(200, view({ ...resolved, reachable: false }, null, query, true));
  const cache = recordKey(followedSetup(link)?.id ?? "", resolved.key ?? "", kind, id, offset ?? "");
  const peer = peersOf(link).find(each => each.key === resolved.key);
  const answer = local ?? (resolved.local
    ? await localRead(ref, context, { targets: targetsOf(context), bridge: Boolean(context.bridge()) }, resolved.key).catch(() => null)
    : peer && resolved.reachable ? await forwardAct(peer, id, { method: "GET", area: "ownership", item: id, body: { kind, offset } }, signal, link) : null);
  if (!resolved.local) {
    const current = readLink();
    if (followedSetup(current)?.id !== followedSetup(link)?.id || !peersOf(current).some(each => each.key === resolved.key)
      || ownerOf(address, { link: current, localRecord: Boolean(hereItem) })?.key !== resolved.key)
      return json(409, { message: "The owner changed. Read the record again." });
  }
  if (answer?.status === 200) {
    const saved = { at: Date.now(), ...snapshot(answer.value, kind) };
    if (!resolved.local) keepRecord("reads", cache, saved);
    return json(200, view(resolved, saved, query, false));
  }
  if (answer?.status === 404 && !("refused" in answer)) {
    const absent: Snapshot = { at: Date.now(), absent: true, files: [], messages: [] };
    if (!resolved.local) keepRecord("reads", cache, absent);
    return json(200, view(resolved, absent, query, false));
  }
  const unavailable = !answer || ("refused" in answer && answer.status !== 409);
  const snapshotAllowed = !resolved.local && kind !== "pendingSessionWork" && (unavailable || (answer && answer.status >= 500));
  const last = snapshotAllowed ? readRecord<Snapshot>("reads", cache) : undefined;
  return json(200, view(unavailable && !resolved.local ? { ...resolved, reachable: false } : resolved, last ?? null, query, true));
}

function view(owner: Owner, saved: Snapshot | null, query: URLSearchParams, stale: boolean) {
  const requested = (kind: string) => query.getAll(kind).slice(0, 100).filter(value => value.length > 0 && value.length <= 500);
  const heldMessages = new Set(saved?.messages);
  return {
    owner, stale,
    state: saved && !saved.absent ? "received" : "neverReceived",
    lastReceivedAt: saved?.at ?? null,
    ...(saved && !saved.absent ? { data: saved.value } : {}),
    unavailable: {
      files: [...new Set(stale || !owner.local ? [...(saved?.files ?? []), ...requested("file")] : [])],
      messages: [...new Set(requested("message").filter(id => !heldMessages.has(id)))],
    },
  };
}
