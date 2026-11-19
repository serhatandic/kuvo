import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { wipePreparationLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import type { MoveKind } from "../../shared/routes/moves";
import { destinationOf, type Destination } from "../backup/destination";
import { exportBeforeWipe, pointId, selectedProjects, verifyExport, type ExportPoint } from "../backup/engine";
import { noteDocsChanged } from "../documents/changed";
import { exclusiveWhile, listOf, writeDurablePrivate } from "../files";
import { K, RUN } from "../home";
import { beginMove, readOutcome } from "../moves/move";
import { readMove } from "../moves/records";
import { transferAccepted } from "../ownership/transfer";
import { recordKey } from "../ownership/store";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { peerIsAway, readReach } from "../peers/reach";
import { readPeerAgents } from "../peers/directory";
import { readLink, type LinkState } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { updateAccess } from "./access";
import { acceptedMembership, membershipOf } from "./membership";
import { pendingShared } from "./shared";
import { primaryFenced } from "./lease";
import { followedSetup } from "./record";
import { removalPlan } from "./removal-plan";
import { PREPARATION_FEATURE, removalsOf, type LocalWipe } from "./removal-record";
import { wipeSupportedHere } from "./wipe";


export const WIPE_PREPARATION_FOLDER = path.join(RUN, "wipe-preparation");
const sourceFile = (setup: string, key: string) => path.join(WIPE_PREPARATION_FOLDER, `source-${recordKey(setup, key)}.json`);
const intentFile = (setup: string, key: string) => path.join(WIPE_PREPARATION_FOLDER, `intent-${recordKey(setup, key)}.json`);
export const removalLock = (setup: string, key: string) => intentFile(setup, key) + ".lock";

type ExportRequest = { destination: Destination; recoveryKey: string; projects: string[] };
export type Preparation = { moveTo?: string; export?: ExportRequest };

export function preparationOf(value: unknown): Preparation | null {
  if (!isObject(value) || (value.moveTo !== undefined && (typeof value.moveTo !== "string" || !KEY.test(value.moveTo)))) return null;
  const send = value.export;
  if (send === undefined) return typeof value.moveTo === "string" ? { moveTo: value.moveTo } : null;
  if (!isObject(send) || typeof send.recoveryKey !== "string" || !/^[\w-]{43}$/.test(send.recoveryKey)
      || Buffer.from(send.recoveryKey, "base64url").toString("base64url") !== send.recoveryKey || !isObject(send.destination)) return null;
  const projects = send.projects ?? [];
  if (!Array.isArray(projects) || projects.length > 100 || !projects.every(folder => typeof folder === "string" && path.isAbsolute(folder) && !folder.includes("\0"))) return null;
  try {
    const destination = destinationOf(send.destination, { resolveDisk: false });
    return { ...(typeof value.moveTo === "string" ? { moveTo: value.moveTo } : {}),
      export: { destination, projects, recoveryKey: send.recoveryKey } };
  } catch { return null; }
}

type Intent = { setup: string; key: string; name: string; operation: string; choice?: Preparation; point?: ExportPoint; error?: string };
type Source = { operation: string; choice: Preparation; point?: ExportPoint; exported?: true;
  moves?: { id: string; kind: MoveKind }[]; ready?: true; error?: string };

function read<T>(file: string): T | undefined {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; return undefined; }
}

const save = (file: string, value: Intent | Source) => writeDurablePrivate(file, JSON.stringify(value), K);
const choiceHash = (choice: Preparation) => recordKey(JSON.stringify(choice));
const same = (a: Preparation, b: Preparation) => choiceHash(a) === choiceHash(b);
export const preparationClaim = (link: LinkState | null, key: string) => acceptedMembership(link)?.wipePreparations?.[key]
  ?? membershipOf(pendingShared(link, "membershipAndAccess")?.body.value)?.wipePreparations?.[key];
const message = (error: unknown) => (error instanceof Error ? error.message : "Wipe preparation failed.").slice(0, 500);
const reply = (status: number, value: unknown): Acted => ({ status, value });

export const removalIntent = (setup: string, key: string) => read<Intent>(intentFile(setup, key));
const localIntents = () => listOf(WIPE_PREPARATION_FOLDER)
  .filter(name => /^intent-[A-Za-z0-9_-]+\.json$/.test(name))
  .map(name => read<Intent>(path.join(WIPE_PREPARATION_FOLDER, name)));

