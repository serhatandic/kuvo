import type { ForwardGrant } from "../work-queue/forward-grant";
// Changing the main computer: the main checks the chosen computer has caught up, then asks the relay
// to move its lease there. The chosen computer then signs the next certificate (`takeover.ts`).

import { isObject } from "../../shared/json";
import { ownSign } from "../documents/format";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { awaySince } from "../peers/reach";
import { readLink, updateLink, type LinkState } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { acceptedInstallerRecoveries, acceptedMembership, acceptedName } from "./membership";
import { LEADERSHIP_WAIT, primaryFenced, relayLease } from "./lease";
import { askRelayLease } from "./relay-lease";
import { followedSetup } from "./record";
import { offerSetupCopies } from "./seed";

export const RELAY_LEASE_FEATURE = "setup.relay-lease";
const answer = (status: number, message: string): Acted => ({ status, value: { message } });

/** After the hand-over, the new main unlinks the old one, wiping it with `wipe`. */
type Unlink = { wipe: boolean };

export async function handover(given: unknown, signal?: AbortSignal | null, grant?: ForwardGrant | null): Promise<Acted> {
  if (!isObject(given) || typeof given.primary !== "string" || !KEY.test(given.primary)) return answer(400, "Choose a computer by its key as the main computer.");
  const link = readLink();
  const setup = followedSetup(link);
  if (!setup) return answer(409, "This computer doesn't have an active setup.");
  const unlink = isObject(given.unlink) ? { wipe: given.unlink.wipe === true } : undefined;
  if (setup.primary === given.primary && !unlink) return { status: 200, value: { primary: setup.primary, epoch: setup.epoch } };
  if (given.epoch !== undefined && given.epoch !== setup.epoch) return answer(409, "The main computer changed. Refresh your setup and try again.");
  const position = { setup: setup.id, epoch: setup.epoch, ...(unlink ? { unlink } : {}) };
  if (setup.role === "primary") return handoverHere(given.primary, position);
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  if (!primary) return answer(503, "This computer can't reach your main computer.");
  const result = await forwardAct(primary, "Changing the main computer", { area: "setup", method: "POST", item: given.primary, body: position }, signal, link, Date.now(), { grant });
  return "refused" in result ? answer(result.status, result.refused) : result;
}

/** Who takes over when the main is unlinked and you don't pick: a computer that's reachable and can
 * lead, always-on computers (Linux machines) first. Null when there is none. */
export function nextMain(link: LinkState | null, except: string): string | null {
  const own = link && ownKey(link);
  const provisional = acceptedInstallerRecoveries(link);
  const boxes = acceptedMembership(link)?.boxes ?? [];
  const candidates = boxes.filter(box => box.key !== except && box.sign && !provisional[box.key] &&
    (box.key === own || takesForwarded(box.key, RELAY_LEASE_FEATURE) && peersOf(link).some(peer => peer.key === box.key)));
  const rank = (box: (typeof boxes)[number]) => (box.key === own || awaySince(box.key) === null ? 2 : 0) + (box.kind === "machine" ? 1 : 0);
  return candidates.sort((a, b) => rank(b) - rank(a) || a.name.localeCompare(b.name))[0]?.key ?? null;
}

/** Unlink the main computer: hand the role over first (to `picked`, else `nextMain`), and the new
 * main unlinks this one once it leads. */
export async function unlinkMain(key: string, wipe: boolean, picked?: string, signal?: AbortSignal | null, grant?: ForwardGrant | null): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  const target = picked ?? nextMain(link, key);
  if (!link || !setup || setup.primary !== key || !target) return answer(409, "Add another computer to this setup before unlinking its main computer.");
  const done = await handover({ primary: target, epoch: setup.epoch, unlink: { wipe } }, signal, grant);
  if (done.status !== 202 || !isObject(done.value)) return done;
  const name = (box: string) => acceptedName(link, box, acceptedMembership(link)?.boxes.find(each => each.key === box)?.name || "Another computer");
  return { status: 202, value: { ...done.value, unlinking: true, removal: { state: "handing-over", accessRemoved: false },
    message: `${name(target)} is becoming the main computer. Then ${name(key)} is ${wipe ? "unlinked and wiped" : "unlinked"}.` } };
}

async function handoverHere(target: string, position: { setup: string; epoch: number; unlink?: Unlink }, requester?: string, admit?: Act["admit"]): Promise<Acted> {
  const before = readLink();
  const setup = followedSetup(before);
  if (setup?.role !== "primary" || setup.id !== position.setup || setup.epoch !== position.epoch) return answer(409, "The main computer changed. Refresh your setup and try again.");
  if (primaryFenced(before)) return answer(503, LEADERSHIP_WAIT);
  const provisional = acceptedInstallerRecoveries(before);
  const next = peersOf(before).find(peer => peer.key === target && !provisional[peer.key]);
  if (!next?.sign) return answer(404, "Choose a member of this setup.");
  // Only the chosen computer has to be able to lead; the others follow by version.
  if (!takesForwarded(target, RELAY_LEASE_FEATURE)) return answer(409, `${next.name || "That computer"} needs a newer Kuvo Code before it can be the main computer.`);
  const ready = await forwardAct(next, "Changing the main computer", { area: "setup", method: "GET", item: target, body: position }, undefined, before, Date.now(), { backgroundRelay: true });
  if ("refused" in ready) return answer(ready.status, ready.refused);
  if (ready.status !== 200) return ready;
  if (!isObject(ready.value) || ready.value.sign !== next.sign) return answer(409, "The chosen computer's signing key changed. Refresh your setup and try again.");

  const veto = admit?.();
  if (veto) return veto;
  const current = readLink();
  const lease = relayLease(current);
  if (!current || !lease || current.setup?.id !== position.setup || current.setup.epoch !== position.epoch ||
    primaryFenced(current) || (requester && !peersOf(current).some(peer => peer.key === requester))) return answer(503, LEADERSHIP_WAIT);
  const changed = await askRelayLease(current, "transfer", lease.term, target);
  return changed?.lease?.holder === target
    ? { status: 202, value: { primary: target, epoch: setup.epoch + 1, ...(position.unlink ? { unlinking: true } : {}) } }
    : answer(503, "The relay couldn't hand over to this computer. Try again in a moment.");
}

export async function setupAct(sender: MailSender, act: Act): Promise<Acted> {
  if (!act.body) return answer(400, "Send the setup and epoch.");
  if (act.method === "POST") {
    const { setup, epoch } = act.body;
    if (typeof setup !== "string" || typeof epoch !== "number") return answer(400, "Send the setup and epoch.");
    const unlink = isObject(act.body.unlink) ? { wipe: act.body.unlink.wipe === true } : undefined;
    return handoverHere(act.item, { setup, epoch, ...(unlink ? { unlink } : {}) }, sender.key, act.admit);
  }
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || setup?.role !== "member" || setup.primary !== sender.key || setup.id !== act.body.setup
    || setup.epoch !== act.body.epoch || ownKey(link) !== act.item || !ownSign(link)) {
    return answer(409, "The chosen computer hasn't caught up with the main computer yet.");
  }
  if (isObject(act.body.unlink)) {
    const intent = { setup: setup.id, epoch: setup.epoch, key: sender.key, wipe: act.body.unlink.wipe === true };
    updateLink(current => current ? Object.assign(current, { unlinkAfterHandover: intent }) : null, { durable: true });
  }
  return { status: 200, value: { sign: ownSign(link), ...(act.body.sharedSeed === true ? offerSetupCopies(link, act.body.only) : {}) } };
}
