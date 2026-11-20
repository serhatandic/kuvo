// Renaming and removing your machines (PROTOCOL.md, "Computers", Managing them), from any box,
// with no client and no Mac's app open. A machine's record (its name, that it's one of your
// computers) is in the peers document of the Mac that set it up, which your other computers take
// it from: so one box writes it, as one numbers the Work Queue. That's this Mac's box for its own
// machines (a Mac that isn't a member of another's setup); every other box hands what you do to
// the computer that writes it (its setup's main computer, or the one Mac that set this machine
// up), by mail (`work-queue/forward.ts`), and answers with what it said, or why it can't.
// Connect is every box's own (`reach-again.ts`). What each computer offers is said on it
// (`actions` in `kuvo/computers`), with the words for its confirm, so no client works it out.
// Letting a computer add Kuvo Code's app to your devices again (`allow-devices`) is the main
// computer's own, for its own clients: never handed on from another box (areas/computers.ts).

import crypto from "node:crypto";
import { installOffersLayoutReady, removalLayoutReady, removalOperationsLayoutReady, wipePreparationLayoutReady } from "../../shared/folder-version";
import { acceptedMembership, membershipOf } from "../setup/membership";
import { REMOVAL_ID, REMOVAL_OPERATIONS_FEATURE, removalsOf, type MemberRemoval } from "../setup/removal-record";
import { setupSaveReceipt } from "../../shared/setup-local";
import { choiceHash, keepRemovalChoice, removalRecipe, preparationClaim, preparationOf, removalLock, removalPlanForPeer,
  type Preparation } from "../setup/before-wipe";
