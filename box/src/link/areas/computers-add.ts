// The client asks any box. The primary keeps the attempts and runs SSH there.

import { addComputersLayoutReady, credentialSelectionLayoutReady } from "../../shared/folder-version";
import { isObject, parsed } from "../../shared/json";
import type { Adding } from "../../shared/routes/box";
import { sleep } from "../../shared/wait";
import { addressProblem, cleanAddress, suggestedHosts } from "../add/address";
import { beginAdd, beginSetup, beginUninstall, canAdd, sshAddressOf, takenAddresses } from "../add/flow";
import { createInstallerInvite, installerCodeFrom, mergeTarget } from "../add/invite";
import { bringUp } from "../add/keep-up";
import { credentialChecklist, credentialSelection } from "../add/seed";
import { allTries, answerTry, endTry, tryFollowers } from "../add/tries";
import { followed } from "../follow";
import { writePrivate } from "../files";
import { IS_MAC, K } from "../home";
import { KEY, peersOf, type MailSender } from "../peers/known";
import { keepKeeper } from "../keeper";
import { pairedPhone } from "../phones";
import { runningServe, SERVE_NEEDED } from "../serve-kept";
import { followedSetup } from "../setup/record";
import { MERGE_FEATURE } from "../setup/merge-data";
import { readLink } from "../state";
import type { AnswerPiece, Asker, RouteContext } from "../server/context";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";

export const LOCAL_COMPUTER_ADD = "kuvo/internal/computers/add";
export const COMPUTERS_ADD_ROUTE = /^kuvo\/computers\/(?:add(?:(\/events|\/code|\/credentials|\/merge-code)|\/(add_[a-z0-9]{1,40})(\/answer)?)?|([A-Za-z0-9_-]{43})\/(setup|uninstall|update))$/;
const KEY_ACTION = /^(setup|uninstall|update|autoUpdate)\/([A-Za-z0-9_-]{43})$/;
const TRY_ID = /^add_[a-z0-9]{1,40}$/;
const refused = (status: number, message: string): Acted => ({ status, value: { message } });

/** Adding computers, as this box says it now. */
export function adding(): Adding {
  const can = canAdd();
  return { can: can.can, ...(can.can ? {} : { why: can.message }), hosts: can.can ? suggestedHosts(takenAddresses()) : [], tries: allTries() };
}

// Re-setup, update and uninstall use the SSH configuration of the box which saved that address.
export const ONCE = "Ask this through Kuvo running on this computer: a single request can’t stay to do it.";

async function addHere(method: string, item: string, body: unknown, asker?: Asker): Promise<Acted> {
  const keyed = KEY_ACTION.exec(item);
  if (keyed) {
    if (method !== "POST") return refused(405, "Only POST.");
    const can = canAdd();
    if (!can.can) return { status: can.status, value: { message: can.message, ...(can.primary ? { primary: can.primary } : {}) } };
    const action = keyed[1]!;
    const key = keyed[2]!;
    if (!sshAddressOf(key)) return refused(404, "That isn’t a computer this one set up over SSH.");
    const begun = action === "setup" ? beginSetup(key)
      : action === "uninstall" ? beginUninstall(key) : bringUp(key, action === "autoUpdate");
    if (!begun) return refused(404, "That isn’t a computer this one set up over SSH.");
    return "busy" in begun ? refused(409, begun.busy) : { status: 202, value: { try: begun.seen } };
  }
  if (item === "credentials") {
    if (method !== "GET") return refused(405, "Only GET.");
    if (!credentialSelectionLayoutReady(K)) return refused(503, "Wait for the Kuvo Code update to finish before choosing sign-ins and keys.");
    return { status: 200, value: await credentialChecklist() };
  }
  if (item === "code") {
    if (method !== "POST") return refused(405, "Only POST.");
    const input = isObject(body) ? body : {};
    const selected = credentialSelection(input.credentials);
    if (!selected) return refused(400, "Choose sign-ins and keys by their checklist IDs.");
    if (selected.length && !credentialSelectionLayoutReady(K)) return refused(503, "Wait for the Kuvo Code update to finish before choosing sign-ins and keys.");
    return createInstallerInvite(input.invite, asker, selected);
  }
  if (item === "merge-code") {
    if (method !== "POST") return refused(405, "Only POST.");
    const input = isObject(body) ? body : {};
    const merge = mergeTarget(input);
    if (!merge) return refused(400, "Choose the other setup and its primary computer.");
    return createInstallerInvite(input.invite, asker, [], merge);
  }
  const given = isObject(body) ? body : {};
  if (item.startsWith("answer/") && TRY_ID.test(item.slice(7))) {
    if (method !== "POST") return refused(405, "Only POST.");
    const answered = typeof given.answer === "string" && typeof given.at === "number" ? answerTry(item.slice(7), given.answer, given.at) : null;
    return answered ? { status: 200, value: { try: answered } } : refused(409, "It isn’t asking that now.");
  }
  if (TRY_ID.test(item)) {
    if (method !== "DELETE") return refused(405, "Only DELETE.");
    return endTry(item) ? { status: 200, value: {} } : refused(404, "There’s no such try.");
  }
  if (item !== "add") return refused(404, "There’s no such add request.");
  if (method === "GET") return { status: 200, value: adding() };
  if (method !== "POST") return refused(405, "GET, or POST {destination}.");
  const can = canAdd();
  if (!can.can) return { status: can.status, value: { message: can.message, ...(can.primary ? { primary: can.primary } : {}) } };
  const destination = cleanAddress(given.destination);
  const { replace, discardLocalPairings } = given;
  if (replace !== undefined && (typeof replace !== "string" || !KEY.test(replace))) return refused(400, "Choose the previous membership by its key.");
  if (discardLocalPairings !== undefined && typeof discardLocalPairings !== "boolean") return refused(400, "Choose whether to discard local pairings.");
  const problem = addressProblem(destination);
  if (problem) return refused(400, problem);
  const selected = credentialSelection(given.credentials);
  if (!selected) return refused(400, "Choose sign-ins and keys by their checklist IDs.");
  if (selected.length && !credentialSelectionLayoutReady(K)) return refused(503, "Wait for the Kuvo Code update to finish before choosing sign-ins and keys.");
  const begun = beginAdd(destination, asker, replace, discardLocalPairings, selected);
  return "busy" in begun ? refused(409, begun.busy) : { status: 202, value: { try: begun.seen } };
}

