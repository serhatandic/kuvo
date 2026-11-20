// Revoked members can finish a prepared move through its change identity. This path
// never dispatches forwarded routes, ordinary peer mail, or an arbitrary ownership request.
import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import { takeMoveMail } from "../moves/move";
import { endedFor, readMove } from "../moves/records";
import { assignmentFor } from "../ownership/resolve";
import { verifySessionPayloads } from "../moves/session-files";
import { verifyQueuePayloads } from "../work-queue/moves";
import { moveReference, transferForPeer, type Transfer } from "../ownership/transfer";
import { ownKey, peersOf, type Peer } from "../peers/known";
import type { LinkHost } from "../relay/host";
import { liveLink, type LinkState } from "../state";
import { primaryFenced } from "./lease";
import { followedSetup } from "./record";
import { sendRemoval } from "./removal";
import { REMOVAL_ID, removalMoveID, removalsOf } from "./removal-record";

type Host = Pick<LinkHost, "welcomed" | "send" | "keepOnly">;
type Decision = { status: number; value: unknown };
const waiting = new Map<string, (result: Decision) => void>();

export async function removalTransfer(link: LinkState | null, move: Transfer, operation: string, settled: boolean) {
  const setup = followedSetup(link);
  const primary = peersOf(link).find(peer => peer.key === setup?.primary);
  if (!link || !setup || !primary) return;
  const key = JSON.stringify([setup.id, setup.epoch, operation, move.id, settled]);
  return new Promise<Decision | undefined>(resolve => {
    const timer = setTimeout(() => { waiting.delete(key); resolve(undefined); }, 3_000);
    waiting.set(key, result => {
      clearTimeout(timer);
      waiting.delete(key);
      resolve(result);
    });
    sendRemoval(liveLink(), primary, link, { transfer: move, operation, settled, setup: setup.id, epoch: setup.epoch });
  });
}

const probeLifetime = 30_000;
let probe: { setup: string; epoch: number; operation: string; peer: string; nonce: string; sent: number; answered?: number } | undefined;

export function removalDestinationReady(link: LinkState, to: string, operation: string, host: Pick<Host, "welcomed" | "send"> | null | undefined = liveLink()): boolean {
  const setup = followedSetup(link);
  const peer = peersOf(link).find(each => each.key === to);
  if (!setup || !peer || link.removalSource?.operation !== operation || link.removalSource.moveTo !== to) return false;
  const now = Date.now();
  if (probe?.setup === setup.id && probe.epoch === setup.epoch && probe.operation === operation && probe.peer === to) {
    if (probe.answered !== undefined && now >= probe.answered && now - probe.answered < probeLifetime) return true;
    if (now >= probe.sent && now - probe.sent < 2_000) return false;
  }
  probe = { setup: setup.id, epoch: setup.epoch, operation, peer: to, nonce: crypto.randomBytes(16).toString("base64url"), sent: now };
  sendRemoval(host, peer, link, { setup: setup.id, epoch: setup.epoch, operation, moveProbe: probe.nonce });
  return false;
}

type FileCheck = { setup: string; epoch: number; operation: string; peer: string; accept: (ok: boolean) => void };
const fileChecks = new Map<string, FileCheck>();

/** The removal destination verifies the exact set accepted for this operation using its
 * bundle-bound switch decisions. A fresh nonce prevents replaying an earlier answer. */
export function confirmRemovalFiles(link: LinkState, to: string, operation: string, host: Pick<Host, "welcomed" | "send"> | null | undefined = liveLink()): Promise<boolean | undefined> {
  const setup = followedSetup(link);
  const peer = peersOf(link).find(each => each.key === to);
  if (!setup || !peer || !host?.welcomed || link.removalSource?.operation !== operation || link.removalSource.moveTo !== to) return Promise.resolve(undefined);
  const nonce = crypto.randomBytes(16).toString("base64url");
  return new Promise(resolve => {
    const request = () => sendRemoval(host, peer, link, { setup: setup.id, epoch: setup.epoch, operation, moveFilesCheck: nonce });
    const finish = (result?: boolean) => {
      fileChecks.delete(nonce);
      clearInterval(repeat);
      clearTimeout(timeout);
      resolve(result);
    };
    const repeat = setInterval(request, 5_000);
    const timeout = setTimeout(finish, 30_000);
    fileChecks.set(nonce, { setup: setup.id, epoch: setup.epoch, operation, peer: to, accept: finish });
    request();
  });
}

