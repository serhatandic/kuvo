// One mechanism picks the main computer: the lease at Kuvo's relay. The main renews it; another
// computer claims it once it lapses (always-on computers first, the relay decides); whoever holds it
// signs the next certificate, and everyone else follows. Nothing else votes, ranks or waits.
import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import type { Doc } from "../../shared/documents";
import { activeInstallerClaim } from "../add/invite-state";
import { sendMergeOffers } from "../add/invite";
import { noteDocsChanged } from "../documents/changed";
import { sendDocs } from "../documents/exchange";
import { docKey, held, ownSign } from "../documents/format";
import { mailFor, ownKey, peersOf, type MailSender } from "../peers/known";
import { removeComputerHere } from "../peers/manage";
import type { LinkHost } from "../relay/host";
import { readLink, updateLink, type LinkState } from "../state";
import { captureMembership } from "./access";
import { writeCard } from "./card";
import { checkpointedTransition, leadershipDocuments } from "./checkpoint";
import { leadershipOf, takeLeadership } from "./leadership";
import { HEARTBEAT_MS, holdsRelayLease, primaryFenced, relayLease, RELAY_HOST_WARNING, testSetupClock, type RelayLease } from "./lease";
import { acceptedMembership, membershipOf } from "./membership";
import { followedSetup, setupDoc } from "./record";
import { askRelayLease, refreshPrimaryLease } from "./relay-lease";
import { publishedShared, sharedValue } from "./shared";

function send(host: LinkHost, link: LinkState, to: string, value: Record<string, unknown>) {
  const peer = peersOf(link).find(each => each.key === to);
  const { id, epoch } = link.setup!;
  const bytes = peer && mailFor(peer, { setupAuthority: { id, epoch, ...value }, at: Date.now() }, link);
  if (bytes) host.send(bytes);
}

const tipSent = new Map<string, number>();

export function receiveAuthority(sender: MailSender, given: Record<string, unknown>, host: LinkHost | null | undefined) {
  if (!host?.welcomed) return;
  const link = readLink();
  const setup = followedSetup(link);
  if (!link || !setup || setup.id !== given.id) return;
  const peer = peersOf(link).find(each => each.key === sender.key);
  if (!peer || setup.role !== "primary") return;
  // A computer behind this main's certificate gets it, whatever it asked (once a minute: an old one may not take it).
  const tip = leadershipOf(link);
  if (tip && typeof given.epoch === "number" && given.epoch < setup.epoch) {
    if (Date.now() - (tipSent.get(peer.key) ?? 0) < (testSetupClock() ? 1_000 : 60_000)) return;
    tipSent.set(peer.key, Date.now());
    sendDocs(host, link, [{ to: peer, docs: { give: leadershipDocuments(tip.envelope) } }]);
    return;
  }
  if (given.kind === "available") {
    const give = publishedShared(link);
    if (give.length) sendDocs(host, link, [{ to: peer, docs: { give } }]);
    return;
  }
  // COMPAT(layout.4 setup): answer an old standby's ping while it can reach this main. Goes once supported upgrades and rollbacks no longer use layout 4 setup records.
  if (given.kind === "ping" && typeof given.token === "string" && /^[\w-]{22}$/.test(given.token) &&
    (relayLease(link)?.holder ?? setup.primary) === setup.primary) {
    send(host, link, sender.key, { kind: "pong", token: given.token, reply: crypto.randomBytes(16).toString("base64url") });
  }
}

/** The main's certificate at its relay term: itself as main, and the setup's membership and shared
 * values in one checkpoint. A takeover and the conversion of an older setup are the same step. */
function leaseCertificate(link: LinkState, lease: RelayLease): Doc | null {
  const setup = followedSetup(link);
  const own = ownKey(link);
  if (!link.sign || !setup || !own) return null;
  const roster = setupDoc(link)?.body;
  const legacy = sharedValue(link, "membershipAndAccess");
  const phones = held(link.docs?.[docKey(setup.sign, "phones")]);
  const membership = acceptedMembership(link) ?? (setup.role === "primary" ? captureMembership(link) : membershipOf({
    ...roster, ...(isObject(legacy) ? legacy : {}),
    devices: (isObject(legacy) ? legacy.devices : undefined) ?? (phones?.body.epoch === setup.epoch ? phones!.body : roster?.devices),
  }));
  if (!membership?.boxes.some(peer => peer.key === own && peer.sign === ownSign(link))) return null;
  const shared = link.sharedSetup === setup.id ? link.shared : undefined;
  return checkpointedTransition(link, {
    setup: { id: setup.id, epoch: setup.epoch + 1, primary: own }, previous: setup.primary, relayTerm: lease.term,
  }, { ...shared, membershipAndAccess: { sequence: shared?.membershipAndAccess?.sequence ?? 1, value: membership } });
}

