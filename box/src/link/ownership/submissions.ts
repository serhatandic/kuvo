import { isObject } from "../../shared/json";
import { KEY, ownKey, peersOf, type MailSender } from "../peers/known";
import { deviceConnectionsAllowed, pairedPhone } from "../phones";
import type { RouteContext } from "../server/context";
import type { LinkHost } from "../relay/host";
import { followedSetup, SETUP_ID } from "../setup/record";
import { readLink, type LinkState } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { readSubmitter, submitterAuthority, submitterOf, type Submitter } from "./authority";
import { fingerprint } from "./merge";
import { MUTATION_ID, type Claim } from "./mutation";
import { ownerOf, replacementOwner } from "./resolve";
import { ownershipReady, readRecord, recordKey, records } from "./store";

export const SUBMISSION_FEATURE = "typed.submission-recovery";
export const SUBMIT_ROUTE = /^kuvo\/sessions\/(ses_[A-Za-z0-9_-]{1,95})\/submit$/;
export const SUBMISSION_ROUTE = /^kuvo\/sessions\/(ses_[A-Za-z0-9_-]{1,95})\/submissions(?:\/([A-Za-z0-9_.-]+))?$/;
const PAGE = 40;
const PEER_VALUE_BYTES = 100 * 1024;

export function readSubmissionID(value: unknown) {
  if (typeof value !== "string") return;
  const parts = value.split(".");
  if (parts.length !== 3 || !SETUP_ID.test(parts[0]!) || !KEY.test(parts[1]!) || !MUTATION_ID.test(parts[2]!)) return;
  return { setup: parts[0]!, origin: parts[1]!, id: parts[2]!, submission: value };
}

export const submissionKey = (setup: string, session: string, id: string) => recordKey("draft-submission", setup, session, id);
export const submissionIntent = (given: unknown, submitter: Submitter, through: string) =>
  ({ given, submitter, ...(submitter.kind === "computer" ? { through } : {}) });
const ordered = (left: { submission: string }, right: { submission: string }) =>
  left.submission < right.submission ? -1 : left.submission > right.submission ? 1 : 0;

export function submissionOwner(link: LinkState | null, session: string): string | null {
  return ownerOf({ kind: "conversations", id: session }, { link })?.key ?? (link && ownKey(link)) ?? null;
}

function viewerAllowed(link: LinkState | null, viewer: Submitter) {
  if (viewer.kind === "paired") return deviceConnectionsAllowed(link) && Boolean(pairedPhone(link, viewer.key, viewer.pairing));
  return viewer.kind === "computer" || ownKey(link ?? {}) === viewer.key || peersOf(link).some(peer => peer.key === viewer.key);
}

/** A newly paired generation of the same device may read its history. Only the original
 * generation can match the execution digest, and the reader must have a current grant. */
export function sameReceiptAuthor(saved: unknown, viewer: Submitter): boolean {
  const original = readSubmitter(saved);
  return Boolean(original && (original.kind === "paired" && viewer.kind === "paired"
    ? original.key === viewer.key : fingerprint(original) === fingerprint(viewer)));
}

function canRead(claim: Claim, viewer: Submitter, through: string) {
  const saved = claim.submission;
  if (!saved || !sameReceiptAuthor(saved.submitter, viewer)) return false;
  return viewer.kind !== "computer" || saved.submittedThrough === through;
}

function report(claim: Claim, includeDraft: boolean) {
  const { submission: saved, answer, digest, at } = claim;
  if (!saved) return null;
  const value = isObject(answer?.value) ? answer.value : {};
  const decision = isObject(value.answer) ? value.answer : {};
  const status = !answer || value.ambiguous || decision.ambiguous || (decision.error && !decision.restore)
    || (answer.status >= 500 && !answer.notAdmitted && !value.restore) ? "unknown"
    : answer.notAdmitted || answer.status >= 400 || decision.restore ? "notAdmitted" : "committed";
  return { submission: saved.id, originalOwner: saved.origin, status, digest, submitter: saved.submitter, evidence: "owner",
    ...(status === "committed" ? { acceptedAt: at, effect: includeDraft ? value : effectSummary(value) } : {}),
    ...(status === "notAdmitted" ? { decision: value, ...(answer?.notAdmitted ? { retryable: true } : {}) } : {}),
    ...(status !== "committed" && saved.draft ? { draftAvailable: true, ...(includeDraft ? { draft: saved.draft } : {}) } : {}) };
}

