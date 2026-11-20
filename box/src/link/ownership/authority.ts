import { isObject } from "../../shared/json";
import { provedRequest } from "../areas/vault";
import { phoneTrustWait } from "../documents/phone-sync";
import { KEY, ownKey, peersOf } from "../peers/known";
import { deviceConnectionsAllowed, pairedPhone } from "../phones";
import type { LinkHost } from "../relay/host";
import { fromLocalApp, type RouteContext } from "../server/context";
import type { LinkState } from "../state";

export const SUBMITTER_FEATURE = "data.queued-device-authority";
export const SUBMITTER_REVOKED = "The submitting device no longer has access. Check the record before sending again.";
export const SUBMITTER_UNKNOWN = "This work predates device tracking. Check the record before submitting it again.";

export type Submitter =
  | { kind: "computer" }
  | { kind: "paired"; key: string; pairing: string }
  | { kind: "ssh"; key: string };

export function readSubmitter(value: unknown): Submitter | undefined {
  if (!isObject(value)) return;
  if (value.kind === "computer") return { kind: value.kind };
  if (typeof value.key !== "string" || !KEY.test(value.key)) return;
  if (value.kind === "ssh") return { kind: value.kind, key: value.key };
  if (value.kind === "paired" && typeof value.pairing === "string" && /^[A-Za-z0-9_-]{0,32}$/.test(value.pairing))
    return { kind: value.kind, key: value.key, pairing: value.pairing };
  return undefined;
}

export function pairedSubmitter(context: RouteContext, link: LinkState | null): Submitter | undefined {
  if (!context.phone?.key) return;
  const key = context.phone.key.toString("base64url");
  const pairing = context.phone.pairing ?? "";
  if (deviceConnectionsAllowed(link) && pairedPhone(link, key, pairing)) return { kind: "paired", key, pairing };
  return undefined;
}

/** Legacy ordinary SSH edits require the request proof. The local shell already has filesystem
 * authority for its existing conversation routes. Addressed SSH supplies a paired identity. */
export function submitterOf(context: RouteContext, link: LinkState | null, legacyShell = false): Submitter | undefined {
  const { request, phone } = context;
  if (phone?.key) return pairedSubmitter(context, link);
  if (fromLocalApp(request) || request.fromApp) return { kind: "computer" };
  if (request.fromMac) {
    const key = provedRequest(request);
    if (key) return { kind: "ssh", key };
    if (legacyShell && !request.served && !request.headers["x-kuvo-vault-proof"]) return { kind: "computer" };
  }
  return undefined;
}

export function submitterAuthority(link: LinkState | null, submitter: Submitter | undefined, host?: Pick<LinkHost, "send"> | null) {
  if (!submitter) return "unknown";
  if (submitter.kind === "computer") return "allowed";
  if (submitter.kind === "ssh") return link && (ownKey(link) === submitter.key || peersOf(link).some(peer => peer.key === submitter.key)) ? "allowed" : "revoked";
  if (!deviceConnectionsAllowed(link) || !pairedPhone(link, submitter.key, submitter.pairing)) return "revoked";
  const trust = phoneTrustWait(link, submitter.key, host, { queued: true });
  return trust?.reason === "pairing-readd" ? "revoked" : trust ? "checking" : "allowed";
}
