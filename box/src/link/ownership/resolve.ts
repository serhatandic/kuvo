import os from "node:os";
import { dataOwner, type DataKind, type DataOwner } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { ownKey, peersOf, movedOf, KEY } from "../peers/known";
import { readPeerAgents } from "../peers/directory";
import { acceptedMembership, acceptedName } from "../setup/membership";
import { followedSetup } from "../setup/record";
import { sharedValue } from "../setup/shared";
import { readLink, type LinkState } from "../state";
import { committedDepartures, frozen, liveMoves } from "../moves/records";
import { peerIsAway } from "../peers/reach";

export interface DataReference {
  kind: DataKind;
  /** Stable ID of the authority-bearing container, such as a session, group or bot. */
  id?: string;
  /** Work Queue owner key, independent of the item’s ID. */
  queue?: string;
  /** Provenance of a particular computer-local list/record or a legacy addressed read. */
  computer?: string;
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
  const assignments = sharedValue(link, "ownershipAssignments");
  if (!roster || !isObject(assignments)) return key;
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
  const assignments = sharedValue(link, "ownershipAssignments");
  const known = new Set([...membership.left, ...membership.boxes.map(({ key }) => key)]);
  return membership.left.flatMap(key => {
    const match = isObject(assignments) ? assignments[`replacement:${key}`] : undefined;
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
  return `${dataOwner(ref.kind, ref.queue)}:${queueRecord ? ref.queue : ref.id}`;
}

export function assignmentFor(link: LinkState | null, ref: DataReference) {
  const assignments = sharedValue(link, "ownershipAssignments");
  const address = authorityAddress(ref);
  const value = isObject(assignments) && Object.hasOwn(assignments, address) ? assignments[address] : undefined;
  return isObject(value) && typeof value.computer === "string" && KEY.test(value.computer)
    ? { computer: value.computer, move: value.move, queue: value.queue } : undefined;
}

/** Accepted setup state is the ownership decision. Routing hints cannot replace it. */
export function acceptedOwner(link: LinkState | null, ref: DataReference): string | undefined {
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
  const accepted = assignment?.computer ?? (moved ? moved.key : undefined);
  return accepted ? replacementOwner(link, accepted) : undefined;
}

/** Aggregate stores such as the engine DB use the same decision for each affected ID. */
export function sessionsFencedHere(authority: { link: LinkState | null; origin?: string | null }): string[] {
  const { link } = authority;
  const assignments = sharedValue(link, "ownershipAssignments");
  const prefix = "session computer:";
  const sessions = new Set([...movedOf(link).keys(), ...frozen(), ...committedDepartures().map(each => each.session)].filter(id => id.startsWith("ses_")));
  if (isObject(assignments)) for (const address of Object.keys(assignments)) {
    if (address.startsWith(prefix)) sessions.add(address.slice(prefix.length));
  }
  return [...sessions].filter(id => !mayActivate({ kind: "conversations", id }, authority));
}

/** Decide whether a stored copy can run. Advertisements can locate a record, but cannot
 * authorize a copy. A replacement has the old computer's provenance, with a new key. */
export function mayActivate(ref: DataReference, { link = readLink(), origin, activating, moves }: {
  link?: LinkState | null; origin?: string | null; activating?: string; moves?: ReturnType<typeof liveMoves>;
} = {}): boolean {
  const scope = dataOwner(ref.kind, ref.queue);
  if (!scope || scope === "client") return false;
  const own = link && ownKey(link);
  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  if (id && (activating ? frozen((moves ?? liveMoves()).filter(move => move.id !== activating)) : frozen(moves)).has(id)) return false;
  if (dataOwner(ref.kind) === undefined && ref.id) {
    const queue = assignmentFor(link, ref)?.queue;
    if (typeof queue === "string" && queue !== ref.queue) return false;
  }
  const accepted = acceptedOwner(link, ref);
  if (activating) {
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
  if (accepted) return named(accepted);
  if (scope === "local computer") return ref.computer ? named(ref.computer) : here();
  if (!id || !DATA_ID.test(id)) return null;

  const switched = liveMoves().find(move => move.role === "from" && (move.stage === "switching" || move.stage === "finishing")
    && (move.thing === id || move.sessions.includes(id)));
  const moved = switched?.peer ?? (scope === "session computer" ? movedOf(link).get(id) : undefined)
    ?? committedDepartures().find(each => each.session === id)?.key;
  if (moved) return named(replacementOwner(link, moved));
  if (localRecord) return here();
  const candidates = peers.filter(peer => scope === "group host" ? told[peer.key]?.groups?.includes(id) : (told[peer.key]?.list ?? []).some(agent =>
    scope === "session computer" ? agent.session === id : agent.kind === "bot" && (agent.id === id || agent.session === id)));
  if (candidates.length === 1) return named(candidates[0]!.key);
  if (candidates.length > 1) return null;
  return ref.computer ? named(ref.computer) : null;
}

/** Decide what a replacement can install from the available item and ownership evidence. */
export function queueCopyDecision(ref: DataReference, authority: Parameters<typeof mayActivate>[1],
  evidence: { deleted: boolean; handing: boolean; departed: boolean; claimed: boolean }): "active" | "deleted" | "held" {
  if (evidence.deleted) return "deleted";
  if (evidence.handing || evidence.departed || evidence.claimed || !mayActivate(ref, authority)) return "held";
  return "active";
}
