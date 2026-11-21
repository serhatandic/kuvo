import { mutationSubmitter } from "./authority";
import { forwardGrant } from "../work-queue/forward-grant";
import { dataOwner } from "../../shared/data-declarations";
import { isObject, parsed } from "../../shared/json";
import { LABEL_RESTORE_FEATURE, LABEL_RESTORE_ROUTE } from "../../shared/routes/work-queue";
import { groupsRoute, listedGroups } from "../areas/groups";
import { personalityRoute } from "../areas/personalities";
import { QUEUE_ROUTE, queueRoute } from "../areas/queue";
import { SETTINGS_PAGE_ROUTE, settingsPageRoute } from "../areas/settings-pages";
import { answerWakes, type WakeAction } from "../areas/wakes";
import { LABEL_RESTORE_REFUSAL, labelRestoreClient, WORK_QUEUE_ROUTE, workQueueClient, workQueueRoute } from "../areas/work-queue";
import { bridgeTarget, readBridgeFile, readEngineFile, type PeerPlaces } from "../engine/client";
import { placeOfSession } from "../letters/letter";
import { awaySessions, frozen } from "../moves/records";
import { ownKey, peersOf, type MailSender } from "../peers/known";
import type { AnswerPiece, RouteContext } from "../server/context";
import { collected } from "../server/sent-once";
import { followedSetup } from "../setup/record";
import { readLink } from "../state";
import { forwardAct, runForwardDecision, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { type DataReference, type Owner } from "./resolve";
import { MUTATION_FEATURE, mutationKey, mutationNonce, runMutation } from "./mutation";
import { ownershipReady } from "./store";
import { acceptsMutation } from "./admission";

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
      answerWakes(session, wakes[2] as WakeAction | undefined, context.request, context.readEngine, context.bridge, context.signal, context.admitQueued) };
  }
  const group = /^kuvo\/groups\/(grp_[a-z0-9]{1,60})$/.exec(path);
  if (group && method === "PATCH")
    return { ref: { kind: "groupSettings", id: group[1] }, method, path, answer: groupsRoute };
  if (method === "POST" && path === "kuvo/work-queue/items" && isObject(body) && typeof body.owner === "string" && body.files === undefined)
    return { ref: { kind: "queueItems", queue: body.owner }, method, path, answer: context => workQueueRoute(context, path.match(WORK_QUEUE_ROUTE)!) };
  const restore = path === LABEL_RESTORE_ROUTE && (!isObject(body) || body.dryRun === undefined || body.dryRun === false);
  if (method === "POST" && (path === "kuvo/work-queue/labels" || restore))
    return { ref: { kind: "queueLabels" }, method, path, answer: context => workQueueRoute(context, path.match(WORK_QUEUE_ROUTE)!) };
  return null;
}

export async function sendOwnerWrite(write: Write, body: unknown, owner: Owner, context: RouteContext, operation?: string): Promise<Acted> {
  const restore = write.path === LABEL_RESTORE_ROUTE;
  if (restore && !labelRestoreClient(context))
    return { status: 403, value: { message: LABEL_RESTORE_REFUSAL } };
  if (!restore && write.path.startsWith("kuvo/work-queue/") && !workQueueClient(context))
    return { status: 403, value: { message: "Open this from a paired Kuvo Code client." } };
  if (write.ref.kind === "queueItems" && !operation)
    return { status: 400, value: { message: "Send a valid operation id." } };
  const link = readLink();
  const peer = peersOf(link).find(each => each.key === owner.key);
  if (!peer) return { status: 409, value: { message: `Open this record on ${owner.name} to change it.` } };
  if (restore && !takesForwarded(peer.key, LABEL_RESTORE_FEATURE))
    return { status: 409, value: { message: `Update the box on ${owner.name} before restoring labels from another computer.` } };
  if (operation && !takesForwarded(peer.key, MUTATION_FEATURE))
    return { status: 409, value: { message: `Update Kuvo Code on ${owner.name} before retrying this change from another computer.` } };
  const result = await forwardAct(peer, "This record", { method: write.method, item: write.path, area: "ownership", operation, body: { value: body } }, context.signal, link, Date.now(), { grant: forwardGrant(context, link) });
  return "refused" in result ? { status: result.status, value: { message: result.refused } } : result;
}

