import { isObject } from "../../shared/json";
import { provedRequest } from "../areas/vault";
import { phoneTrustWait } from "../documents/phone-sync";
import { KEY, ownKey, peersOf } from "../peers/known";
import { deviceConnectionsAllowed, pairedPhone } from "../phones";
import type { LinkHost } from "../relay/host";
import { fromLocalApp, type RouteContext } from "../server/context";
import { liveLink, readLink, type LinkState } from "../state";
import { awaySessions } from "../moves/records";
import { mayActivate, ownerOf } from "./resolve";
import { holdsBack, permissionsHere } from "../bots/tell";
import { partsOf } from "../work/policy";
import { call } from "../engine/client";
import { matches } from "../../agents/rules";
import type { Target } from "../../shared/records";

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

type PairedSubmitter = Extract<Submitter, { kind: "paired" }>;

export function pairedSubmitter(context: Pick<RouteContext, "phone">, link: LinkState | null): PairedSubmitter | undefined {
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

/** Live consent stays on the serving connection and at the current session owner. */
export function liveConsentRefusal(context: Pick<RouteContext, "request" | "phone">, session: string) {
  const { request, phone } = context;
  const link = readLink();
  const held = request.consentGrant;
  if (request.fromOwnerPeer || (phone?.key && !pairedSubmitter(context, link))
    || (held && (!deviceConnectionsAllowed(link) || !pairedPhone(link, held.key, held.pairing)))) {
    return { status: 403, value: { message: "This device no longer has access to answer here." } };
  }
  const addressed = request.addressedSsh ? phone?.key?.toString("base64url") : held?.addressedSsh ? held.key : undefined;
  const trust = addressed && phoneTrustWait(link, addressed, liveLink() ?? undefined);
  if (trust) return { status: trust.reason === "pairing-readd" ? 403 : 503, value: { message: trust.message } };
  if (session !== "global") {
    const ref = { kind: "conversations" as const, id: session };
    const owner = ownerOf(ref, { link });
    if ((owner && !owner.local) || !mayActivate(ref, { link }) || awaySessions().includes(session)) {
      return { status: 409, value: { message: "Open the current owner’s live conversation before answering." } };
    }
  }
  return null;
}

const localDenials = () => (permissionsHere() ?? []).filter(holdsBack);

/** An Always reply can release other native asks. Refuse a potentially overlapping grant
 * even when the next covered ask hasn't appeared yet. */
export async function permissionGrantRefusal(target: Target, route: string, id: string, decision: string,
  signal?: AbortSignal, folder?: string, headers?: Record<string, string>) {
  if ((decision !== "once" && decision !== "always") || !localDenials().length) return null;
  const wait = AbortSignal.timeout(5_000);
  const pending = await call(target, "GET", route + "?live=1", undefined, signal ? AbortSignal.any([signal, wait]) : wait, folder, headers);
  if (!Array.isArray(pending)) return { status: 503, value: { message: "This computer couldn’t verify the live permission." } };
  const asks = pending.filter(isObject);
  const ask = asks.find(ask => ask.id === id);
  if (!ask) return { status: 404, value: { message: "It isn’t waiting any more." } };
  const action = typeof ask.action === "string" ? ask.action : "";
  const shell = action === "shell" || action === "bash";
  const edits = ["edit", "write", "patch", "apply_patch", "multiedit"];
  const family = shell ? ["shell", "bash"] : edits.includes(action) ? edits : [action];
  const resources = Array.isArray(ask.resources) ? ask.resources.filter((value): value is string => typeof value === "string") : [];
  const parts = shell ? [...resources, ...resources.flatMap(partsOf)] : resources;
  const rules = localDenials().filter(rule => family.some(each => matches(rule.action, each)));
  if (rules.some(rule => (parts.length ? parts : ["*"]).some(resource => matches(rule.resource, resource)))) {
    return { status: 403, value: { message: "This computer’s permissions no longer allow that action." } };
  }
  if (decision === "always" && rules.length) {
    return { status: 403, value: { message: "An Always answer could override a local restriction for this tool. Use Allow Once for this request." } };
  }
  return null;
}