import { exclusiveWhile } from "../../shared/files";
import { advanceLocalRemoval } from "../setup/removal";
import { scheduleWipe, wipeSupportedHere } from "../setup/wipe";
import { REINSTALL_LOCAL } from "../setup/install-offer";
import { noteDocsChanged } from "../documents/changed";
import { updateAccess } from "../setup/access";
import type { CommittedReceipt, SaveReply, SavedReply, UnsavedReply } from "../../shared/routes/setup-save";
import { awaitSharedSave, pendingShared, queryOperation, removalReceipt, sharedSaveReply, stageShared, withSaveOutcome, type Receipt } from "../setup/shared";
import { setupSeedRefusal } from "../setup/seed";
import { isObject } from "../../shared/json";
import { LEADERSHIP_WAIT, primaryFenced } from "../setup/lease";
import { plainName } from "../../shared/text";
import type { ComputerAction, ListedComputer } from "../../shared/routes/box";
import { ownDoc, primaryPeersBody } from "../documents/work";
import { macDocsChanged } from "../documents/exchange";
import { APP_FOLDER, IS_MAC, K } from "../home";
import { movedFromOldApp } from "../rehome/moved";
import { tellGone } from "../mac/link";
import { LIST_FILE, localDestination, peerAtDestination, savedMachines } from "../mac/machines";
import { setMacPeers } from "../mac/routes";
import { proposeLeadership, standbyHere } from "../setup/handover";
import { PRIMARY_UNLINK, STANDBY_UNLINK } from "../setup/leadership";
import { followedSetup, requiresStandby } from "../setup/record";
import { macName, mainElsewhere } from "../setup/main-only";
import { macLink, readLink, updateLink } from "../state";
import { writePrivate } from "../files";
import { ownName } from "../letters/start";
import type { LinkState } from "../state";
import { takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { readPeerAgents } from "./directory";
import { KEY, fitsPeer, macsOf, namedBoxes, peersOf } from "./known";
import { awaySince } from "./reach";
import type { MailSender, Peer } from "./known";

/** What a box's agents' mail says when it renames and removes the machines it writes for others. */
export const MANAGE_FEATURE = "computers.manage";

/** The longest name a computer is given. */
export const MOST_NAME = 100;

/** Who writes your machines' records, as this box knows: itself, the computer it hands what you do
 * to (by its key and name), or none (no Mac's). */
export type Writer = { here: true } | { here: false; key: string; name: string } | null;

export function writerOf(link: LinkState | null | undefined): Writer {
  if (!link) return null;
  // (Named as its own record names it, else as this box lists it.)
  const named = (key: string, name: string, or: string) => name || peersOf(link).find(peer => peer.key === key)?.name || or;
  const main = mainElsewhere(link);
  if (main) return { here: false, key: main.key, name: named(main.key, main.name, "your main computer") };
  if (IS_MAC || followedSetup(link)?.role === "primary") return { here: true };
  const macs = macsOf(link);
  return macs.length === 1 ? { here: false, key: macs[0]!, name: named(macs[0]!, macName(link, macs[0]!), "the Mac that set it up") } : null;
}

/** The machines whose records the writer keeps, by key, as this box knows them: a Mac's own, or
 * those the writer's own document names (this machine among them; never another Mac's machines,
 * which that Mac keeps). */
function writtenMachines(link: LinkState | null | undefined, writer: Writer): Set<string> {
  if (!writer) return new Set();
  const boxes = acceptedMembership(link)?.boxes ?? (writer.here ? peersOf(link) : namedBoxes(link, writer.key));
  const keys = new Set(boxes.filter(box => box.kind === "machine" || link?.setup?.managed).map(box => box.key));
  if (link?.setup?.managed) keys.add(link.setup.primary);
  return keys;
}

/** What this box knows of the writer, worked out once for a whole list: who it is, the machines
 * whose records it keeps, whether it takes what you do from here, and whether this box can hand it
 * on at all (a Mac's box reaches only its own machines, never another Mac). */
export interface WriterFacts {
  writer: Writer;
  machines: Set<string>;
  takes: boolean;
  reachable: boolean;
  /** The old Kuvo Code app (before 1.0) is attached to the writer's box: it keeps its own list of
   * machines and writes it back, so a removal there wouldn't hold. */
  oldApp: boolean;
}

/** How long after the old app last said its link's config (`kuvo/link/config`, the first thing it
 * does as it attaches) it counts as attached, with no follow yet. */
export const OLD_APP_GRACE = 60_000;

/** Whether the old Kuvo Code app (the Mac app before 1.0) is attached to this Mac's box now: it,
 * and only it, follows the box's link (`kuvo/link/status/events`) for as long as it's open, and
 * says its link's config first, as it attaches or attaches again. */
export const oldAppAttached = (now = Date.now()): boolean => IS_MAC && !movedFromOldApp() && (macLink.statusFollowers.size > 0 || now - (macLink.appSaid ?? -Infinity) < OLD_APP_GRACE);


/** Why a removal isn't taken while the old app is attached to the Mac `mac` (its name). */
export const oldAppWords = (mac: string, name: string): string => `Quit the old Kuvo Code app on ${mac} first: it keeps its own list of machines, and would add ${name} back.`;

export function writerFacts(link: LinkState | null = readLink()): WriterFacts {
  const writer = writerOf(link);
  const reachable = Boolean(writer && (writer.here || peersOf(link).some(peer => peer.key === writer.key)));
  const told = writer && !writer.here ? readPeerAgents()[writer.key] : undefined;
  return { writer, machines: writtenMachines(link, writer), takes: Boolean(writer && (writer.here || (told?.features ?? []).includes(MANAGE_FEATURE))), reachable,
    // (A writer that's away is said to be away: what it last said of its app may be long over.)
    oldApp: Boolean(writer && !link?.setup?.managed && (writer.here ? oldAppAttached() : told?.computer?.oldApp === true && awaySince(writer.key) === null)) };
}

/** Why `computer` can't be renamed (`rename`) or removed (`remove`) from this box, in words a client
 * shows as they are; null when it can. `here`: this box's own name. */
export function whyNot(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "via">, what: "rename" | "remove", here: string, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): string | null {
  const name = computer.name || "This computer";
  const { writer } = facts;
  if (what === "remove" && computer.key && computer.key === link?.setup?.primary) {
    return lonePrimary(link, computer.key) ? null : PRIMARY_UNLINK;
  }
  if (computer.kind === "mac" && !link?.setup?.managed) return what === "rename" ? `${name} is a Mac: it goes by the name it has there.` : `A Mac can't be removed from here.`;
  if (what === "remove" && computer.here && !link?.setup?.managed) return `${here} can't remove itself: remove it from another of your computers.`;
  if (!writer) return macsOf(link).length > 1 ? `${what === "rename" ? "Rename" : "Remove"} ${name} from the Mac that added it.` : `No Mac keeps ${name}'s record, so it can't be ${what === "rename" ? "renamed" : "removed"} from here.`;
  if (!facts.reachable) return `${here} can't reach ${(writer as { name: string }).name}, which keeps your machines' records: ${what} ${name} from there.`;
  if (!computer.key || !facts.machines.has(computer.key)) return `${name} isn't one of ${writer.here ? here : writer.name}’s machines, so it can't be ${what === "rename" ? "renamed" : "removed"} from here.`;
  if (!facts.takes) return `${(writer as { name: string }).name} needs a newer Kuvo Code to ${what} your machines. Update it, then try again.`;
  return null;
}

/** Why removing `computer` waits now, where it would be offered otherwise: the old app attached to
 * the writer's box. */
function waitsFor(computer: Pick<ListedComputer, "name">, here: string, facts: WriterFacts): string | null {
  if (!facts.oldApp || !facts.writer) return null;
  return oldAppWords(facts.writer.here ? here : facts.writer.name, computer.name || "it");
}

/** Why `computer` can't be removed (or renamed) from this box now, all of it: what `whyNot` says,
 * or that it waits for the old app to be quit. */
export function refusal(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here" | "via">, what: "rename" | "remove", here: string, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): string | null {
  return whyNot(computer, what, here, link, facts) ?? (what === "remove" ? waitsFor(computer, here, facts) : null);
}

/** What can be done with `computer` from this box now, in the order to offer them. `trying`: it's
 * being reached again now (Connect was asked). */
export function actionsOf(computer: ListedComputer, here: string, trying: boolean, link: LinkState | null = readLink(), facts: WriterFacts = writerFacts(link)): ComputerAction[] {
  const actions: ComputerAction[] = [];
  const name = computer.name || "this machine";
  if (!whyNot(computer, "rename", here, link, facts)) actions.push({ id: "rename", label: "Rename" });
  if (computer.via === "relay" && computer.reach === "away" && !trying) actions.push({ id: "connect", label: "Connect" });
  if (!whyNot(computer, "remove", here, link, facts)) {
    // (Offered, and while it waits for the old app to be quit, said why rather than left out.)
    const waits = waitsFor(computer, here, facts);
    const last = computer.key && lonePrimary(link, computer.key);
    if (last && wipeSupportedHere(true)) actions.push({ id: "remove", label: "Wipe", confirm: { title: `Wipe ${name}?`, message: "This is the only computer. Wiping deletes Kuvo Code and its sessions and bots. No setup is left.", label: "Wipe" } });
    else if (!last) actions.push({ id: "remove", label: link?.setup?.checkpointed ? "Unlink" : "Remove", ...(waits ? { why: waits } : { confirm: { title: `Remove ${name}?`, message: `Your other computers stop reaching ${name}. It keeps running, with its sessions, and you can add it again later.`, label: "Remove" } }) });
  }
  return actions;
}

/** A name as given for a computer, cleaned: null for none, or one too long. */
export function givenName(name: unknown): string | null {
  const clean = plainName(name);
  return clean && clean.length <= MOST_NAME ? clean : null;
}

type RenameReply = SaveReply<{ name: string }> | UnsavedReply<200, { name: string }> | UnsavedReply<404 | 409 | 503>;
type RemovalReply = SaveReply | { status: 503; value: CommittedReceipt } | UnsavedReply<200 | 404 | 409 | 503>;

const answer = <S extends number, T extends UnsavedReply<S>["value"]>(status: S, value: T): UnsavedReply<S, T> => ({ status, value });

/** Renames the machine `key` here, on the box that writes its record: its peers document names it so
 * from now on, and your other computers take it from there. */
export async function renameHere(key: string, name: string): Promise<RenameReply> {
  const seed = await setupSeedRefusal("setupNames");
  if (seed) return answer(seed.status, { message: seed.message });
  let found = false;
  let waitingForStandby = false;
  let save: Receipt | null = null;
  const changed = updateLink(link => {
    if (!link || mainElsewhere(link) || !writtenMachines(link, { here: true }).has(key)) return null;
    found = true;
    if (requiresStandby(link.setup) && !link.setup?.standby) {
      waitingForStandby = true;
      return null;
    }
    if (link.setup?.standby || link.setup?.checkpointed) {
      const before = link.shared?.setupNames?.value;
      save = stageShared(link, "setupNames", { ...(isObject(before) ? before : link.peerNames), [key]: name });
      return save ? link : null;
    }
    if (link.peerNames?.[key] === name) return null;
    link.peerNames = { ...(link.peerNames ?? {}), [key]: name };
    return link;
  }, { durable: true });
  if (waitingForStandby) return answer(503, { message: LEADERSHIP_WAIT });
  if (!found) return answer(404, { message: `That isn't one of ${ownName()}'s machines.` });
  const receipt = await awaitSharedSave(save);
  if (receipt && receipt.status !== "committed") return sharedSaveReply(receipt);
  if (!changed && (readLink()?.setup?.standby || readLink()?.setup?.checkpointed)) return answer(409, { message: "A computer name is already waiting to be saved. Try again when it has saved." });
  macDocsChanged();
  if (receipt) return { status: 200, value: { name, ...receipt } } satisfies SavedReply<{ name: string }>;
  return answer(200, { name });
}

export function lonePrimary(link: LinkState | null, key: string): boolean {
  return followedSetup(link)?.role === "primary" && link?.setup?.primary === key && !peersOf(link).length
    && !link.leadershipPending && !pendingShared(link, "membershipAndAccess");
}

export function readRemovalRequest(body: unknown): RemovalRequest | null {
  if (body == null) return {};
  if (!isObject(body) || (body.wipe !== undefined && typeof body.wipe !== "boolean")
    || (body.operation !== undefined && (typeof body.operation !== "string" || !REMOVAL_ID.test(body.operation)))
    || (body.save !== undefined && !setupSaveReceipt(body.save))) return null;
  const supersedes = typeof body.supersedes === "string" && REMOVAL_ID.test(body.supersedes) ? body.supersedes : undefined;
  const resolve = body.resolve === "cancel" || body.resolve === "lost" ? body.resolve : undefined;
  if ((body.supersedes !== undefined && !supersedes) || (body.resolve !== undefined && !resolve)
    || ((supersedes || resolve) && (!body.operation || body.save !== undefined || body.retryPreparation || (supersedes && resolve)))) return null;
  const prepare = body.prepare === undefined ? undefined : preparationOf(body.prepare);
  if (prepare === null || (body.retryPreparation !== undefined && typeof body.retryPreparation !== "boolean")
    || ((prepare || body.retryPreparation) && (typeof body.operation !== "string" || body.save !== undefined))) return null;
  if ((supersedes && (!prepare || body.operation === supersedes)) || (resolve && prepare)) return null;
  return { supersedes, resolve, wipe: body.wipe as boolean | undefined, operation: body.operation as string | undefined, save: body.save, prepare, retryPreparation: body.retryPreparation === true ? true : undefined };
}

export interface RemovalRequest {
  wipe?: boolean;
  operation?: string;
  save?: unknown;
  prepare?: Preparation;
  retryPreparation?: true;
  supersedes?: string;
  resolve?: "cancel" | "lost";
}

function removalReply(record: Omit<MemberRemoval, "peer">, receipt: Receipt | null, status?: number): Acted {
  const { operation, wipe, delivered, export: point, plan, generation, resolution, error, unresolved } = record;
  const receiptStatus = receipt ? sharedSaveReply(receipt).status : 200;
  let state = "unlinked";
  if (receipt?.status === "pending") state = "saving";
  else if (receipt?.status === "interrupted") state = "interrupted";
  else if (resolution) state = resolution;
  else if (wipe) state = delivered ? "wiped" : error ? "review" : generation ? "wipe-pending" :
    plan?.moveTo ? "moving" : plan?.archive ? "exporting" : "wipe-pending";
  const offer = record.local && (state === "wiped" || !APP_FOLDER && state === "wipe-pending") && installOffersLayoutReady(K) ? REINSTALL_LOCAL : undefined;
  return { status: status ?? receiptStatus, value: { ...receipt, removal: { operation, state, accessRemoved: !receipt || receipt.status === "committed" },
    ...(offer ? { offer } : {}),
    ...(point ? { export: point } : {}), ...(error ? { message: error } : {}), ...(unresolved?.length ? { unresolved } : {}) } };
}

export async function removeComputerHere(key: string, pendingOnly = false, request: RemovalRequest = {}): Promise<Acted> {
  const setup = followedSetup(readLink());
  if (!setup || !KEY.test(key) || !wipePreparationLayoutReady(K)) return settleComputerRemoval(key, pendingOnly, request);
  try {
    return await exclusiveWhile(removalLock(setup.id, key), () => settleComputerRemoval(key, pendingOnly, request));
  } catch (error) {
    const message = error instanceof Error ? error.message : "";
    return answer(409, { message: message === "A backup or restore is already running."
      ? "This computer’s removal is already being checked. Try the same operation again."
      : message || "The removal is busy." });
  }
}

async function amendRemoval(setup: string, record: MemberRemoval, next: MemberRemoval, operation?: string): Promise<Acted> {
  const { save } = updateAccess(current => {
    if (current?.setup?.id !== setup || primaryFenced(current) || pendingShared(current, "ownershipAssignments") ||
      JSON.stringify(current.memberRemovals?.[record.peer.key]) !== JSON.stringify(record)) return null;
    current.memberRemovals = { ...current.memberRemovals, [record.peer.key]: next };
    return current;
  }, { checkpoint: true, operation });
  if (!save) return answer(409, { message: "The removal changed. Refresh it and try again." });
  noteDocsChanged();
  const receipt = await awaitSharedSave(save);
  return removalReply(receipt?.status === "committed" ? next : record,
    operation ? receipt : removalReceipt(readLink()!, record.operation), receipt ? sharedSaveReply(receipt).status : undefined);
}

async function settleComputerRemoval(key: string, pendingOnly: boolean, request: RemovalRequest): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  const previous = removalsOf(link)[key];
  const pending = membershipOf(pendingShared(link, "membershipAndAccess")?.body.value)?.removals?.[key];
  const last = lonePrimary(link, key);
  const modern = !pendingOnly && (request.wipe !== undefined || request.operation !== undefined || setup?.checkpointed || last);
  if (modern && !removalLayoutReady(K)) return answer(503, { message: "Finish updating Kuvo Code before removing this computer." });
  const ticket = setupSaveReceipt(request.save);
  if (ticket) {
    const outcome = queryOperation(ticket.setup, ticket.operation, ticket.proof, "membershipAndAccess");
    if (!outcome) return answer(404, { message: "That removal receipt is unknown." });
    const receipt = link ? removalReceipt(link, ticket.operation) : null;
    const record = previous?.operation === ticket.operation ? previous : pending?.operation === ticket.operation ? pending : undefined;
    if (record) return removalReply(record, receipt);
    if (previous?.superseded?.[ticket.operation]) return removalReply({ operation: ticket.operation, wipe: true, resolution: "cancelled" }, receipt);
    if (last || outcome.status === "interrupted") return removalReply({ operation: ticket.operation, wipe: last || request.wipe === true, ...(last ? { local: true } : {}) },
      receipt ?? withSaveOutcome(ticket, outcome.status));
    return answer(404, { message: "That removal receipt is unknown for this computer." });
  }
  if (modern && (!setup || setup.role !== "primary")) return answer(409, { message: "Only the main computer can remove a computer." });
  if (modern && !setup?.checkpointed && requiresStandby(setup)) {
    return answer(409, { message: "Finish upgrading accepted setup saves before removing this computer." });
  }
  if (key === setup?.primary && !last) return answer(409, { message: PRIMARY_UNLINK });
  if (last && !wipeSupportedHere(true)) return answer(409, { message: "This computer’s data layout cannot be wiped from here." });
  if (last && oldAppAttached()) return answer(409, { message: "Quit the old Kuvo Code app before wiping this computer." });
  if (link?.localWipe && !last) return answer(409, { message: "This computer is already scheduled to be wiped." });

  if (request.prepare && (!request.wipe || pendingOnly || !wipePreparationLayoutReady(K))) {
    return answer(409, { message: "Wipe preparation requires a confirmed wipe on the current layout." });
  }
  const exportDestination = request.prepare?.export?.destination;
  if (exportDestination?.kind === "computer" && !exportDestination.ssh) {
    return answer(409, { message: "Make the Relay backup before unlinking. For the wipe export, choose a separate disk or a computer over SSH." });
  }
  const claim = preparationClaim(link, key);
  const existing = previous ?? pending;
  const operation = request.operation ?? existing?.operation ?? (last ? link?.localWipe?.operation : undefined)
    ?? crypto.randomBytes(16).toString("base64url");
  if ((request.prepare || request.resolve || request.supersedes) && !removalOperationsLayoutReady(K)) {
    return answer(503, { message: "Finish updating Kuvo Code before changing a removal plan." });
  }
  if (request.prepare && setup) {
    if (request.prepare.moveTo === key || (request.prepare.moveTo &&
      !acceptedMembership(link)?.boxes.some(box => box.key === request.prepare!.moveTo))) {
      return answer(409, { message: "Choose another computer in this setup for the moves." });
    }
    if ((!last && !previous && !takesForwarded(key, REMOVAL_OPERATIONS_FEATURE)) ||
      (setup.standby && !takesForwarded(setup.standby.key, REMOVAL_OPERATIONS_FEATURE)) ||
      (request.prepare.moveTo && request.prepare.moveTo !== setup.primary && !takesForwarded(request.prepare.moveTo, REMOVAL_OPERATIONS_FEATURE))) {
      return answer(409, { message: "Update the source, standby and move destination before preparing this removal." });
    }
  }
  const restartLocal = last && previous?.local && previous.resolution === "cancelled" && previous.plan &&
    operation !== previous.operation && !previous.superseded?.[operation] && !request.resolve && !request.supersedes && !request.retryPreparation
    ? previous : undefined;
  if (modern && existing && setup && !restartLocal) {
    if (existing.superseded?.[operation]) {
      return removalReply({ operation, wipe: true, resolution: "cancelled" }, removalReceipt(link!, operation));
    }
    if (request.supersedes && existing.operation !== operation) {
      if (!request.prepare || !previous?.plan || previous.operation !== request.supersedes ||
        previous.generation || previous.withdrawnGeneration || previous.delivered || previous.resolution || primaryFenced(link) ||
        (pending?.operation === operation && pending.plan?.choiceHash !== choiceHash(request.prepare))) {
        return answer(409, { message: "This removal is no longer open for revision." });
      }
      keepRemovalChoice(setup.id, key, previous.peer.name, operation, request.prepare);
      if (pending?.operation === operation) return removalReply(pending, removalReceipt(link!, operation));
      return amendRemoval(setup.id, previous, { peer: previous.peer, operation, wipe: true, local: previous.local, requiresPreparation: true,
        plan: removalRecipe(request.prepare), superseded: { ...previous.superseded, [previous.operation]: previous.plan } }, operation);
    }
    const record = pending?.operation === operation && previous?.operation !== operation ? pending : existing;
    if (operation !== record.operation || (request.wipe !== undefined && request.wipe !== record.wipe))
      return answer(409, { message: "This computer has already been removed with a different request." });
    if (request.prepare) {
      if (record.plan?.choiceHash !== choiceHash(request.prepare)) return answer(409, { message: "Revise this removal with a new operation to change the choices." });
      if (!record.resolution && !record.delivered && !primaryFenced(link)) keepRemovalChoice(setup.id, key, record.peer.name, operation, request.prepare);
    }
    if (request.resolve && previous?.operation === operation) {
      if (previous.resolution === (request.resolve === "cancel" ? "cancelled" : "lost")) return removalReply(previous, removalReceipt(link!, operation));
      if (!previous.wipe || (previous.local && request.resolve === "lost") || previous.delivered || previous.resolution ||
        (request.resolve === "cancel" && (!previous.plan || previous.generation || previous.withdrawnGeneration))) {
        return answer(409, { message: "The removal has already advanced past this change." });
      }
      const next = { ...previous, resolution: request.resolve === "cancel" ? "cancelled" as const : "lost" as const };
      delete next.error;
      delete next.unresolved;
      return amendRemoval(setup.id, previous, next);
    }
    if (request.retryPreparation && previous?.operation === operation) {
      if (!previous.plan || previous.resolution || previous.delivered || !previous.error) {
        return answer(409, { message: "This removal has no preparation failure to retry." });
      }
      const next = { ...previous, retry: crypto.randomBytes(16).toString("base64url") };
      delete next.error;
      delete next.unresolved;
      return amendRemoval(setup.id, previous, next);
    }
    const receipt = removalReceipt(link!, operation);
    if (previous?.operation === operation) {
      if (previous.local) advanceLocalRemoval(link!);
      else cleanRemovalList(key);
    }
    return removalReply(record, receipt);
  }
  if (request.supersedes || request.resolve) return answer(404, { message: "That removal operation is unknown." });
  if (request.retryPreparation) return answer(409, { message: "There is no wipe preparation to retry." });
  if (last && link?.localWipe) {
    if (link.localWipe.operation !== operation) return answer(409, { message: "A wipe is already scheduled for this computer." });
    scheduleWipe();
    return removalReply({ operation, wipe: true, local: true }, removalReceipt(link, operation));
  }
  const removal: Omit<MemberRemoval, "peer"> | undefined = modern ? { operation, wipe: last || request.wipe === true,
    ...(claim || request.prepare ? { requiresPreparation: true } : {}) } : undefined;
  const requested = claim?.operation;
  if (requested && (!removal?.wipe || requested !== operation)) {
    return answer(409, { message: "Retry the existing wipe operation with the same choices." });
  }
  if (claim && (!request.prepare || claim.choiceHash !== choiceHash(request.prepare))) return answer(409, { message: "Resume this removal with its original choices." });
  if (last && setup) {
    const currentLayout = removalOperationsLayoutReady(K);
    const own = primaryPeersBody(link!).boxes.find(box => box.key === key);
    if (!own || !fitsPeer(own)) return answer(409, { message: "This computer’s setup identity is unavailable." });
    if (request.prepare) keepRemovalChoice(setup.id, key, own.name, operation, request.prepare);
    const plan = request.prepare && removalRecipe(request.prepare);
    const { save, busy } = updateAccess(current => {
      if (!current || !lonePrimary(current, key) || current.localWipe || current.setup?.id !== setup.id) return null;
      if (current.wipePreparations) delete current.wipePreparations[key];
      if (currentLayout) {
        current.memberRemovals = { ...current.memberRemovals, [key]: { peer: own, operation, wipe: true, local: true,
          ...(plan ? { plan, requiresPreparation: true } : {}),
          ...(restartLocal?.plan ? { superseded: { ...restartLocal.superseded, [restartLocal.operation]: restartLocal.plan } } : {}) } };
      }
      if (!plan) current.localWipe = { setup: setup.id, operation };
      current.phones = [];
      delete current.pairing;
      delete current.appPairing;
      return current;
    }, { operation, checkpoint: true });
    if (busy || !save) return answer(409, { message: "The setup changed before the wipe could be saved. Refresh it and try again." });
    if (plan) advanceLocalRemoval(readLink()!);
    else scheduleWipe();
    return removalReply({ operation, wipe: true, local: true, ...(plan ? { plan } : {}) }, save);
  }
  if (removal && setup) {
    const peer = peersOf(link).find(each => each.key === key);
    if (peer && request.prepare) {
      keepRemovalChoice(setup.id, key, peer.name, operation, request.prepare);
      removal.plan = removalRecipe(request.prepare);
    }
  }
  return removePeer(key, pendingOnly, removal, setup?.id);
}

