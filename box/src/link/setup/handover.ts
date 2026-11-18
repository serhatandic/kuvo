// The primary checks that the successor is running this protocol, then atomically commits
// a signed epoch change. Peers deliver the committed record with the regular document exchange.

import path from "node:path";
import { UsageSettings } from "../../agents/usage/settings";
import { isObject } from "../../shared/json";
import { readPersonalities } from "../personalities/library";
import { labels } from "../work-queue/labels";
import { readZoneSetting } from "../zone/setting";
import { captureMembership } from "./access";
import { CHECKPOINT_FEATURE, checkpointedTransition } from "./checkpoint";
import { acceptedMembership } from "./membership";
import { MAX_SHARED_BYTES, type SharedValues } from "./shared";
import { noteDocsChanged } from "../documents/changed";
import { ownSign, readDoc, signDoc } from "../documents/format";
import { primaryPeersBody, pruneDocs } from "../documents/work";
import { APP_FOLDER, K } from "../home";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { ownPhones } from "../phones";
import { readLink, updateLink } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { changeNumbers, installNumberAuthority, MOST_NUMBER, topOf } from "../work-queue/numbers";
import { writeCard } from "./card";
import { NUMBER_RESERVE, takeLeadership } from "./leadership";
import { LEADERSHIP_WAIT, primaryFenced } from "./lease";
import { followedSetup, requiresStandby, type SetupRecord } from "./record";

export const HANDOVER_FEATURE = "setup.handover";
const answer = (status: number, message: string): Acted => ({ status, value: { message } });

export async function handover(given: unknown, signal?: AbortSignal | null): Promise<Acted> {
  if (!isObject(given) || typeof given.primary !== "string" || !KEY.test(given.primary)) return answer(400, "Choose a computer by its key as primary.");
  const link = readLink();
  const setup = followedSetup(link);
  if (!setup) return answer(409, "This computer doesn't have an active setup.");
  if (setup.primary === given.primary) return { status: 200, value: { primary: setup.primary, epoch: setup.epoch } };
  if (given.epoch !== undefined && given.epoch !== setup.epoch) return answer(409, "The primary changed. Refresh your setup and try again.");
  const position = { setup: setup.id, epoch: setup.epoch };
  if (setup.role === "primary") return handoverHere(given.primary, position);
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  if (!primary) return answer(503, "This computer can't reach your primary.");
  const result = await forwardAct(primary, "Changing the primary", { area: "setup", method: "POST", item: given.primary, body: position }, signal, link);
  return "refused" in result ? answer(result.status, result.refused) : result;
}

async function handoverHere(target: string, position: Record<string, unknown>, requester?: string): Promise<Acted> {
  const before = readLink();
  const setup = followedSetup(before);
  if (setup?.role !== "primary" || setup.id !== position.setup || setup.epoch !== position.epoch) return answer(409, "The primary changed. Refresh your setup and try again.");
  const peers = peersOf(before);
  const next = peers.find(peer => peer.key === target);
  if (!next?.sign) return answer(404, "Choose a member of this setup.");
  const older = peers.find(peer => !peer.sign || !takesForwarded(peer.key, HANDOVER_FEATURE));
  if (older) return answer(409, `${older.name || "A member"} needs to connect with a newer Kuvo Code before you can change the primary.`);
  const ready = await forwardAct(next, "Changing the primary", { area: "setup", method: "GET", item: target, body: position }, undefined, before, Date.now(), { backgroundRelay: true });
  if ("refused" in ready) return answer(ready.status, ready.refused);
  if (ready.status !== 200) return ready;
  if (!isObject(ready.value) || ready.value.sign !== next.sign) return answer(409, "The chosen computer's signing key changed. Refresh your setup and try again.");

  return proposeLeadership(position, target, setup.standby?.key !== target ? setup.standby ?? null : null, requester, next.sign);
}

/** The number ledger and the fenced proposal are frozen together. With a standby, only it
 * can turn the proposal into the next certificate. Sending is safe to retry after a restart. */
