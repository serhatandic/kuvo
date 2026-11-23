import { noteDocsChanged } from "./documents/changed";
// What this box keeps of its link in `run/link.json` (PROTOCOL.md, "Pairing"; only you can read it):
// its keys and relay, the Macs that set it up, the documents it holds, its phones. One change at a
// time, from this process and from `setup` and `pair` (other processes), under a lock.
//
// Nothing here knows what follows a change: the box says that as it starts (`onLinkWritten`).

import fs from "node:fs";
import path from "node:path";
import type { SignedDoc } from "../shared/documents";
import { objectFormat, UnsupportedFormatVersion } from "../shared/format-version";
import { isObject } from "../shared/json";
import { SIGN } from "./documents/format";
import { locked, privateFolder, writeDurablePrivate, writeSecret } from "./files";
import { IS_MAC, LINK_FILE, RUN } from "./home";
import type { NoticeHold } from "./notices/hold";
import type { Peer } from "./peers/known";
import type { LinkHost } from "./relay/host";
import type { OperationOutcomes, SharedKind, SharedValues } from "./setup/shared";
import type { LocalWipe, MemberRemoval, WipePreparationClaim } from "./setup/removal-record";
import type { ComputerHistory } from "./setup/computer-history";
import type { SetupRecord } from "./setup/record";

/** A phone this box lets in (PROTOCOL.md, "Paired phones"). */
export interface Phone {
  key: string;
  name?: string;
  added?: number;
  /** Paired here, or listed by one of its Macs (`mac`, that Mac's key). */
  from?: "pair" | "mac";
  /** A member's independent pairing, kept local when it later becomes primary. */
  local?: true;
  mac?: string;
  /** Where it is on the relay, for its notifications. */
  address?: string;
  pairing?: string;
  /** What it is, as it said (`clients.kind`): Kuvo Code's desktop app; none for a phone. */
  kind?: "desktop";
}

/** A pairing link waiting: its token, good until `expires` (ms). */
export interface Pairing {
  token: string;
  expires: number;
  /** The relay pairing that requested this link, or null for a link issued locally. */
  issuer?: { key: string; pairing: string } | null;
}


