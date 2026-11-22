import { lockedWhile } from "../../shared/files";
import { isObject } from "../../shared/json";
import type { LinkRequest } from "../server/context";
import type { Acted } from "../work-queue/forward";
import { fingerprint } from "./merge";
import { readRecord, recordKey } from "./store";
import { MUTATION_LOCK as LOCK, readForwardEvidence, type ForwardEvidence } from "./forward-evidence";
import { keepReceipt, removeReceipt } from "./receipt-worker";
import type { Submitter } from "./authority";

export const MUTATION_ID = /^[A-Za-z0-9_-]{1,80}$/;
export const mutationKey = (setup: string, nonce: string) => recordKey(setup, nonce);

/** The caller supplies the same operation on SSH, HTTP and the relay. */
export function mutationNonce(request: LinkRequest, body: unknown): string | null | undefined {
  const header = request.headers["x-kuvo-operation"];
  const field = isObject(body) ? body.operation : undefined;
  if (header !== undefined && field !== undefined && header !== field) return null;
  const value = header ?? field;
  if (value === undefined) return undefined;
  return typeof value === "string" && MUTATION_ID.test(value) ? value : null;
}

type MutationAnswer = Acted & { notAdmitted?: true };
type OrphanDecision = MutationAnswer | "again" | "resume" | null;
export interface Submission {
  setup: string;
  session: string;
  id: string;
  origin: string;
  submittedThrough: string;
  submitter: Submitter;
  draft?: Record<string, unknown>;
}
export interface Claim { digest: string; answer?: MutationAnswer; at?: number; submission?: Submission; forward?: ForwardEvidence }
const running = new Map<string, Promise<Claim>>();
// Claims a restart left unanswered, being settled (`orphaned`): one look for each at a time.
const settling = new Map<string, Promise<OrphanDecision>>();

export const MUTATION_FEATURE = "data.mutation-identity";

const reused: MutationAnswer = { status: 409, value: { message: "That operation id is already in use." } };

export function hasMutation(key: string): boolean {
  return readRecord<Claim>("mutations", key) !== undefined || readRecord<Claim>("submissions", key) !== undefined;
}

/** A finished native outcome can be recovered while the engine or its record is unavailable. */
export function mutationOutcome(key: string, change: unknown): (MutationAnswer & { acceptedAt?: number }) | undefined {
  const claim = readRecord<Claim>("mutations", key);
  if (!claim) return undefined;
  if (claim.digest !== fingerprint(change)) return reused;
  return claim.answer && { ...claim.answer, acceptedAt: claim.at };
}

/** Native adapters with an external side effect claim it before calling out. Adapters
 * able to commit their result with the data (queue edits, setup saves) use that result.
 * `orphaned`: for a claim an earlier link left unanswered (it stopped partway: an update, a
 * restart), looks where the effect would be: its answer once it's there, `again` when nothing of it
 * happened (it runs now), `resume` when the adapter can safely continue the originally claimed work, else null (`uncertain`). */
