import path from "node:path";
import { locked, syncDurableFile } from "../../shared/files";
import { isObject } from "../../shared/json";
import { RUN } from "../home";
import type { LinkRequest } from "../server/context";
import type { Acted } from "../work-queue/forward";
import { fingerprint } from "./merge";
import { keepRecord, readRecord, recordKey, removeRecord } from "./store";

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

const LOCK = path.join(RUN, "owned-data", ".mutations.lock");
type MutationAnswer = Acted & { notAdmitted?: true };
interface Claim { digest: string; answer?: MutationAnswer; at?: number }
const running = new Map<string, Promise<Claim>>();

export const MUTATION_FEATURE = "data.mutation-identity";

const reused: MutationAnswer = { status: 409, value: { message: "That operation id is already in use." } };

/** A finished native outcome can be recovered while the engine or its record is unavailable. */
export function mutationOutcome(key: string, change: unknown): (MutationAnswer & { acceptedAt?: number }) | undefined {
  const claim = readRecord<Claim>("mutations", key);
  if (!claim) return undefined;
  if (claim.digest !== fingerprint(change)) return reused;
  return claim.answer && { ...claim.answer, acceptedAt: claim.at };
}

/** Native adapters with an external side effect claim it before calling out. Adapters
 * able to commit their result with the data (queue edits, setup saves) use that result. */
export async function runMutation(key: string, change: unknown, run: () => Promise<MutationAnswer>, admit?: () => MutationAnswer | null): Promise<MutationAnswer & { acceptedAt?: number }> {
  const digest = fingerprint(change);
  const { earlier, refused } = locked(LOCK, () => {
    const known = readRecord<Claim>("mutations", key);
    if (known) return { earlier: known };
    const veto = admit?.();
    if (veto) {
      if (veto.notAdmitted) return { refused: veto };
      const at = Date.now();
      keepRecord("mutations", key, { digest, answer: veto, at });
      return { refused: { ...veto, acceptedAt: at } };
    }
    keepRecord("mutations", key, { digest });
    return {};
  }, { required: true });
  if (refused) return refused;
  if (earlier) {
    if (earlier.digest !== digest) return reused;
    const current = earlier.answer ? earlier : await running.get(key);
    return current?.answer
      ? { ...current.answer, acceptedAt: current.at }
      : { status: 409, value: { message: "The owner has claimed this operation. Check the record before trying again." } };
  }
  const work: Promise<Claim & { answer: MutationAnswer }> = Promise.resolve().then(async () => {
    const answer = await run();
    if (answer.notAdmitted) {
      // There was no side effect. Release only this pending claim, so the same operation can
      // be retried. A lost or ambiguous POST reply never takes this path.
      locked(LOCK, () => {
        const pending = readRecord<Claim>("mutations", key);
        if (pending?.digest === digest && !pending.answer) {
          removeRecord("mutations", key);
          syncDurableFile(path.join(RUN, "owned-data", "mutations"), RUN);
        }
      }, { required: true });
      return { digest, answer };
    }
    const done = { digest, answer, at: Date.now() };
    keepRecord("mutations", key, done);
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