export function pendingPreparations(): { key: string; name: string; operation: string; error?: string }[] {
  const link = readLink();
  const setup = followedSetup(link);
  if (!setup || !wipePreparationLayoutReady(K)) return [];
  const membership = acceptedMembership(link);
  const known = Object.entries(membership?.wipePreparations ?? {}).map(([key, { operation }]) => ({
    key, name: membership?.boxes.find(box => box.key === key)?.name ?? key, operation,
  }));
  if (setup.role !== "primary") return known;
  const removed = removalsOf(link);
  const byKey = new Map<string, { key: string; name: string; operation: string; error?: string }>(known.map(each => [each.key, each]));
  for (const intent of localIntents()) {
    if (!intent || intent.setup !== setup.id || removed[intent.key] || link?.localWipe?.operation === intent.operation
      || (!intent.choice && membership?.wipePreparations?.[intent.key]?.operation !== intent.operation)) continue;
    const { key, name: display, operation, error } = intent;
    byKey.set(key, { key, name: display, operation, ...(error ? { error } : {}) });
  }
  return [...byKey.values()];
}

export function finishPreparation(setup: string, key: string, operation: string) {
  const file = intentFile(setup, key);
  const kept = read<Intent>(file);
  if (kept?.operation === operation) fs.rmSync(file, { force: true });
}

/** A committed membership record has the export reference. Drop obsolete private recipes
 * even if the request was answered before the standby committed or after a handover. */
export function forgetFinishedPreparations(link: LinkState | null) {
  if (!wipePreparationLayoutReady(K)) return;
  const removed = removalsOf(link);
  if (!Object.keys(removed).length) return;
  for (const intent of localIntents()) {
    if (intent && intent.setup === link?.setup?.id && removed[intent.key]?.operation === intent.operation) {
      finishPreparation(intent.setup, intent.key, intent.operation);
    }
  }
}

async function askSource(key: string, method: "GET" | "POST", body: Record<string, unknown>, signal?: AbortSignal | null) {
  const link = readLink();
  const peer = peersOf(link).find(each => each.key === key);
  if (!peer) return reply(404, { message: "That isn’t one of your computers." });
  if (!takesForwarded(key, PREPARATION_FEATURE)) return reply(503, { message: "The computer is unavailable or needs an update." });
  const there = await forwardAct(peer, "Removal preparation", { area: "computers", method, item: key,
    action: "removal-plan", body }, signal);
  return "refused" in there ? reply(503, { message: there.refused }) : there;
}

export async function planForComputer(key: string, projects: unknown, signal?: AbortSignal | null): Promise<Acted> {
  try {
    if (ownKey(readLink() ?? {}) !== key) return await askSource(key, "GET", { projects }, signal);
    return reply(200, { plan: await removalPlan(projects) });
  } catch (error) { return reply(409, { message: message(error) }); }
}

function exported(source: Source) {
  return source.exported ? { export: source.point } : {};
}

function sourceReply(source: Source): Acted {
  if (source.error) return reply(409, { message: source.error, ...exported(source) });
  return reply(source.ready ? 200 : 202, { ...exported(source) });
}

function changeID(operation: string, kind: MoveKind, id: string) {
  return "mov_" + Buffer.from(recordKey(operation, kind, id), "base64url").toString("hex").slice(0, 32);
}

function targetOf(choice: ExportRequest) {
  return { destination: destinationOf(choice.destination), recoveryKey: choice.recoveryKey };
}

async function prepareSource(file: string, journal: Source): Promise<void> {
  const { choice, operation } = journal;
  if (choice.export && !journal.exported) {
    const target = targetOf(choice.export);
    const projects = selectedProjects(choice.export.projects);
    const at = Date.now();
    journal.point ??= { source: "b_" + crypto.randomBytes(16).toString("base64url"), id: pointId("wipe", at), at };
    save(file, journal);
    await exportBeforeWipe(journal.point, target, projects);
    journal.exported = true;
    save(file, journal);
  }
  if (choice.moveTo) {
    const to = choice.moveTo;
    if (!journal.moves) {
      const link = readLink();
      if (!followedSetup(link)?.checkpointed || !peersOf(link).some(peer => peer.key === to)) {
        throw new Error("Move all requires a destination in the current accepted setup.");
      }
      const told = readPeerAgents()[to];
      if (!told?.features?.includes("moves.accepted-ownership") || peerIsAway(to, told.heard, readReach())) return;
      const plan = await removalPlan();
      if (!plan.complete) return;
      if (plan.moving.length) return;
      if (plan.movable.some(item => !item.destinations.some(destination => destination.key === to))) {
        throw new Error("Some movable items cannot go to the selected computer. Review the removal plan.");
      }
      journal.moves = plan.movable.map(({ id, kind }) => ({ id, kind }));
      save(file, journal);
    }
    for (const { id, kind } of journal.moves) {
      const change = changeID(operation, kind, id);
      const outcome = readOutcome(change);
      if (outcome?.ok === false) throw new Error(outcome.message);
      if (outcome?.ok) {
        const { id: receipt, thing, from, to: destination, sessions } = outcome.move;
        const link = readLink();
        if (receipt !== change || thing !== id || outcome.move.kind !== kind || from !== ownKey(link ?? {}) || destination !== to) {
          throw new Error("The move receipt doesn’t match the requested move.");
        }
        // The acknowledgement can arrive before the source learns the new owner, and the
        // primary may already have compacted the completed transfer by the next check.
        if (!transferAccepted(link, { id: change, thing: id, kind, from, to, sessions }, { allowSettled: true })) return;
        continue;
      }
      if (!readMove(change)) {
        const started = await beginMove(kind, id, to, undefined, false, change);
        if (!started.ok) throw new Error(started.message);
      }
      return;
    }
    // Re-read the source to catch an item created while an earlier move was underway.
    const plan = await removalPlan();
    if (!plan.complete || plan.moving.length) return;
    if (plan.movable.length) throw new Error("More movable work appeared. Review the removal plan before wiping this computer.");
  }
  if (choice.export && journal.point) await verifyExport(journal.point, targetOf(choice.export));
  journal.ready = true;
  save(file, journal);
}

