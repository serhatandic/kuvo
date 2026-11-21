// The standby renews the primary lease. Ranked computers probe their predecessors before taking over.
import { standbyIntentLayoutReady, successorRevisionLayoutReady } from "../../shared/folder-version";
import { K } from "../home";
import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import { noteDocsChanged } from "../documents/changed";
import { sendDocs } from "../documents/exchange";
import { docKey, held, ownSign, readDoc, signDoc } from "../documents/format";
import { cleanPeer, fitsPeer, mailFor, ownKey, peersOf, type MailSender } from "../peers/known";
import type { LinkHost } from "../relay/host";
import { readLink, updateLink, type LinkState } from "../state";
import { readNumbers } from "../work-queue/numbers";
import { takesForwarded } from "../work-queue/forward";
import { writeCard } from "./card";
import { finishFirstStandbyPin, leadershipPeers, proposeLeadership, standbyHere } from "./handover";
import { CHECKPOINT_FEATURE, checkpointedTransition, leadershipDocuments } from "./checkpoint";
import { acceptedMembership, acceptedName } from "./membership";
import { canSupersedeStandby, leadershipOf, leadershipPosition, pendingStandby, successorCertificates, successorHistory, takeLeadership } from "./leadership";
import { DEFAULT_DELAY, fresh, grantLease, HEARTBEAT_MS, leaseClock, LEASE_MS, primaryFenced, standbySettings, takeoverProbeWindow, testSetupClock } from "./lease";
import { availableComputer, observeAvailability, observedUptime, rankComputers, RANK_INTERVAL, resetAvailability } from "./reliability";
import { pendingShared, publishedShared, sharedValue, stageShared } from "./shared";
import { sendMergeOffers } from "../add/invite";
import { revocationDocuments } from "./revocations";
import { followedSetup, setupDoc } from "./record";

type Clock = ReturnType<typeof leaseClock>;
type Position = { id: string; epoch: number; primary: string };
const nonce = () => crypto.randomBytes(16).toString("base64url");
const signal = (token: unknown): token is string => typeof token === "string" && /^[\w-]{22}$/.test(token);

let position = "";
let since: Clock;
let lastTick: Clock | undefined;
let lastTip: Clock | undefined;
let lastRound = 0;
let selecting = false;
let logged = false;
let pendingWait: { signature: string; since: number } | undefined;
const recentTips = new Map<string, number>();
let ping: { token: string; at: Clock } | undefined;
let pong: { token: string; reply: string; at: Clock } | undefined;
let lastPrimary: Clock | undefined;
let ownPrimarySince: Clock | undefined;
let rankCheckedAt = 0;
let eligibleSince: number | undefined;
let standbyUnavailable: Clock | undefined;
const probes = new Map<string, { token: string; at: Clock }>();
const reached = new Map<string, Clock>();

function clearProbes() {
  probes.clear();
  reached.clear();
  eligibleSince = undefined;
}


function reset(setup: Position, now: Clock) {
  const next = `${setup.id}:${setup.epoch}:${setup.primary}`;
  if (position === next) return;
  position = next;
  since = now;
  ping = pong = lastTick = lastTip = lastPrimary = ownPrimarySince = undefined;
  resetAvailability();
  clearProbes();
  standbyUnavailable = undefined;
  rankCheckedAt = Date.now();
  logged = false;
  pendingWait = undefined;
  recentTips.clear();
}

function send(host: LinkHost, link: LinkState, to: string, value: Record<string, unknown>) {
  const peer = peersOf(link).find(each => each.key === to);
  const { id, epoch } = link.setup!;
  const tip = link.setup!.successorChain ? leadershipOf(link)?.h ?? link.setup!.leadershipHash : undefined;
  const bytes = peer && mailFor(peer, { setupAuthority: { id, epoch, tip, ...value }, at: Date.now() }, link);
  if (bytes) host.send(bytes);
}