function effectSummary(value: Record<string, unknown>) {
  const decision = isObject(value.answer) ? value.answer : {};
  return {
    ...(typeof value.message === "string" ? { message: value.message } : {}),
    actions: Object.keys(decision),
  };
}

function recorded(link: LinkState, session: string, submission: string | undefined, viewer: Submitter, through: string, after: string) {
  const setup = followedSetup(link);
  const own = ownKey(link);
  if (!setup || !own || !viewerAllowed(link, viewer)) return [];
  const ids = submission ? [submissionKey(setup.id, session, submission)] : records("submissions").sort();
  const out: NonNullable<ReturnType<typeof report>>[] = [];
  for (const id of ids) {
    const claim = readRecord<Claim>("submissions", id);
    const saved = claim?.submission;
    if (!claim || !saved || saved.session !== session || saved.setup !== setup.id || !canRead(claim, viewer, through)) continue;
    if (saved.origin !== own && replacementOwner(link, saved.origin) !== own || saved.id <= after) continue;
    const outcome = report(claim, Boolean(submission));
    if (outcome) out.push(outcome);
  }
  return out.sort(ordered).slice(0, PAGE);
}

/** The active Relay membership authenticates the forwarding Box. Read in the requesting
 * device's current scope. No execution adapter is reachable from this route. */
export async function readSubmissionForPeer(sender: MailSender, act: Act, host?: Pick<LinkHost, "send"> | null): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  const viewer = readSubmitter(act.body?.viewer);
  const session = act.body?.session;
  const submission = act.body?.submission;
  if (!link || !setup || !viewer || !peersOf(link).some(peer => peer.key === sender.key) || submitterAuthority(link, viewer, host) !== "allowed" ||
      act.body?.setup !== setup.id || typeof session !== "string" || !/^ses_[A-Za-z0-9_-]{1,95}$/.test(session) ||
      (submission !== undefined && readSubmissionID(submission)?.setup !== setup.id)) return { status: 403, value: {} };
  const after = typeof act.body?.after === "string" ? act.body.after : "";
  const { listingSubmissions } = await import("./messages");
  const current = readLink();
  if (!current || followedSetup(current)?.id !== setup.id || !peersOf(current).some(peer => peer.key === sender.key)
    || submitterAuthority(current, viewer, host) !== "allowed") return { status: 403, value: {} };
  const digest = submission && typeof act.body?.digest === "string" ? act.body.digest : null;
  const outcomes = combine([
    ...listingSubmissions(setup.id, session, viewer, submission as string | undefined, after, false),
    ...recorded(current, session, submission as string | undefined, viewer, sender.key, after),
  ].filter(value => digest === null || value.digest === digest)).slice(0, PAGE);
  return { status: 200, value: { outcomes: outcomes.map(outcome => {
    if (Buffer.byteLength(JSON.stringify(outcome)) <= PEER_VALUE_BYTES) return outcome;
    const { draft, effect, decision, ...summary } = outcome;
    return { ...summary, recoveryComputer: ownKey(current), ...(draft ? { draftAvailable: true } : {}),
      ...(isObject(effect) ? { effect: effectSummary(effect) } : {}) };
  }) } };
}

function combine(values: unknown[]) {
  const priority = (value: Record<string, unknown>) => value.status === "committed" ? 4
    : value.evidence === "owner" && !value.retryable ? 3 : value.status === "pending" ? 2 : value.status === "unknown" ? 1 : 0;
  const found = new Map<string, Record<string, unknown> & { submission: string }>();
  for (const value of values) {
    const id = isObject(value) && readSubmissionID(value.submission);
    if (!id || !isObject(value)) continue;
    const prior = found.get(id.submission);
    const conflict = prior && (prior.digest !== value.digest || prior.conflict === true || value.conflict === true);
    const best = !prior || priority(value) > priority(prior) || (priority(value) === priority(prior) && value.evidence === "owner") ? value : prior;
    found.set(id.submission, { ...best, submission: id.submission, ...(conflict ? { conflict: true } : {}) });
  }
  return [...found.values()].sort(ordered);
}

