import { supportsLegacyObjectVersion } from "../../shared/format-version";
import { isObject, parsed } from "../../shared/json";
import { readSubmitter, submitterOf, type Submitter } from "../ownership/authority";
import { MUTATION_ID, mutationNonce } from "../ownership/mutation";
import { KEY } from "../peers/known";
import type { RouteContext } from "../server/context";
import { followedSetup } from "../setup/record";
import { readLink, type LinkState } from "../state";

export const SCOPED_FORWARD = "data.scoped-forward";
export interface ForwardGrant { setup: string; submitter: Submitter; operation?: string }

/** Call only for client-originated operations. Worker, installer and agent routes have separate grants. */
export function forwardGrant(context: RouteContext, link = readLink()): ForwardGrant | null | undefined {
  const setup = followedSetup(link);
  // Preserve OS-local and old SSH entry points. Addressed SSH has a paired actor like Relay.
  if (!context.phone?.key) return undefined;
  const submitter = submitterOf(context, link);
  const nonce = mutationNonce(context.request, parsed(context.request.body));
  if (nonce === null) return null;
  const operation = nonce ?? context.request.headers["idempotency-key"];
  if (!setup || !submitter || (operation !== undefined && (typeof operation !== "string" || !MUTATION_ID.test(operation)))) return null;
  return { setup: setup.id, submitter, operation };
}

export function readForwardGrant(value: unknown, link: LinkState | null): (ForwardGrant & { operation: string }) | undefined {
  if (!isObject(value) || !supportsLegacyObjectVersion(value) || typeof value.setup !== "string" || followedSetup(link)?.id !== value.setup
    || typeof value.operation !== "string" || !MUTATION_ID.test(value.operation)) return undefined;
  const submitter = readSubmitter(value.submitter);
  if (!submitter || submitter.kind === "computer" || !KEY.test(submitter.key)) return undefined;
  return { setup: value.setup, operation: value.operation, submitter };
}