export function receiveAuthority(sender: MailSender, given: Record<string, unknown>, host: LinkHost | null | undefined) {
  if (!host?.welcomed) return;
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || !setup || setup.id !== given.id) return;
  if ((setup.successorChain || canSupersedeStandby(link)) && given.kind === "tip" && isObject(given.history)) {
    const peer = peersOf(link).find(peer => peer.key === sender.key);
    if (!peer) return;
    recentTips.set(sender.key, Date.now());
    const remote = Object.fromEntries(Object.entries(given.history).slice(0, 16)
      .filter(([epoch, hash]) => /^[1-9][0-9]*$/.test(epoch) && typeof hash === "string")) as Record<number, string>;
    const give = successorCertificates(link, remote);
    give.push(...revocationDocuments(link));
    if (give.length) sendDocs(host, link, [{ to: peer, docs: { give } }]);
    return;
  }
  if (setup.epoch !== given.epoch || setup.successorChain && (given.tip !== (leadershipOf(link)?.h ?? setup.leadershipHash) ||
    link.setupRevocations && acceptedMembership(link)?.left.includes(ownKey(link) ?? ""))) return;
  const now = leaseClock();
  reset(setup, now);
  if (given.kind === "available" && setup.role === "primary") {
    if (typeof given.preferred !== "boolean" || typeof given.reason !== "string") return;
    observeAvailability(sender.key, now, given.preferred, given.reason.slice(0, 40));
    if (sender.key === setup.standby?.key) return;
    const peer = peersOf(link).find(peer => peer.key === sender.key);
    const give = peer ? publishedShared(link) : [];
    if (peer && give.length) sendDocs(host, link, [{ to: peer, docs: { give } }]);
    return;
  }
  if (!signal(given.token)) return;
  if (setup.successorChain && given.kind === "rank-probe") {
    send(host, link, sender.key, { kind: "rank-answer", token: given.token });
    return;
  }
  if (setup.successorChain && given.kind === "rank-answer") {
    const pending = probes.get(sender.key);
    if (pending?.token === given.token && fresh(pending.at, now, LEASE_MS / 2)) {
      probes.delete(sender.key);
      reached.set(sender.key, now);
      if (sender.key === setup.primary) lastPrimary = now;
    }
    return;
  }
  if (!setup.standby) return;
  if (setup.role === "primary" && setup.standby.key === sender.key && !link.leadershipPending) {
    if (given.kind === "ping") {
      pong = { token: given.token, reply: nonce(), at: now };
      send(host, link, sender.key, { kind: "pong", token: given.token, reply: pong.reply });
    } else if (given.kind === "lease" && pong?.token === given.token && pong.reply === given.reply && fresh(pong.at, now, LEASE_MS / 2)) {
      const fenced = primaryFenced(link);
      grantLease(link, pong.at);
      standbyUnavailable = undefined;
      pong = undefined;
      if (fenced) noteDocsChanged();
    }
  } else if (setup.standby.key === ownKey(link) && setup.standby.sign === ownSign(link) && sender.key === setup.primary
    && given.kind === "pong" && signal(given.reply) && ping?.token === given.token && fresh(ping.at, now, LEASE_MS / 2)) {
    ping = undefined;
    lastPrimary = now;
    send(host, link, sender.key, { kind: "lease", token: given.token, reply: given.reply });
  }
}

export function startTakeover(host: () => LinkHost | null | undefined) {
  setInterval(() => {
    try { takeoverTick(host()); } catch (error) { console.warn("Standby check:", (error as Error).message); }
  }, testSetupClock() ? 200 : 2_000).unref();
}