/** Retire the old standby before saving the access change. */
async function removePeer(key: string, pendingOnly: boolean, removal?: Omit<MemberRemoval, "peer">, setupID?: string): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  if (!pendingOnly && setup?.role === "primary" && setup.standby?.key === key) {
    const candidates = peersOf(link).filter(peer => peer.key !== key && takesForwarded(peer.key, "setup.shared-saves"));
    const order = acceptedMembership(link)?.successorOrder;
    if (order) candidates.sort((a, b) => order.indexOf(a.key) - order.indexOf(b.key));
    const position = { setup: setup.id, epoch: setup.epoch };
    let outcome = peersOf(link).length === 1
      ? await proposeLeadership(position, setup.primary, null, { retire: key })
      : answer(409, { message: STANDBY_UNLINK });
    for (const candidate of candidates) {
      outcome = await standbyHere(candidate.key, position, { reason: "standby retirement", retiring: setup.successorChain ? key : undefined });
      const current = readLink();
      if (outcome.status === 202) break;
      if (current?.leadershipPending || current?.setup?.id !== setup.id || current.setup.epoch !== setup.epoch) return outcome;
    }
    if (outcome.status !== 202) return outcome;
  }
  const result = await finishRemoval(prepareRemoval(key, pendingOnly, removal, setupID));
  const receipt = removal && setupSaveReceipt(result.value);
  return removal && receipt ? removalReply(removal, receipt, result.status) : result;
}