const working = new Set<string>();

function startSource(setup: string, key: string, operation: string, choice: Preparation, retry = false): Acted {
  if (!wipePreparationLayoutReady(K) || !wipeSupportedHere()) return reply(409, { message: "This computer cannot run a prepared wipe." });
  const file = sourceFile(setup, key);
  const journal = read<Source>(file) ?? { operation, choice };
  if (journal.operation !== operation || !same(journal.choice, choice)) return reply(409, { message: "This computer already has a different wipe preparation." });
  if (!journal.ready && (!journal.error || retry) && !working.has(file)) {
    working.add(file);
    void exclusiveWhile(file + ".lock", async () => {
      const current = read<Source>(file) ?? journal;
      if (current.operation !== operation || !same(current.choice, choice) || current.ready || (current.error && !retry)) return;
      delete current.error;
      save(file, current);
      try { await prepareSource(file, current); }
      catch (error) { save(file, { ...current, error: message(error) }); }
    }).catch(error => console.error("Wipe preparation:", error)).finally(() => working.delete(file));
  }
  return sourceReply(retry && working.has(file) ? { ...journal, error: undefined } : journal);
}

export async function removalPlanForPeer(sender: MailSender, act: Act): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  const key = ownKey(link ?? {});
  if (!key || act.item !== key || !setup || setup.conflict || link?.localWipe) return reply(409, { message: "The computer’s removal plan is unavailable." });
  if (act.method === "GET") return planForComputer(key, act.body?.projects);
  if (act.body?.setup === setup.id && typeof act.body?.epoch === "number" && act.body.epoch > setup.epoch) {
    return reply(503, { message: "Wait for the current primary’s checkpoint." });
  }
  if (setup.role !== "member" || setup.primary !== sender.key || act.body?.setup !== setup.id || act.body?.epoch !== setup.epoch) {
    return reply(409, { message: "Only the current primary can prepare this computer’s wipe." });
  }
  const operation = act.body?.operation;
  const claim = acceptedMembership(link)?.wipePreparations?.[key];
  if (!claim || claim.operation !== operation) return reply(202, { message: "Wait for the accepted wipe preparation." });
  const choice = act.body?.choice === undefined ? read<Source>(sourceFile(setup.id, key))?.choice : preparationOf(act.body.choice);
  if (!choice) return reply(202, { message: "Resume this operation with its original choices on the current primary." });
  if (claim.choiceHash !== choiceHash(choice)) return reply(409, { message: "The choices don’t match the accepted wipe preparation." });
  return startSource(setup.id, key, claim.operation, choice, act.body?.retry === true);
}

type WipePreparation =
  | { phase: "before-unlink"; setup: string; key: string; operation: string; choice?: Preparation; retry?: boolean }
  | ({ phase: "before-wipe" } & LocalWipe);