function takeoverTick(host: LinkHost | null | undefined): void {
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || !setup) return;
  const now = leaseClock();
  reset(setup, now);
  const testing = testSetupClock();
  const realNow = Date.now();
  if (!host?.welcomed || realNow - lastRound > 3 * HEARTBEAT_MS || realNow < lastRound) {
    lastPrimary = ping = undefined;
    clearProbes();
    since = now;
  }
  lastRound = realNow;
  if (!host?.welcomed) return;
  if (setup.role === "primary") sendMergeOffers(host);
  if (setup.successorChain && (!lastTip || !fresh(lastTip, now, testing ? 1_000 : HEARTBEAT_MS))) {
    lastTip = now;
    const history = Object.fromEntries(successorHistory(link).map(doc => [doc.n, doc.h]));
    const tip = leadershipPosition(link);
    if (tip && !history[tip.epoch]) history[tip.epoch] = tip.h;
    for (const peer of peersOf(link)) send(host, link, peer.key, { kind: "tip", history });
  }
  if (setup.role === "primary" && link.leadershipPending) {
    const standby = pendingStandby(link) ?? peersOf(link).find(peer => peer.key === setup.standby?.key);
    const signature = link.leadershipPending.s;
    const wait = testing ? 3_000 : 3 * HEARTBEAT_MS;
    if (pendingWait?.signature !== signature || standby && realNow - (recentTips.get(standby.key) ?? 0) < wait) {
      pendingWait = { signature, since: realNow };
    }
    if (!selecting && canSupersedeStandby(link) && standbyIntentLayoutReady(K) && realNow - pendingWait.since >= wait) {
      const next = availableStandby(link, now, standby?.key);
      if (next?.peer.sign) {
        assignStandby(setup, { key: next.peer.key, sign: next.peer.sign },
          { supersedes: signature, ...(setup.standby && setup.standby.key !== next.peer.key ? { replace: true as const } : {}) });
        return;
      }
    }
    if (standby) sendDocs(host, link, [{ to: standby, docs: { give: leadershipDocuments(link.leadershipPending) } }]);
    return;
  }
  if (setup.role === "primary" && setup.standby && !primaryFenced(link, true) && Object.keys(link.sharedPending ?? {}).length) {
    const standby = peersOf(link).find(peer => peer.key === setup.standby!.key);
    if (standby) sendDocs(host, link, [{ to: standby, docs: { give: Object.values(link.sharedPending!) } }]);
  }
  if (link.setupMerge || sharedValue(link, "setupDeparture")) {
    if (setup.role === "primary" && (!lastTick || !fresh(lastTick, now, testing ? 400 : HEARTBEAT_MS))) {
      lastTick = now;
      const give = publishedShared(link);
      if (give.length) sendDocs(host, link, peersOf(link).filter(peer => peer.key !== setup.standby?.key).map(to => ({ to, docs: { give } })));
    }
    return;
  }
  if (lastTick && fresh(lastTick, now, testing ? 400 : HEARTBEAT_MS)) return;
  lastTick = now;
  const settings = standbySettings();
  if (setup.role === "primary") {
    if (!primaryFenced(link) && finishFirstStandbyPin(link)) return;
    ownPrimarySince ??= now;
    observeAvailability(setup.primary, now, settings.alwaysOn, settings.reason);
    if (realNow - rankCheckedAt >= RANK_INTERVAL && !primaryFenced(link)) {
      rankCheckedAt = realNow;
      refreshRank(setup, now);
    }
    if (setup.standby) {
      const ledger = sharedValue(link, "queueNumberAllocation");
      const next = Math.max(readNumbers().next ?? 1, isObject(ledger) ? Number(ledger.next) || 1 : 1);
      const migrating = !setup.checkpointed && peersOf(link).every(peer => takesForwarded(peer.key, CHECKPOINT_FEATURE)) ||
        !setup.successorChain && acceptedMembership(link)?.successorOrder &&
          peersOf(link).every(peer => takesForwarded(peer.key, "setup.successor-chain"));
      if (!primaryFenced(link)) {
        standbyUnavailable = undefined;
        const nextStandby = setup.successorChain && successorRevisionLayoutReady(K) && availableStandby(link, now);
        if (!selecting && nextStandby && nextStandby.peer.sign && nextStandby.peer.key !== setup.standby.key && availableComputer(setup.standby.key, now)) {
          assignStandby(setup, { key: nextStandby.peer.key, sign: nextStandby.peer.sign });
          return;
        }
      }
      else if (setup.successorChain) {
        standbyUnavailable ??= now;
        if (!selecting && now.wall - standbyUnavailable.wall >= DEFAULT_DELAY - LEASE_MS &&
          now.uptime - standbyUnavailable.uptime >= DEFAULT_DELAY - LEASE_MS) {
          const next = availableStandby(link, now, setup.standby.key);
          if (next?.peer.sign) {
            assignStandby(setup, { key: next.peer.key, sign: next.peer.sign }, { replace: true });
          }
        }
      }
      if ((migrating || (setup.numberCeiling && setup.numberCeiling - next < 400)) && !primaryFenced(link)) {
        void proposeLeadership({ setup: setup.id, epoch: setup.epoch }, setup.primary, setup.standby)
          .catch(error => console.warn("Standby number reservation:", error.message));
      }
      return;
    }
    if (selecting || fresh(ownPrimarySince, now, testing ? 2_000 : 90_000)) return;
    if (leadershipPeers(link).some(peer => !peer.sign || !takesForwarded(peer.key, "setup.shared-saves"))) return;
    const next = availableStandby(link, now);
    if (!next) return;
    selecting = true;
    void standbyHere(next.peer.key, { setup: setup.id, epoch: setup.epoch }, { reason: next.reason })
      .catch(error => console.warn("Standby selection:", error.message))
      .finally(() => { selecting = false; });
    return;
  }
  send(host, link, setup.primary, { kind: "available", preferred: settings.alwaysOn, reason: settings.reason });
  const own = ownKey(link);
  const standby = setup.standby?.key === own && setup.standby?.sign === ownSign(link);
  const order = acceptedMembership(link)?.successorOrder ?? [];
  const earlier = setup.successorChain && own ? [setup.primary, ...order.filter(key => key !== setup.primary)] : [];
  const rank = own ? earlier.indexOf(own) : -1;
  if (!standby && rank < 1) return;
  if (!logged) {
    logged = true;
    console.info(`Standby watching epoch ${setup.epoch} with a ${DEFAULT_DELAY / 60_000}-minute takeover delay.`);
  }
  const heard = lastPrimary ?? since;
  const expired = now.uptime - heard.uptime >= DEFAULT_DELAY && now.wall - heard.wall >= DEFAULT_DELAY;
  const higher = rank > 1 ? earlier.slice(1, rank) : [];
  const reachableHigher = higher.some(key => {
    const reply = reached.get(key);
    return reply && fresh(reply, now, testing ? 1_500 : 3 * HEARTBEAT_MS);
  });
  if (expired && !reachableHigher) {
    eligibleSince ??= realNow;
    // Give a fresh probe of the earlier successors time to return before taking their place.
    const grace = takeoverProbeWindow(standby, higher.length, testing ? 800 : HEARTBEAT_MS);
    if (realNow - eligibleSince >= grace) {
      promote(host, setup);
      return;
    }
  } else eligibleSince = undefined;
  for (const key of standby ? higher : [setup.primary, ...higher]) {
    const token = nonce();
    probes.set(key, { token, at: now });
    send(host, link, key, { kind: "rank-probe", token });
  }
  if (standby) {
    ping = { token: nonce(), at: now };
    send(host, link, setup.primary, { kind: "ping", token: ping.token });
  }
}

