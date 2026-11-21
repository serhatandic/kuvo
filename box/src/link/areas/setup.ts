import { forwardGrant } from "../work-queue/forward-grant";
import { computerInSentence } from "../computer-name";
// `kuvo/setup` (PROTOCOL.md, "Your setup"): who's in your setup and who its main computer is, as
// this box says it, on every box, for every client; and its events, a line now and one as it changes.
// `POST kuvo/setup/judge`: what a Mac adding a computer found there over SSH, judged by this box.
// `POST kuvo/setup/attached`: Kuvo Code on this computer attached to this box in place.
// `POST kuvo/setup/welcomed {welcomed}`: you've had the first run with this box (or will again).
// `GET kuvo/setup/ready`: whether a first run has anything to ask about your accounts here.

import fs from "node:fs";
import path from "node:path";
import { sharedKind, sharedPolicy } from "../../shared/data-declarations";
import { isObject, parsed, type Body } from "../../shared/json";
import { writePrivate } from "../files";
import { RUN } from "../home";
import { followed } from "../follow";
import { handover, selectStandby } from "../setup/handover";
import { writeCard } from "../setup/card";
import { appsOf, ATTACHED_FILE, foundOf, judgeComputer, oursNow } from "../setup/judge";
import { addElsewhere, mainElsewhere, setupHosts } from "../setup/main-only";
import { ownMachinesRoute } from "../mac/own-machines";
import { changeShared, OPERATION_ID, queryOperation, retrySharedValue, sharedSaveReply, waitShared } from "../setup/shared";
import { setupSeedRefusal } from "../setup/seed";
import { followedSetup } from "../setup/record";
import { setupView } from "../setup/view";
import { engineRoute } from "../server/forward";
import { onLinkWritten, readLink } from "../state";
import { isWelcomed, setWelcomed } from "../setup/welcomed";
import { firstRunReady, type Listed } from "../setup/ready";
import { call } from "../engine/client";
import { plainName } from "../../shared/text";
import { listsHere } from "../work/model-rule";
import { readEndedSignIns } from "./accounts";
import type { ListedAccount } from "./sign-in";
import type { Target } from "../../shared/records";
import { fromLocalApp, savedJson, type AnswerPiece, type RouteContext } from "../server/context";
import { applyInstallerCancellation, joinSetupMerge } from "../add/invite";
import { INSTALLER_RECOVERY_ROUTE } from "../add/invite-state";
import { mergeSourceProblem } from "../setup/merge-data";
import { pairedPhone } from "../phones";
import { pairedSubmitter } from "../ownership/authority";

// Those following it here: told as this process writes link.json; what another process wrote
// (`setup` and `leave`, over SSH), looked for each second (`followed`, with no folder to watch).
const followers = new Set<() => void>();
onLinkWritten(() => {
  for (const tell of followers) tell();
});

