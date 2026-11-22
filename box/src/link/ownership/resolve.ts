import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { dataOwner, type DataKind, type DataOwner } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import type { ToldAgents } from "../../shared/records";
import { botHere } from "../bots/here";
import { listOf } from "../files";
import { BOTS, GROUPS } from "../home";
import { keptList } from "../lists/sessions";
import { ownKey, peersOf, KEY } from "../peers/known";
import { MOST_OWNED, OWNED_DIGEST, readPeerAgents } from "../peers/directory";
import { acceptedMembership, acceptedName } from "../setup/membership";
import { followedSetup } from "../setup/record";
import { ownershipEntries } from "../setup/shared";
import { readLink, type LinkState } from "../state";
import { committedDepartures, frozen, liveMoves, localMoveOwners } from "../moves/records";
import { peerIsAway } from "../peers/reach";

export interface DataReference {
  kind: DataKind;
  /** Stable ID of the authority-bearing container, such as a session, group or bot. */
  id?: string;
  /** Work Queue owner key, independent of the item’s ID. */
  queue?: string;
  /** Provenance of a particular computer-local list/record or a legacy addressed read. */
  computer?: string;
  /** Setup of a submission issuer address. Computer names its original accepting Box. */
  setup?: string;
  client?: string;
}

export interface Owner {
  key: string | null;
  name: string;
  scope: DataOwner;
  local: boolean;
  reachable: boolean;
}

export const DATA_ID = /^[A-Za-z0-9_-]{1,100}$/;

/** Follow a primary-approved recovery assignment without reactivating the old membership. */
export function replacementOwner(link: LinkState | null, key: string): string {
  const roster = acceptedMembership(link);
  const assignments = ownershipEntries(link);
  if (!roster) return key;
  const members = new Set(roster.boxes.map(box => box.key));
  const seen = new Set<string>();
  let current = key;
  while (!members.has(current) && roster.left.includes(current) && !seen.has(current)) {
    seen.add(current);
    const replacement = assignments[`replacement:${current}`];
    if (!isObject(replacement) || typeof replacement.computer !== "string" || !KEY.test(replacement.computer)) break;
    current = replacement.computer;
  }
  return members.has(current) ? current : key;
}

/** Accepted historical usage provenance is separate from the current sender roster. */
export function historicalInsightOrigins(link: LinkState | null): { key: string; name: string }[] {
  const setup = followedSetup(link);
  const membership = acceptedMembership(link);
  if (!link || !setup || !membership) return [];
  const history = membership.computerHistory?.setup === setup.id ? membership.computerHistory : undefined;
  const recorded = new Set(history?.slots.flatMap(slot => [slot.key, ...slot.previous ?? []]));
  const assignments = ownershipEntries(link);
  const known = new Set([...membership.left, ...membership.boxes.map(({ key }) => key)]);
  return membership.left.flatMap(key => {
    const match = assignments[`replacement:${key}`];
    const matched = isObject(match) && typeof match.source === "string" && typeof match.computer === "string" &&
      match.computer !== key && known.has(match.computer);
    if (!recorded.has(key) && !matched) return [];
    const name = history?.retiredNames?.[key] ?? acceptedName(link, key, membership.removals?.[key]?.peer.name ?? "");
    return [{ key, name: plainName(name) || `Former computer (${key.slice(0, 8)})` }];
  });
}

export function authorityAddress(ref: DataReference): string {
  const queueRecord = dataOwner(ref.kind) === undefined;
  if (queueRecord && ref.id) return `queue item:${ref.id}`;
  if (ref.kind === "draftSubmissionEvidence") return `submission issuer:${ref.setup}:${ref.computer}`;
  return `${dataOwner(ref.kind, ref.queue)}:${queueRecord ? ref.queue : ref.id}`;
}

/** The queue a Work Queue item was handed to, as this computer's move journal has it. */
export function assignmentFor(_link: LinkState | null, ref: DataReference) {
  if (dataOwner(ref.kind) !== undefined || !ref.id) return undefined;
  const placement = localMoveOwners().find(entry => entry.session === ref.id && entry.queue);
  return placement ? { computer: placement.key, queue: placement.queue, move: undefined } : undefined;
}

export function moveReference(kind: "chat" | "bot" | "group", id: string): DataReference {
  return { kind: kind === "chat" ? "conversations" : kind === "bot" ? "botSettings" : "groupSettings", id };
}

const MOVABLE: readonly DataOwner[] = ["session computer", "group host", "bot home"];

/** Where this computer's own move journal says the record went (or that it arrived here). */
export function acceptedOwner(link: LinkState | null, ref: DataReference): string | undefined {
  if (ref.kind === "draftSubmissionReplayFences" && ref.setup !== followedSetup(link)?.id) return;
  if (dataOwner(ref.kind) === "submission issuer") {
    if (!ref.setup || followedSetup(link)?.id !== ref.setup || !ref.computer || !KEY.test(ref.computer)) return;
    return replacementOwner(link, ref.computer);
  }
  const queueRecord = dataOwner(ref.kind) === undefined;
  if (queueRecord && ref.id) {
    const item = assignmentFor(link, ref);
    const queue = item && typeof item.queue === "string" ? item.queue : ref.queue;
    const owner = acceptedOwner(link, { kind: ref.kind, queue });
    return owner ?? (item && replacementOwner(link, item.computer));
  }
  const id = queueRecord ? ref.queue : ref.id;
  const scope = dataOwner(ref.kind, ref.queue);
  const handover = id && scope && MOVABLE.includes(scope) && localMoveOwners().find(entry => entry.session === id && !entry.queue);
  return handover ? replacementOwner(link, handover.key) : undefined;
}

