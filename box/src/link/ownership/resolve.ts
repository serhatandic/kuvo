import os from "node:os";
import { dataOwner, type DataKind, type DataOwner } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { ownKey, peersOf, movedOf, KEY } from "../peers/known";
import { readPeerAgents } from "../peers/directory";
import { acceptedMembership, acceptedName } from "../setup/membership";
import { followedSetup } from "../setup/record";
import { sharedValue } from "../setup/shared";
import { readLink, type LinkState } from "../state";
import { liveMoves } from "../moves/records";
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
  // #905 can publish scope + stable ID assignments here. A committed assignment also
  // fences an obsolete copy when its new owner has stopped answering or been removed.
  const assignments = sharedValue(link, "ownershipAssignments");
  const address = `${scope}:${id}`;
  const assignment = isObject(assignments) && Object.hasOwn(assignments, address) ? assignments[address] : undefined;
  if (isObject(assignment) && typeof assignment.computer === "string" && KEY.test(assignment.computer)) return named(replacementOwner(link, assignment.computer));

  const switched = liveMoves().find(move => move.role === "from" && (move.stage === "switching" || move.stage === "finishing")
    && (move.thing === id || move.sessions.includes(id)));
  const moved = switched?.peer ?? (scope === "session computer" ? movedOf(link).get(id) : undefined);
  if (moved) return named(replacementOwner(link, moved));
  if (localRecord) return here();
  const candidates = peers.filter(peer => scope === "group host" ? told[peer.key]?.groups?.includes(id) : (told[peer.key]?.list ?? []).some(agent =>
    scope === "session computer" ? agent.session === id : agent.kind === "bot" && (agent.id === id || agent.session === id)));
  if (candidates.length === 1) return named(candidates[0]!.key);
  if (candidates.length > 1) return null;
  return ref.computer ? named(ref.computer) : null;
}