export async function* setupRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, signal, json } = context;
  const method = request.method.toUpperCase();
  if (route === INSTALLER_RECOVERY_ROUTE) {
    if (!fromLocalApp(request)) return yield* json(403, { message: "Apply the receipt directly on the setup’s main computer." });
    if (method !== "POST") return yield* json(405, {});
    const body = parsed(request.body);
    if (!isObject(body) || typeof body.receipt !== "string") return yield* json(400, { message: "Enter an installer cancellation receipt." });
    try {
      return yield* json(200, { message: await applyInstallerCancellation(body.receipt) });
    } catch (error) {
      return yield* json(409, { message: error instanceof Error ? error.message : "The roster retirement could not be confirmed." });
    }
  }
  if (route === "kuvo/setup/merge") {
    const link = readLink();
    const device = context.phone?.key?.toString("base64url");
    if (!fromLocalApp(request) && !(device && pairedPhone(link, device, context.phone?.pairing))) {
      return yield* json(403, { message: "Use a client paired with this setup to bring it over." });
    }
    if (method === "GET") {
      const why = mergeSourceProblem(link);
      const pending = link?.setupMerge;
      return yield* json(200, { can: !why, ...(why ? { why } : {}), ...(pending ? { pending: { setup: pending.target, primary: pending.primary, operation: pending.operation } } : {}),
        ...(link?.lastSetupMerge ? { completed: link.lastSetupMerge } : {}) });
    }
    if (method !== "POST") return yield* json(405, {});
    const given = parsed(request.body);
    if (!isObject(given) || typeof given.code !== "string") return yield* json(400, { message: "Send the other setup’s merge code." });
    const done = await joinSetupMerge(given.code);
    return yield* json(done.status, done.value);
  }
  if (route === "kuvo/setup/standby" || route === "kuvo/setup/handover") {
    if (method !== "POST") return yield* json(405, {});
    if (!request.fromApp && !request.fromMac && !pairedSubmitter(context, readLink()))
      return yield* json(403, { message: "Use a client paired with this setup to change its leadership." });
    const change = route === "kuvo/setup/standby" ? selectStandby : handover;
    const done = await change(parsed(request.body), signal, forwardGrant(context));
    return yield* json(done.status, done.value);
  }
  if (route === "kuvo/setup/judge") {
    if (method !== "POST") return yield* json(405, {});
    // The probe's lines as it printed them (RemoteEngineInstaller's, in OpenCodeKit).
    const given = parsed(request.body) as Body<"probe">;
    if (typeof given?.probe !== "string" || given.probe.length > 200_000) return yield* json(400, { message: "Send what the probe printed, as probe." });
    const lines = given.probe.split("\n");
    return yield* json(200, judgeComputer(foundOf(lines), oursNow(readLink()), appsOf(lines)));
  }
  if (route === "kuvo/setup/attached") {
    if (method !== "POST") return yield* json(405, {});
    // Only Kuvo Code on this computer (`kuvo-link.js box route`), or its app beside its box.
    if (!request.fromMac && !request.fromApp) return yield* json(403, { message: "Only Kuvo Code on this computer says it attached." });
    // Kept for good (an app that's gone again still asks before a join: the safe way to be wrong).
    fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
    if (!fs.existsSync(path.join(RUN, ATTACHED_FILE))) writePrivate(path.join(RUN, ATTACHED_FILE), JSON.stringify({ at: Date.now() }));
    writeCard();
    return yield* json(200, {});
  }
  if (route === "kuvo/setup/welcomed") {
    if (method !== "POST") return yield* json(405, {});
    // Any client of this box's (the app, the phone): the first run is done, or (`false`) to be
    // had again.
    const given = parsed(request.body) as Body<"welcomed">;
    if (typeof given?.welcomed !== "boolean") return yield* json(400, { message: "Say whether the first run is done, as welcomed (true or false)." });
    setWelcomed(given.welcomed);
    for (const tell of followers) tell();
    return yield* json(200, { welcomed: isWelcomed() });
  }
  if (method !== "GET" && method !== "HEAD") return yield* json(405, {});
  if (route === "kuvo/setup/ready") {
    // Read as it's asked (the models as the box keeps them, model-rule.ts; the accounts anew):
    // what a sign-in just changed is said the next time. A side not running, or not answering in
    // time, can't say: never ready.
    let engine: Target | null = null;
    try { engine = context.readEngine(); } catch {}
    const agents = context.bridge();
    const accounts = async (target: Target | null): Promise<Listed> => {
      if (!target) return null;
      const deadline = AbortSignal.timeout(15_000);
      const listed = await call(target, "GET", "/api/integration", undefined, signal ? AbortSignal.any([signal, deadline]) : deadline).catch(() => null);
      return Array.isArray(listed) ? listed.filter((each): each is ListedAccount => typeof (each as ListedAccount | null)?.id === "string") : null;
    };
    const [lists, fromEngine, fromAgents] = await Promise.all([listsHere(engine, agents), accounts(engine), accounts(agents)]);
    return yield* json(200, firstRunReady(lists, fromEngine, fromAgents, readEndedSignIns(), computerInSentence(context.request)));
  }
  if (route === "kuvo/setup/events") {
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(() => setupView(readLink()), followers, signal);
  }
  return yield* json(200, setupView(readLink()));
}