/** Aggregate stores such as the engine DB use the same decision for each affected ID. */
export function sessionsFencedHere(authority: { link: LinkState | null; origin?: string | null }, recovering = false): string[] {
  const { link } = authority;
  const sessions = new Set([...frozen(), ...committedDepartures().map(each => each.session)].filter(id => id.startsWith("ses_")));
  if (recovering) {
    const told = readPeerAgents();
    for (const peer of peersOf(link)) {
      for (const record of told[peer.key]?.list ?? []) if (record.session?.startsWith("ses_")) sessions.add(record.session);
    }
  }
  const canUse = recovering ? mayRecover : mayActivate;
  return [...sessions].filter(id => !canUse({ kind: "conversations", id }, authority));
}

/** Decide whether a stored copy can run. Advertisements can locate a record, but cannot
 * authorize a copy. A replacement has the old computer's provenance, with a new key. */
export function mayActivate(ref: DataReference, { link = readLink(), origin, activating, moves }: {
  link?: LinkState | null; origin?: string | null; activating?: string; moves?: ReturnType<typeof liveMoves>;
} = {}): boolean {
  const scope = dataOwner(ref.kind, ref.queue);
  if (!scope || scope === "client") return false;
  const own = link && ownKey(link);
  if (ref.kind === "draftSubmissionReplayFences" && ref.setup !== followedSetup(link)?.id) return false;
  // Receipt placement is read-only history, never authority for a move or effect.
  if (scope === "submission issuer") return !activating && Boolean(own && acceptedOwner(link, ref) === own);
  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  if (id && (activating ? frozen((moves ?? liveMoves()).filter(move => move.id !== activating)) : frozen(moves)).has(id)) return false;
  if (dataOwner(ref.kind) === undefined && ref.id) {
    const queue = assignmentFor(link, ref)?.queue;
    if (typeof queue === "string" && queue !== ref.queue) return false;
  }
  const accepted = acceptedOwner(link, ref);
  if (activating) {
    const move = (moves ?? liveMoves()).find(move => move.id === activating);
    if (move?.role === "to" && move.activation && move.to === own && accepted === own &&
      id && (move.thing === id || move.sessions.includes(id))) return true;
    if (!accepted) return true; // The switch was authenticated and durably recorded by the move protocol.
  }
  if (accepted) return accepted === own;
  if (scope === "primary") {
    const primary = followedSetup(link)?.primary ?? link?.setup?.primary;
    return !primary || primary === own;
  }
  if (scope === "local computer") return !ref.computer || replacementOwner(link, ref.computer) === own;
  const departure = id && committedDepartures().find(each => each.session === id);
  if (departure && replacementOwner(link, departure.key) !== own) return false;
  return !origin || replacementOwner(link, origin) === own;
}

/** Restoration can use verified archive provenance. Another computer announcing the record holds
 * the copy, unless this computer's own move journal places it here. */
export function mayRecover(ref: DataReference, { link = readLink(), origin }: { link?: LinkState | null; origin?: string | null } = {}): boolean {
  if (!mayActivate(ref, { link, origin })) return false;
  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  const scope = dataOwner(ref.kind, ref.queue);
  if (!id || !scope || acceptedOwner(link, ref)) return true;
  if (scope !== "session computer" && scope !== "group host" && scope !== "bot home") return true;
  const own = link && ownKey(link);
  const disagrees = (key: string) => replacementOwner(link, key) !== own;
  if (committedDepartures().some(each => each.session === id && disagrees(each.key))) return false;
  return !announcedBy(link, scope, id).some(peer => disagrees(peer.key));
}

// MARK: What each computer has

/** How an ID is announced: a short digest, so every record a computer has fits in one mail. */
export const ownedDigest = (id: string): string => createHash("sha256").update(id).digest("base64url").slice(0, OWNED_DIGEST);

/** Whether this computer has the record: its sessions (the engine's and the coding agents', as the
 * box's session list has them), its bots or its group chats. Not a copy a move is bringing or taking. */
function holdsHere(scope: DataOwner, id: string, moves = liveMoves()): boolean {
  if (frozen(moves).has(id)) return false;
  if (scope === "session computer") {
    const listed = keptList()?.sessions[id];
    return Boolean(listed && !listed.facts.away);
  }
  if (scope === "bot home") return Boolean(botHere(id)) || listOf(BOTS).some(bot => botHere(bot)?.session === id);
  return fs.existsSync(path.join(GROUPS, id, "group.json"));
}

/** Every record this computer has, by the same rule, as it announces them to its peers (`owns`):
 * its bots, then its sessions, newest first, then its group chats. */