export async function runMutation(key: string, change: unknown, run: () => Promise<MutationAnswer>, admit?: () => MutationAnswer | null, uncertain?: () => MutationAnswer, submission?: Submission, legacyKey?: string, forward?: { evidence: ForwardEvidence; digestFor: (owner: string) => string }, orphaned?: () => Promise<OrphanDecision>): Promise<MutationAnswer & { acceptedAt?: number }> {
  const digest = fingerprint(change);
  const family = submission ? "submissions" : "mutations";
  const identity = submission ? { submission } : forward ? { forward: forward.evidence } : {};
  const { earlier, refused } = await lockedWhile(LOCK, async () => {
    const known = readRecord<Claim>("mutations", key) ?? (submission && readRecord<Claim>("submissions", key));
    if (known) readForwardEvidence(known.forward, key);
    if (known && (known.answer?.notAdmitted !== true || known.digest !== digest)) return { earlier: known };
    if (!known && legacyKey && readRecord<Claim>("mutations", legacyKey))
      return { refused: uncertain?.() ?? reused };
    const veto = admit?.();
    if (veto?.notAdmitted && !submission) return { refused: veto };
    // An older keeper still accepts /draft. Reserve its lane before writing the scoped claim.
    // A cut between these writes leaves a fence and no effect.
    if (!known && legacyKey) await keepReceipt("mutations", legacyKey, { digest: `scoped:${key}`,
      ...(submission ? { draftReplayFence: { setup: submission.setup, session: submission.session, id: submission.id.split(".").at(-1) } } : {}),
    });
    if (veto) {
      const at = Date.now();
      await keepReceipt(family, key, { digest, ...identity, answer: veto, at });
      return { refused: { ...veto, acceptedAt: at } };
    }
    await keepReceipt(family, key, { digest, ...identity });
    return {};
  }, { required: true });
  if (refused) return refused;
  if (earlier) {
    const evidence = readForwardEvidence(earlier.forward, key);
    if (earlier.digest !== digest && !(forward && evidence && earlier.digest === forward.digestFor(evidence.owner))) return reused;
    const current = earlier.answer ? earlier : await running.get(key);
    if (current?.answer) return { ...current.answer, acceptedAt: current.at };
    const settled = !earlier.answer && !current && !submission && earlier.digest === digest && orphaned ? await settleOrphan(key, orphaned) : null;
    if (settled === "again") return runMutation(key, change, run, admit, uncertain, submission, legacyKey, forward, orphaned);
    if (settled && settled !== "resume") {
      const at = Date.now();
      await lockedWhile(LOCK, async () => {
        const pending = readRecord<Claim>("mutations", key);
        if (pending?.digest === digest && !pending.answer) await keepReceipt("mutations", key, { ...pending, answer: settled, at });
      }, { required: true });
      const kept = readRecord<Claim>("mutations", key);
      return kept?.answer ? { ...kept.answer, acceptedAt: kept.at } : { ...settled, acceptedAt: at };
    }
    if (settled !== "resume") return uncertain?.() ?? { status: 409, value: { message: "The owner has claimed this operation. Check the record before trying again." } };
    // All callers settling this orphan share the lookup. The first to resume owns the work.
    const work = running.get(key);
    if (work) {
      const done = await work;
      if (done.answer) return { ...done.answer, acceptedAt: done.at };
    }
  }
  const work: Promise<Claim & { answer: MutationAnswer }> = Promise.resolve().then(async () => {
    const answer = await run();
    if (answer.notAdmitted && !submission) {
      // There was no side effect. Release only this pending claim, so the same operation can
      // be retried. A lost or ambiguous POST reply never takes this path.
      await lockedWhile(LOCK, async () => {
        const pending = readRecord<Claim>("mutations", key);
        if (pending?.digest === digest && !pending.answer) {
          await removeReceipt("mutations", key);
        }
      }, { required: true });
      return { digest, answer };
    }
    const value = answer.value;
    // Accepted effects keep their full answer and replay fence. Only recovery material becomes
    // redundant when the owner can name a message or report a successful domain outcome.
    const settled = submission && answer.status === 200 && isObject(value) && isObject(value.answer)
      && !value.answer.ambiguous && !value.answer.restore && !value.answer.error
      ? { submission: { ...submission, draft: undefined } } : identity;
    const done = { digest, ...settled, answer, at: Date.now() };
    let publishing = false;
    try {
      await lockedWhile(LOCK, async () => {
        publishing = true;
        await keepReceipt(family, key, done);
      }, { required: true, wait: 100 });
    } catch (error) {
      if (publishing) throw error;
      // An unrelated lock cannot stop publication after the effect. Leave the index stamp
      // unchanged and let the next reader notice the canonical file change.
      await keepReceipt(family, key, done, false);
    }
    return done;
  });
  running.set(key, work);
  try {
    const done = await work;
    return { ...done.answer, acceptedAt: done.at };
  } finally {
    if (running.get(key) === work) running.delete(key);
  }
}

// One claim settled at a time: the look, then (nothing of it having happened) the claim let go for
// the send that asks next to run it, as one that was never admitted is.
function settleOrphan(key: string, orphaned: () => Promise<OrphanDecision>): Promise<OrphanDecision> {
  const under = settling.get(key);
  if (under) return under.then(found => found === "again" ? null : found);
  const look = orphaned().catch(() => null).then(async found => {
    if (found !== "again") return found;
    let released = false;
    await lockedWhile(LOCK, async () => {
      const pending = readRecord<Claim>("mutations", key);
      if (pending && !pending.answer && !running.has(key)) {
        await removeReceipt("mutations", key);
        released = true;
      }
    }, { required: true });
    return released ? "again" as const : null;
  }).finally(() => settling.delete(key));
  settling.set(key, look);
  return look;
}