/** Removes the machine `key` here, on the box that writes its record: its peers document names it
 * among those you removed (`left`), so it lets this Mac go and your other computers drop it, and
 * this Mac's SSH list no longer has it. It keeps running there, as a box of its own. */
// Legacy in-process callers can complete immediately. The routed removal waits for a receipt.
export function removeHere(key: string, pendingOnly = false): RemovalReply {
  const removal = prepareRemoval(key, pendingOnly);
  return removal.save && removal.save.status !== "committed" ? sharedSaveReply(removal.save) : completeRemoval(removal, removal.save);
}

async function finishRemoval(removal: ReturnType<typeof prepareRemoval>): Promise<RemovalReply> {
  const { save } = removal;
  const receipt = await awaitSharedSave(save);
  if (receipt && receipt.status !== "committed") return sharedSaveReply(receipt);
  return completeRemoval(removal, receipt);
}

function prepareRemoval(key: string, pendingOnly: boolean, removal?: Omit<MemberRemoval, "peer">, setupID?: string) {
  let blocked: string | null = null;
  let gone: Peer[] = [];
  let already = false;
  let needsCleanup = false;
  const result = updateAccess(link => {
    if (!link) return null;
    if (removal && link.setup?.id !== setupID) { blocked = "The setup changed. Refresh it and try again."; return null; }
    if (pendingOnly && primaryFenced(link)) { blocked = LEADERSHIP_WAIT; return null; }
    if (!pendingOnly && link.wipePreparations?.[key] && link.wipePreparations[key].operation !== removal?.operation) {
      blocked = "The wipe preparation changed. Refresh the removal and try again.";
      return null;
    }
    // A new removal needs a receipt, while cleanup on retry only changes the local address list.
    if (link.setup?.standby?.key === key) { blocked = STANDBY_UNLINK; return null; }
    if (link.setup?.primary === key || mainElsewhere(link)) {
      blocked = link.setup?.primary === key ? PRIMARY_UNLINK : "Only the main computer can remove a computer.";
      return null;
    }
    const peer = peersOf(link).find(each => each.key === key);
    if (pendingOnly && (peer || !link.pendingMachineCleanup?.[key])) return null;
    already = Boolean(link.removedHere?.includes(key));
    const destination = peer && localDestination(link, key);
    needsCleanup = Boolean(destination || link.pendingMachineCleanup?.[key]);
    if (!peer) return null;
    const made = setMacPeers(link, { peers: (Array.isArray(link.macPeers) ? link.macPeers : []).filter(each => each?.key !== key), moved: link.macMoved, removed: [key] }, { staging: Boolean(link.setup?.standby) });
    if (!made.link) return null;
    gone = made.gone;
    if (made.link.peerNames) delete made.link.peerNames[key];
    if (removal) {
      const { id: _destination, ...portable } = peer;
      if (made.link.wipePreparations) delete made.link.wipePreparations[key];
      made.link.memberRemovals = { ...made.link.memberRemovals, [key]: { ...removal, peer: portable } };
    } else if (made.link.setup?.managed) {
      // Keep the removal at this epoch for a member that misses it before standby rotation.
      // Build on a copy so the normal publisher still advances its own document counter.
      const roster = ownDoc({ ...made.link }, "peers", primaryPeersBody(made.link));
      if (roster) {
        made.link.untold = [...(made.link.untold ?? []).filter(each => each.key !== key), ...gone].slice(-64);
        made.link.untoldMembership = {
          ...Object.fromEntries(Object.entries(made.link.untoldMembership ?? {}).filter(([key]) => made.link!.untold!.some(peer => peer.key === key))),
          [key]: roster.envelope,
        };
      }
    }
    if (destination) made.link.pendingMachineCleanup = { ...made.link.pendingMachineCleanup, [key]: destination };
    return made.link;
  }, { localOnly: pendingOnly, operation: removal?.operation, checkpoint: Boolean(removal) });
  return { ...result, blocked, gone, already, needsCleanup, key };
}