/** Whether the main's setup still uses an older authority model (Mac-led, layout 4, a standby, ranks or
 * a certificate from before the relay lease): the main converts it in one step. */
function olderModel(link: LinkState): boolean {
  const tip = leadershipOf(link);
  return !link.setup?.checkpointed || tip?.body.relayTerm === undefined || tip.body.standby !== undefined || tip.body.successorChain !== undefined;
}

export function leadAtTerm(host: LinkHost | null | undefined, lease: RelayLease, why: string) {
  const position = followedSetup(readLink());
  if (!position) return;
  const changed = updateLink(link => {
    const setup = followedSetup(link);
    if (!link?.sign || !setup || setup.id !== position.id || setup.epoch !== position.epoch || setup.primary !== position.primary ||
      link.setupMerge || sharedValue(link, "setupDeparture") || relayLease(link)?.term !== lease.term) return null;
    const doc = leaseCertificate(link, lease);
    return doc && takeLeadership(link, doc) ? link : null;
  }, { durable: true });
  if (!changed?.setup) return;
  console.info(`${why} at relay term ${lease.term}.`);
  if (host?.welcomed) {
    const give = leadershipDocuments(changed.leadership![changed.setup.epoch]!);
    sendDocs(host, changed, peersOf(changed).map(to => ({ to, docs: { give } })));
  }
  noteDocsChanged();
  try { writeCard(); } catch {}
}

/** The previous main asked this computer to unlink it once it leads (PROTOCOL.md, "Unlinking the main
 * computer"). Done when this computer leads at the next epoch; dropped once the setup moves on otherwise. */
async function unlinkPreviousMain(link: LinkState) {
  const intent = link.unlinkAfterHandover;
  const setup = followedSetup(link);
  if (!intent || !setup) return;
  const due = setup.id === intent.setup && setup.epoch === intent.epoch + 1 && setup.role === "primary";
  const waiting = setup.id === intent.setup && setup.epoch === intent.epoch;
  if (due && primaryFenced(link)) return;
  const done = due ? await removeComputerHere(intent.key, false, { wipe: intent.wipe }) : null;
  if (waiting || done && done.status >= 300 && done.status !== 404) return;
  updateLink(current => {
    if (!current || current.unlinkAfterHandover?.key !== intent.key) return null;
    delete current.unlinkAfterHandover;
    return current;
  }, { durable: true });
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
      const result = setup.role === "primary" ? await refreshPrimaryLease(link) : await askRelayLease(link, "read");
      const lease = result?.lease;
      const live = host();
      if (setup.role === "primary" && lease?.holder === own && lease?.relayHost && !warned) {
        warned = true;
        console.warn(RELAY_HOST_WARNING);
      }
      if (setup.role === "primary" && !wasHeld && holdsRelayLease(link)) noteDocsChanged();
      if (setup.role === "primary" && lease && holdsRelayLease(link) && !primaryFenced(link) && olderModel(link)) {
        leadAtTerm(live, lease, "Converted the setup to the relay lease");
      }
      if (setup.role === "primary" && live?.welcomed) {
        const current = readLink();
        if (current && !primaryFenced(current, true)) sendMergeOffers(live);
      }
      if (setup.role === "member" && live?.welcomed && Date.now() >= nextAvailability) {
        nextAvailability = Date.now() + heartbeat;
        send(live, link, setup.primary, { kind: "available" });
      }
      // A main this computer hasn't followed yet: ask it for its certificate.
      if (lease && lease.holder !== setup.primary && lease.holder !== own && live?.welcomed)
        send(live, link, lease.holder, { kind: "relay-leadership", term: lease.term });
      if (lease && setup.role === "member" && !link.setupMerge && !sharedValue(link, "setupDeparture")) {
        const won = lease.holder === own ? lease : result?.takeoverIn === 0
          ? (await askRelayLease(link, "claim", lease.term))?.lease : null;
        if (won?.holder === own) leadAtTerm(live, won, "Took over as main computer");
      }
      const after = readLink();
      if (after?.unlinkAfterHandover) await unlinkPreviousMain(after);
      next = Date.now() + (setup.role === "primary" && (!lease || lease.holder === own) ? heartbeat : testing ? 200 : 2_000);
    } finally { busy = false; }
  }
  const check = () => void tick().catch(error => console.warn("Relay lease:", (error as Error).message));
  check();
  setInterval(check, testing ? 200 : 1_000).unref();
}
