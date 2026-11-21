import { isObject } from "../../shared/json";
import { OWNER } from "../../shared/work-queue-owner";
import { noteDocsChanged } from "../documents/changed";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { acceptedMembership } from "../setup/membership";
import { primaryFenced } from "../setup/lease";
import { followedSetup } from "../setup/record";
import { removalMoveID, removalsOf } from "../setup/removal-record";
import { SharedCapacity, sharedValue, stageShared, type Receipt } from "../setup/shared";
import { readLink, updateLink, type LinkState } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { acceptedOwner, assignmentFor, authorityAddress, type DataReference } from "./resolve";
import { keepRecord, ownershipReady, readRecord, recordKey, records, removeRecord } from "./store";

export type Transfer = { id: string; from: string; to: string } & (
  { kind: "chat" | "bot" | "group"; thing: string; sessions: readonly string[] } |
  { kind: "queue"; thing: string; queue: string; fromQueue?: string }
);
const unavailable = { status: 409, value: { message: "The main computer couldn’t confirm the transfer yet." } };

const session = /^ses_[A-Za-z0-9_-]{1,80}$/;
function transferOf(value: unknown): Transfer | null {
  if (!isObject(value) || typeof value.id !== "string" || typeof value.thing !== "string"
    || typeof value.from !== "string" || !KEY.test(value.from) || typeof value.to !== "string" || !KEY.test(value.to)) return null;
  const base = { id: value.id, from: value.from, to: value.to, thing: value.thing };
  const { kind, thing, sessions, queue, id } = value;
  if (kind === "queue") return /^wqh_[A-Za-z0-9_-]{8,40}$/.test(id) && /^wqi_[A-Za-z0-9_-]{20}$/.test(thing) &&
    typeof queue === "string" && OWNER.test(queue) &&
    (value.from !== value.to || typeof value.fromQueue === "string" && OWNER.test(value.fromQueue) && value.fromQueue !== queue)
    ? { ...base, kind, queue, ...(typeof value.fromQueue === "string" ? { fromQueue: value.fromQueue } : {}) } : null;
  if (value.from === value.to) return null;
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

/** The source's completed move receipt can also be checked against a compacted assignment. */
export function transferAccepted(link: LinkState | null, move: Transfer, { allowSettled = false } = {}): boolean {
  return references(move).every(ref => {
    const assignment = assignmentFor(link, ref);
    return assignment?.computer === move.to && (assignment.move === move.id || (allowSettled && assignment.move === undefined)) &&
      (move.kind !== "queue" || assignment.queue === move.queue);
  });
}

const committed = { status: 200, value: { committed: true } };
const refused = (message: string): Acted => ({ status: 422, value: { message, transferRefused: true } });

export async function transferForPeer(sender: MailSender, act: Pick<Act, "body">, removalOperation?: string): Promise<Acted> {
  const move = transferOf(act.body?.transfer);
  const settled = act.body?.settled === true;
  // A move is proposed by its source, a remote handoff by the recipient holding the item.
  // Only the source that finished cleaning up can settle a transfer.
  if (!move || sender.key !== (settled || move.kind !== "queue" ? move.from : move.to)) return unavailable;
  let save: Receipt | null = null;
  let result = unavailable as Acted;
  updateLink(link => {
    const setup = followedSetup(link);
    const roster = acceptedMembership(link);
    if (!link || !roster || setup?.role !== "primary" || setup.id !== act.body?.setup || setup.conflict || primaryFenced(link) ||
      (act.body?.epoch !== undefined && setup.epoch !== act.body.epoch)) return;
    const removal = removalsOf(link)[move.from];
    const plan = removal?.operation === removalOperation ? removal?.plan : removalOperation ? removal?.superseded?.[removalOperation] : undefined;
    const scoped = removalOperation !== undefined && move.kind !== "queue" && plan?.moveTo === move.to
      && move.id === removalMoveID(removalOperation, move.kind, move.thing);
    if (removalOperation && !scoped) { result = refused("This move is outside the accepted removal."); return; }
    const refs = references(move);
    const prior = sharedValue(link, "ownershipAssignments");
    const assignments = isObject(prior) ? prior : {};
    const operation = recordKey(settled ? "settle transfer" : "transfer", move.id, move.from, move.to, String(setup.epoch));
    const outcome = link.sharedOutcomes?.[operation];
    if (settled) {
      if (scoped && removal?.operation === removalOperation && !removal?.resolution && !removal?.generation) return;
      // A newer transfer may already have replaced some or all of these entries.
      const entries = refs.flatMap(ref => {
        const address = authorityAddress(ref);
        const entry = assignments[address];
        return isObject(entry) && entry.move === move.id && entry.computer === move.to && entry.from === move.from
          ? [[address, { computer: entry.computer, ...(typeof entry.queue === "string" ? { queue: entry.queue } : {}) }]]
          : [];
      });
      if (!entries.length) { result = committed; return; }
      save = stageShared(link, "ownershipAssignments", { ...assignments, ...Object.fromEntries(entries) });
      return save ? link : undefined;
    }

    if (transferAccepted(link, move)) { result = committed; return link; }
    if (outcome?.transferRefusal) { result = refused(outcome.transferRefusal); return link; }
    const reject = (message: string) => {
      result = refused(message);
      const sequence = (link.shared?.ownershipAssignments?.sequence ?? 0) + 1;
      // Persist the refusal under the same operation ID. Delayed retries cannot
      // accept it if capacity or membership changes after the source lets go.
      link.sharedOutcomes = { ...link.sharedOutcomes, [operation]: {
        kind: "ownershipAssignments", status: "interrupted", epoch: setup.epoch,
        sequence, baseRevision: sequence - 1, proposal: "", transferRefusal: message,
      } };
      return link;
    };
    if (outcome?.status === "committed") {
      result = refused("This transfer already finished.");
      return;
    }
    if (scoped && (removal?.operation !== removalOperation || removal?.resolution || removal?.delivered || removal?.generation ||
      !removal?.selection?.some(item => item.id === move.thing && item.kind === move.kind)))
      return reject("The removal plan changed before ownership was transferred.");
    if (!(scoped ? [move.to] : [move.from, move.to]).every(key => roster.boxes.some(box => box.key === key)))
      return reject("Both computers must still belong to this setup.");
    if (move.kind === "queue") {
      const owner = acceptedOwner(link, { kind: "queueItems", queue: move.queue });
      if (owner && owner !== move.to) return reject("The destination queue moved to another computer.");
    }
    if (refs.some(ref => {
      const previous = move.kind === "queue" ? assignmentFor(link, ref) : undefined;
      if (move.kind === "queue" && move.fromQueue && previous?.queue !== undefined && previous.queue !== move.fromQueue) return true;
      const current = move.kind === "queue" && !previous
        ? acceptedOwner(link, { ...ref, id: undefined, queue: move.fromQueue })
        : acceptedOwner(link, ref);
      return current && current !== move.from;
    })) return reject("The record already belongs to another owner.");
    const entries = Object.fromEntries(refs.map(ref => [
      authorityAddress(ref), { computer: move.to, move: move.id, from: move.from, ...(move.kind === "queue" ? { queue: move.queue } : {}) },
    ]));
    try {
      save = stageShared(link, "ownershipAssignments", { ...assignments, ...entries }, undefined, operation, undefined, { reportCapacity: true });
    } catch (error) {
      if (!(error instanceof SharedCapacity)) throw error;
      return reject("The main computer has reached its saved ownership capacity.");
    }
    return save ? link : undefined;
  }, { durable: true });
  if (!save) return result;
  noteDocsChanged();
  return committed;
}

export const TRANSFER_FEATURE = "ownership.transfer-decisions";
type TransferDecision = { status: "committed" | "pending" } | { status: "refused"; message: string };

export function transferSupport(link: LinkState | null): string | undefined {
  const setup = followedSetup(link);
  if (!setup?.checkpointed || setup.role === "primary") return;
  if (takesForwarded(setup.primary, TRANSFER_FEATURE)) return;
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  return primary ? `Update ${primary.name}, the main computer, before transferring ownership.` : undefined;
}

async function requestTransfer(link: LinkState | null, move: Transfer & { removalOperation?: string }, settled = false) {
  const setup = followedSetup(link);
  if (!setup?.checkpointed) return;
  const { id, from, to, kind, thing } = move;
  const transfer = { id, from, to, kind, thing,
    ...(move.kind === "queue" ? { queue: move.queue, fromQueue: move.fromQueue } : { sessions: move.sessions }),
  };
  if (move.removalOperation) {
    const { removalTransfer } = await import("../setup/removal-moves");
    const scoped = transferOf(transfer);
    if (scoped) return removalTransfer(link, scoped, move.removalOperation, settled);
    return;
  }
  const body = { setup: setup.id, epoch: setup.epoch, transfer, ...(settled ? { settled } : {}) };
  if (setup.role === "primary") {
    const key = link && ownKey(link);
    return key ? transferForPeer({ key, kind: "mac" }, { body }) : undefined;
  }
  const primary = peersOf(link).find(peer => peer.key === setup.primary);
  return primary ? forwardAct(primary, "Transfer ownership", { area: "ownership", method: "POST", item: "transfer-ownership", body },
    undefined, link, Date.now(), { backgroundRelay: true }) : undefined;
}

export async function decideTransfer(link: LinkState | null, move: Transfer & { removalOperation?: string }): Promise<TransferDecision> {
  const setup = followedSetup(link);
  if (!setup?.checkpointed || (setup.role !== "primary" && transferAccepted(link, move))) return { status: "committed" };
  const unsupported = move.removalOperation ? undefined : transferSupport(link);
  if (unsupported) return { status: "refused", message: unsupported };
  const result = await requestTransfer(link, move);
  if (result?.status === 200) return { status: "committed" };
  if (result?.status === 422 && "value" in result && isObject(result.value) && result.value.transferRefused === true && typeof result.value.message === "string") {
    // A reply from a primary that has since been replaced cannot settle this attempt.
    const before = followedSetup(link), now = followedSetup(readLink());
    if (before?.id === now?.id && before?.epoch === now?.epoch) return { status: "refused", message: result.value.message };
  }
  return { status: "pending" };
}

type Settlement = { setup: string; move: Transfer & { removalOperation?: string } };
let settling = false;
let settledAt = 0;

/** Only activation/arrival followed by durable source cleanup can compact a receipt.
 * The owner and the queue still fence older copies and backups. */
export function noteTransferSettled(link: LinkState | null, move: Transfer & { removalOperation?: string }): void {
  const setup = followedSetup(link);
  if (!setup?.checkpointed || !ownershipReady()) return;
  const transfer = transferOf(move);
  if (transfer) keepRecord("transfer-settlements", recordKey(setup.id, move.id), { setup: setup.id, move: { ...transfer, ...(move.removalOperation ? { removalOperation: move.removalOperation } : {}) } });
}

export function settleTransfers(link: LinkState | null, now: number, queueSettled: (id: string) => boolean): void {
  const setup = followedSetup(link);
  if (!setup?.checkpointed || transferSupport(link) || settling || now - settledAt < 5_000) return;
  settledAt = now;
  for (const key of records("transfer-settlements")) {
    const kept = readRecord<Settlement>("transfer-settlements", key);
    if (!kept) continue;
    if (kept.setup !== setup.id) {
      removeRecord("transfer-settlements", key);
      continue;
    }
    if (kept.move.kind === "queue" && !queueSettled(kept.move.id)) continue;
    settling = true;
    void requestTransfer(link, kept.move, true).then(result => {
      if (result?.status === 200) removeRecord("transfer-settlements", key);
    }).catch(error => console.error("kuvo-link: ownership receipt compaction will retry", error))
      .finally(() => { settling = false; });
    break;
  }
}