export async function addComputerAct(sender: MailSender, act: Act): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  if (!addComputersLayoutReady(K) || setup?.role !== "primary" || setup.id !== act.body?.setup || setup.epoch !== act.body.epoch
    || !peersOf(link).some(peer => peer.key === sender.key)) return refused(409, "The setup changed. Refresh it and try again.");
  if (KEY_ACTION.test(act.item)) return refused(409, "Ask the computer that saved that SSH address.");
  return addOnPrimary(act.method, act.item, act.body.input, act.body.actor);
}

function actorOf(phone?: Asker) {
  return phone?.key ? { key: phone.key.toString("base64url"), pairing: phone.pairing ?? "" } : null;
}

function addForActor(method: string, item: string, input: unknown, actor: unknown): Promise<Acted> {
  if (actor === null) return addHere(method, item, input);
  if (!isObject(actor) || typeof actor.key !== "string" || !KEY.test(actor.key) || typeof actor.pairing !== "string"
    || !pairedPhone(readLink(), actor.key, actor.pairing)) return Promise.resolve(refused(403, "Pair this device before adding a computer."));
  return addHere(method, item, input, { key: Buffer.from(actor.key, "base64url"), pairing: actor.pairing });
}

/** The machine's kept local server owns SSH attempts even if a relay session starts one.
 * Code issuance has no in-process attempt and can run on any process of the primary. */
async function addOnPrimary(method: string, item: string, input: unknown, actor: unknown, served = false, signal?: AbortSignal, oneShot = false): Promise<Acted> {
  if (served || IS_MAC && !oneShot || !IS_MAC && (item === "code" || item === "credentials" || item === "merge-code")) return addForActor(method, item, input, actor);
  try {
    let server = runningServe();
    if (!server) {
      if (method === "GET") return addForActor(method, item, input, actor);
      if (IS_MAC) return refused(409, ONCE);
      writePrivate(SERVE_NEEDED(), "");
      keepKeeper();
      const until = Date.now() + 20_000;
      while (!server && Date.now() < until && !signal?.aborted) {
        await sleep(200);
        server = runningServe();
      }
    }
    if (!server) return refused(503, "Kuvo’s local server hasn’t started yet. Try again in a moment.");
    const timeout = AbortSignal.timeout(oneShot ? 50_000 : 8_000);
    const response = await fetch(`http://127.0.0.1:${server.port}/${LOCAL_COMPUTER_ADD}`, {
      method: "POST",
      headers: { authorization: `Basic ${Buffer.from(`opencode:${server.password}`).toString("base64")}` },
      signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
      body: JSON.stringify({ method, item, input, actor, forward: IS_MAC && oneShot }),
    });
    return { status: response.status, value: await response.json() };
  } catch { return refused(503, "Kuvo’s local server did not answer. Check the add attempts before retrying."); }
}