// What adds or removes a computer, as a Mac's app takes it (`POST kuvo/machines`, `DELETE
// kuvo/machines/<id>`, `POST kuvo/machines/<id>/uninstall`, `DELETE kuvo/hosts/<id>`): on a
// member, its main computer's to do.
const ADDS_OR_REMOVES = (method: string, route: string) =>
  (method === "POST" && (route === "kuvo/machines" || /^kuvo\/machines\/[^/]+\/uninstall$/.test(route)))
  || (method === "DELETE" && /^kuvo\/(machines|hosts)\/[^/]+$/.test(route));

/** A machine's `kuvo/machines…` and `kuvo/hosts…` (a Mac's box answers them with its app,
 * `mac/machines.ts`): on a member of a setup, `GET kuvo/hosts` lists the setup's machines from its
 * document, as a Mac lists its own, and adding or removing one is its main computer's, named in a
 * 409. The rest go on to the engine, as before (which has none of them). */
export async function* memberComputersRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, json } = context;
  const method = request.method.toUpperCase();
  const link = readLink();
  const main = mainElsewhere(link);
  if (!main) {
    if (followedSetup(link)?.role === "primary") return yield* ownMachinesRoute(context);
    return yield* engineRoute(context);
  }
  if (link?.setup?.checkpointed && (route.startsWith("kuvo/machines") || method === "DELETE" && route.startsWith("kuvo/hosts/"))) return yield* ownMachinesRoute(context);
  if (ADDS_OR_REMOVES(method, route)) return yield* json(409, addElsewhere(main));
  if ((method !== "GET" && method !== "HEAD") || route !== "kuvo/hosts") return yield* engineRoute(context);
  const hosts = setupHosts(link);
  if (hosts) return yield* json(200, hosts);
  return yield* json(503, { message: "This computer can't list your computers yet. Try again in a moment." });
}

export async function* saveLocalSetup(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (!request.fromHolder || request.method !== "POST") return yield* json(403, {});
  const given = parsed(request.body);
  if (isObject(given) && typeof given.id === "string" && typeof given.operation === "string" && given.value === undefined) {
    const expectedKind = given.kind;
    if (expectedKind !== undefined && !sharedKind(expectedKind)) return yield* json(400, {});
    const outcome = queryOperation(given.id, given.operation, given.proof, expectedKind);
    if (!outcome) return yield* json(404, {});
    const { kind, epoch, sequence, baseRevision, status } = outcome;
    return yield* json(200, { operation: given.operation, kind, epoch, sequence, baseRevision, status });
  }
  if (!isObject(given) || !isObject(given.value)) return yield* json(400, { message: "Send the setup settings to save." });
  const kind = given.kind;
  if (!sharedKind(kind)) return yield* json(400, {});
  const policy = sharedPolicy(kind).localSave;
  if (!policy) return yield* json(400, {});
  const operation = given.operation;
  if (operation !== undefined && (typeof operation !== "string" || !OPERATION_ID.test(operation))) return yield* json(400, {});
  const value = given.value;
  if (policy === "usage" && (!isObject(value.units) || typeof value.imported !== "boolean")) return yield* json(400, {});
  const expected = given.sequence;
  const current = readLink();
  if (current?.setup?.id === given.id && current?.setup?.epoch === given.epoch && operation) {
    const previous = retrySharedValue(current!, kind, operation, expected, value);
    if (previous) return yield* savedJson(json, sharedSaveReply(await waitShared(previous)));
  }
  const seed = await setupSeedRefusal(kind);
  if (seed) return yield* json(seed.status, { message: seed.message });
  const save = changeShared(kind, (_before, link) => {
    if (!link.setup || link.setup.id !== given.id || link.setup.epoch !== given.epoch
      || (link.sharedSetup === given.id ? link.shared?.[kind]?.sequence ?? 0 : 0) !== expected) return undefined;
    return value;
  }, operation);
  if (!save) {
    const latest = readLink();
    const previous = latest?.setup?.id === given.id && latest?.setup?.epoch === given.epoch && operation
      ? retrySharedValue(latest!, kind, operation, expected, value) : null;
    if (previous) return yield* savedJson(json, sharedSaveReply(await waitShared(previous)));
    return yield* json(409, { message: "The setup changed or a save is already pending." });
  }
  return yield* savedJson(json, sharedSaveReply(await waitShared(save)));
}