export interface LinkState {
  installerInvites?: Record<string, import("./add/invite-state").InstallerInvite>;
  installerClaim?: import("./add/invite-state").InstallerClaim;
  installerCancellations?: import("./add/invite-state").InstallerCancellation[];
  installerRecoveries?: import("./setup/membership").Membership["installerRecoveries"];
  setupMerge?: import("./setup/merge-data").MergeJournal;
  lastSetupMerge?: import("./setup/merge-data").FinishedMerge;
  /** A successful unlink established this standalone setup with the owner's local work. */
  releasedSetup?: string;
  releasedWork?: string | null;
  releasedDatabaseWork?: string;
  /** The old enrollment's key, retained for unlink acknowledgement and SSH discovery. */
  departedKey?: string;
  formerPhoneIssuers?: Record<string, string[]>;
  codeIdentity?: string;
  macIdentityVersion?: string;
  macRotation?: { attempt: string; previous: string };
  formerDeviceRevocations?: Record<string, import("./setup/device-revocations").DeviceRevocation[]>;
  relay?: string;
  secret?: string;
  /** Its static key, and its signing key's seed (base64url). */
  key?: string;
  sign?: string;
  name?: string;
  /** Its id on its Macs (their name for this machine). */
  id?: string;
  /** Direct SSH enrollers, including a Linux primary, in legacy fields. `mac` names the
   * last enroller. Last-heard times (ms) account for time this box was off (`noteOnline`). */
  mac?: string;
  macs?: string[];
  macSigns?: Record<string, string>;
  macSeen?: Record<string, number>;
  /** Each Mac's name, as its setup said it. */
  macNames?: Record<string, string>;
  /** The documents it holds, by origin and kind; how far its own are, and the views it's met. */
  docs?: Record<string, SignedDoc>;
  own?: Record<string, { n: number; h: string; b: string }>;
  views?: Record<string, { n: number; h: string }>;
  phones?: Phone[];
  removed?: { key: string; pairing: string }[];
  /** A phone's pairing link, and (this Mac's) the one Settings › Phone shows. */
  pairing?: Pairing;
  appPairing?: Pairing;
  /** Removed members, for the primary's peers document. */
  left?: string[];
  /** Whether each Mac that started its engine leaves a newer box as it is, by the Mac's id. */
  tolerantIDs?: Record<string, boolean>;
  // COMPAT(beta.1 peers-list): a Mac that signs no documents yet, its peers as it listed them. Goes on the user's word, once no Mac from before 1.0 connects.
  legacy?: Record<string, { at: number; peers: unknown[]; moved: unknown[] }>;
  macPeers?: unknown;
  macMoved?: unknown;
  /** The names you gave this Mac's machines, by key, over what its app says (`kuvo/computers/<key>`). */
  peerNames?: Record<string, string>;
  seedNames?: { setup: string; names: Record<string, string> };
  /** Machines removed through `kuvo/computers` (by key), which this Mac's app saying its machines
   * doesn't bring back: only completing an explicit add does. */
  removedHere?: string[];
  /** Per-key removal revisions, retained across re-adds so an older SSH Add can't restore trust. */
  removalRevisions?: Record<string, number>;
  computerHistory?: ComputerHistory;
  memberRemovals?: Record<string, MemberRemoval>;
  wipePreparations?: Record<string, WipePreparationClaim>;
  mergedComputers?: Record<string, import("./setup/merge-data").MergePlan>;
  preJoinPairings?: Phone[];
  localWipe?: LocalWipe;
  localUnlink?: { setup: string; operation: string; primary: Peer; generation?: string; completed?: true };
  removalSource?: { setup: string; operation: string; moveTo?: string; review?: "choose-export" | "unsupported-layout"; stopped?: true; hostKeysCleared?: true };
  removalAcks?: Record<string, string>;
  /** A removed peer's saved destination, kept until its machines-list cleanup succeeds. */
  pendingMachineCleanup?: Record<string, string>;
  /** Machines this Mac removed that haven't heard it yet (they hear it once it's back on the relay). */
  untold?: Peer[];
  /** Peers documents at the epoch that revoked an offline member. */
  untoldMembership?: Record<string, SignedDoc>;
  /** The setup it's in, and its role there (PROTOCOL.md, "Your setup"). */
  setup?: SetupRecord;
  /** Verified leadership changes, indexed by setup epoch for offline catch-up. */
  leadership?: Record<number, SignedDoc>;
  /** The previous main asked this computer to unlink it once it takes over at `epoch` + 1 (`takeover.ts`). */
  unlinkAfterHandover?: { setup: string; epoch: number; key: string; wipe: boolean };
  /**
   * COMPAT(layout.4 setup): keep an unfinished proposal until the main holds its lease. Goes once supported upgrades and rollbacks no longer use layout 4 setup records.
   */
  leadershipPending?: SignedDoc;
  sharedSetup?: string;
  shared?: SharedValues;
  sharedOutcomes?: OperationOutcomes;
  /** This Mac's static key as its box last had it, and the ones before (made anew since): a machine
   * this Mac sets up again lets those go (`replaces` in setup). */
  keyNow?: string;
  keysBefore?: string[];
  /** The #181 switch, as you set it (none: it follows the role, `startsFromOthers`). */
  startsFromOthers?: "on" | "off";
}

// This Mac's box's link (`runMacLink`): what its app said (`kuvo/link/config`, or as the box kept
// it, mac/link.ts), its keys from the app once it has them (with the relay and name, `overlay`,
// never written in link.json), how it stands, and who follows that and its phones.
const MAC_OVERLAY = ["key", "secret", "sign", "relay", "name"];
/** What this Mac's app says of its box's link (`kuvo/link/config`): whether the phone link is on
 * and there are machines to meet, the relay, this Mac's name and its app's version. */
export interface MacLinkConfig {
  phones: boolean;
  machines: boolean;
  relay: string;
  name: string;
  app: string;
}

export const macLink: {
  config: MacLinkConfig | null;
  overlay: Partial<LinkState> | null;
  status: { state: "off" | "connecting" | "connected" | "failed"; reason?: string };
  host: LinkHost | null;
  /** The notices this Mac's box holds while you're at a computer (its link's). */
  hold?: NoticeHold;
  /** Stopped for good (a test's, as it ends): the link's loop goes. */
  stopped?: boolean;
  /** Stops this Mac's box's part among your other computers (`runMacPeers`). */
  stopPeers?: (() => void) | null;
  wake: (() => void) | null;
  /** When this Mac's app last said its link's config (the old app, as it attaches). */
  appSaid?: number;
  statusFollowers: Set<() => void>;
  phoneFollowers: Set<() => void>;
} = { config: null, overlay: null, status: { state: "off" }, host: null, wake: null, statusFollowers: new Set(), phoneFollowers: new Set() };