/** Returns the preparation status before revocation, or verifies the local evidence before deletion. */
export async function beforeMembershipWipe(wipe: Readonly<WipePreparation>): Promise<Acted | undefined> {
  const link = readLink();
  const setup = followedSetup(link);
  const own = ownKey(link ?? {});
  if (!own || setup?.id !== wipe.setup) throw new Error("The setup changed during wipe preparation.");
  if (wipe.phase === "before-wipe") {
    const local = read<Source>(sourceFile(wipe.setup, own));
    if (wipe.requiresPreparation || local) {
      if (!local?.ready || local.operation !== wipe.operation) throw new Error("The requested move or export has not been verified.");
      if (local.choice.export) {
        if (!local.exported || !local.point) throw new Error("The requested export has not been verified.");
        await verifyExport(local.point, targetOf(local.choice.export));
      }
    }
    return;
  }
  const file = intentFile(wipe.setup, wipe.key);
  const peer = peersOf(link).find(each => each.key === wipe.key);
  let intent = read<Intent>(file);
  let claim = preparationClaim(link, wipe.key);
  if (!claim && !intent?.choice && !wipe.choice) return;
  if (!wipePreparationLayoutReady(K) || setup.role !== "primary" || setup.conflict || primaryFenced(link)
    || link?.leadershipPending || (wipe.key !== own && !peer)) throw new Error("Wait for the current primary before preparing the wipe.");
  if (intent?.choice && (intent.operation !== wipe.operation || (wipe.choice && !same(intent.choice, wipe.choice)))) {
    throw new Error("Retry the existing wipe operation with the same choices.");
  }
  if (claim && (claim.operation !== wipe.operation || (wipe.choice && claim.choiceHash !== choiceHash(wipe.choice)))) {
    throw new Error("Retry the existing wipe operation with the same choices.");
  }
  if (!intent?.choice && wipe.choice) {
    if (wipe.choice.moveTo === wipe.key
      || (wipe.choice.moveTo && wipe.choice.moveTo !== own && !peersOf(link).some(each => each.key === wipe.choice?.moveTo))) {
      throw new Error("Choose another computer in this setup for the moves.");
    }
    intent = { setup: wipe.setup, key: wipe.key, name: peer?.name ?? link?.name ?? "This computer", operation: wipe.operation, choice: wipe.choice };
    save(file, intent);
  }
  if (!claim && intent?.choice) {
    if (setup.standby && !takesForwarded(setup.standby.key, PREPARATION_FEATURE))
      throw new Error("Update the standby before preparing a wipe.");
    const proposal = { operation: wipe.operation, choiceHash: choiceHash(intent.choice) };
    const { save: staged } = updateAccess(current => {
      if (!current || current.setup?.id !== setup.id || current.setup.epoch !== setup.epoch
        || (wipe.key !== own && !peersOf(current).some(each => each.key === wipe.key))) return null;
      current.wipePreparations = { ...current.wipePreparations, [wipe.key]: proposal };
      return current;
    }, { checkpoint: true, operation: recordKey("prepare wipe", setup.id, String(setup.epoch), wipe.key, wipe.operation).slice(0, 22) });
    if (staged) noteDocsChanged();
    claim = preparationClaim(readLink(), wipe.key);
  }
  const accepted = acceptedMembership(readLink())?.wipePreparations?.[wipe.key];
  if (!claim || accepted?.operation !== wipe.operation || accepted.choiceHash !== claim.choiceHash) {
    return reply(202, { removal: { operation: wipe.operation, state: "saving" } });
  }
  if (intent?.error) {
    if (!wipe.retry) return reply(409, { message: intent.error, ...(intent.point ? { export: intent.point } : {}) });
    intent = { ...intent, error: undefined };
    save(file, intent);
  }
  const choice = intent?.choice;
  const moving = Boolean(choice?.moveTo);
  const local = choice ?? (own === wipe.key ? read<Source>(sourceFile(setup.id, own))?.choice : undefined);
  const result = wipe.key !== own
    ? await askSource(wipe.key, "POST", { setup: setup.id, epoch: setup.epoch, operation: wipe.operation, choice, retry: wipe.retry })
    : local && choiceHash(local) === claim.choiceHash ? startSource(setup.id, own, wipe.operation, local, wipe.retry)
    : reply(202, { message: "Resume this operation with its original choices on this computer." });
  const value = isObject(result.value) ? result.value : {};
  const point = isObject(value.export) && typeof value.export.source === "string" &&
    typeof value.export.id === "string" && typeof value.export.at === "number"
    ? { source: value.export.source, id: value.export.id, at: value.export.at } : undefined;
  if (point && intent && !intent.point) { intent = { ...intent, point }; save(file, intent); }
  if (result.status === 200) return result;
  if (result.status >= 400 && result.status !== 503) {
    save(file, {
      setup: setup.id, key: wipe.key, name: peer?.name ?? link?.name ?? "This computer", operation: wipe.operation,
      ...intent, error: typeof value.message === "string" ? value.message : "Wipe preparation failed.",
    });
    return result;
  }
  return reply(202, { removal: { operation: wipe.operation, state: result.status === 503 ? "wipe-pending"
    : moving ? "moving" : choice ? "exporting" : "wipe-pending" }, ...(isObject(result.value) ? result.value : {}) });
}
