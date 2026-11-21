import { dataOwner } from "../../shared/data-declarations";
import { isObject } from "../../shared/json";
import { KEY, SETUP_ID, recordKey } from "./record-address";
import { readSubmitter, type Submitter } from "./submitter";
import type { DataReference } from "./resolve";
import type { Act } from "../work-queue/forward";

export const decisions = {
  primary: "forwardedPrimaryDecisions",
  queue: "forwardedQueueDecisions",
  "session computer": "forwardedSessionDecisions",
  "group host": "forwardedGroupDecisions",
  "local computer": "forwardedComputerDecisions",
} as const;

export type ForwardDecisionKind = typeof decisions[keyof typeof decisions];

export interface ForwardEvidence {
  kind: ForwardDecisionKind | "forwardedOperationOrigins";
  setup: string;
  operation: string;
  owner: string;
  listedBy?: string;
  submitter: Submitter;
  area: NonNullable<Act["area"]> | "queue";
  record: string;
  ref?: DataReference;
}

export const draftKey = (setup: string, session: string, id: string) => recordKey("typed-draft", setup, session, id);

export function draftReplayFence(value: unknown, key: string) {
  if (value === undefined) return undefined;
  if (!isObject(value) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup)
    || typeof value.session !== "string" || !/^ses_[A-Za-z0-9_-]{1,95}$/.test(value.session)
    || typeof value.id !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(value.id)
    || key !== draftKey(value.setup, value.session, value.id))
    throw new Error("Invalid draft replay fence.");
  return value;
}

const keyOf = (kind: ForwardEvidence["kind"], setup: string, operation: string) =>
  recordKey(kind === "forwardedOperationOrigins" ? "forward-origin" : "scoped-forward", setup, operation);

export const forwardOriginKey = (setup: string, operation: string) => keyOf("forwardedOperationOrigins", setup, operation);
export const forwardDecisionKey = (setup: string, operation: string) => keyOf("forwardedComputerDecisions", setup, operation);

export function forwardDecision(ref: DataReference | undefined): ForwardDecisionKind {
  if (ref?.kind === "queueItems") return decisions.queue;
  const owner = ref && dataOwner(ref.kind, ref.queue);
  return owner === "primary" || owner === "session computer" || owner === "group host" ? decisions[owner] : decisions["local computer"];
}

export function readForwardEvidence(value: unknown, key: string): ForwardEvidence | undefined {
  if (value === undefined) return undefined;
  if (!isObject(value) || typeof value.setup !== "string" || !SETUP_ID.test(value.setup)
    || typeof value.operation !== "string" || !/^[A-Za-z0-9_-]{1,80}$/.test(value.operation)
    || typeof value.owner !== "string" || !KEY.test(value.owner) || !readSubmitter(value.submitter)
    || (value.listedBy !== undefined && (typeof value.listedBy !== "string" || !KEY.test(value.listedBy)))
    || typeof value.area !== "string" || typeof value.record !== "string" || !value.record
    || (value.kind !== "forwardedOperationOrigins" && !Object.values(decisions).some(kind => kind === value.kind)))
    throw new Error("Invalid forwarded operation evidence.");
  const evidence = value as unknown as ForwardEvidence;
  if (key !== keyOf(evidence.kind, evidence.setup, evidence.operation)) throw new Error("Forwarded operation evidence has the wrong address.");
  if (evidence.ref !== undefined) {
    const ref = evidence.ref;
    if (!isObject(ref) || typeof ref.kind !== "string" ||
        !["queueItems", "queueLabels", "groupSettings", "sessionQueue", "sessionSchedules", "setupNames", "setupTimeZone",
          "setupPersonalities", "membershipAndAccess", "clientIdentity", "setupLeadership"].includes(ref.kind)
        || (ref.id !== undefined && (typeof ref.id !== "string" || !/^[A-Za-z0-9_-]{1,100}$/.test(ref.id)))
        || (ref.queue !== undefined && (typeof ref.queue !== "string" || !/^[A-Za-z0-9_-]{1,100}$/.test(ref.queue))))
      throw new Error("Invalid forwarded operation record address.");
  }
  if (evidence.kind !== "forwardedOperationOrigins" && evidence.kind !== forwardDecision(evidence.ref))
    throw new Error("Forwarded operation evidence has the wrong owner policy.");
  return evidence;
}