function assignStandby(setup: Position, peer: { key: string; sign: string }, options?: { replace?: true; supersedes?: string }) {
  selecting = true;
  void proposeLeadership({ setup: setup.id, epoch: setup.epoch }, setup.primary, peer, options)
    .catch(error => console.warn("Standby selection:", error.message))
    .finally(() => { selecting = false; });
}

function availableStandby(link: LinkState, now: Clock, unavailable?: string) {
  const tip = leadershipOf(link);
  const retiring = tip?.body.retiringStandby ?? tip?.body.retiredStandby;
  const available = leadershipPeers(link).flatMap(peer => {
    if (peer.key === retiring || peer.key === unavailable || !peer.sign || !takesForwarded(peer.key, "setup.shared-saves")) return [];
    const candidate = availableComputer(peer.key, now);
    return candidate ? [{ peer, ...candidate }] : [];
  });
  const order = link.setup?.successorChain ? acceptedMembership(link)?.successorOrder : undefined;
  if (order) {
    const byKey = new Map(available.map(candidate => [candidate.peer.key, candidate]));
    for (const key of order) {
      const candidate = byKey.get(key);
      if (candidate) return candidate;
    }
    return undefined;
  }
  // Before there is a signed order, bootstrap from the availability reports.
  available.sort((a, b) => observedUptime(b.since, now) - observedUptime(a.since, now) || Number(b.preferred) - Number(a.preferred) || a.peer.key.localeCompare(b.peer.key));
  return available[0];
}

