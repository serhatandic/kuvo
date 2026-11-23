import { readPeerAgents } from "./directory";
import { peersOf } from "./known";
import { readReach } from "./reach";
import { readLink } from "../state";

/** The peer round requests these streams from the reachable setup members. A complete empty copy
 * also counts as kept. An explicitly unreachable or incompatible member isn't pending. */
export function pendingComputers(feature: string, kept: (key: string) => boolean, link = readLink()) {
  const agents = readPeerAgents();
  const reach = readReach();
  return peersOf(link).filter(peer => {
    const told = agents[peer.key];
    return peer.address && reach[peer.key] === undefined && !kept(peer.key) &&
      (!told?.features || told.featuresCut || told.features.includes(feature));
  }).map(peer => ({ id: peer.key, name: peer.name })).sort((a, b) =>
    a.name.localeCompare(b.name) || a.id.localeCompare(b.id));
}