function completeRemoval({ changed, blocked, gone, already, needsCleanup, key, busy }: ReturnType<typeof prepareRemoval>, save: CommittedReceipt | null): RemovalReply {
  if (busy) return answer(409, { message: "An access change is already waiting to be saved." });
  if (blocked) return answer(409, { message: blocked });
  if (!gone.length && !already) return answer(404, { message: `That isn't one of ${ownName()}'s machines.` });
  macDocsChanged({ closeRemoved: save ? readLink() : changed });
  const removals = removalsOf(readLink());
  tellGone(gone.filter(peer => !removals[peer.key]));
  // Once the removal is durable, clean up its saved address. Leave a retry if the list fails.
  if (needsCleanup && !cleanRemovalList(key)) {
    const message = "The computer’s access was removed, but the saved machines list couldn’t be updated.";
    if (!save) return answer(503, { message });
    const value: CommittedReceipt = { ...save, message };
    return { status: 503, value };
  }
  return save ? sharedSaveReply(save) : answer(200, {});
}

function cleanRemovalList(key: string): boolean {
  let saved = true;
  try {
    updateLink(link => {
      const pending = link?.pendingMachineCleanup;
      const destination = pending?.[key];
      if (!link || !pending || !destination) return null;
      // A fresh Add may have kept the destination again since revocation.
      saved = Boolean(peerAtDestination(link, destination)) || forgetSaved(destination);
      if (!saved) return null;
      delete pending[key];
      if (!Object.keys(pending).length) delete link.pendingMachineCleanup;
      return link;
    });
  } catch (error) {
    console.error("Removal machines-list cleanup failed:", error);
    return false;
  }
  return saved;
}

