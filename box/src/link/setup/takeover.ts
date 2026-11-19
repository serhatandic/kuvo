// The current standby alone can advance a delegated epoch. Its fresh round trip with the
// primary renews a short lease. A failed round trip never refreshes either side's clock.
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
import { proposeLeadership, standbyHere } from "./handover";
import { CHECKPOINT_FEATURE, checkpointedTransition, leadershipDocuments } from "./checkpoint";
import { acceptedMembership, acceptedName } from "./membership";
import { leadershipOf, pendingStandby, takeLeadership } from "./leadership";
import { fresh, grantLease, HEARTBEAT_MS, leaseClock, LEASE_MS, primaryFenced, standbySettings, testSetupClock } from "./lease";
import { publishedShared, sharedValue } from "./shared";
import { followedSetup, setupDoc } from "./record";

type Clock = ReturnType<typeof leaseClock>;
type Position = { id: string; epoch: number };
const nonce = () => crypto.randomBytes(16).toString("base64url");
const signal = (token: unknown): token is string => typeof token === "string" && /^[\w-]{22}$/.test(token);

let position = "";
let since: Clock;
let lastTick: Clock | undefined;
let lastRound = 0;
let selecting = false;
let loggedDelay: number | undefined;
let ping: { token: string; at: Clock } | undefined;
let pong: { token: string; reply: string; at: Clock } | undefined;
let lastPrimary: Clock | undefined;
let ownPrimarySince: Clock | undefined;
const candidates = new Map<string, { at: Clock; since: Clock; preferred: boolean; reason: string }>();

function reset(setup: Position, now: Clock) {
  const next = `${setup.id}:${setup.epoch}`;
  if (position === next) return;
  position = next;
  since = now;
  ping = pong = lastTick = lastPrimary = ownPrimarySince = undefined;
  candidates.clear();
  loggedDelay = undefined;
}

function send(host: LinkHost, link: LinkState, to: string, value: Record<string, unknown>) {
  const peer = peersOf(link).find(each => each.key === to);
  const { id, epoch } = link.setup!;
  const bytes = peer && mailFor(peer, { setupAuthority: { id, epoch, ...value }, at: Date.now() }, link);
  if (bytes) host.send(bytes);
}

export function receiveAuthority(sender: MailSender, given: Record<string, unknown>, host: LinkHost | null | undefined) {
  if (!host?.welcomed) return;
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || !setup || setup.id !== given.id || setup.epoch !== given.epoch) return;
  const now = leaseClock();
  reset(setup, now);
  if (given.kind === "available" && setup.role === "primary") {
    if (typeof given.preferred !== "boolean" || typeof given.reason !== "string") return;
    const last = candidates.get(sender.key);
    const since = last && fresh(last.at, now, 3 * HEARTBEAT_MS) ? last.since : now;
    candidates.set(sender.key, { at: now, since, preferred: given.preferred, reason: given.reason.slice(0, 40) });
    const peer = peersOf(link).find(peer => peer.key === sender.key);
    const give = publishedShared(link);
    if (peer && give.length && peer.key !== setup.standby?.key) sendDocs(host, link, [{ to: peer, docs: { give } }]);
    return;
  }
  if (!setup.standby || !signal(given.token)) return;
  if (setup.role === "primary" && setup.standby.key === sender.key && !link.leadershipPending) {
    if (given.kind === "ping") {
      pong = { token: given.token, reply: nonce(), at: now };
      send(host, link, sender.key, { kind: "pong", token: given.token, reply: pong.reply });
    } else if (given.kind === "lease" && pong?.token === given.token && pong.reply === given.reply && fresh(pong.at, now, LEASE_MS / 2)) {
      const fenced = primaryFenced(link);
      grantLease(link, pong.at);
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
    since = now;
  }
  lastRound = realNow;
  if (!host?.welcomed) return;
  if (setup.role === "primary" && link.leadershipPending) {
    const standby = peersOf(link).find(peer => peer.key === setup.standby?.key) ?? pendingStandby(link);
    if (standby) sendDocs(host, link, [{ to: standby, docs: { give: leadershipDocuments(link.leadershipPending) } }]);
    return;
  }
  if (setup.role === "primary" && setup.standby && !primaryFenced(link) && Object.keys(link.sharedPending ?? {}).length) {
    const standby = peersOf(link).find(peer => peer.key === setup.standby!.key);
    if (standby) sendDocs(host, link, [{ to: standby, docs: { give: Object.values(link.sharedPending!) } }]);
  }
  if (lastTick && fresh(lastTick, now, testing ? 400 : HEARTBEAT_MS)) return;
  lastTick = now;
  const settings = standbySettings();
  if (setup.role === "primary") {
    ownPrimarySince ??= now;
    if (setup.standby) {
      const ledger = sharedValue(link, "queueNumberAllocation");
      const next = Math.max(readNumbers().next ?? 1, isObject(ledger) ? Number(ledger.next) || 1 : 1);
      const migrating = !setup.checkpointed && peersOf(link).every(peer => takesForwarded(peer.key, CHECKPOINT_FEATURE));
      if ((migrating || (setup.numberCeiling && setup.numberCeiling - next < 400)) && !primaryFenced(link)) {
        void proposeLeadership({ setup: setup.id, epoch: setup.epoch }, setup.primary, setup.standby)
          .catch(error => console.warn("Standby number reservation:", error.message));
      }
      return;
    }
    if (selecting || fresh(ownPrimarySince, now, testing ? 2_000 : 90_000)) return;
    if (peersOf(link).some(peer => !peer.sign || !takesForwarded(peer.key, "setup.shared-saves"))) return;
    const next = availableStandby(link, now);
    if (!next) return;
    selecting = true;
    void standbyHere(next.peer.key, { setup: setup.id, epoch: setup.epoch }, undefined, next.reason)
      .catch(error => console.warn("Standby selection:", error.message))
      .finally(() => { selecting = false; });
    return;
  }
  if (setup.standby?.key !== ownKey(link) || setup.standby.sign !== ownSign(link)) {
    send(host, link, setup.primary, { kind: "available", preferred: settings.alwaysOn, reason: settings.reason });
    return;
  }
  if (loggedDelay !== settings.delay) {
    loggedDelay = settings.delay;
    console.info(`Standby watching epoch ${setup.epoch} with a ${settings.delay / 60_000}-minute takeover delay.`);
  }
  const heard = lastPrimary ?? since;
  if (now.uptime - heard.uptime >= settings.delay && now.wall - heard.wall >= settings.delay) {
    promote(host, setup);
    return;
  }
  ping = { token: nonce(), at: now };
  send(host, link, setup.primary, { kind: "ping", token: ping.token });
}

