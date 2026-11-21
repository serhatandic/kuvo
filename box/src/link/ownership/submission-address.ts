import { isObject } from "../../shared/json";
import { KEY, SETUP_ID, recordKey } from "./record-address";
import type { DataReference } from "./resolve";

const SESSION = /^ses_[A-Za-z0-9_-]{1,95}$/;
const MESSAGE_ID = /^[A-Za-z0-9_-]{1,80}$/;

export function readSubmissionID(value: unknown) {
  if (typeof value !== "string") return;
  const parts = value.split(".");
  if (parts.length !== 3 || !SETUP_ID.test(parts[0]!) || !KEY.test(parts[1]!) || !MESSAGE_ID.test(parts[2]!)) return;
  return { setup: parts[0]!, origin: parts[1]!, id: parts[2]!, submission: value };
}

export const submissionKey = (setup: string, session: string, id: string) => recordKey("draft-submission", setup, session, id);

/** Recover an authority address only when the handle, metadata and physical key agree. */
export function submissionReference(saved: unknown, key: string): DataReference | undefined {
  if (!isObject(saved) || typeof saved.session !== "string" || !SESSION.test(saved.session)) return;
  const handle = readSubmissionID(saved.id);
  if (!handle || handle.setup !== saved.setup || handle.origin !== saved.origin ||
      key !== submissionKey(handle.setup, saved.session, handle.submission)) return;
  return { kind: "draftSubmissionEvidence", id: saved.session, computer: handle.origin, setup: handle.setup };
}
