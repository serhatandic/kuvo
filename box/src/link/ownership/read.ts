import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { dataKind, dataOwner, type DataKind } from "../../shared/data-declarations";
import { locked } from "../../shared/files";
import { isObject, parsed } from "../../shared/json";
import { LABEL_RESTORE_ROUTE } from "../../shared/routes/work-queue";
import { OWNER } from "../../shared/work-queue-owner";
import { forwardGroups } from "../areas/groups";
import { QUEUE_ROUTE, queueRoute } from "../areas/queue";
import { SESSION_ASKS_ROUTE, sessionAsksRoute } from "../areas/session-asks";
import { answerWakes } from "../areas/wakes";
import { listBots } from "../bots/list";
import { callReply, bridgeTarget, readBridgeFile, readEngineFile, type PeerPlaces } from "../engine/client";
import { attachmentRoots, RUN } from "../home";
import { placeOfSession, sessionReadRefusal } from "../letters/letter";
import { targetsOf } from "../letters/tools";
import { payloadPath } from "../moves/payload";
import { awaySessions } from "../moves/records";
import { attachmentNote } from "../moves/session-files";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import type { RouteContext } from "../server/context";
import { collected } from "../server/sent-once";
import { followedSetup } from "../setup/record";
import { sharedValue } from "../setup/shared";
import { readLink } from "../state";
import { goalPlans } from "../work/goals";
import { actHere } from "../work-queue/acts";
import { itemElsewhere } from "../work-queue/across";
import { previewLabelsHere } from "../work-queue/label-preview";
import { allItems, deletionFor, filesOf, matching } from "../work-queue/store";
import { forwardAct, type Act, type Acted } from "../work-queue/forward";
import { DATA_ID, mayActivate, ownerOf, type DataReference, type Owner } from "./resolve";
import { keepRecord, readRecord, recordKey, ownershipReady } from "./store";

const MAX_READ = 512 * 1024;
const primaryReads = ["setupNames", "setupTimeZone", "setupStarredModels", "setupPersonalities", "setupUsageDisplay", "queueLabels"] as const;
type PrimaryRead = typeof primaryReads[number];
const isPrimaryRead = (kind: DataKind): kind is PrimaryRead => (primaryReads as readonly DataKind[]).includes(kind);

interface Snapshot {
  at: number;
  value?: unknown;
  absent?: true;
  generation?: string;
  files: string[];
  messages: string[];
}
interface ReadMark extends Snapshot {
  revision?: string;
  started?: number;
  cleared?: boolean;
  withheld?: boolean;
}
type Refresh = "live" | "unavailable" | "failed" | "withheld" | "metadataOnly";
type ReadAnswer = Acted & { availableFiles?: string[] };

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

function presentFile(root: string, relative: string, size?: number): boolean {
  try {
    const info = fs.lstatSync(payloadPath(root, relative));
    return info.isFile() && (size === undefined || info.size === size);
  } catch {
    return false;
  }
}

function attachmentFiles(value: unknown, session?: string) {
  const names = new Map<string, boolean>();
  const add = (name: string, available: boolean) => {
    if (!name) return;
    const label = name.slice(0, 500);
    names.set(label, (names.get(label) ?? true) && available);
  };
  const managed = (file: string) => Boolean(session && attachmentRoots().some(root => {
    const relative = path.relative(root, file);
    return path.isAbsolute(file) && relative.startsWith(session + "/") && presentFile(root, relative);
  }));
  const note = (text: unknown) => {
    if (typeof text !== "string") return;
    for (const file of attachmentNote(text)) add(path.basename(file.path), !file.stayed && managed(file.path));
  };
  const reference = (file: unknown) => {
    if (typeof file === "string") add(file, false);
    else if (isObject(file)) {
      const source = isObject(file.source) ? file.source : undefined;
      const uri = source?.uri ?? file.uri ?? file.url;
      let location = typeof file.path === "string" ? file.path : "";
      if (typeof uri === "string" && uri.startsWith("file:")) {
        try { location = fileURLToPath(uri); } catch { location = ""; }
      }
      const label = file.name ?? file.filename ?? (location ? path.basename(location) : file.id ?? (uri ? "attachment" : undefined));
      const embedded = source?.type === "inline" && typeof file.data === "string"
        || typeof uri === "string" && uri.startsWith("data:") && uri.includes(",");
      if (typeof label === "string") add(label, embedded || managed(location));
    }
  };
  for (const msg of Array.isArray(value) ? value : []) {
    if (!isObject(msg)) continue;
    if (Array.isArray(msg.files)) msg.files.forEach(reference);
    note(msg.text);
    for (const part of Array.isArray(msg.content) ? msg.content : []) {
      if (!isObject(part)) continue;
      if (["file", "image", "attachment"].includes(String(part.type))) reference(part);
      if (part.type === "text") note(part.text ?? part.content);
      if (part.type === "tool" && isObject(part.state) && Array.isArray(part.state.content)) {
        for (const output of part.state.content) if (isObject(output) && output.type === "file") reference(output);
      }
    }
  }
  return names;
}