// The machine `id` gone from this Mac's list of machines (`machines.json`), as its app would take
// it off: what `kuvo/machines` and whether the link meets machines go by. One that won't read stays.
export function forgetSaved(id: string): boolean {
  const saved = savedMachines();
  if (!saved) return false;
  if (!saved.some(each => each.destination === id)) return true;
  try {
    writePrivate(LIST_FILE, JSON.stringify(saved.filter(each => each.destination !== id), null, 2));
    return true;
  } catch {
    return false;
  }
}

/** What another of your computers' box handed here (`area: "computers"`): done as this box's own
 * client's, where this box writes that machine's record. */
export async function writerAct(sender: MailSender, act: Act): Promise<Acted> {
  if (act.action === "removal-plan") return removalPlanForPeer(sender, act);
  const link = readLink();
  const writer = writerOf(link);
  if (!writer?.here) return answer(409, { message: `${ownName()} doesn't keep your machines' records.` });
  // (The same rules as the asking box's, again here: it may be older, or not say true.)
  const target = peersOf(link).find(peer => peer.key === act.item);
  if (target && act.method === "DELETE" && !link?.setup?.managed && oldAppAttached()) return answer(409, { message: oldAppWords(ownName(), target.name) });
  if (act.method === "PATCH") {
    const name = givenName(act.body?.name);
    if (!name) return answer(400, { message: `Give it a name of up to ${MOST_NAME} characters.` });
    return renameHere(act.item, name);
  }
  if (act.method === "DELETE") {
    const request = readRemovalRequest(act.body);
    return request ? removeComputerHere(act.item, act.body?.cleanup === true, request) : answer(400, { message: "Send a valid removal request." });
  }
  return answer(405, { message: "Rename or remove." });
}
