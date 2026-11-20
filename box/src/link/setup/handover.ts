import type { ForwardGrant } from "../work-queue/forward-grant";
// The primary checks that the successor is running this protocol, then atomically commits
// a signed epoch change. Peers deliver the committed record with the regular document exchange.

import { setupLayoutDeferred, successorRankLayoutReady, successorRevisionLayoutReady, standbyIntentLayoutReady, standbyRevisionLayoutReady, unlinkCleanupLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import type { PersonalityLibrary } from "../personalities/library";
import { captureMembership } from "./access";
import { CHECKPOINT_FEATURE, checkpointedTransition } from "./checkpoint";
import { acceptedMembership, type Membership } from "./membership";
import { REMOVAL_OPERATIONS_FEATURE, removalsOf, PREPARATION_FEATURE } from "./removal-record";
import { changeShared, membershipLayoutAdmission, operationOutcome, sharedFits, waitShared, type SharedValues } from "./shared";
import type { ReaderLayouts } from "../updates/choice";
import { noteDocsChanged } from "../documents/changed";
import { held, ownSign, readDoc, signDoc } from "../documents/format";
import { primaryPeersBody, pruneDocs } from "../documents/work";
import { K } from "../home";
import { KEY, ownKey, peersOf, type MailSender, type Peer } from "../peers/known";
import { ownPhones } from "../phones";
import { readLink, updateLink, type LinkState } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { changeNumbers, installNumberAuthority, MOST_NUMBER, topOf } from "../work-queue/numbers";
import { writeCard } from "./card";
import { canSupersedeStandby, firstStandbyRevision, leadershipOf, leadershipProposalHash, NUMBER_RESERVE, takeLeadership, type ReplacementEnrollment } from "./leadership";
import { LEADERSHIP_WAIT, primaryFenced } from "./lease";
import { rankComputers } from "./reliability";
import { followedSetup, requiresStandby, type SetupRecord } from "./record";
import { gatherSetupCopies, offerSetupCopies, seedSetupCopies } from "./seed";

export const SOLE_PRIMARY_FEATURE = "setup.sole-primary";
export const STANDBY_ACCEPTANCE = "setup.standby-acceptance";
export const HANDOVER_FEATURE = "setup.handover";
const answer = (status: number, message: string): Acted => ({ status, value: { message } });

export async function handover(given: unknown, signal?: AbortSignal | null, grant?: ForwardGrant | null): Promise<Acted> {
  if (!isObject(given) || typeof given.primary !== "string" || !KEY.test(given.primary)) return answer(400, "Choose a computer by its key as the main computer.");
  const link = readLink();
  const setup = followedSetup(link);
  if (!setup) return answer(409, "This computer doesn't have an active setup.");
  if (setup.primary === given.primary) return { status: 200, value: { primary: setup.primary, epoch: setup.epoch } };
  if (given.epoch !== undefined && given.epoch !== setup.epoch) return answer(409, "The main computer changed. Refresh your setup and try again.");
  const position = { setup: setup.id, epoch: setup.epoch };
  if (setup.role === "primary") return handoverHere(given.primary, position);
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  if (!primary) return answer(503, "This computer can't reach your main computer.");
  const result = await forwardAct(primary, "Changing the main computer", { area: "setup", method: "POST", item: given.primary, body: position }, signal, link, Date.now(), { grant });
  return "refused" in result ? answer(result.status, result.refused) : result;
}

async function handoverHere(target: string, position: Record<string, unknown>, requester?: string, admit?: Act["admit"]): Promise<Acted> {
  const before = readLink();
  const setup = followedSetup(before);
  if (setup?.role !== "primary" || setup.id !== position.setup || setup.epoch !== position.epoch) return answer(409, "The main computer changed. Refresh your setup and try again.");
  if (primaryFenced(before) || requiresStandby(setup) && !setup.standby && peersOf(before).length) return answer(503, LEADERSHIP_WAIT);
  const peers = peersOf(before);
  const next = peers.find(peer => peer.key === target);
  if (!next?.sign) return answer(404, "Choose a member of this setup.");
  const older = peers.find(peer => !peer.sign || !takesForwarded(peer.key, HANDOVER_FEATURE));
  if (older) return answer(409, `${older.name || "A member"} needs to connect with a newer Kuvo Code before you can change the main computer.`);
  const ready = await forwardAct(next, "Changing the main computer", { area: "setup", method: "GET", item: target, body: position }, undefined, before, Date.now(), { backgroundRelay: true });
  if ("refused" in ready) return answer(ready.status, ready.refused);
  if (ready.status !== 200) return ready;
  if (!isObject(ready.value) || ready.value.sign !== next.sign) return answer(409, "The chosen computer's signing key changed. Refresh your setup and try again.");

  const veto = admit?.();
  if (veto) return veto;
  return proposeLeadership(position, target, setup.standby?.key !== target ? setup.standby ?? null : null, { requester, targetSign: next.sign, admit });
}

/** Recover a delegated setup by adding one previously unknown computer to its accepted replica. */
export function enrollStandby(setup: SetupRecord, peer: Peer & { sign: string }, readers: ReaderLayouts, prepare: (link: LinkState, enrollment: ReplacementEnrollment) => void, record?: (membership: Membership, link: LinkState) => void): Promise<Acted> {
  return proposeLeadership({ setup: setup.id, epoch: setup.epoch }, setup.primary, peer, { recovery: { peer, readers, prepare, record } });
}

/** The number ledger and the fenced proposal are frozen together. With a standby, only it
 * can turn the proposal into the next certificate. Sending is safe to retry after a restart. */
export async function proposeLeadership(
  position: Record<string, unknown>,
  target: string,
  standby: NonNullable<SetupRecord["standby"]> | null,
  { requester, targetSign, recovery, retire, replace, supersedes, pin, retiring, numberThrough, admit }: { admit?: Act["admit"]; retiring?: string; pin?: true; supersedes?: string; replace?: true; retire?: string; requester?: string; targetSign?: string; numberThrough?: number; recovery?: { peer: Peer; readers: ReaderLayouts; prepare: (link: LinkState, enrollment: ReplacementEnrollment) => void; record?: (membership: Membership, link: LinkState) => void } } = {},
): Promise<Acted> {
  if (setupLayoutDeferred(K)) return answer(503, "Wait for the Kuvo Code update to finish before changing the setup.");
  const beforeSeed = readLink();
  const beforeSetup = followedSetup(beforeSeed);
  const gathered = beforeSeed && beforeSetup?.role === "primary" && beforeSetup.id === position.setup
    && beforeSetup.epoch === position.epoch && standby && !beforeSetup.standby
    ? await gatherSetupCopies(beforeSeed, position) : [];
  const veto = admit?.();
  if (veto) return veto;
  let oversized = false;
  let unsupportedLibrary = false;
  let unsupportedWipePreparation = false;
  let layoutBlocked: string | null = null;
  const changed = changeNumbers(numbers => updateLink(link => {
    const current = followedSetup(link);
    if (!link?.sign || current?.role !== "primary" || current.sign !== ownSign(link) || current.id !== position.setup || current.epoch !== position.epoch
      || link.localWipe || link.leadershipPending?.s !== supersedes) return null;
    const fenced = primaryFenced(link);
    if (replace && (!fenced || !current.successorChain || !current.standby || !standby || standby.key === current.standby.key)) return null;
    if (replace || supersedes) {
      if (!(supersedes ? canSupersedeStandby(link) : current.successorChain) || !standby || target !== current.primary || acceptedMembership(link)?.left.includes(current.primary)) return null;
    } else if (fenced || Object.keys(link.sharedPending ?? {}).length) return null;
    if (recovery && (!standby || current.standby || !requiresStandby(current) || target !== current.primary
      || recovery.peer.key !== standby.key || recovery.peer.sign !== standby.sign)) return null;
    if (recovery && (layoutBlocked = membershipLayoutAdmission(link, [recovery.peer], recovery.readers).hold ?? null)) return null;
    const members = peersOf(link);
    if (target !== current.primary && requiresStandby(current) && !current.standby && members.length) return null;
    if (retire && (!unlinkCleanupLayoutReady(K) || !current.checkpointed || standby || target !== current.primary
      || current.standby?.key !== retire || members.length !== 1 || members[0]?.key !== retire
      || !takesForwarded(retire, SOLE_PRIMARY_FEATURE))) return null;
    if (Object.keys(acceptedMembership(link)?.wipePreparations ?? {}).length
      && ((target !== current.primary && !takesForwarded(target, PREPARATION_FEATURE))
        || (standby && standby.key !== current.standby?.key && !takesForwarded(standby.key, PREPARATION_FEATURE)))) {
      unsupportedWipePreparation = true;
      return null;
    }
    if (Object.values(removalsOf(link)).some(record => record.plan && !record.delivered && !record.resolution)
      && ((target !== current.primary && !takesForwarded(target, REMOVAL_OPERATIONS_FEATURE))
        || (standby && standby.key !== current.standby?.key && !takesForwarded(standby.key, REMOVAL_OPERATIONS_FEATURE)))) {
      unsupportedWipePreparation = true;
      return null;
    }
    if (requester && !members.some(peer => peer.key === requester)) return null;
    if (target !== current.primary && (!members.some(peer => peer.key === target && peer.sign === targetSign)
      || members.some(peer => !peer.sign || !takesForwarded(peer.key, HANDOVER_FEATURE)
        || (Object.keys(link.shared ?? {}).length && !takesForwarded(peer.key, "setup.shared-saves"))))) return null;
    if (standby?.key === target) return null;
    if (standby && !recovery && (!members.some(peer => peer.key === standby.key && peer.sign === standby.sign)
      || members.some(peer => !peer.sign || !takesForwarded(peer.key, "setup.shared-saves")))) return null;
    const supportsCheckpoint = members.every(peer => takesForwarded(peer.key, CHECKPOINT_FEATURE));
    const acceptance = Boolean(replace || standby && !current.standby && (recovery || members.every(peer => takesForwarded(peer.key, STANDBY_ACCEPTANCE))));
    const successorChain = current.successorChain || Boolean(standby && supportsCheckpoint && !recovery && successorRankLayoutReady(K) &&
      (!current.checkpointed || !current.standby || acceptedMembership(link)?.successorOrder)
      && members.every(peer => takesForwarded(peer.key, "setup.successor-chain")));
    if (supersedes && !current.standby && !acceptance) return null;
    const pinAfterLease = pin && successorChain && !current.standby && acceptance && standbyIntentLayoutReady(K);
    if (retiring && (!standbyIntentLayoutReady(K) || current.standby?.key !== retiring || target !== current.primary || !standby || standby.key === retiring)) return null;
    const migrating = Boolean(current.standby && (supportsCheckpoint && !current.checkpointed || successorChain && !current.successorChain));
    const renewing = target === current.primary && (standby?.key ?? null) === (current.standby?.key ?? null);
    if (renewing && (!standby || !current.numberCeiling || (!migrating && !supersedes && current.numberCeiling >= MOST_NUMBER - 1))) return null;
    installNumberAuthority(numbers, link);
    const granted = { ...numbers.granted };
    if (granted.self) {
      granted[current.primary] = granted.self;
      delete granted.self;
    }
    const { boxes, left, moved } = primaryPeersBody(link);
    const epoch = current.epoch + 1;
    const next = topOf(numbers) + 1;
    const numberCeiling = Math.min(MOST_NUMBER - 1, Math.max(numberThrough ?? 0, next + NUMBER_RESERVE - 1));
    if (renewing && !migrating && !supersedes && numberCeiling <= current.numberCeiling!) return null;
    const shared: SharedValues = { ...(link.sharedSetup === current.id ? link.shared : {}) };
    let hasLibrary = false;
    if (standby && !current.standby) {
      if (!seedSetupCopies(link, shared, gathered)) {
        oversized = true;
        return null;
      }
      const library = shared.setupPersonalities?.value as PersonalityLibrary;
      hasLibrary = Boolean(library?.personalities?.length || library?.removed?.length);
    }
    const priorAssignment = supersedes && !current.standby && standbyRevisionLayoutReady(K)
      ? firstStandbyRevision(held(link.leadershipPending)) : undefined;
    const authority = {
      setup: { id: current.id, epoch, primary: target }, previous: current.primary,
      ...(retire ? { retiredStandby: retire } : standby || requiresStandby(current) ? { sharedSaves: true } : {}),
      ...(standby ? { standby: standby.key, numberCeiling: renewing && migrating && !numberThrough ? current.numberCeiling : numberCeiling } : {}),
      ...(current.standby || acceptance ? { proposal: true } : {}),
      ...(acceptance ? { standbyAcceptance: true } : {}),
      ...(successorChain ? { successorChain: true } : {}),
      ...(supersedes ? { supersedes } : {}),
      ...(priorAssignment !== undefined ? { assignmentRevision: priorAssignment + 1 } : {}),
      ...(pinAfterLease ? { pinAfterLease: true } : {}),
      ...(retiring ? { retiringStandby: retiring } : {}),
      ...(replace ? { replacesStandby: current.standby!.key } : {}),
      ...(recovery ? { recovery: true } : {}),
    };
    const ledger = { next, given: numbers.given, granted };
    const checkpointed = current.checkpointed || supportsCheckpoint || acceptance;
    if (checkpointed) {
      const membership = acceptedMembership(link) ?? captureMembership(link);
      if (!membership) return null;
      if (successorChain && !membership.successorOrder) membership.successorOrder = rankComputers(membership.boxes);
      if (recovery) {
        if (membership.boxes.some(peer => peer.key === recovery.peer.key) || membership.left.includes(recovery.peer.key)) return null;
        const { id: _destination, ...portable } = recovery.peer;
        membership.boxes.push(portable);
        membership.successorOrder = membership.successorOrder ? [...membership.successorOrder, recovery.peer.key] : rankComputers(membership.boxes);
        recovery.record?.(membership, link);
      }
      shared.membershipAndAccess = { sequence: (shared.membershipAndAccess?.sequence ?? (recovery ? 0 : 1)) + (recovery ? 1 : 0), value: membership };
      shared.queueNumberAllocation = { sequence: shared.queueNumberAllocation?.sequence ?? 1, value: ledger };
    }
    if (standby && !current.standby && !sharedFits(shared)) {
      oversized = true;
      return null;
    }
    if (hasLibrary && !acceptance) {
      unsupportedLibrary = true;
      return null;
    }
    const doc = checkpointed
      ? checkpointedTransition(link, authority, shared)
      : readDoc(signDoc(link.sign, "leadership", epoch, {
        ...authority, boxes, left, moved, devices: ownPhones(link),
        shared, removalRevisions: link.removalRevisions, numbers: ledger,
      }));
    if (!doc || !boxes.every(box => box.key && box.sign && box.address)) return null;
    recovery?.prepare(link, { id: current.id, epoch, key: recovery.peer.key, proposalHash: leadershipProposalHash(doc.envelope.s) });
    if (current.standby || acceptance) link.leadershipPending = doc.envelope;
    else {
      if (!takeLeadership(link, doc)) return null;
      pruneDocs(link);
    }
    return link;
  }, { durable: true }));
  if (layoutBlocked) return answer(409, layoutBlocked);
  if (oversized) return answer(413, "The personality library and setup settings are too large to replicate. Shorten the personality library before choosing a standby.");
  if (unsupportedWipePreparation) return answer(409, "Update the chosen computer so it can keep the pending wipe preparation.");
  if (unsupportedLibrary) return answer(409, "Update all members before choosing a standby so it can verify the personality library.");
  if (!changed) return answer(409, "The setup changed or the standby couldn't be reached. Refresh your setup and try again.");
  if (recovery && changed.leadershipPending) {
    return { status: 202, value: { primary: target, epoch: changed.setup!.epoch + 1 } };
  }
  if (changed.leadershipPending) {
    const deadline = Date.now() + 12_000;
    while (Date.now() < deadline) {
      const setup = readLink()?.setup;
      if (setup?.id !== position.setup || !setup || setup.epoch !== position.epoch) {
        if (setup?.id === position.setup && setup?.primary === target && setup.standby?.key === standby?.key && setup.standby?.sign === standby?.sign && (!retire || !requiresStandby(setup))) {
          return { status: 202, value: { primary: target, epoch: setup.epoch } };
        }
        return answer(409, "The setup changed. Refresh your setup and try again.");
      }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    return answer(503, LEADERSHIP_WAIT);
  }
  noteDocsChanged();
  try { writeCard(); } catch {}
  return { status: 202, value: { primary: target, epoch: changed.setup!.epoch } };
}

export async function setupAct(sender: MailSender, act: Act): Promise<Acted> {
  if (!act.body) return answer(400, "Send the setup and epoch.");
  if (act.method === "POST") {
    if (act.body.standby === true) return standbyHere(act.item, act.body, { requester: sender.key, pin: true, reason: "user choice", admit: act.admit });
    return handoverHere(act.item, act.body, sender.key, act.admit);
  }
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || setup?.role !== "member" || setup.primary !== sender.key || setup.id !== act.body.setup
    || setup.epoch !== act.body.epoch || ownKey(link) !== act.item || !ownSign(link)) {
    return answer(409, "The chosen computer hasn't caught up with the main computer yet.");
  }
  return { status: 200, value: { sign: ownSign(link), ...(act.body.sharedSeed === true ? offerSetupCopies(link, act.body.only) : {}) } };
}

export async function selectStandby(given: unknown, signal?: AbortSignal | null, grant?: ForwardGrant | null): Promise<Acted> {
  if (!isObject(given) || typeof given.standby !== "string" || !KEY.test(given.standby)) return answer(400, "Choose a member by its key as standby.");
  const link = readLink();
  const setup = followedSetup(link);
  if (!setup || (given.epoch !== undefined && setup.epoch !== given.epoch)) return answer(409, "The setup changed. Refresh it and try again.");
  if (setup.standby?.key === given.standby && (!setup.successorChain || acceptedMembership(link)?.successorPin === given.standby)) return { status: 200, value: { primary: setup.primary, epoch: setup.epoch } };
  const position = { setup: setup.id, epoch: setup.epoch };
  if (setup.role === "primary") return standbyHere(given.standby, position, { pin: true, reason: "user choice" });
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  if (!primary) return answer(503, "This computer can't reach your main computer.");
  const result = await forwardAct(primary, "Changing the standby", { area: "setup", method: "POST", item: given.standby, body: { ...position, standby: true } }, signal, link, Date.now(), { grant });
  return "refused" in result ? answer(result.status, result.refused) : result;
}

/** First assignment accepts the unchanged rank. Once leased, commit the stored choice
 * as an ordinary rank save. The certificate makes the intent recoverable across restart. */
export function finishFirstStandbyPin(link: LinkState): boolean {
  const setup = followedSetup(link);
  const tip = leadershipOf(link);
  const target = setup?.standby?.key;
  if (!setup || !target || tip?.body.pinAfterLease !== true || !standbyIntentLayoutReady(K)) return false;
  const operation = leadershipProposalHash(tip.envelope.s);
  const outcome = operationOutcome(link, setup.id, operation);
  if (outcome && outcome.status !== "pending") return false;
  if (!outcome && !primaryFenced(link)) {
    return Boolean(changeShared("membershipAndAccess", (_value, current) => {
      if (primaryFenced(current) || leadershipOf(current)?.h !== tip.h) return undefined;
      const membership = acceptedMembership(current);
      if (!membership?.successorOrder?.includes(target)) return undefined;
      return { ...membership, successorPin: target, successorOrder: [target, ...membership.successorOrder.filter(key => key !== target)] };
    }, operation));
  }
  return true;
}

export async function standbyHere(target: string, position: Record<string, unknown>,
  { requester, reason, retiring, pin, admit }: { requester?: string; reason: string; retiring?: string; pin?: true; admit?: Act["admit"] },
): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  if (setup?.role !== "primary" || setup.id !== position.setup || setup.epoch !== position.epoch) return answer(409, "The setup changed. Refresh it and try again.");
  const peers = peersOf(link);
  const peer = peers.find(peer => peer.key === target);
  if (!peer?.sign || !takesForwarded(target, "setup.shared-saves")) return answer(409, "Choose a member that supports standby.");
  if (peers.some(peer => !takesForwarded(peer.key, "setup.shared-saves"))) return answer(409, "Every member must connect on a version that supports standby first.");
  const ready = await forwardAct(peer, "Changing the standby", { area: "setup", method: "GET", item: target, body: position }, undefined, link, Date.now(), { backgroundRelay: true });
  if ("refused" in ready) return answer(ready.status, ready.refused);
  if (ready.status !== 200 || !isObject(ready.value) || ready.value.sign !== peer.sign) return answer(409, "The chosen standby hasn't caught up yet.");
  const veto = admit?.();
  if (veto) return veto;
  if (pin && setup.successorChain && setup.standby && successorRevisionLayoutReady(K)) {
    if (finishFirstStandbyPin(link!)) return answer(503, LEADERSHIP_WAIT);
    if (requester && !peers.some(member => member.key === requester)) return answer(403, "Only members can choose the standby.");
    const save = changeShared("membershipAndAccess", (_value, current) => {
      if (primaryFenced(current) || current.setup?.id !== setup.id || current.setup.epoch !== setup.epoch) return undefined;
      const membership = acceptedMembership(current);
      if (!membership?.successorOrder?.includes(target)) return undefined;
      return { ...membership, successorPin: target, successorOrder: [target, ...membership.successorOrder.filter(key => key !== target)] };
    });
    if (!save || (await waitShared(save, 12_000)).status !== "committed") return answer(503, LEADERSHIP_WAIT);
    const deadline = Date.now() + 12_000;
    while (Date.now() < deadline) {
      const current = readLink()?.setup;
      if (current?.id !== setup.id || current.primary !== setup.primary) break;
      if (current.standby?.key === target) {
        return { status: current.epoch === setup.epoch ? 200 : 202, value: { primary: current.primary, epoch: current.epoch } };
      }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    return answer(503, LEADERSHIP_WAIT);
  }
  const outcome = await proposeLeadership(position, setup.primary, { key: target, sign: peer.sign }, { requester, retiring, pin, admit });
  if (outcome.status === 202) {
    const tip = leadershipOf(readLink());
    if (pin && tip?.body.pinAfterLease === true && tip.body.standby === target) {
      const operation = leadershipProposalHash(tip.envelope.s);
      const deadline = Date.now() + 12_000;
      while (Date.now() < deadline) {
        const current = readLink();
        if (current?.setup?.id !== setup.id) break;
        if (operationOutcome(current, setup.id, operation)?.status === "committed") {
          console.info(`Standby selected: ${target} (${reason}).`);
          return outcome;
        }
        if (leadershipOf(current)?.h !== tip.h) break;
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      return answer(503, LEADERSHIP_WAIT);
    }
    console.info(`Standby selected: ${target} (${reason}).`);
  }
  return outcome;
}