export function ownedDigests(): string {
  const held = frozen();
  for (const entry of committedDepartures()) held.add(entry.session);
  const bots = listOf(BOTS).flatMap(id => botHere(id) ? [id] : []);
  const sessions = Object.entries(keptList()?.sessions ?? {}).filter(([id, { facts }]) => !facts.away && !facts.parent && !held.has(id))
    .sort(([, a], [, b]) => b.facts.updated - a.facts.updated).map(([id]) => id);
  const groups = listOf(GROUPS).filter(id => fs.existsSync(path.join(GROUPS, id, "group.json")));
  return [...bots, ...sessions, ...groups].filter(id => !held.has(id)).slice(0, MOST_OWNED).map(ownedDigest).join("");
}

/** Whether a peer's announcement lists the record. (A computer from before `owns` lists only its
 * bots, its 40 newest sessions and its group chats.) */
function announces(told: ToldAgents | undefined, scope: DataOwner, id: string): boolean {
  if (!told) return false;
  if (told.owns !== undefined) {
    const digest = ownedDigest(id);
    for (let at = 0; at < told.owns.length; at += OWNED_DIGEST) if (told.owns.startsWith(digest, at)) return true;
    return false;
  }
  return scope === "group host" ? Boolean(told.groups?.includes(id)) : told.list.some(agent =>
    scope === "session computer" ? agent.session === id : agent.kind === "bot" && (agent.id === id || agent.session === id));
}

function announcedBy(link: LinkState | null, scope: DataOwner, id: string) {
  const told = readPeerAgents();
  return peersOf(link).filter(peer => announces(told[peer.key], scope, id));
}

/** Where a record lives, by one rule (`docs/architecture/how-kuvo-works.md`): here, if this computer
 * has it; else where this computer's own move journal sent it; else the one computer that announces
 * it. Null when none does. Setup-wide data is the main computer's, a client's its own. */
export function ownerOf(ref: DataReference, {
  link = readLink(), name = os.hostname(),
}: { link?: LinkState | null; name?: string } = {}): Owner | null {
  const scope = dataOwner(ref.kind, ref.queue);
  if (!scope) return null;
  if (scope === "client") return ref.client ? { key: ref.client, name: "This client", scope, local: false, reachable: true } : null;
  const own = link && ownKey(link);
  const setup = followedSetup(link);
  const peers = peersOf(link);
  const told = readPeerAgents();
  const here = (): Owner => ({ key: own, name: (link && acceptedName(link, own, name)) || name, scope, local: true, reachable: true });
  const named = (key: string): Owner => {
    if (key === own) return here();
    const peer = peers.find(each => each.key === key);
    return { key, scope, name: peer?.name || key, local: false, reachable: Boolean(peer && !peerIsAway(key, told[key]?.heard ?? told[key]?.at)) };
  };
  if (scope === "primary") {
    const primary = setup?.primary ?? link?.setup?.primary;
    return primary ? named(primary) : here();
  }
  // A Work Queue item is its queue's. One handed on from here, or to here, is in the queue (on the
  // computer) this computer's move journal says.
  const handed = assignmentFor(link, ref);
  if (handed && typeof handed.queue === "string") {
    const queue = { kind: ref.kind, queue: handed.queue };
    return dataOwner(ref.kind, handed.queue) === "local computer" ? named(handed.computer)
      : ownerOf(queue, { link, name }) ?? named(handed.computer);
  }
  if (scope === "submission issuer") {
    const issuer = acceptedOwner(link, ref);
    return issuer ? named(issuer) : null;
  }
  if (ref.kind === "draftSubmissionReplayFences" && ref.setup !== setup?.id) return null;
  if (scope === "local computer") return ref.computer ? named(ref.computer) : here();
  if (!MOVABLE.includes(scope)) return ref.computer ? named(ref.computer) : null;
  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  if (!id || !DATA_ID.test(id)) return null;

  // 1. Here. A copy this computer is handing on, or has handed on, isn't here any more.
  const moves = liveMoves();
  const switching = moves.find(move => move.role === "from" && (move.stage === "switching" || move.stage === "finishing")
    && (move.thing === id || move.sessions.includes(id)));
  const journal = switching?.peer ?? acceptedOwner(link, ref);
  if (journal === undefined && holdsHere(scope, id, moves)) return here();
  // 2. Where this computer's move journal sent it. That computer's journal covers a later hop.
  if (journal !== undefined) return named(replacementOwner(link, journal));
  // 3. The one computer that says it has it.
  const announced = announcedBy(link, scope, id);
  return announced.length === 1 ? named(announced[0]!.key) : null;
}

/** Decide what a replacement can install from the available item and ownership evidence. */
export function queueCopyDecision(ref: DataReference, authority: Parameters<typeof mayActivate>[1],
  evidence: { deleted: boolean; handing: boolean; departed: boolean; claimed: boolean }): "active" | "deleted" | "held" {
  if (evidence.deleted) return "deleted";
  if (evidence.handing || evidence.departed || evidence.claimed || !mayRecover(ref, authority)) return "held";
  return "active";
}
