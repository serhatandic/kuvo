import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import { activeInstallerClaim } from "../add/invite-state";
import { sendMergeOffers } from "../add/invite";
import { noteDocsChanged } from "../documents/changed";
import { sendDocs } from "../documents/exchange";
import { docKey, held, ownSign } from "../documents/format";
import { mailFor, ownKey, peersOf, type MailSender } from "../peers/known";
import type { LinkHost } from "../relay/host";
import { readLink, updateLink, type LinkState } from "../state";
import { writeCard } from "./card";
import { checkpointedTransition, leadershipDocuments } from "./checkpoint";
import { leadershipOf, takeLeadership } from "./leadership";
import { HEARTBEAT_MS, holdsRelayLease, leaseClock, primaryFenced, relayLease, RELAY_HOST_WARNING, standbySettings, testSetupClock, type RelayLease } from "./lease";
import { acceptedMembership, membershipOf } from "./membership";
import { followedSetup, setupDoc, type SetupRecord } from "./record";
import { askRelayLease, refreshPrimaryLease } from "./relay-lease";
import { observeAvailability } from "./reliability";
import { publishedShared, sharedValue } from "./shared";

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
  const peer = peersOf(link).find(each => each.key === sender.key);
  if (!peer) return;
  if (given.kind === "relay-leadership") {
    const tip = leadershipOf(link);
    if (setup.role === "primary" && tip && tip.body.relayTerm === given.term && given.epoch !== setup.epoch)
      sendDocs(host, link, [{ to: peer, docs: { give: leadershipDocuments(tip.envelope) } }]);
    return;
  }
  if (setup.epoch !== given.epoch || setup.successorChain && given.tip !== (leadershipOf(link)?.h ?? setup.leadershipHash)) return;
  if (given.kind === "available" && setup.role === "primary" && typeof given.preferred === "boolean" && typeof given.reason === "string") {
    observeAvailability(sender.key, leaseClock(), given.preferred, given.reason.slice(0, 40));
    const give = publishedShared(link);
    if (give.length) sendDocs(host, link, [{ to: peer, docs: { give } }]);
    return;
  }
  if (typeof given.token !== "string" || !/^[\w-]{22}$/.test(given.token)) return;
  // COMPAT(beta.40): Keep old standbys from starting their own takeover while they can reach this main.
  if (given.kind === "ping" && setup.role === "primary" && sender.key === setup.standby?.key &&
    (relayLease(link)?.holder ?? setup.primary) === setup.primary) {
    send(host, link, sender.key, { kind: "pong", token: given.token, reply: crypto.randomBytes(16).toString("base64url") });
  }
  if (given.kind === "rank-probe" && setup.successorChain) send(host, link, sender.key, { kind: "rank-answer", token: given.token });
}

export function startTakeover(host: () => LinkHost | null | undefined, active = () => true) {
  const testing = testSetupClock();
  const heartbeat = testing ? 400 : HEARTBEAT_MS;
  let next = 0;
  let nextAvailability = 0;
  let position = "";
  let busy = false;
  let warned = false;
  async function tick() {
    const link = readLink();
    const setup = followedSetup(link);
    if (!active() || busy || !link || !setup || !link.relay || activeInstallerClaim(link)) return;
    const current = `${setup.id}:${setup.epoch}:${setup.primary}`;
    if (position !== current) { position = current; next = nextAvailability = 0; warned = false; }
    if (Date.now() < next) return;
    busy = true;
    try {
      const own = ownKey(link);
      const wasHeld = holdsRelayLease(link);
      const result = setup.role === "primary"
        ? await refreshPrimaryLease(link)
        : await askRelayLease(link, "read");
      const lease = result?.lease;
      const live = host();
      if (setup.role === "primary" && lease?.holder === own && lease?.relayHost && !warned) {
        warned = true;
        console.warn(RELAY_HOST_WARNING);
      }
      if (setup.role === "primary" && !wasHeld && holdsRelayLease(link)) noteDocsChanged();
      if (setup.role === "primary" && live?.welcomed) {
        const current = readLink();
        if (current && !primaryFenced(current, true)) {
          sendMergeOffers(live);
        }
      }
      if (setup.role === "member" && live?.welcomed && Date.now() >= nextAvailability) {
        nextAvailability = Date.now() + heartbeat;
        const settings = standbySettings();
        send(live, link, setup.primary, { kind: "available", preferred: settings.alwaysOn, reason: settings.reason });
      }
      if (lease && lease.holder !== setup.primary && lease.holder !== own && live?.welcomed)
        send(live, link, lease.holder, { kind: "relay-leadership", term: lease.term });
      if (lease && setup.role === "member" && !link.setupMerge && !sharedValue(link, "setupDeparture")) {
        const won = lease.holder === own ? lease : result?.takeoverIn === 0
          ? (await askRelayLease(link, "claim", lease.term))?.lease : null;
        if (won?.holder === own) promote(live, setup, won);
      }
      next = Date.now() + (setup.role === "primary" && (!lease || lease.holder === own) ? heartbeat : testing ? 200 : 2_000);
    } finally { busy = false; }
  }
  const check = () => void tick().catch(error => console.warn("Relay lease:", (error as Error).message));
  check();
  setInterval(check, testing ? 200 : 1_000).unref();
}

function promote(host: LinkHost | null | undefined, position: SetupRecord, lease: RelayLease) {
  const changed = updateLink(link => {
    const setup = followedSetup(link);
    if (!link?.sign || !setup || setup.id !== position.id || setup.epoch !== position.epoch || setup.primary !== position.primary ||
      setup.role !== "member" || link.setupMerge || sharedValue(link, "setupDeparture")) return null;
    const own = ownKey(link);
    const legacy = sharedValue(link, "membershipAndAccess");
    const roster = setupDoc(link)?.body;
    const phones = held(link.docs?.[docKey(setup.sign, "phones")]);
    const membership = acceptedMembership(link) ?? membershipOf({
      ...roster, ...(isObject(legacy) ? legacy : {}),
      devices: (isObject(legacy) ? legacy.devices : undefined) ?? (phones?.body.epoch === setup.epoch ? phones!.body : roster?.devices),
    });
    if (!own || !membership?.boxes.some(peer => peer.key === own && peer.sign === ownSign(link))) return null;
    const epoch = setup.epoch + 1;
    const doc = checkpointedTransition(link, {
      setup: { id: setup.id, epoch, primary: own },
      previous: setup.primary, takeover: true, relayTerm: lease.term,
      ...(setup.successorChain ? { successorChain: true } : {}),
    }, {
      ...link.shared,
      membershipAndAccess: { sequence: link.shared?.membershipAndAccess?.sequence ?? 1, value: membership },
    });
    return doc && takeLeadership(link, doc) ? link : null;
  }, { durable: true });
  if (!changed?.setup) return;
  console.info(`Took over as main computer at relay term ${lease.term}.`);
  if (host?.welcomed) {
    const give = leadershipDocuments(changed.leadership![changed.setup.epoch]!);
    sendDocs(host, changed, peersOf(changed).map(to => ({ to, docs: { give } })));
  }
  noteDocsChanged();
  try { writeCard(); } catch {}
}