function snapshot(value: unknown, kind: DataKind): Pick<Snapshot, "value" | "files" | "messages"> {
  const rows = Array.isArray(value) ? value : isObject(value) && Array.isArray(value.messages) ? value.messages : [];
  return {
    value, files: kind === "sessionFiles" && Array.isArray(value) ? value.filter((file): file is string => typeof file === "string")
      : [...attachmentFiles(isObject(value) && isObject(value.item) ? [value.item] : rows).keys()],
    messages: rows.flatMap(row => isObject(row) && typeof row.id === "string" ? [row.id] : []),
  };
}

async function localRead(ref: ReadRequest, context: RouteContext, places: PeerPlaces, computer: string | null, inventoryHere = true): Promise<ReadAnswer> {
  const kind = ref.kind;
  const id = ref.id!;
  const link = readLink();
  const me = (queue?: string) => {
    const address = { ...ref, computer: computer ?? undefined, queue };
    const current = readLink();
    return ownerOf(address, { link: current })?.local && mayActivate(address, { link: current });
  };
  if (isPrimaryRead(kind)) {
    if (!me()) return { status: 409, value: null };
    const value = sharedValue(link, kind);
    return value === undefined ? { status: 404, value: null } : { status: 200, value };
  }
  if (dataOwner(kind) === undefined) {
    const item = matching(allItems(), { id })[0];
    if (!me(item?.owner ?? deletionFor(id)?.queue)) return { status: 409, value: null };
    if (!item || item.removed) return { status: 404, value: null };
    const answer = await actHere({ method: "GET", ref: { id }, targets: places.targets, signal: context.signal });
    if (!me(item.owner)) return { status: 409, value: null };
    return { ...answer, availableFiles: inventoryHere ? item.files.filter(file => presentFile(filesOf(id), file.name, file.size)).map(file => file.name) : [] };
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
    if (!me() || awaySessions().includes(id)) return { status: 409, value: null };
    if (answer.status !== 200) return { status: answer.status, value: null };
    return isObject(value) ? { status: 200, value } : { status: 503, value: null };
  }
  let place: Awaited<ReturnType<typeof placeOfSession>>;
  try {
    place = await placeOfSession(id, places.targets, 4_000, undefined, context.signal);
  } catch (error) {
    return { status: sessionReadRefusal(error) ?? 503, value: null };
  }
  if (!me() || awaySessions().includes(id)) return { status: 409, value: null };
  // Either runtime can own the session. One runtime's miss doesn't settle the other one's absence.
  if (!place) return { status: places.bridge ? 404 : 503, value: null };
  if (kind === "sessionGoals") return { status: 200, value: goalPlans().filter(goal => goal.session === id) };
  if (kind === "conversations" || kind === "sessionFiles") {
    const after = ref.offset ? `&cursor=${encodeURIComponent(ref.offset)}` : "";
    const wait = AbortSignal.timeout(8_000);
    const answer = await callReply(place.target, "GET", `/api/session/${id}/message?limit=40&order=desc${after}`, undefined,
      context.signal ? AbortSignal.any([context.signal, wait]) : wait).catch(error => {
      const status = sessionReadRefusal(error);
      return status ? { refused: status } : null;
    });
    if (answer && "refused" in answer) return { status: answer.refused, value: null };
    if (!isObject(answer) || !Array.isArray(answer.data)) return { status: 503, value: null };
    if (!me() || awaySessions().includes(id)) return { status: 409, value: null };
    const files = kind === "sessionFiles" || inventoryHere ? attachmentFiles(answer.data, inventoryHere ? id : undefined) : new Map<string, boolean>();
    return {
      status: 200, value: kind === "sessionFiles" ? [...files.keys()] : { messages: answer.data, cursor: "cursor" in answer ? answer.cursor : undefined },
      availableFiles: [...files].filter(([, available]) => available).map(([name]) => name),
    };
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
  if (act.item === LABEL_RESTORE_ROUTE) return previewLabelsHere(act.body?.library, act.body?.force === true);
  const ref = requestOf(act.body?.kind, act.item, act.body?.offset);
  // Asks can include locally unlockable permissions. They remain on the existing authenticated client route.
  if (!ref || ref.kind === "pendingSessionWork") return { status: 400, value: null };
  const json: RouteContext["json"] = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const answer = await localRead(ref, {
    request: { method: "GET", path: "", headers: {}, desktop: true },
    phone: undefined, signal: AbortSignal.timeout(10_000), route: "", info: { name: "" }, phonesChanged: () => {}, hold: null,
    readEngine: readEngineFile, readBridge: readBridgeFile, bridge: () => bridgeTarget() ? readBridgeFile() : null, json, text: json,
  }, places, null, false).catch(() => ({ status: 503, value: null }));
  return Buffer.byteLength(JSON.stringify(answer.value)) <= MAX_READ
    ? { status: answer.status, value: answer.value } : { status: 413, value: null };
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
  const hereQueue = queueKind ? matching(allItems(), { id })[0]?.owner ?? deletionFor(id)?.queue : undefined;
  const farItem = queueKind && !hereQueue ? itemElsewhere({ id }, link, { deleted: true }) : null;
  const address = { kind, id, computer: computer ?? farItem?.computer.key, queue: hereQueue ?? farItem?.owner ?? queue,
    client: context.phone?.key?.toString("base64url") };
  const resolved = ownerOf(address, { link, name: info.name });
  const placesHere = (): PeerPlaces => {
    const targets = targetsOf(context);
    return { targets, bridge: targets.length === 2 };
  };
  if (!resolved) return json(404, { message: "This box doesn’t know which computer owns that record." });
  if (!ref) return json(200, view(resolved, null, query, "metadataOnly"));
  if (!ownershipReady()) return json(200, view(resolved, null, query, "withheld"));
  if (!resolved.local && (!resolved.key || !peersOf(link).some(peer => peer.key === resolved.key)))
    return json(200, view({ ...resolved, reachable: false }, null, query, "unavailable"));
  const record = [followedSetup(link)?.id ?? "", resolved.key ?? "", kind, id];
  const markKey = recordKey(...record);
  const cache = recordKey(...record, offset ?? "");
  const markOf = () => resolved.local ? undefined : readRecord<ReadMark>("reads", markKey);
  const before = markOf();
  const started = Date.now();
  const peer = peersOf(link).find(each => each.key === resolved.key);
  const answer = resolved.local
    ? await localRead(ref, context, placesHere(), resolved.key).catch(() => null)
    : peer && resolved.reachable ? await forwardAct(peer, id, { method: "GET", area: "ownership", item: id, body: { kind, offset } }, signal, link) : null;
  let displayed = resolved;
  if (!resolved.local) {
    const current = readLink();
    const currentOwner = ownerOf(address, { link: current, name: info.name });
    if (followedSetup(current)?.id !== followedSetup(link)?.id || !peersOf(current).some(each => each.key === resolved.key)
      || currentOwner?.key !== resolved.key)
      return json(409, { message: "The owner changed. Read the record again." });
    displayed = answer && !("refused" in answer) ? { ...currentOwner, reachable: true } : currentOwner;
  }
  const status = answer?.status;
  const ownerRefused = answer && !("refused" in answer) && (status === 401 || status === 403 || status === 409);
  if (status === 200 || status === 404 || ownerRefused) {
    const save = () => {
      const mark = markOf();
      if (mark?.revision !== before?.revision && ((mark?.started ?? 0) >= started || mark?.generation !== before?.generation))
        return view(displayed, lastPage(cache, mark), query, mark?.withheld ? "withheld" : "failed");
      const at = Date.now();
      const revision = randomUUID();
      if (status !== 200) {
        const gone: ReadMark = { at, absent: true, files: [], messages: [], generation: revision, revision, started, withheld: !!ownerRefused };
        if (!resolved.local) keepRecord("reads", markKey, gone);
        return view(displayed, ownerRefused ? null : gone, query, ownerRefused ? "withheld" : "live");
      }
      const saved = { at, ...snapshot(answer!.value, kind), generation: mark?.generation };
      if (!resolved.local) {
        keepRecord("reads", markKey, { ...mark, at: mark?.at ?? at, files: [], messages: [], revision, started, cleared: true, withheld: false });
        keepRecord("reads", cache, saved);
      }
      const available = (answer as ReadAnswer).availableFiles;
      return view(displayed, saved, query, "live", available);
    };
    return json(200, resolved.local ? save() : locked(path.join(RUN, "owned-data", "reads", markKey + ".lock"), save, { required: true }));
  }
  const refresh = !answer ? resolved.local ? "failed" : "unavailable"
    : answer.status === 503 && "refused" in answer ? "unavailable"
      : answer.status >= 500 ? "failed" : "withheld";
  const useSnapshot = !resolved.local && kind !== "pendingSessionWork" && (refresh === "unavailable" || refresh === "failed");
  return json(200, view(refresh === "unavailable" ? { ...displayed, reachable: false } : displayed,
    useSnapshot ? lastPage(cache, markOf()) : null, query, refresh));
}

function lastPage(cache: string, mark?: ReadMark): Snapshot | null {
  if (mark?.withheld) return null;
  const page = readRecord<Snapshot>("reads", cache);
  return page && page.generation === mark?.generation ? page : mark?.absent && !mark.cleared ? mark : null;
}

function view(owner: Owner, saved: Snapshot | null, query: URLSearchParams, refresh: Refresh, availableFiles: string[] = []) {
  const requested = (kind: string) => query.getAll(kind).slice(0, 100).filter(value => value.length > 0 && value.length <= 500);
  const heldMessages = new Set(saved?.messages);
  const heldFiles = new Set(availableFiles);
  return {
    owner, stale: refresh !== "live" && (refresh !== "metadataOnly" || !owner.reachable), refresh,
    state: saved ? saved.absent ? "absent" : "received" : "neverReceived",
    lastReceivedAt: saved?.at ?? null,
    ...(saved && !saved.absent ? { data: saved.value } : {}),
    unavailable: {
      files: [...new Set([...(saved?.files ?? []), ...requested("file")])].filter(name => !heldFiles.has(name)),
      messages: [...new Set(requested("message").filter(id => !heldMessages.has(id)))],
    },
  };
}