/** The automatic SSH retry uses the same worker as a manual attempt. */
export async function automaticSshUpdate(key: string): Promise<void> {
  const result = await addOnPrimary("POST", `autoUpdate/${key}`, undefined, null);
  if (result.status === 503 && isObject(result.value)) console.error(result.value.message);
}

export async function* localComputerAdd(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.served !== true || request.method !== "POST") return yield* json(403, {});
  const given = parsed(request.body);
  if (!isObject(given) || typeof given.method !== "string" || typeof given.item !== "string") return yield* json(400, {});
  if (!["GET", "POST", "DELETE"].includes(given.method)) return yield* json(405, {});
  if (given.forward !== true && readLink()?.setup?.role === "member") return yield* json(409, { message: "The setup changed. Refresh it and try again." });
  const result = given.forward === true
    ? await addFrom(context, given.method as Act["method"], given.item, given.input, given.actor)
    : await addForActor(given.method, given.item, given.input, given.actor);
  return yield* json(result.status, result.value);
}

async function addFrom(context: RouteContext, method: Act["method"], item: string, input?: unknown, actor: unknown = actorOf(context.phone)): Promise<Acted> {
  const oneShot = context.request.fromMac === true && context.request.served !== true;
  if (IS_MAC && oneShot) return addOnPrimary(method, item, input, actor, false, context.signal, true);
  const link = readLink();
  const setup = followedSetup(link);
  if (KEY_ACTION.test(item) || setup?.role !== "member") return addOnPrimary(method, item, input, actor, context.request.served, context.signal);
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  if (!primary) return refused(503, "This computer can't reach your primary.");
  if (item === "merge-code" && !takesForwarded(primary.key, MERGE_FEATURE)) return refused(409, "Update your primary before merging setups.");
  if ((item === "credentials" || method === "POST" && (item === "add" || item === "code"))
    && !takesForwarded(primary.key, "computers.add.credentials")) {
    return refused(409, "Update your primary to choose sign-ins and keys before adding a computer.");
  }
  const result = await forwardAct(primary, "Adding computers", {
    area: "addComputer", method, item,
    body: { setup: setup.id, epoch: setup.epoch, actor, input },
  }, context.signal, link, Date.now(), { backgroundRelay: true });
  if ("refused" in result) {
    return method === "GET" && item === "add" ? { status: 200, value: { can: false, why: result.refused, hosts: [], tries: [] } } : refused(result.status, result.refused);
  }
  return result;
}

export async function* computersAddRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  const method = request.method.toUpperCase() as Act["method"];
  const [, suffix, id, answering, key, action] = match;
  if (suffix === "/events") {
    if (method !== "GET") return yield* json(405, { message: "Only GET." });
    if (followedSetup(readLink())?.role !== "member" && (request.served || IS_MAC && !request.fromMac)) {
      yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
      for await (const part of followed(adding, tryFollowers, signal)) {
        if (followedSetup(readLink())?.role === "member") break;
        yield part;
      }
      return;
    }
    const initial = await addFrom(context, "GET", "add");
    if (initial.status !== 200) return yield* json(initial.status, initial.value);
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    let last = JSON.stringify(initial.value);
    yield { body: Buffer.from(last + "\n") };
    while (!signal?.aborted) {
      await sleep(1500);
      if (signal?.aborted) break;
      const answer = await addFrom(context, "GET", "add");
      const next = JSON.stringify(answer.status === 200 ? answer.value : {
        can: false, why: isObject(answer.value) ? answer.value.message : "Your primary stopped answering.", hosts: [], tries: [],
      });
      if (next === last) continue;
      last = next;
      yield { body: Buffer.from(next + "\n") };
    }
    return;
  }
  if (key) {
    if (method !== "POST") return yield* json(405, { message: "Only POST." });
    const result = await addFrom(context, method, `${action}/${key}`);
    return yield* json(result.status, result.value);
  }
  if (method !== "GET" && method !== "POST" && method !== "DELETE") return yield* json(405, { message: "Only GET, POST or DELETE." });
  const input = parsed(request.body);
  const item = suffix === "/credentials" ? "credentials" : suffix === "/code" ? "code" : suffix === "/merge-code" ? "merge-code" : id ? answering ? `answer/${id}` : id : "add";
  const result = (item === "code" || item === "merge-code") && method === "POST"
    ? await installerCodeFrom(invite => addFrom(context, method, item, { ...(isObject(input) ? input : {}), invite }))
    : await addFrom(context, method, item, input);
  return yield* json(result.status, result.value);
}
