// Ranked leadership can fork. Number votes are local durable state, outside the
// checkpoint that can be replaced when two histories meet.
import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import type { LinkState } from "../state";
import { ownKey } from "../peers/known";
import { leadershipOf } from "../setup/leadership";
import { acceptedMembership } from "../setup/membership";
import { numberVoters } from "../setup/checkpoint";
import { followedSetup, SETUP_ID } from "../setup/record";
import { sharedValue } from "../setup/shared";

export interface NumberProposal {
  setup: string;
  epoch: number;
  authority: string;
  nonce: string;
  from: number;
  to: number;
}

export interface NumberVote {
  proposal: NumberProposal;
  floor: number;
  accepted: boolean;
}

interface NumberGrant {
  id: string;
  by: string;
  from: number;
  to: number;
  granted: [number, number][];
}

export interface NumberQuorum {
  votes?: Record<string, NumberProposal>;
  observed?: Record<string, number>;
  pending?: {
    proposal: NumberProposal;
    grant: NumberGrant;
    replies: Record<string, NumberVote>;
    claims: [number, number][];
    count: number;
  };
  grants?: Record<string, NumberGrant & { proposal: NumberProposal; voters: string[] }>;
}

export function numberPosition(link: LinkState | null | undefined) {
  const setup = followedSetup(link);
  const tip = leadershipOf(link);
  const voters = link && numberVoters(link);
  const members = acceptedMembership(link)?.boxes;
  const own = link && ownKey(link);
  if (!setup?.successorChain || !tip || !voters || !members || !own || link?.setupMerge
    || link?.localWipe || link?.localUnlink || sharedValue(link, "setupDeparture") || !members.some(peer => peer.key === own)) return null;
  return { setup: setup.id, epoch: setup.epoch, authority: tip.h, primary: setup.primary, own, voters, members };
}

type Position = NonNullable<ReturnType<typeof numberPosition>>;
type Scope = Pick<NumberProposal, "setup" | "epoch" | "authority">;
export const sameNumberPosition = (a: Scope, b: Scope) =>
  a.setup === b.setup && a.epoch === b.epoch && a.authority === b.authority;

export function newNumberProposal(position: Position, from: number, to: number): NumberProposal {
  return { setup: position.setup, epoch: position.epoch, authority: position.authority,
    nonce: crypto.randomBytes(16).toString("base64url"), from, to };
}

export function numberProposal(value: unknown): NumberProposal | null {
  if (!isObject(value) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup)
    || !Number.isSafeInteger(value.epoch) || Number(value.epoch) < 1
    || typeof value.authority !== "string" || !/^[\w-]{11}$/.test(value.authority)
    || typeof value.nonce !== "string" || !/^[\w-]{22}$/.test(value.nonce)
    || !Number.isSafeInteger(value.from) || !Number.isSafeInteger(value.to)
    || Number(value.from) < 1 || Number(value.to) < Number(value.from) || Number(value.to) >= 1_000_000) return null;
  return { setup: value.setup, epoch: Number(value.epoch), authority: value.authority, nonce: value.nonce,
    from: Number(value.from), to: Number(value.to) };
}

const sameProposal = (a: NumberProposal, b: NumberProposal) =>
  sameNumberPosition(a, b) && a.nonce === b.nonce && a.from === b.from && a.to === b.to;

/** The caller holds the number and link locks and persists the vote before replying. */
export function voteForNumbers(quorum: NumberQuorum, position: Position, by: string, proposal: NumberProposal, localHigh: number): NumberVote | null {
  if (by !== position.primary || !sameNumberPosition(position, proposal) || !position.voters.includes(position.own)) return null;
  const before = quorum.votes?.[position.setup];
  const floor = Math.max(localHigh, before?.to ?? 0, quorum.observed?.[position.setup] ?? 0);
  if (before && sameProposal(before, proposal)) return { proposal, floor, accepted: true };
  if (proposal.epoch < (before?.epoch ?? 0)) return null;
  if (proposal.from <= floor) return { proposal, floor, accepted: false };
  quorum.votes = { ...quorum.votes, [position.setup]: proposal };
  return { proposal, floor: proposal.to, accepted: true };
}

export function recordNumberVote(quorum: NumberQuorum, position: Position, by: string, value: unknown): void {
  const pending = quorum.pending;
  if (!pending || position.primary !== position.own || !position.voters.includes(by)
    || !position.members.some(peer => peer.key === by) || !sameNumberPosition(position, pending.proposal)
    || !isObject(value) || typeof value.accepted !== "boolean" || !Number.isSafeInteger(value.floor)
    || Number(value.floor) < 0 || Number(value.floor) >= 1_000_000) return;
  const proposal = numberProposal(value.proposal);
  if (!proposal || !sameProposal(proposal, pending.proposal)
    || Number(value.floor) < (value.accepted ? proposal.to : proposal.from)) return;
  if (pending.replies[by]?.accepted) return;
  const floor = Number(value.floor);
  pending.replies[by] = { proposal, floor, accepted: value.accepted };
  quorum.observed = { ...quorum.observed, [position.setup]: Math.max(floor, quorum.observed?.[position.setup] ?? 0) };
}

export function numberVotes(quorum: NumberQuorum, position: Position) {
  const replies = position.voters.flatMap(by => {
    const reply = quorum.pending?.replies[by];
    return reply && position.members.some(peer => peer.key === by) ? [{ ...reply, by }] : [];
  });
  return { majority: Math.floor(position.voters.length / 2) + 1, replies };
}
