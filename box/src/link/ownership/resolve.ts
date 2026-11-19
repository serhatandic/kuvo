import os from "node:os";
import { dataOwner, type DataKind, type DataOwner } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
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

/** Accepted setup state is the ownership decision. Routing hints cannot replace it. */
export function acceptedOwner(link: LinkState | null, ref: DataReference): string | undefined {
  const scope = dataOwner(ref.kind, ref.queue);
  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  if (!id || !DATA_ID.test(id)) return;
  const assignments = sharedValue(link, "ownershipAssignments");
  const address = `${scope}:${id}`;
  const assignment = isObject(assignments) && Object.hasOwn(assignments, address) ? assignments[address] : undefined;
  if (isObject(assignment) && typeof assignment.computer === "string" && KEY.test(assignment.computer))
    return replacementOwner(link, assignment.computer);
  if (scope === "session computer") {
    const moved = acceptedMembership(link)?.moved.find(each => each.session === id);
    if (moved) return replacementOwner(link, moved.key);
  }
  return undefined;
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
export function mayActivate(ref: DataReference, { link = readLink(), origin }: {
  link?: LinkState | null; origin?: string | null;
} = {}): boolean {
  const scope = dataOwner(ref.kind, ref.queue);
  if (!scope || scope === "client") return false;
  const own = link && ownKey(link);
  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  if (id && frozen().has(id)) return false;
  const accepted = acceptedOwner(link, ref);
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
  const hinted = origin && scope === "session computer" && id ? movedOf(link).get(id) : undefined;
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
  if (scope === "local computer") return ref.computer ? named(ref.computer) : here();

  const id = dataOwner(ref.kind) === undefined ? ref.queue : ref.id;
  if (!id || !DATA_ID.test(id)) return null;
  const accepted = acceptedOwner(link, ref);
  if (accepted) return named(accepted);

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
