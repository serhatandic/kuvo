import { isObject } from "../../shared/json";
import { cleanPeer, fitsPeer, type Peer } from "../peers/known";
import type { LinkState } from "../state";
import { acceptedMembership } from "./membership";

export const REMOVAL_FEATURE = "computers.removal";
export { OPERATION_ID as REMOVAL_ID } from "./shared";
import { OPERATION_ID } from "./shared";

export interface MemberRemoval {
  peer: Peer;
  operation: string;
  wipe: boolean;
  delivered?: true;
}

export interface LocalWipe {
  setup: string;
  operation: string;
  primary?: Peer;
  prepared?: true;
}

export function removalRecords(value: unknown, left: Set<string>, current: Set<string>): Record<string, MemberRemoval> {
  if (!isObject(value)) return {};
  return Object.fromEntries(Object.entries(value).flatMap(([key, record]) => {
    if (!left.has(key) || current.has(key) || !isObject(record) || !fitsPeer(record.peer)
      || record.peer.key !== key || typeof record.operation !== "string" || !OPERATION_ID.test(record.operation)
      || typeof record.wipe !== "boolean") return [];
    const { id: _destination, ...peer } = cleanPeer(record.peer);
    return [[key, { peer, operation: record.operation, wipe: record.wipe, ...(record.delivered === true ? { delivered: true } : {}) }]];
  }));
}

export const removalsOf = (link: LinkState | null | undefined) => acceptedMembership(link)?.removals ?? {};

export function pendingWipes(link: LinkState | null | undefined) {
  return Object.values(removalsOf(link)).flatMap(({ peer, operation, wipe, delivered }) =>
    wipe && !delivered ? [{ key: peer.key, name: peer.name, operation }] : []);
}
