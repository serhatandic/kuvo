import { removalDisk } from "../server/disk-worker";
import { additiveObjectFormat } from "../../shared/format-version";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { wipePreparationLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import type { MoveKind } from "../../shared/routes/moves";
import { destinationOf, separateDisk, type Destination } from "../backup/destination";
import { exportBeforeWipe, pointId, selectedProjects, usesAutomaticRelayBackup, verifyExport, type ExportPoint } from "../backup/engine";
import { exclusiveWhile, listOf, writeDurablePrivate } from "../files";
import { K, RUN } from "../home";
import { beginMove, readOutcome } from "../moves/move";
import { readMove, readMoves } from "../moves/records";
import { recordKey } from "../ownership/store";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { peerIsAway, readReach } from "../peers/reach";
import { readPeerAgents } from "../peers/directory";
import type { LinkHost } from "../relay/host";
import { readLink, type LinkState } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { acceptedMembership, membershipOf } from "./membership";
import { followedSetup } from "./record";
import { removalPlan } from "./removal-plan";
import { confirmRemovalFiles, removalDestinationReady } from "./removal-moves";
import { EXPORT_INVENTORY_VERSION, MOVE_INVENTORY_VERSION, MAX_SELECTED_MOVES, PREPARATION_FEATURE, REMOVAL_EXPORT_REQUIRED, REMOVAL_EXPORT_SAME_DISK, REMOVAL_LAYOUT_UNSUPPORTED, removalMoveID, removalsOf, type LocalWipe, type MemberRemoval, type RemovalPlan, type SelectedMove } from "./removal-record";
import { sourceGeneration } from "./removal-generation";
import { wipeSupportedHere } from "./wipe";


export const WIPE_PREPARATION_FOLDER = path.join(RUN, "wipe-preparation");
const sourceFile = (setup: string, key: string, operation?: string) => path.join(WIPE_PREPARATION_FOLDER, `source-${recordKey(setup, key, ...(operation ? [operation] : []))}.json`);
const intentFile = (setup: string, key: string, operation?: string) => path.join(WIPE_PREPARATION_FOLDER, `intent-${recordKey(setup, key, ...(operation ? [operation] : []))}.json`);
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
type PreparationHost = Pick<LinkHost, "welcomed" | "send"> | null | undefined;
type Source = { recheckGeneration?: string; retry?: string; selectedGeneration?: string; generation?: string; unresolved?: string[]; invalidated?: string; operation: string; choice: Preparation; point?: ExportPoint; exported?: true;
  moves?: { id: string; kind: MoveKind; verified?: true; completed?: true; sessions?: string[] }[]; ready?: true; error?: string };

const preparationFormat = additiveObjectFormat("Wipe preparation");

function read<T>(file: string): T | undefined {
  try { return preparationFormat.read(JSON.parse(fs.readFileSync(file, "utf8"))) as T; }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; return undefined; }
}

function save(file: string, value: Intent | Source): void {
  read(file);
  writeDurablePrivate(file, JSON.stringify(preparationFormat.write(value)), K);
}
export const choiceHash = (choice: Preparation) => recordKey(JSON.stringify(choice));
const same = (a: Preparation, b: Preparation) => choiceHash(a) === choiceHash(b);
export const preparationClaim = (link: LinkState | null, key: string) => acceptedMembership(link)?.wipePreparations?.[key];
const message = (error: unknown) => ((error as NodeJS.ErrnoException)?.path ? "A source or export path is unavailable." :
  error instanceof Error ? error.message : "Wipe preparation failed.").slice(0, 500);
const reply = (status: number, value: unknown): Acted => ({ status, value });

export function removalRecipe(choice: Preparation): RemovalPlan {
  return { choiceHash: choiceHash(choice), ...(choice.moveTo ? { moveTo: choice.moveTo } : {}),
    ...(choice.export ? { archive: true } : {}) };
}

export function keepRemovalChoice(setup: string, key: string, name: string, operation: string, choice: Preparation) {
  save(intentFile(setup, key, operation), { setup, key, name, operation, choice });
}

export const removalChoice = (setup: string, key: string, operation: string) =>
  read<Intent>(intentFile(setup, key, operation))?.choice;

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
    if (!intent || intent.setup !== link?.setup?.id) continue;
    const record = removed[intent.key];
    if (record?.superseded?.[intent.operation] || (record?.operation === intent.operation && (record.delivered || record.resolution))) {
      fs.rmSync(intentFile(intent.setup, intent.key, intent.operation), { force: true });
      finishPreparation(intent.setup, intent.key, intent.operation);
    }
  }
}

