import os from "node:os";
import { dataOwner, type DataKind, type DataOwner } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { ownKey, peersOf, movedOf, KEY } from "../peers/known";
import { readPeerAgents } from "../peers/directory";
import { acceptedMembership, acceptedName } from "../setup/membership";
import { followedSetup } from "../setup/record";
import { ownershipEntries } from "../setup/shared";
import { readLink, type LinkState } from "../state";
import { committedDepartures, frozen, liveMoves, localMoveOwners, type MoveRecord } from "../moves/records";
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

export function assignmentFor(link: LinkState | null, ref: DataReference) {
  if (dataOwner(ref.kind) === undefined && ref.id) {
    const placement = localMoveOwners().find(entry => entry.session === ref.id && entry.queue);
    if (placement) return { computer: placement.key, queue: placement.queue, move: undefined };
  }
  const assignments = ownershipEntries(link);
  const address = authorityAddress(ref);
  const value = Object.hasOwn(assignments, address) ? assignments[address] : undefined;
  return isObject(value) && typeof value.computer === "string" && KEY.test(value.computer)
    ? { computer: value.computer, move: value.move, queue: value.queue } : undefined;
}

export function moveReference(kind: "chat" | "bot" | "group", id: string): DataReference {
  return { kind: kind === "chat" ? "conversations" : kind === "bot" ? "botSettings" : "groupSettings", id };
}

/** One release of read compatibility for journals staged before peer handover. */
export function legacyMoveAccepted(link: LinkState | null, move: MoveRecord): boolean {
  return [moveReference(move.kind, move.thing), ...move.sessions.map(id => moveReference("chat", id))].every(ref => {
    const assignment = assignmentFor(link, ref);
    return assignment?.computer === move.to && assignment.move === move.id;
  });
}

/** Resolve durable handovers and legacy setup ownership. */
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
  const assignment = id && DATA_ID.test(id) ? assignmentFor(link, ref) : undefined;
  const moved = scope === "session computer" && acceptedMembership(link)?.moved.find(each => each.session === id);
  const handover = id && scope && ["session computer", "group host", "bot home"].includes(scope)
    && localMoveOwners().find(entry => entry.session === id);
  const accepted = (handover ? handover.key : undefined) ?? assignment?.computer ?? (moved ? moved.key : undefined);
  return accepted ? replacementOwner(link, accepted) : undefined;
}

/** Aggregate stores such as the engine DB use the same decision for each affected ID. */
export function sessionsFencedHere(authority: { link: LinkState | null; origin?: string | null }, recovering = false): string[] {
  const { link } = authority;
  const assignments = ownershipEntries(link);
  const prefix = "session computer:";
  const sessions = new Set([...movedOf(link).keys(), ...frozen(), ...committedDepartures().map(each => each.session)].filter(id => id.startsWith("ses_")));
  for (const address of Object.keys(assignments)) {
    if (address.startsWith(prefix)) sessions.add(address.slice(prefix.length));
  }
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
    if (followedSetup(link)?.checkpointed) return accepted === own && assignmentFor(link, ref)?.move === activating;
    if (!accepted) return true; // The legacy switch was authenticated and durably recorded by the move protocol.
  }
  if (accepted) return accepted === own;
  if (scope === "primary") {
    const primary = followedSetup(link)?.primary ?? link?.setup?.primary;
    return !primary || primary === own;
  }
  if (scope === "local computer") return !ref.computer || replacementOwner(link, ref.computer) === own;
  const departure = id && committedDepartures().find(each => each.session === id);
  if (departure && replacementOwner(link, departure.key) !== own) return false;
  // Legacy move notices lack a setup-level decision. Treat a remote notice as
  // a reason to hold a recovered copy, never as permission to activate one.
  const hinted = origin && id ? movedOf(link).get(id) : undefined;
  if (hinted && replacementOwner(link, hinted) !== own) return false;
  return !origin || replacementOwner(link, origin) === own;
}

/** Restoration can use verified archive provenance. A legacy advert or move notice can
 * hold a copy, but only an accepted assignment overrides conflicting recovery evidence. */
export function mayRecover(ref: DataReference, { link = readLink(), origin }: { link?: LinkState | null; origin?: string | null } = {}): boolean {
  if (!mayActivate(ref, { link, origin })) return false;
  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  const scope = dataOwner(ref.kind, ref.queue);
  if (!id || !scope || acceptedOwner(link, ref)) return true;
  if (scope !== "session computer" && scope !== "group host" && scope !== "bot home") return true;
  const own = link && ownKey(link);
  const disagrees = (key: string) => replacementOwner(link, key) !== own;
  if (committedDepartures().some(each => each.session === id && disagrees(each.key))) return false;
  const moved = movedOf(link).get(id);
  if (moved && disagrees(moved)) return false;
  return !advertisedOwners(link, scope, id).some(peer => disagrees(peer.key));
}

function advertisedOwners(link: LinkState | null, scope: DataOwner, id: string) {
  const told = readPeerAgents();
  return peersOf(link).filter(peer => scope === "group host" ? told[peer.key]?.groups?.includes(id) : (told[peer.key]?.list ?? []).some(agent =>
    scope === "session computer" ? agent.session === id : agent.kind === "bot" && (agent.id === id || agent.session === id)));
}

/** Resolve the declaration against accepted state first, then the evidence used by
 * existing boxes. An address can locate an unlisted record, but cannot override its owner. */
export function ownerOf(ref: DataReference, {
  link = readLink(), name = os.hostname(), localRecord = false,
}: { link?: LinkState | null; name?: string; localRecord?: boolean } = {}): Owner | null {
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


  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  const accepted = acceptedOwner(link, ref);
  if (id && !localRecord && (scope === "session computer" || scope === "bot home" || scope === "group host")) {
    const announced = advertisedOwners(link, scope, id).filter(peer => told[peer.key]?.features?.includes("moves.two-computers"));
    if (announced.length === 1) return named(announced[0]!.key);
  }
  if (accepted) return named(accepted);
  if (scope === "submission issuer" || (ref.kind === "draftSubmissionReplayFences" && ref.setup !== setup?.id)) return null;
  if (scope === "local computer") return ref.computer ? named(ref.computer) : here();
  if (!id || !DATA_ID.test(id)) return null;

  const switched = liveMoves().find(move => move.role === "from" && (move.stage === "switching" || move.stage === "finishing")
    && (move.thing === id || move.sessions.includes(id)));
  const moved = switched?.peer ?? (scope === "session computer" ? movedOf(link).get(id) : undefined)
    ?? committedDepartures().find(each => each.session === id)?.key;
  if (moved) return named(replacementOwner(link, moved));
  if (localRecord) return here();
  const candidates = advertisedOwners(link, scope, id);
  if (candidates.length === 1) return named(candidates[0]!.key);
  if (candidates.length > 1) return null;
  return ref.computer ? named(ref.computer) : null;
}

/** Decide what a replacement can install from the available item and ownership evidence. */
export function queueCopyDecision(ref: DataReference, authority: Parameters<typeof mayActivate>[1],
  evidence: { deleted: boolean; handing: boolean; departed: boolean; claimed: boolean }): "active" | "deleted" | "held" {
  if (evidence.deleted) return "deleted";
  if (evidence.handing || evidence.departed || evidence.claimed || !mayRecover(ref, authority)) return "held";
  return "active";
}
