// Setup leadership (PROTOCOL.md, "Your setup"). The first independent box creates epoch 1
// and signs its membership. Existing Mac-led setups still follow their legacy numbering rule.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { setupLayoutDeferred } from "../../shared/folder-version";
import { newKey } from "../crypto/keys";
import { noteDocsChanged } from "../documents/changed";
import { newSigningSeed, ownSign } from "../documents/format";
import { IS_MAC, K, REAL_BOX } from "../home";
import { computerName, DEFAULT_RELAY } from "../mac/link";
import { ADD_RESERVATION } from "../marks";
import { macsOf, ownKey } from "../peers/known";
import { readLink, updateLink } from "../state";
import { rankedPrimary } from "../work-queue/numbers";
import { archiveDeviceRevocations } from "./device-revocations";
import { captureMembership } from "./access";
import { writeCard } from "./card";
import { checkpointedTransition } from "./checkpoint";
import { followedSetup, newSetupID } from "./record";
import { installShared } from "./shared";
import { settleSetup } from "./settle";
import type { LinkState } from "../state";

/** Keep an independent primary's role after it adds members. Legacy Mac numbering still applies. */
export function leadsNow(link: LinkState, now = Date.now()): boolean {
  const setup = followedSetup(link);
  if (setup?.managed) return setup.role === "primary";
  // (A machine its Mac let go waits for that Mac as numbering always has, days, before it numbers
  // on its own: never at once.)
  if (!IS_MAC) {
    if (macsOf(link).length || link.setup?.role === "member") return false;
    if (followedSetup(link)?.role === "primary") return true;
  }
  // COMPAT: setups from the Mac-ranked queue numbering era.
  return "self" in rankedPrimary(link, undefined, now);
}

/** Keep a primary's previous static keys for SSH re-enrollment after a key rotation. */
export function noteKeyNow(link: LinkState): void {
  const own = IS_MAC || !macsOf(link).length ? ownKey(link) : null;
  if (!own) return;
  if (link.keyNow && link.keyNow !== own) link.keysBefore = [...(link.keysBefore ?? []).filter(key => key !== own && key !== link.keyNow), link.keyNow].slice(-8);
  link.keyNow = own;
}

function sshAccountReserved(): boolean {
  return fs.lstatSync(path.join(path.dirname(K), ADD_RESERVATION), { throwIfNoEntry: false }) !== undefined;
}

/** This box's setup as it stands now (in place, under link.json's lock): one it leads made, or set
 * aside while it doesn't lead it; a machine's taken as its documents say. */
export function settleOwnSetup(link: LinkState, now = Date.now()): void {
  if (link.macRotation) return;
  noteKeyNow(link);
  settleSetup(link);
  if (link.localWipe || (link.setup?.managed && followedSetup(link))) return;
  const own = ownKey(link);
  const sign = ownSign(link);
  if (!own || !sign || (!IS_MAC && macsOf(link).length)) return;
  const leads = leadsNow(link, now);
  // (Keys made anew, its folder kept: a new setup, as it's a new computer to the others; the one
  // it led with its old keys is no longer its own.)
  if (!link.setup || link.setup.role !== "primary" || link.setup.primary !== own || link.setup.sign !== sign) {
    if (!leads) {
      if (link.setup?.role === "primary") delete link.setup;
      return;
    }
    // While SSH owns the account it supplies the enrollment. A startup in the
    // gap between reservations can create an empty standalone setup.
    if (!IS_MAC && sshAccountReserved()) return;
    if (!link.setup && !macsOf(link).length) {
      if (link.removed?.length) link.departedKey ??= own;
      archiveDeviceRevocations(link);
    }
    startSetup(link);
  }
  if (leads) delete link.setup!.aside;
  else link.setup!.aside = true;
}

/** A new setup of this computer alone, as every setup is now: epoch 1's certificate, and the
 * checkpoint naming this computer its only member. (Below the checkpoint layout, the bare setup
 * that its first relay lease converts.) */
export function startSetup(link: LinkState, id = newSetupID()): void {
  const own = ownKey(link);
  const sign = ownSign(link);
  if (!own || !sign) return;
  link.setup = { id, epoch: 1, primary: own, sign, role: "primary" };
  installShared(link, undefined);
  const membership = captureMembership(link);
  const shared = membership ? { membershipAndAccess: { sequence: 1, value: membership } } : null;
  // The relay starts a new setup at term 1. Its first lease should not advance the epoch
  // and invalidate an installer code issued while this computer was starting.
  const doc = shared && checkpointedTransition(link, { setup: { id, epoch: 1, primary: own }, relayTerm: 1 }, shared);
  if (!doc || !shared) return;
  link.setup = { ...link.setup, managed: true, checkpointed: true };
  link.leadership = { 1: doc.envelope };
  installShared(link, shared);
}

/** Initialize an independent machine before any client needs to pair or add a member.
 * An account reserved by SSH setup gets its identity and trust from that setup command. */
export function initializeMachineSetup(): void {
  if (IS_MAC || setupLayoutDeferred(K)) return;
  const kept = readLink();
  if (kept && (macsOf(kept).length || (kept.relay && followedSetup(kept)))) return;
  const changed = updateLink(current => {
    if (setupLayoutDeferred(K)) return null;
    if (current && (macsOf(current).length || current.localUnlink || current.localWipe || current.removalSource
      || current.installerClaim && !current.installerClaim.adopted && current.installerClaim.until > Date.now())) return null;
    if (sshAccountReserved()) return null;
    const link = current ?? { phones: [] };
    link.key ??= newKey().toString("base64url");
    link.secret ??= crypto.randomBytes(32).toString("base64url");
    link.sign ??= newSigningSeed();
    link.name ??= computerName();
    const relay = REAL_BOX ? DEFAULT_RELAY : process.env.KUVO_TEST_RELAY;
    if (!link.relay && relay) {
      try {
        if (["ws:", "wss:"].includes(new URL(relay).protocol)) link.relay = relay.replace(/\/$/, "");
      } catch {}
    }
    settleOwnSetup(link);
    return link;
  }, { durable: true });
  if (changed) writeCard();
}

let lookedAt = 0;
/** Looked at now and then (a box's rounds, every few seconds: once a minute at most, the first at
 * once): this box's setup settled, and when that changed it, its documents told to its peers (this
 * Mac's peers document names it, or stops); its card written. */
export function noteSetupNow(now = Date.now(), { soon = 60_000 }: { soon?: number } = {}): void {
  if (now - lookedAt < soon) return;
  lookedAt = now;
  let changed = false;
  try {
    updateLink(link => {
      if (!link) return null;
      const before = JSON.stringify(link.setup ?? null);
      settleOwnSetup(link, now);
      if (JSON.stringify(link.setup ?? null) === before) return null;
      changed = true;
      return link;
    }, { durable: true });
  } catch {}
  if (changed) noteDocsChanged();
  try { writeCard(); } catch {}
}
