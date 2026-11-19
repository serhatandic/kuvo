// A fork can discard settings. Access revocations from a verified primary only accumulate.
import type { Doc, SignedDoc } from "../../shared/documents";
import { isObject } from "../../shared/json";
import { held, ownSign, SIGN, signDoc } from "../documents/format";
import { KEY } from "../peers/known";
import type { LinkState } from "../state";
import { setupOf } from "./record";
import { leadershipDocuments, resolveCheckpoint } from "./checkpoint";
import type { SuccessorForks } from "./leadership";
import { acceptedMembership, membershipRevocations, mergeRevocations, projectMembership } from "./membership";

export function publishRevocations(link: LinkState): SignedDoc | null {
  const setup = link.setup;
  if (!setup?.successorChain || setup.role !== "primary" || !link.sign || ownSign(link) !== setup.sign) return null;
  const tip = held(link.leadership?.[setup.epoch]);
  const membership = acceptedMembership(link);
  if (!tip || !membership) return null;
  const revocations = membershipRevocations(setup.id, membership);
  if (!revocations.computers.length && !revocations.signs.length && !revocations.devices.length) return null;
  return signDoc(link.sign, "setup-shared", setup.epoch, { tip: tip.h, forkRevocations: revocations });
}

export function revocationDocuments(link: LinkState): SignedDoc[] {
  const current = publishRevocations(link);
  const kept = link.discardedRevocations;
  const previous = kept && kept.setup === link.setup?.id ? [...kept.certificates.flatMap(leadershipDocuments), kept.proof] : [];
  return current ? [...previous, current] : previous;
}

export function takeRevocations(link: LinkState, doc: Doc, forks: SuccessorForks) {
  const setup = link.setup;
  const { tip, forkRevocations: value } = doc.body;
  if (!setup?.successorChain || !isObject(value) || value.setup !== setup.id || typeof tip !== "string") return;
  const current = held(link.leadership?.[doc.n]);
  const branch = forks.get(tip)?.source.setup;
  const active = current?.h === tip && doc.n === setup.epoch && doc.origin === setup.sign;
  const historical = current?.h === tip && doc.n < setup.epoch &&
    resolveCheckpoint(current)?.membership.boxes.some(peer => peer.key === setupOf(current.body)?.primary && peer.sign === doc.origin);
  const competing = branch?.id === setup.id && branch.epoch === doc.n && branch.sign === doc.origin;
  if (!active && !historical && !competing) return;
  const keys = (given: unknown, rule: RegExp): given is string[] =>
    Array.isArray(given) && given.every(key => typeof key === "string" && rule.test(key));
  if (!keys(value.computers, KEY) || !keys(value.signs, SIGN) || !Array.isArray(value.devices)
    || !value.devices.every(device => isObject(device) && typeof device.key === "string" && KEY.test(device.key)
      && typeof device.pairing === "string" && device.pairing.length <= 128)) return;
  if (!mergeRevocations(link, { setup: setup.id, computers: value.computers, signs: value.signs, devices: value.devices })) return;
  const merged = acceptedMembership(link);
  if (merged) projectMembership(link, merged);
}