async function askSource(key: string, body: Record<string, unknown>, signal?: AbortSignal | null) {
  const link = readLink();
  const peer = peersOf(link).find(each => each.key === key);
  if (!peer) return reply(404, { message: "That isn’t one of your computers." });
  if (!takesForwarded(key, PREPARATION_FEATURE)) return reply(503, { message: "The computer is unavailable or needs an update." });
  const there = await forwardAct(peer, "Removal preparation", { area: "computers", method: "GET", item: key,
    action: "removal-plan", body }, signal);
  return "refused" in there ? reply(503, { message: there.refused }) : there;
}

export async function planForComputer(key: string, projects: unknown, signal?: AbortSignal | null): Promise<Acted> {
  try {
    if (ownKey(readLink() ?? {}) !== key) return await askSource(key, { projects }, signal);
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

function needsIndependentExport(operation: string) {
  const link = readLink();
  return followedSetup(link)?.role === "member" && (usesAutomaticRelayBackup() ||
    (link?.removalSource?.operation === operation && link.removalSource.review === "choose-export"));
}

function targetOf(choice: ExportRequest, independent: boolean) {
  const destination = destinationOf(choice.destination);
  if (independent) {
    if (destination.kind === "disk") {
      let separate: boolean;
      try { separate = separateDisk(destination.path); }
      catch { throw new Error("The selected export disk is unavailable. Reconnect it and retry."); }
      if (!separate) throw new Error(REMOVAL_EXPORT_SAME_DISK);
    } else if (!destination.ssh) throw new Error(REMOVAL_EXPORT_REQUIRED);
  }
  return { destination, recoveryKey: choice.recoveryKey };
}

async function prepareSource(file: string, journal: Source, scoped = false, selection?: SelectedMove[], host?: PreparationHost): Promise<void> {
  const { choice, operation } = journal;
  const independent = scoped && needsIndependentExport(operation);
  if (independent && !choice.export) {
    throw new Error(REMOVAL_EXPORT_REQUIRED);
  }
  const problems = (ids: string[]) => {
    journal.unresolved = ids;
    throw new Error(ids.length ? "Some source work could not be verified. Review the removal plan." : "The source changed. Review the removal plan.");
  };
  const checkPlan = (plan: Awaited<ReturnType<typeof removalPlan>>) => {
    if (!plan.complete) {
      const unavailable = plan.files?.filter(file => file.status === "unavailable").slice(0, 3).map(file => file.name);
      if (unavailable?.length) {
        journal.unresolved = plan.unresolved;
        throw new Error(`Source files could not be verified: ${unavailable.join(", ")}. Review and revise the removal plan.`);
      }
      problems(plan.unresolved);
    }
    if (!choice.export && (plan.exportOnly.length || plan.childSessions.length || plan.files?.some(file => file.status === "export-only"))) {
      throw new Error("Some source work is export-only. Review the removal plan and choose an export before wiping.");
    }
  };
  delete journal.unresolved;
  let scopedDestination: string | undefined;
  if (choice.moveTo) {
    const to = choice.moveTo;
    const link = readLink();
    if (!followedSetup(link)?.checkpointed || !peersOf(link).some(peer => peer.key === to)) {
      throw new Error("Move all requires a destination in the current accepted setup.");
    }
    const told = readPeerAgents()[to];
    if (!told?.features?.includes("moves.two-computers") || peerIsAway(to, told.heard, readReach())) {
      if (!scoped || !link || !removalDestinationReady(link, to, operation, host)) return;
      scopedDestination = to;
    }
    if (!journal.moves) {
      const plan = await removalPlan(undefined, to, scopedDestination);
      if (plan.moving.length) return;
      checkPlan(plan);
      if (plan.movable.length > MAX_SELECTED_MOVES) throw new Error("There are too many items to verify in this removal. Move some work first or choose an export.");
      journal.moves = plan.movable.map(({ id, kind }) => ({ id, kind }));
      save(file, journal);
    }
    if (scoped && selection === undefined) return;
    if (scoped && JSON.stringify(journal.moves.map(({ id, kind }) => ({ id, kind }))) !== JSON.stringify(selection)) {
      throw new Error("The source inventory does not match the accepted removal selection.");
    }
    for (const item of journal.moves) {
      const { id, kind } = item;
      if (item.completed && item.verified) continue;
      const change = removalMoveID(operation, kind, id);
      const outcome = readOutcome(change);
      if (outcome?.ok === false) throw new Error(outcome.message);
      if (outcome?.ok) {
        if (!outcome.verifiedPayloads) throw new Error("The move receipt does not verify its files. Review and revise the removal plan.");
        const { id: receipt, thing, from, to: destination, sessions } = outcome.move;
        const link = readLink();
        if (receipt !== change || thing !== id || outcome.move.kind !== kind || from !== ownKey(link ?? {}) || destination !== to) {
          throw new Error("The move receipt doesn’t match the requested move.");
        }
        if (readMove(change)) return;
        item.completed = true;
        item.verified = true;
        item.sessions = [...sessions];
        save(file, journal);
        continue;
      }
      if (!readMove(change)) {
        if (item.completed) throw new Error("The move receipt does not verify its files. Review and revise the removal plan.");
        if (scoped && readLink()?.removalSource?.operation !== operation) return;
        const started = await beginMove(kind, id, to, undefined, false, change, scoped ? operation : undefined);
        if (!started.ok) throw new Error(started.message);
      }
      return;
    }
  }
  const projects = selectedProjects(choice.export?.projects);
  const { generation, attachmentHashes } = await removalDisk.run("scanSourceGeneration", projects, Boolean(choice.export));
  if (choice.moveTo && journal.recheckGeneration !== generation) {
    const verifiedSessions = new Set(journal.moves?.flatMap(move => move.verified ? [move.id, ...(move.sessions ?? [])] : []));
    const plan = await removalPlan(undefined, choice.moveTo, scopedDestination, verifiedSessions);
    if (plan.moving.length) return;
    checkPlan(plan);
    if (plan.movable.length) return problems(plan.movable.map(item => item.id));
  }
  if (choice.export && attachmentHashes) {
    if (journal.selectedGeneration !== generation) {
      const at = Date.now();
      journal.point = { source: "b_" + crypto.randomBytes(16).toString("base64url"), id: pointId("wipe", at), at };
      journal.selectedGeneration = generation;
      delete journal.exported;
      save(file, journal);
    }
    await exportBeforeWipe(journal.point!, targetOf(choice.export, independent), projects, {
      attachmentHashes,
      checkSource: async () => {
        if (generation !== (await removalDisk.run("scanSourceGeneration", projects, false)).generation) throw new Error("Source work changed during export. Review or retry the removal plan.");
      },
    });
    journal.exported = true;
  }
  if (choice.moveTo && journal.moves?.length) {
    const link = readLink();
    const verified = link && await confirmRemovalFiles(link, choice.moveTo, operation, host);
    if (verified === false) throw new Error("The destination could not verify the moved files. Review and revise the removal plan.");
    if (!verified) return;
  }
  if (generation !== (await removalDisk.run("scanSourceGeneration", projects, false)).generation) return problems([]);
  delete journal.recheckGeneration;
  journal.generation = generation;
  journal.ready = true;
  save(file, journal);
}

const working = new Set<string>();

function startSource(setup: string, key: string, operation: string, choice: Preparation, retry = false, scoped = false, selection?: SelectedMove[], host?: PreparationHost): Acted {
  if (!wipePreparationLayoutReady(K) || !wipeSupportedHere()) return reply(409, { message: "This computer cannot run a prepared wipe." });
  const file = sourceFile(setup, key, scoped ? operation : undefined);
  const journal = read<Source>(file) ?? { operation, choice };
  if (journal.operation !== operation || !same(journal.choice, choice)) return reply(409, { message: "This computer already has a different wipe preparation." });
  if (!journal.ready && (!journal.error || retry) && !working.has(file)) {
    working.add(file);
    void exclusiveWhile(file + ".lock", async () => {
      const current = read<Source>(file) ?? journal;
      if (current.operation !== operation || !same(current.choice, choice) || current.ready || (current.error && !retry)) return;
      delete current.error;
      save(file, current);
      try { await prepareSource(file, current, scoped, selection, host); }
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
    return reply(503, { message: "Wait for the main computer’s checkpoint." });
  }
  if (setup.role !== "member" || setup.primary !== sender.key || act.body?.setup !== setup.id || act.body?.epoch !== setup.epoch) {
    return reply(409, { message: "Only the main computer can prepare this computer’s wipe." });
  }
  const operation = act.body?.operation;
  const claim = acceptedMembership(link)?.wipePreparations?.[key];
  if (!claim || claim.operation !== operation) return reply(202, { message: "Wait for the accepted wipe preparation." });
  const choice = act.body?.choice === undefined ? read<Source>(sourceFile(setup.id, key))?.choice : preparationOf(act.body.choice);
  if (!choice) return reply(202, { message: "Resume this operation with its original choices on the main computer." });
  if (claim.choiceHash !== choiceHash(choice)) return reply(409, { message: "The choices don’t match the accepted wipe preparation." });
  return startSource(setup.id, key, claim.operation, choice, act.body?.retry === true);
}

export function invalidatedPreparation(wipe: LocalWipe, key: string, failure = "Source work changed after verification. Review or retry the removal plan.") {
  const file = sourceFile(wipe.setup, key, wipe.generation ? wipe.operation : undefined);
  const journal = read<Source>(file);
  if (!journal) return;
  delete journal.ready;
  delete journal.generation;
  journal.invalidated = wipe.generation;
  journal.recheckGeneration = wipe.generation;
  journal.error = failure;
  save(file, journal);
}

export async function currentPreparation(setup: string, key: string, record: Pick<MemberRemoval, "operation" | "plan" | "generation" | "selection">,
  given: unknown, host?: PreparationHost): Promise<{ generation?: string; export?: ExportPoint; inventoryVersion?: number; moveInventoryVersion?: number; invalidated?: string; error?: string; unresolved?: string[]; selection?: SelectedMove[] }> {
  const file = sourceFile(setup, key, record.operation);
  let journal = read<Source>(file);
  if (journal?.ready && journal.generation) {
    const generation = journal.generation;
    let changed = false;
    let needsExport = false;
    try {
      needsExport = !journal.choice.export && needsIndependentExport(record.operation);
      changed = needsExport || !movesVerified(journal) ||
        (await removalDisk.run("scanSourceGeneration", selectedProjects(journal.choice.export?.projects), false)).generation !== generation;
    } catch { changed = true; }
    // The scan can outlive a retry or an amended plan. Its result only applies to
    // the exact generation that was scanned.
    journal = read<Source>(file);
    if (!journal?.ready || journal.generation !== generation) return {};
    if (changed) {
      invalidatedPreparation({ setup, operation: record.operation, generation }, key, needsExport ? REMOVAL_EXPORT_REQUIRED : undefined);
      journal = read<Source>(file);
    }
  }

  if (journal?.invalidated && journal.invalidated === record.generation) {
    return { invalidated: journal.invalidated, error: journal.error };
  }
  const choice = preparationOf(given) ?? journal?.choice;
  if (!choice || choiceHash(choice) !== record.plan?.choiceHash) return {};
  if (!wipeSupportedHere()) return { error: REMOVAL_LAYOUT_UNSUPPORTED };
  if (journal?.ready) return { generation: journal.generation, ...exported(journal),
    ...(journal.exported ? { inventoryVersion: EXPORT_INVENTORY_VERSION } : {}),
    ...(journal.choice.moveTo ? { moveInventoryVersion: MOVE_INVENTORY_VERSION } : {}) };
  if (journal?.error) return { error: journal.error, unresolved: journal.unresolved };
  if (choice.moveTo && journal?.moves && record.selection === undefined) return { selection: journal.moves.map(({ id, kind }) => ({ id, kind })) };
  startSource(setup, key, record.operation, choice, false, true, record.selection, host);
  return {};
}

export function retrySource(setup: string, key: string, operation: string, retry: string) {
  const file = sourceFile(setup, key, operation);
  const journal = read<Source>(file);
  if (!journal || journal.retry === retry || working.has(file)) return;
  save(file, { ...journal, retry, error: undefined, unresolved: undefined, invalidated: undefined });
}

/** Verify the archive again before the wipe command stops the local writers. */
export async function beforeMembershipWipe(wipe: Readonly<LocalWipe>, own: string, commit?: () => void): Promise<boolean> {
  if (!wipe.requiresPreparation) return false;
  const local = read<Source>(sourceFile(wipe.setup, own, wipe.generation ? wipe.operation : undefined));
  const independent = needsIndependentExport(wipe.operation);
  if (independent && !local?.choice.export) throw new Error(REMOVAL_EXPORT_REQUIRED);
  verifySourceGeneration(wipe, own);
  if (local?.choice.export) {
    if (!local.exported || !local.point) throw new Error("The requested export has not been verified.");
    await verifyExport(local.point, targetOf(local.choice.export, independent), commit);
    return true;
  }
  return false;
}

export function preparedMoveDestination(wipe: LocalWipe, own: string): string | undefined {
  if (!wipe.requiresPreparation) return undefined;
  const journal = read<Source>(sourceFile(wipe.setup, own, wipe.generation ? wipe.operation : undefined));
  return journal?.moves?.length ? journal.choice.moveTo : undefined;
}

const movesVerified = (source: Source) => !source.choice.moveTo || Boolean(source.moves?.every(move => move.completed && move.verified));

export function verifySourceGeneration(wipe: LocalWipe, own: string) {
  if (!wipe.requiresPreparation) return;
  const local = read<Source>(sourceFile(wipe.setup, own, wipe.generation ? wipe.operation : undefined));
  if (readMoves(true).some(move => !move.failed) || !local?.ready || !movesVerified(local) || local.operation !== wipe.operation || !local.generation || (wipe.generation && wipe.generation !== local.generation)
    || sourceGeneration(selectedProjects(local.choice.export?.projects)) !== local.generation) {
    throw new Error("Source work changed after verification. Review or retry the removal plan.");
  }
}
