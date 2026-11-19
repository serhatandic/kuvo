import { dataOwner } from "../../shared/data-declarations";
import { isObject, parsed } from "../../shared/json";
import { groupsRoute, listedGroups } from "../areas/groups";
import { personalityRoute } from "../areas/personalities";
import { QUEUE_ROUTE, queueRoute } from "../areas/queue";
import { SETTINGS_PAGE_ROUTE, settingsPageRoute } from "../areas/settings-pages";
import { answerWakes, type WakeAction } from "../areas/wakes";
import { WORK_QUEUE_ROUTE, workQueueClient, workQueueRoute } from "../areas/work-queue";
import { bridgeTarget, readBridgeFile, readEngineFile, type PeerPlaces } from "../engine/client";
import { placeOfSession } from "../letters/letter";
import { awaySessions, frozen } from "../moves/records";
import { peersOf, type MailSender } from "../peers/known";
import type { AnswerPiece, RouteContext } from "../server/context";
import { collected } from "../server/sent-once";
import { followedSetup } from "../setup/record";
import { readLink } from "../state";
import { forwardAct, type Act, type Acted } from "../work-queue/forward";
import { ownerOf, type DataReference, type Owner } from "./resolve";
import { ownershipReady } from "./store";

type Method = "PATCH" | "POST" | "DELETE";
interface Write {
  ref: DataReference;
  path: string;
  method: Method;
  answer: (context: RouteContext) => AsyncIterable<AnswerPiece>;
}

/** This is the peer write surface. Permissions, commands and watch checks use their direct routes
 * so the owner can still identify the client and validate the current request. */
export function ownerWrite(method: string, path: string, body: unknown): Write | null {
  const settings = method === "PATCH" && path === "kuvo/settings/clock" ? path.match(SETTINGS_PAGE_ROUTE) : null;
  if (settings) return { ref: { kind: "setupTimeZone" }, method: "PATCH", path, answer: context => settingsPageRoute(context, settings) };
  const personality = /^(?:kuvo\/personalities)(?:\/([A-Za-z0-9_-]{1,100}))?$/.exec(path);
  if (personality && (method === "PATCH" || method === "POST" || method === "DELETE"))
    return { ref: { kind: "setupPersonalities" }, method, path, answer: context => personalityRoute(context, personality) };
  const queue = /^kuvo\/sessions\/(ses_[A-Za-z0-9_-]{1,80})\/queue\/(hold|resume|reorder)$/.exec(path);
  if (queue && method === "POST")
    return { ref: { kind: "sessionQueue", id: queue[1] }, method, path, answer: context => queueRoute(context, path.match(QUEUE_ROUTE)!) };
  const wakes = /^kuvo\/wakes\/(ses_[A-Za-z0-9]{1,60})(?:\/(pause|cancel))?$/.exec(path);
  if (wakes && method === "POST") {
    const session = wakes[1]!;
    return { ref: { kind: "sessionSchedules", id: session }, method, path, answer: context =>
      answerWakes(session, wakes[2] as WakeAction | undefined, context.request, context.readEngine, context.bridge, context.signal) };
  }
  const group = /^kuvo\/groups\/(grp_[a-z0-9]{1,60})$/.exec(path);
  if (group && method === "PATCH")
    return { ref: { kind: "groupSettings", id: group[1] }, method, path, answer: groupsRoute };
  if (method === "POST" && path === "kuvo/work-queue/items" && isObject(body) && typeof body.owner === "string" && body.files === undefined)
    return { ref: { kind: "queueItems", queue: body.owner }, method, path, answer: context => workQueueRoute(context, path.match(WORK_QUEUE_ROUTE)!) };
  if (method === "POST" && path === "kuvo/work-queue/labels")
    return { ref: { kind: "queueLabels" }, method, path, answer: context => workQueueRoute(context, path.match(WORK_QUEUE_ROUTE)!) };
  return null;
}

export async function sendOwnerWrite(write: Write, body: unknown, owner: Owner, context: RouteContext): Promise<Acted> {
  if (write.path.startsWith("kuvo/work-queue/") && !workQueueClient(context))
    return { status: 403, value: { message: "Open this from a paired Kuvo Code client." } };
  const link = readLink();
  const peer = peersOf(link).find(each => each.key === owner.key);
  if (!peer) return { status: 409, value: { message: `Open this record on ${owner.name} to change it.` } };
  const result = await forwardAct(peer, "This record", { method: write.method, item: write.path, area: "ownership", body: { value: body } }, context.signal, link);
  return "refused" in result ? { status: result.status, value: { message: result.refused } } : result;
}

export async function writeForPeer(sender: MailSender, act: Act, places: PeerPlaces): Promise<Acted> {
  const denied = { status: 409, value: { message: "This computer couldn’t confirm it owns that record." } };
  const write = ownerWrite(act.method, act.item, act.body?.value);
  if (!write || !ownershipReady()) return denied;
  const scope = dataOwner(write.ref.kind, write.ref.queue);
  const id = write.ref.queue ?? write.ref.id;
  const session = scope === "session computer" ? id : undefined;
  if (id && (frozen().has(id) || awaySessions().includes(id))) return denied;
  if (session && !await placeOfSession(session, places.targets, 3_000)) return denied;
  if (scope === "group host") {
    const target = bridgeTarget();
    if (!target || !(await listedGroups(target, AbortSignal.timeout(3_000)))?.some(group => group.id === id)) return denied;
  }

  const link = readLink();
  if (!followedSetup(link) || !peersOf(link).some(peer => peer.key === sender.key)
    || !ownerOf(write.ref, { link, localRecord: true })?.local
    || (id && (frozen().has(id) || awaySessions().includes(id)))) return denied;
  const request: RouteContext["request"] = { fromOwnerPeer: true, method: write.method, path: write.path, headers: {}, body: Buffer.from(JSON.stringify(act.body?.value ?? null)), desktop: true };
  const json: RouteContext["json"] = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const answer = await collected(write.answer({
    request, route: write.path, phone: undefined, signal: AbortSignal.timeout(15_000), info: { name: "" },
    phonesChanged: () => {}, hold: null, readEngine: readEngineFile, readBridge: readBridgeFile,
    bridge: () => bridgeTarget() ? readBridgeFile() : null, json, text: json,
  }));
  return { status: answer.status, value: parsed(answer.body) ?? {} };
}