/** A machine's bridge owns the relay even when a separate local server answers clients. */
export const machineLink: { host: LinkHost | null } = { host: null };
export const liveLink = () => IS_MAC ? macLink.host : machineLink.host;

const written: ((link: LinkState) => void)[] = [];
/** What follows each write, from now on (the box says it as it starts). */
export const onLinkWritten = (then: (link: LinkState) => void) => {
  written.push(then);
};

// COMPAT(beta.1 peers-list): a Mac that gave no signing key signs no documents. Goes on the user's word, once no Mac from before 1.0 connects.
// The signing key a Mac of this box's gave in setup, over SSH; null for one that gave none yet (a
// Mac with Kuvo Code 0.8.0-beta.1, PROTOCOL.md "For released Macs").
export const macSign = (link: LinkState | null | undefined, mac: string): string | null => (typeof link?.macSigns?.[mac] === "string" && SIGN.test(link.macSigns[mac]) ? link.macSigns[mac] : null);

// COMPAT(beta.1 peers-list): link.json as the bridge beta.1 put on machines (0.9.23) left it. Goes on the user's word, once no Mac from before 1.0 connects.
// link.json as an earlier bridge left it: each Mac's list it had (`peersFrom`) kept as that Mac's
// word until it signs a document; what lists passed on and their dates needed, gone.
function migratedLink(link: Record<string, any>): Record<string, unknown> {
  // (Read as it was left, whatever its shape.)
  if (!(link.peersFrom || link.vouched || link.macAts)) return link;
  for (const [mac, given] of Object.entries(link.peersFrom ?? {})) {
    if (!macSign(link, mac) && isObject(given)) {
      link.legacy ??= {};
      link.legacy[mac] ??= { at: given.at ?? 0, peers: Array.isArray(given.peers) ? given.peers : [], moved: given.moved ?? [] };
    }
  }
  delete link.peersFrom;
  delete link.vouched;
  delete link.macAts;
  return link;
}

const linkFormat = objectFormat("Link state", 1, (fields, from) => from === 0 ? migratedLink(fields) : fields);

export function readLink(): LinkState | null {
  let link = null;
  try {
    link = linkFormat.read(JSON.parse(fs.readFileSync(LINK_FILE, "utf8"))) as LinkState;
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
  }
  // This Mac's: its keys from its app (kept in your Keychain, never in a file here), and its relay
  // and name as its app says, beside what the file keeps.
  if (IS_MAC && macLink.overlay) return { phones: [], ...(link ?? {}), ...macLink.overlay };
  return link;
}

export function writeLink(link: LinkState, durable = false) {
  // This Mac's keys and what its app says stay out of the file.
  const kept: Record<string, unknown> = IS_MAC
    ? Object.fromEntries(Object.entries(link).filter(([key]) => !MAC_OVERLAY.includes(key)))
    : { ...link };
  delete kept.sharedPending;
  delete kept.sharedInterrupted;
  delete kept.sharedReceipts;
  if (link.setup) {
    const setup: Record<string, unknown> = { ...link.setup };
    delete setup.sharedSaves;
    kept.setup = setup;
  }
  // (Its keys, on a machine.)
  readLink();
  const text = JSON.stringify(linkFormat.write(kept), null, 2);
  if (durable || link.shared || link.sharedOutcomes) {
    privateFolder(path.dirname(LINK_FILE));
    writeDurablePrivate(LINK_FILE, text);
  } else writeSecret(LINK_FILE, text);
  for (const then of written) then(link);
}

// One change to link.json at a time, here and from `setup` and `pair` (other processes): read,
// changed and written under a lock (run/.link.lock), so none writes over another's (a removal,
// say). `change` returns the link to write, or nothing to leave it. A busy lock fails the update.
const LOCK = path.join(RUN, ".link.lock");
export function updateLink(change: (link: LinkState | null) => LinkState | null | undefined | void, { durable = false } = {}): LinkState | null {
  let sharedSaved = false;
  const result = locked(LOCK, () => {
    const current = readLink();
    const managed = current?.setup?.managed;
    const shared = current?.shared;
    const before = JSON.stringify(current?.setup ?? null);
    const changed = change(current);
    if (changed) {
      const sharedChanged = shared !== changed.shared;
      writeLink(changed, durable || sharedChanged || (before !== JSON.stringify(changed.setup ?? null) && Boolean(changed.setup?.managed || managed)));
      sharedSaved = sharedChanged && changed.setup?.role === "primary";
    }
    return changed ?? null;
  }, { exclusive: true });
  if (sharedSaved) queueMicrotask(noteDocsChanged);
  return result;
}