export function receiveRemovalMove(sender: string, message: Record<string, unknown>, host: Host | null | undefined, link: LinkState): boolean {
  const { scopedMove, transfer, transferResult, operation, moveProbe, moveReady, moveFilesCheck, moveFilesResult } = message;
  if (scopedMove === undefined && transfer === undefined && transferResult === undefined && moveProbe === undefined && moveReady === undefined && moveFilesCheck === undefined && moveFilesResult === undefined) return false;
  const setup = followedSetup(link);
  const own = ownKey(link);
  if (!setup?.checkpointed || !own || setup.conflict) return true;
  if (isObject(scopedMove) && typeof scopedMove.id === "string") {
    const removal = removalsOf(link)[sender];
    if ((!link.removalSource && !removal) || !/^mov_[A-Za-z0-9]{1,40}$/.test(scopedMove.id)) return true;
    const journal = readMove(scopedMove.id);
    const peer = peersOf(link).find(candidate => candidate.key === sender);
    if (journal?.role === "from" && journal.removalOperation && journal.peer === sender && peer) {
      void takeMoveMail(peer, scopedMove).catch(console.error);
      return true;
    }
    const plan = removal?.operation === operation ? removal.plan : typeof operation === "string" ? removal?.superseded?.[operation] : undefined;
    const active = removal?.operation === operation && !removal?.resolution && !removal?.delivered && !removal?.generation;
    const finishing = ["switch", "drop", "decision?"].includes(String(scopedMove.op)) &&
      ((journal?.role === "to" && journal.peer === sender) || endedFor(scopedMove.id, sender));
    const kind = journal?.kind ?? scopedMove.kind;
    const thing = journal?.thing ?? scopedMove.thing;
    if (removal && typeof operation === "string" && plan?.moveTo === own && typeof thing === "string" &&
      (kind === "chat" || kind === "bot" || kind === "group") && scopedMove.id === removalMoveID(operation, kind, thing) &&
      (finishing || (active && removal.selection?.some(item => item.id === thing && item.kind === kind)))) {
      void takeMoveMail(removal.peer, scopedMove).catch(console.error);
    }
    return true;
  }
  if (message.setup !== setup.id || message.epoch !== setup.epoch || typeof operation !== "string") return true;
  if (typeof moveFilesCheck === "string" && REMOVAL_ID.test(moveFilesCheck)) {
    const removal = removalsOf(link)[sender];
    if (removal?.operation === operation && removal.plan?.moveTo === own && !removal.resolution && !removal.delivered && removal.selection) {
      const ok = removal.selection.every(({ kind, id }) => {
        const change = removalMoveID(operation, kind, id);
        const assignment = assignmentFor(link, moveReference(kind, id));
        if (assignment?.computer !== own || (assignment.move !== change && (assignment.move !== undefined || !removal.generation))) return false;
        const decision = endedFor(change, sender);
        if (decision?.as !== "switched" || !decision.generation || !decision.hash || !decision.payloads) return false;
        try {
          verifySessionPayloads(decision.payloads.sessions);
          verifyQueuePayloads(decision.payloads.queue);
          return true;
        } catch { return false; }
      });
      sendRemoval(host, removal.peer, link, { setup: setup.id, epoch: setup.epoch, operation, moveFilesResult: { nonce: moveFilesCheck, ok } });
    }
  } else if (isObject(moveFilesResult) && typeof moveFilesResult.nonce === "string" && typeof moveFilesResult.ok === "boolean") {
    const waiting = fileChecks.get(moveFilesResult.nonce);
    if (waiting && waiting.peer === sender && waiting.setup === setup.id && waiting.epoch === setup.epoch && waiting.operation === operation) {
      waiting.accept(moveFilesResult.ok);
    }
  } else if (typeof moveProbe === "string" && REMOVAL_ID.test(moveProbe)) {
    const removal = removalsOf(link)[sender];
    if (removal?.operation === operation && removal.plan?.moveTo === own && !removal.resolution && !removal.delivered && !removal.generation) {
      sendRemoval(host, removal.peer, link, { setup: setup.id, epoch: setup.epoch, operation, moveReady: moveProbe });
    }
  } else if (probe && moveReady === probe.nonce && probe.setup === setup.id && probe.epoch === setup.epoch &&
    probe.operation === operation && probe.peer === sender && link.removalSource?.operation === operation &&
    link.removalSource.moveTo === sender) {
    const now = Date.now();
    if (now >= probe.sent && now - probe.sent < probeLifetime && probe.answered === undefined) probe.answered = now;
  } else if (isObject(transfer) && setup.role === "primary" && !primaryFenced(link)) {
    const removal = removalsOf(link)[sender];
    if (!removal || typeof transfer.id !== "string") return true;
    void transferForPeer(removal.peer, { body: { setup: setup.id, epoch: setup.epoch, transfer, settled: message.settled } }, operation).then(result => {
      if (result.status !== 200 && result.status !== 422) return;
      sendRemoval(host, removal.peer, link, { transferResult: result, operation, id: transfer.id,
        settled: message.settled === true, setup: setup.id, epoch: setup.epoch });
    }).catch(console.error);
  } else if (isObject(transferResult) && sender === setup.primary && typeof message.id === "string" &&
    (transferResult.status === 200 || transferResult.status === 422)) {
    waiting.get(JSON.stringify([setup.id, setup.epoch, operation, message.id, message.settled === true]))?.(
      { status: transferResult.status, value: transferResult.value });
  }
  return true;
}
