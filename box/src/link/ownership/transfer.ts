import { isObject } from "../../shared/json";
import { OWNER } from "../../shared/work-queue-owner";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { acceptedMembership } from "../setup/membership";
import { primaryFenced } from "../setup/lease";
import { followedSetup } from "../setup/record";
import { sharedValue, stageShared, waitShared, type Receipt } from "../setup/shared";
import { readLink, updateLink, type LinkState } from "../state";
import { forwardAct, type Act, type Acted } from "../work-queue/forward";
import { acceptedOwner, assignmentFor, authorityAddress, type DataReference } from "./resolve";
import { recordKey } from "./store";

type Transfer = { id: string; from: string; to: string } & (
  { kind: "chat" | "bot" | "group"; thing: string; sessions: readonly string[] } |
  { kind: "queue"; thing: string; queue: string }
);
const unavailable = { status: 409, value: { message: "The primary couldn’t confirm the transfer yet." } };

const session = /^ses_[A-Za-z0-9_-]{1,80}$/;
function transferOf(value: unknown): Transfer | null {
  if (!isObject(value) || typeof value.id !== "string" || typeof value.thing !== "string"
    || typeof value.from !== "string" || !KEY.test(value.from) || typeof value.to !== "string" || !KEY.test(value.to) || value.from === value.to) return null;
  const base = { id: value.id, from: value.from, to: value.to, thing: value.thing };
  const { kind, thing, sessions, queue, id } = value;
  if (kind === "queue") return /^wqh_[A-Za-z0-9_-]{8,40}$/.test(id) && /^wqi_[A-Za-z0-9_-]{20}$/.test(thing) &&
    typeof queue === "string" && OWNER.test(queue) ? { ...base, kind, queue } : null;
  if (!/^mov_[A-Za-z0-9]+$/.test(id) || !Array.isArray(sessions) || !sessions.every(id => typeof id === "string" && session.test(id))) return null;
  if (kind === "chat" && session.test(thing) && sessions.includes(thing) ||
      kind === "bot" && /^bot_[A-Za-z0-9]+$/.test(thing) && sessions.length > 0 ||
      kind === "group" && /^grp_[a-z0-9]+$/.test(thing) && sessions.length === 0) {
    return { ...base, kind, sessions };
  }
  return null;
}

export function moveReference(kind: "chat" | "bot" | "group", id: string): DataReference {
  return { kind: kind === "chat" ? "conversations" : kind === "bot" ? "botSettings" : "groupSettings", id };
}

function references(move: Transfer): DataReference[] {
  if (move.kind === "queue") return [{ kind: "queueItems", id: move.thing, queue: move.queue }];
  return [moveReference(move.kind, move.thing), ...move.sessions.map(id => ({ kind: "conversations" as const, id }))];
}

export function transferAccepted(link: LinkState | null, move: Transfer): boolean {
  return references(move).every(ref => {
    const assignment = assignmentFor(link, ref);
    return assignment?.move === move.id && assignment.computer === move.to;
  });
}

export async function transferForPeer(sender: MailSender, act: Pick<Act, "body">): Promise<Acted> {
  const move = transferOf(act.body?.transfer);
  // The frozen source proposes a move. A handoff is proposed by the recipient
  // that validated the item it received directly from the source.
  if (!move || sender.key !== (move.kind === "queue" ? move.to : move.from)) return unavailable;
  let save: Receipt | null = null;
  let accepted = false;
  updateLink(link => {
    const setup = followedSetup(link);
    const roster = acceptedMembership(link);
    if (!link || setup?.role !== "primary" || setup.id !== act.body?.setup || setup.conflict || primaryFenced(link) ||
      ![move.from, move.to].every(key => roster?.boxes.some(box => box.key === key))) return;
    accepted = transferAccepted(link, move);
    if (accepted) return;
    const refs = references(move);
    if (move.kind === "queue") {
      const owner = acceptedOwner(link, { kind: "queueItems", queue: move.queue });
      if (owner && owner !== move.to) return;
    }
    if (refs.some(ref => {
      const previous = move.kind === "queue" ? assignmentFor(link, ref) : undefined;
      const current = move.kind === "queue"
        ? previous && acceptedOwner(link, { ...ref, queue: typeof previous.queue === "string" ? previous.queue : undefined })
        : acceptedOwner(link, ref);
      return current && current !== move.from;
    })) return;
    const prior = sharedValue(link, "ownershipAssignments");
    const entries = Object.fromEntries(refs.map(ref => [
      authorityAddress(ref), { computer: move.to, move: move.id, ...(move.kind === "queue" ? { queue: move.queue } : {}) },
    ]));
    const operation = recordKey("transfer", move.id, move.from, move.to, String(setup.epoch));
    save = stageShared(link, "ownershipAssignments", { ...(isObject(prior) ? prior : {}), ...entries }, undefined, operation);
    return save ? link : undefined;
  }, { durable: true });
  if (!accepted && save) accepted = (await waitShared(save)).status === "committed";
  return accepted ? { status: 200, value: { committed: true } } : unavailable;
}

export async function commitTransfer(link: LinkState | null, move: Transfer): Promise<boolean> {
  const setup = followedSetup(link);
  if (!setup?.checkpointed) return true;
  if (transferAccepted(link, move)) return true;
  const { id, from, to, kind, thing } = move;
  const body = { setup: setup.id, transfer: { id, from, to, kind, thing,
    ...(move.kind === "queue" ? { queue: move.queue } : { sessions: move.sessions }),
  } };
  if (setup.role === "primary") {
    const key = link && ownKey(link);
    return Boolean(key && (await transferForPeer({ key, kind: "mac" }, { body })).status === 200);
  }
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  if (!primary) return false;
  const result = await forwardAct(primary, "Transfer ownership", { area: "ownership", method: "POST", item: "transfer-ownership", body },
    undefined, link, Date.now(), { backgroundRelay: true });
  return result.status === 200;
}