export async function runOwnerWrite(write: Write, body: unknown, context: RouteContext, nonce?: string): Promise<Acted> {
  const value = isObject(body) ? Object.fromEntries(Object.entries(body).filter(([key]) => key !== "operation")) : body;
  const run = async () => {
    if (!acceptsMutation("ownerWrite", write.ref))
      return { status: 409, value: { message: "This record is held while its owner changes." }, notAdmitted: true as const };
    const veto = context.admitQueued?.();
    if (veto) return veto;
    const input = nonce && write.ref.kind !== "queueItems" && isObject(body) && Object.hasOwn(body, "operation")
      ? { ...context, request: { ...context.request, body: Buffer.from(JSON.stringify(value)) } } : context;
    const answer = await collected(write.answer(input));
    return { status: answer.status, value: parsed(answer.body) ?? {}, ...(answer.notAdmitted ? { notAdmitted: true as const } : {}) };
  };
  // Queue creation records its result in the same queue write as the new item.
  if (!nonce || write.ref.kind === "queueItems") return run();
  const link = readLink();
  const setup = followedSetup(link);
  const own = link && ownKey(link);
  const grant = setup && !context.request.fromOwnerPeer && context.phone?.key ? forwardGrant(context, link) : undefined;
  if (grant === null) return { status: 409, value: { message: "Use a current device grant and a valid operation id." } };
  const admit = () => acceptsMutation("ownerWrite", write.ref) && mutationSubmitter("ownerWrite", readLink(), grant?.submitter) === "allowed" ? null
    : { status: 409, value: { message: "The record's owner or the device's access changed." }, notAdmitted: true as const };
  const mutate = () => runMutation(mutationKey(setup?.id ?? "", nonce), { path: write.path, method: write.method, value }, run);
  const answer = own && grant?.operation
    ? await runForwardDecision({ method: write.method, item: write.path, area: "ownership", operation: nonce, body: { value: body } },
      { ...grant, operation: grant.operation }, own, write.ref, admit, mutate)
    : await mutate();
  return answer;
}

export async function writeForPeer(sender: MailSender, act: Act, places: PeerPlaces): Promise<Acted> {
  const denied = { status: 409, value: { message: "This computer couldn’t confirm it owns that record." } };
  if (act.admit && act.item === LABEL_RESTORE_ROUTE) return { status: 403, value: { message: LABEL_RESTORE_REFUSAL } };
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
    || !acceptsMutation("ownerWrite", write.ref, link)
    || (id && (frozen().has(id) || awaySessions().includes(id)))) return denied;
  const veto = act.admit?.();
  if (veto) return veto;
  const request: RouteContext["request"] = { fromOwnerPeer: true, method: write.method, path: write.path, headers: act.operation ? { "x-kuvo-operation": act.operation } : {}, body: Buffer.from(JSON.stringify(act.body?.value ?? null)), desktop: true };
  const json: RouteContext["json"] = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  if (write.ref.kind === "queueItems" && (!act.operation || !mutationNonce(request, act.body?.value))) {
    return { status: 400, value: { message: "Send a valid operation id." } };
  }
  return runOwnerWrite(write, act.body?.value, {
    request, route: write.path, phone: undefined, admitQueued: act.admit, signal: AbortSignal.timeout(15_000), info: { name: "" },
    phonesChanged: () => {}, hold: null, readEngine: readEngineFile, readBridge: readBridgeFile,
    bridge: () => bridgeTarget() ? readBridgeFile() : null, json, text: json,
  }, act.operation);
}