function availableStandby(link: LinkState, now: Clock) {
  const retiring = leadershipOf(link)?.body.retiredStandby;
  const available = peersOf(link).flatMap(peer => {
    if (peer.key === retiring || !peer.sign || !takesForwarded(peer.key, "setup.shared-saves")) return [];
    const candidate = candidates.get(peer.key);
    return candidate && fresh(candidate.at, now, testSetupClock() ? 10_000 : 3 * HEARTBEAT_MS) ? [{ peer, ...candidate }] : [];
  });
  // Use platform preference during bootstrap, then prefer sustained observed availability.
  const observed = (since: Clock) => Math.floor((now.uptime - since.uptime) / (6 * HEARTBEAT_MS));
  available.sort((a, b) => observed(b.since) - observed(a.since) || Number(b.preferred) - Number(a.preferred) || a.peer.key.localeCompare(b.peer.key));
  return available[0];
}

function promote(host: LinkHost, position: Position) {
  const changed = updateLink(link => {
    const setup = followedSetup(link);
    if (!link?.sign || !setup?.standby || setup.id !== position.id || setup.epoch !== position.epoch
      || ownKey(link) !== setup.standby.key || ownSign(link) !== setup.standby.sign) return null;
    const ceiling = setup.numberCeiling;
    if (typeof ceiling !== "number") return null;
    const epoch = setup.epoch + 1;
    const authority = {
      setup: { id: setup.id, epoch, primary: setup.standby.key },
      previous: setup.primary, takeover: true, sharedSaves: true,
    };
    const ledger = sharedValue(link, "queueNumberAllocation");
    let doc;
    if (setup.checkpointed) {
      if (!acceptedMembership(link) || !isObject(ledger)) return null;
      doc = checkpointedTransition(link, authority, {
        ...link.shared,
        queueNumberAllocation: { sequence: link.shared!.queueNumberAllocation!.sequence, value: { ...ledger, next: ceiling + 1 } },
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
  console.info(`Standby took over at setup epoch ${changed.setup.epoch} after ${standbySettings().delay / 60_000} minutes without its primary.`);
  sendDocs(host, changed, peersOf(changed).map(to => ({ to, docs: { give: leadershipDocuments(changed.leadership![changed.setup!.epoch]!) } })));
  noteDocsChanged();
  try { writeCard(); } catch {}
}
