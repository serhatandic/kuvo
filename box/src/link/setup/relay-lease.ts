import { isObject } from "../../shared/json";
import { noteDocsChanged } from "../documents/changed";
import { ownSign, signWith } from "../documents/format";
import { ownKey, peersOf } from "../peers/known";
import { readLink, updateLink, type LinkState } from "../state";
import { BLOCK, legacyNumberHigh, MOST_NUMBER, readNumbers, toldTop, topOf } from "../work-queue/numbers";
import { holdsRelayLease, leaseClock, relayLease, rememberRelayLease, standbySettings, type RelayLease } from "./lease";
import { followedSetup } from "./record";
import { sharedValue } from "./shared";

const DOMAIN = "kuvo-link setup lease v1\0";

function workQueueHigh(link: LinkState) {
  const kept = readNumbers();
  const setup = link.setup!;
  // One-time floor when upgrading from the earlier numbering ledgers.
  const ledger = sharedValue(link, "queueNumberAllocation");
  const acceptedHigh = isObject(ledger) && typeof ledger.next === "number" ? ledger.next - 1 : 0;
  const high = Math.max(topOf(kept), toldTop(), setup.numberCeiling ?? 0, acceptedHigh, legacyNumberHigh(setup.id));
  return { high, requested: kept.reserveThrough ?? 0 };
}

export async function askRelayLease(link: LinkState, action: "read" | "claim" | "renew" | "transfer", term = 0, target?: string) {
  const setup = followedSetup(link);
  const computer = ownKey(link);
  const sign = ownSign(link);
  if (!setup || !computer || !sign || !link.sign || !link.relay) return;
  const url = URL.parse("/setup-lease", link.relay);
  if (!url) return;
  if (url.protocol === "wss:") url.protocol = "https:";
  else if (url.protocol === "ws:") url.protocol = "http:";
  else return;
  const since = leaseClock();
  const bootstrap = action === "claim" && term === 0 && setup.role === "primary";
  const roster = action === "renew" || action === "transfer" || bootstrap
    ? Object.fromEntries([[computer, sign], ...peersOf(link).flatMap(peer => peer.sign ? [[peer.key, peer.sign]] : [])])
    : undefined;
  const { alwaysOn } = standbySettings();
  const allocation = action === "read" ? null : workQueueHigh(link);
  const floor = relayLease(link);
  // Reserve headroom on renewal before any number in it can be granted.
  const high = allocation && (action === "renew"
    ? Math.min(MOST_NUMBER - 1, Math.max(allocation.requested, Math.max(allocation.high, floor?.numberFloor ?? floor?.high ?? 0) + 4 * BLOCK))
    : allocation.high);
  const body = JSON.stringify({ action, setup: setup.id, computer, sign, at: Date.now(), term, bootstrap, alwaysOn, target,
    ...(action === "read" ? {} : { high, roster }) });
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ data: Buffer.from(body).toString("base64url"), signature: signWith(link.sign, Buffer.from(DOMAIN + body)) }),
    signal: AbortSignal.timeout(4_000),
    redirect: "error",
  }).catch(() => null);
  if (!response?.ok) return;
  const reply: unknown = await response.json().catch(() => null);
  if (!isObject(reply)) return;
  if (reply.lease === null) return { lease: null, takeoverIn: 0 };
  const { lease, remaining, takeoverIn, relayHost, startsIn } = reply;
  if (!isObject(lease) || typeof lease.holder !== "string" || typeof lease.sign !== "string" ||
    typeof lease.term !== "number" || typeof lease.expires !== "number" || typeof lease.high !== "number" ||
    typeof remaining !== "number" || typeof startsIn !== "number" || typeof takeoverIn !== "number" || followedSetup(readLink())?.id !== setup.id) return;
  const received = leaseClock();
  const observation: RelayLease = {
    setup: setup.id, holder: lease.holder, sign: lease.sign, term: lease.term, expires: lease.expires, high: lease.high,
    relayHost: relayHost === true, since, duration: Math.min(120_000, remaining),
    validAfter: startsIn > 0 ? { wall: received.wall + startsIn, uptime: received.uptime + startsIn } : since,
  };
  if (!rememberRelayLease(observation, allocation?.high)) return;
  return { lease: observation, takeoverIn };
}

export async function ensurePrimaryLease() {
  const link = readLink();
  if (followedSetup(link)?.role === "primary" && !holdsRelayLease(link)) await refreshPrimaryLease(link!);
}

export async function refreshPrimaryLease(link: LinkState) {
  const setup = followedSetup(link);
  if (setup?.role !== "primary") return;
  const kept = relayLease(link);
  let result;
  if (kept?.holder === setup.primary) result = await askRelayLease(link, "renew", kept.term);
  else {
    result = await askRelayLease(link, "read");
    if (result?.lease === null && !kept) result = await askRelayLease(link, "claim");
    else if (result?.lease?.holder === setup.primary) result = await askRelayLease(link, "renew", result.lease.term);
  }
  // Outside restart grace an expired holder has to claim, with the same wait as other computers.
  if (result?.lease?.holder === setup.primary && result.lease.duration === 0 && result.takeoverIn === 0) {
    result = await askRelayLease(link, "claim", result.lease.term);
  }
  // An unfinished standby assignment from the preceding release no longer decides leadership.
  if (result?.lease?.holder === setup.primary && link.leadershipPending) {
    const changed = updateLink(current => {
      if (current?.setup?.id !== setup.id || current.setup.epoch !== setup.epoch || !holdsRelayLease(current)) return null;
      const pending = current.leadershipPending?.s === link.leadershipPending?.s && !!current.leadershipPending;
      if (pending) delete current.leadershipPending;
      return pending ? current : null;
    }, { durable: true });
    if (changed) noteDocsChanged();
  }
  return result;
}