function refreshRank(position: Position, now: Clock) {
  const changed = updateLink(link => {
    const setup = followedSetup(link);
    if (!link || setup?.role !== "primary" || setup.id !== position.id || setup.epoch !== position.epoch
      || link.localWipe || primaryFenced(link) || pendingShared(link, "membershipAndAccess")) return null;
    const membership = acceptedMembership(link);
    if (!membership) return null;
    const order = rankComputers(membership.boxes, membership.successorOrder, now, membership.successorPin);
    if (!order || order.every((key, index) => key === membership.successorOrder?.[index])) return null;
    return stageShared(link, "membershipAndAccess", { ...membership, successorOrder: order }) ? link : null;
  }, { durable: true });
  if (changed) noteDocsChanged();
}

function promote(host: LinkHost, position: Position) {
  const changed = updateLink(link => {
    const setup = followedSetup(link);
    if (!link?.sign || !setup || setup.id !== position.id || setup.epoch !== position.epoch || setup.primary !== position.primary ||
      setup.role !== "member" || link.setupMerge || sharedValue(link, "setupDeparture")) return null;
    const own = ownKey(link);
    const membership = acceptedMembership(link);
    const allowed = setup.standby?.key === own && setup.standby?.sign === ownSign(link) ||
      setup.successorChain && membership?.boxes.some(peer => peer.key === own && peer.sign === ownSign(link));
    if (!own || !allowed) return null;
    const ledger = sharedValue(link, "queueNumberAllocation");
    const ceiling = setup.numberCeiling ?? (isObject(ledger) ? Number(ledger.next) - 1 : NaN);
    if (!Number.isSafeInteger(ceiling)) return null;
    const epoch = setup.epoch + 1;
    const authority = {
      setup: { id: setup.id, epoch, primary: own },
      previous: setup.primary, takeover: true, sharedSaves: true,
      ...(setup.successorChain ? { successorChain: true } : {}),
    };
    let doc;
    if (setup.checkpointed) {
      if (!membership || !isObject(ledger)) return null;
      doc = checkpointedTransition(link, authority, {
        ...link.shared,
        membershipAndAccess: { sequence: link.shared!.membershipAndAccess!.sequence, value: membership },
        queueNumberAllocation: { sequence: link.shared!.queueNumberAllocation!.sequence, value: { ...ledger, next: Math.max(ceiling + 1, Number(ledger.next) || 1) } },
      });
    } else {
      // COMPAT(layout.4 setup): recover a standby selected by a release that did not checkpoint. Goes once supported upgrades and rollbacks no longer use layout 4 setup records.
      const roster = setupDoc(link);
      const prior = leadershipOf(link);
      if (!roster || !prior) return null;
      const phones = held(link.docs?.[docKey(setup.sign, "phones")]);
      const membership = sharedValue(link, "membershipAndAccess");
      const accepted = isObject(membership) && membership.epoch === setup.epoch ? membership : null;
      const rosterBoxes = accepted?.boxes ?? roster.body.boxes;
      const boxes = Array.isArray(rosterBoxes) ? rosterBoxes.filter(fitsPeer).map(cleanPeer)
        .map(peer => ({ ...peer, name: acceptedName(link, peer.key, peer.name) })) : [];
      doc = readDoc(signDoc(link.sign, "leadership", epoch, {
        ...authority, boxes, left: accepted?.left ?? roster.body.left, moved: accepted?.moved ?? roster.body.moved,
        devices: accepted?.devices ?? (phones?.body.epoch === setup.epoch ? phones!.body : prior.body.devices),
        removalRevisions: isObject(accepted?.fields) ? accepted.fields.removalRevisions : prior.body.removalRevisions,
        shared: link.shared ?? {},
        numbers: { ...(isObject(ledger) ? ledger : isObject(prior.body.numbers) ? prior.body.numbers : {}), next: ceiling + 1 },
      }));
    }
    return doc && takeLeadership(link, doc) ? link : null;
  }, { durable: true });
  if (!changed?.setup) return;
  console.info(`Standby took over at setup epoch ${changed.setup.epoch} after ${DEFAULT_DELAY / 60_000} minutes without its main computer.`);
  sendDocs(host, changed, peersOf(changed).map(to => ({ to, docs: { give: leadershipDocuments(changed.leadership![changed.setup!.epoch]!) } })));
  noteDocsChanged();
  try { writeCard(); } catch {}
}
