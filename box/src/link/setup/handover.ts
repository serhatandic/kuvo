// The primary checks that the successor is running this protocol, then atomically commits
// a signed epoch change. Peers deliver the committed record with the regular document exchange.

import { isObject } from "../../shared/json";
import { noteDocsChanged } from "../documents/changed";
import { ownSign, readDoc, signDoc } from "../documents/format";
import { primaryPeersBody, pruneDocs } from "../documents/work";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { ownPhones } from "../phones";
import { readLink, updateLink } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { changeNumbers, installNumberAuthority, topOf } from "../work-queue/numbers";
import { writeCard } from "./card";
import { takeLeadership } from "./leadership";
import { followedSetup } from "./record";

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

  // Serialize with number allocation too. With this lock held, the committed snapshot includes
  // every number the outgoing primary has assigned, even if nobody else has heard of it yet.
  const epoch = setup.epoch + 1;
  const changed = changeNumbers(numbers => updateLink(link => {
    const current = followedSetup(link);
    if (!link?.sign || current?.role !== "primary" || current.sign !== ownSign(link) || current.id !== setup.id || current.epoch !== setup.epoch) return null;
    const members = peersOf(link);
    if (requester && !members.some(peer => peer.key === requester)) return null;
    if (!members.some(peer => peer.key === target && peer.sign === next.sign)
      || members.some(peer => !peer.sign || !takesForwarded(peer.key, HANDOVER_FEATURE))) return null;
    installNumberAuthority(numbers, link);
    const granted = { ...numbers.granted };
    if (granted.self) {
      granted[current.primary] = granted.self;
      delete granted.self;
    }
    const { boxes, left, moved } = primaryPeersBody(link);
    const envelope = signDoc(link.sign, "leadership", epoch, {
      setup: { id: current.id, epoch, primary: target }, previous: current.primary, boxes, left, moved,
      devices: ownPhones(link), vouchBlocked: link.vouchBlocked ?? [],
      numbers: { next: topOf(numbers) + 1, given: numbers.given, granted },
    });
    const doc = readDoc(envelope);
    if (!doc || !takeLeadership(link, doc)) return null;
    pruneDocs(link);
    return link;
  }, { durable: true }));
  if (!changed) return answer(409, "The setup changed or its state couldn't be handed over. Refresh your setup and try again.");

  noteDocsChanged();
  try { writeCard(); } catch {}
  return { status: 202, value: { primary: target, epoch } };
}

export async function setupAct(sender: MailSender, act: Act): Promise<Acted> {
  if (!act.body) return answer(400, "Send the setup and epoch.");
  if (act.method === "POST") return handoverHere(act.item, act.body, sender.key);
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || setup?.role !== "member" || setup.primary !== sender.key || setup.id !== act.body.setup
    || setup.epoch !== act.body.epoch || ownKey(link) !== act.item || !ownSign(link)) {
    return answer(409, "The chosen computer hasn't caught up with the current primary yet.");
  }
  return { status: 200, value: { sign: ownSign(link) } };
}