export async function proposeLeadership(position: Record<string, unknown>, target: string, standby: NonNullable<SetupRecord["standby"]> | null, requester?: string, targetSign?: string, retiredStandby?: string): Promise<Acted> {
  const changed = changeNumbers(numbers => updateLink(link => {
    const current = followedSetup(link);
    if (!link?.sign || current?.role !== "primary" || current.sign !== ownSign(link) || current.id !== position.setup || current.epoch !== position.epoch
      || primaryFenced(link) || Object.keys(link.sharedPending ?? {}).length) return null;
    if (retiredStandby && (standby || current.standby?.key !== retiredStandby)) return null;
    const members = peersOf(link);
    if (requester && !members.some(peer => peer.key === requester)) return null;
    if (target !== current.primary && (!members.some(peer => peer.key === target && peer.sign === targetSign)
      || members.some(peer => !peer.sign || !takesForwarded(peer.key, HANDOVER_FEATURE)
        || (Object.keys(link.shared ?? {}).length && !takesForwarded(peer.key, "setup.shared-saves"))))) return null;
    if (standby && (standby.key === target || !members.some(peer => peer.key === standby.key && peer.sign === standby.sign)
      || members.some(peer => !peer.sign || !takesForwarded(peer.key, "setup.shared-saves")))) return null;
    const supportsCheckpoint = members.every(peer => takesForwarded(peer.key, CHECKPOINT_FEATURE));
    const migrating = Boolean(!current.checkpointed && current.standby && supportsCheckpoint);
    const renewing = target === current.primary && (standby?.key ?? null) === (current.standby?.key ?? null);
    if (renewing && (!standby || !current.numberCeiling || (!migrating && current.numberCeiling >= MOST_NUMBER - 1))) return null;
    installNumberAuthority(numbers, link);
    const granted = { ...numbers.granted };
    if (granted.self) {
      granted[current.primary] = granted.self;
      delete granted.self;
    }
    const { boxes, left, moved } = primaryPeersBody(link);
    const epoch = current.epoch + 1;
    const next = topOf(numbers) + 1;
    if (renewing && !migrating && next + NUMBER_RESERVE - 1 <= current.numberCeiling!) return null;
    const shared: SharedValues = { ...(link.sharedSetup === current.id ? link.shared : {}) };
    if (standby && !current.standby) {
      const library = readPersonalities();
      const zone = readZoneSetting();
      if (zone && !shared.setupTimeZone) shared.setupTimeZone = { sequence: 1, value: zone };
      if (!shared.queueLabels) shared.queueLabels = { sequence: 1, value: { labels: labels(), removed: [] } };
      if (!shared.setupUsageDisplay) {
        const usage = new UsageSettings(path.join(APP_FOLDER ?? K, "agents", "usage", "settings.json"));
        const units = usage.kept();
        if (usage.imported || Object.keys(units).length) shared.setupUsageDisplay = { sequence: 1, value: { units, imported: usage.imported } };
      }
      if (!shared.setupPersonalities && Buffer.byteLength(JSON.stringify({ ...shared, library })) < MAX_SHARED_BYTES) {
        shared.setupPersonalities = { sequence: 1, value: library };
      }
    }
    const authority = {
      setup: { id: current.id, epoch, primary: target }, previous: current.primary,
      ...(retiredStandby ? { retiredStandby } : {}),
      ...(standby || requiresStandby(current) ? { sharedSaves: true } : {}),
      ...(standby ? { standby: standby.key, numberCeiling: renewing && migrating ? current.numberCeiling : Math.min(MOST_NUMBER - 1, next + NUMBER_RESERVE - 1) } : {}),
      ...(current.standby ? { proposal: true } : {}),
    };
    const ledger = { next, given: numbers.given, granted };
    let doc;
    if (current.checkpointed || supportsCheckpoint) {
      const membership = acceptedMembership(link) ?? captureMembership(link);
      if (!membership) return null;
      shared.membershipAndAccess = { sequence: shared.membershipAndAccess?.sequence ?? 1, value: membership };
      shared.queueNumberAllocation = { sequence: shared.queueNumberAllocation?.sequence ?? 1, value: ledger };
      doc = checkpointedTransition(link, authority, shared);
    } else {
      doc = readDoc(signDoc(link.sign, "leadership", epoch, {
        ...authority, boxes, left, moved, devices: ownPhones(link), vouchBlocked: link.vouchBlocked ?? [],
        shared, removalRevisions: link.removalRevisions, numbers: ledger,
      }));
    }
    if (!doc || !boxes.every(box => box.key && box.sign && box.address)) return null;
    if (current.standby) link.leadershipPending = doc.envelope;
    else {
      if (!takeLeadership(link, doc)) return null;
      pruneDocs(link);
    }
    return link;
  }, { durable: true }));
  if (!changed) return answer(409, "The setup changed or the standby couldn't be reached. Refresh your setup and try again.");
  if (changed.leadershipPending) {
    const deadline = Date.now() + 12_000;
    while (Date.now() < deadline) {
      const setup = readLink()?.setup;
      if (setup?.id !== position.setup || !setup || setup.epoch !== position.epoch) {
        if (setup?.id === position.setup && setup?.primary === target && setup.standby?.key === standby?.key && setup.standby?.sign === standby?.sign) {
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
    if (act.body.standby === true) return standbyHere(act.item, act.body, sender.key);
    return handoverHere(act.item, act.body, sender.key);
  }
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || setup?.role !== "member" || setup.primary !== sender.key || setup.id !== act.body.setup
    || setup.epoch !== act.body.epoch || ownKey(link) !== act.item || !ownSign(link)) {
    return answer(409, "The chosen computer hasn't caught up with the current primary yet.");
  }
  return { status: 200, value: { sign: ownSign(link) } };
}

export async function selectStandby(given: unknown, signal?: AbortSignal | null): Promise<Acted> {
  if (!isObject(given) || typeof given.standby !== "string" || !KEY.test(given.standby)) return answer(400, "Choose a member by its key as standby.");
  const link = readLink();
  const setup = followedSetup(link);
  if (!setup || (given.epoch !== undefined && setup.epoch !== given.epoch)) return answer(409, "The setup changed. Refresh it and try again.");
  if (setup.standby?.key === given.standby) return { status: 200, value: { primary: setup.primary, epoch: setup.epoch } };
  const position = { setup: setup.id, epoch: setup.epoch };
  if (setup.role === "primary") return standbyHere(given.standby, position);
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  if (!primary) return answer(503, "This computer can't reach your primary.");
  const result = await forwardAct(primary, "Changing the standby", { area: "setup", method: "POST", item: given.standby, body: { ...position, standby: true } }, signal, link);
  return "refused" in result ? answer(result.status, result.refused) : result;
}

export async function standbyHere(target: string, position: Record<string, unknown>, requester?: string, reason = "user choice"): Promise<Acted> {
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
  const outcome = await proposeLeadership(position, setup.primary, { key: target, sign: peer.sign }, requester);
  if (outcome.status === 202) console.info(`Standby selected: ${target} (${reason}).`);
  return outcome;
}