export async function* submissionsRoute(context: RouteContext, match: RegExpMatchArray) {
  const { request, json } = context;
  if (request.method !== "GET") return yield* json(405, {});
  if (!ownershipReady()) return yield* json(503, { message: "Wait for the Box update to finish." });
  const link = readLink();
  const viewer = submitterOf(context, link, true);
  if (!viewer || !viewerAllowed(link, viewer)) return yield* json(403, { message: "Open this from a paired Kuvo Code client." });
  const session = match[1]!;
  const id = match[2] && readSubmissionID(match[2]);
  if (match[2] && !id) return yield* json(400, { message: "Use the original submission id." });
  const currentOwner = submissionOwner(link, session);
  const query = new URLSearchParams(request.path.split("?")[1]);
  const setup = followedSetup(link);
  const own = link && ownKey(link);
  if (query.has("scope") || !link || !own || !setup) return yield* json(200, { currentOwner, outcomes: [], complete: Boolean(query.has("scope")),
    ...(setup ? { setup: setup.id } : {}),
    canSubmit: Boolean(setup && currentOwner && (currentOwner === own || takesForwarded(currentOwner, SUBMISSION_FEATURE))) });
  if (id && id.setup !== setup.id) return yield* json(200, { currentOwner, outcomes: [], complete: false, unknown: true });
  const after = id ? "" : query.get("after") ?? "";
  const digest = id ? query.get("digest") : null;
  const wanted = (value: Record<string, unknown>) => digest === null || value.digest === digest;
  const local = recorded(link, session, id ? id.submission : undefined, viewer, own, after);
  if (id && digest && local[0]?.status === "committed" && wanted(local[0])) return yield* json(200, { currentOwner, outcomes: local, complete: true });
  const { listingSubmissions, settleListingOutcome } = await import("./messages");
  const listing = listingSubmissions(setup.id, session, viewer, id ? id.submission : undefined, after);
  const peers = peersOf(link);
  const replies = await Promise.all(peers.map(async peer => {
    if (!takesForwarded(peer.key, SUBMISSION_FEATURE)) return null;
    const result = await forwardAct(peer, "Submission outcome", { method: "GET", area: "ownership", item: "draft-submissions",
      body: { setup: setup.id, viewer, session, ...(id ? { submission: id.submission, ...(digest ? { digest } : {}) } : { after }) } },
    context.signal, link, Date.now(), { backgroundRelay: true });
    return result.status === 200 && "value" in result && isObject(result.value) && Array.isArray(result.value.outcomes)
      ? { peer: peer.key, outcomes: result.value.outcomes } : null;
  }));
  const current = readLink();
  if (followedSetup(current)?.id !== setup.id || !viewerAllowed(current, viewer)) return yield* json(403, { message: "The submitting device no longer has access." });
  const currentPeers = new Set(peersOf(current).map(peer => peer.key));
  const visible = replies.flatMap(reply => !reply || !currentPeers.has(reply.peer) ? [] : reply.outcomes.filter(value => {
    const handle = isObject(value) && readSubmissionID(value.submission);
    return handle && handle.setup === setup.id && isObject(value) && value.originalOwner === handle.origin && sameReceiptAuthor(value.submitter, viewer)
      && (value.evidence === "listing" || (value.evidence === "owner" && (handle.origin === reply.peer || replacementOwner(current, handle.origin) === reply.peer)));
  }));
  const outcomes = combine([...listing, ...local, ...visible].filter(value => isObject(value) && wanted(value))).slice(0, PAGE);
  if (id && outcomes[0]) settleListingOutcome(setup.id, session, outcomes[0]);
  const complete = replies.every(reply => reply !== null);
  return yield* json(200, { currentOwner: submissionOwner(current, session), outcomes,
    complete, ...(outcomes.length === PAGE ? { next: outcomes.at(-1)!.submission } : {}),
    ...(id && !outcomes.length ? { unknown: true } : {}) });
}
