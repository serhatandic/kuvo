import { MUTATION_DECLARATIONS } from "../../shared/mutation-declarations";
import { isObject } from "../../shared/json";
import { provedRequest } from "../areas/vault";
import { phoneTrustWait } from "../documents/phone-sync";
import { KEY, ownKey, peersOf } from "../peers/known";
import { deviceConnectionsAllowed, pairedPhone } from "../phones";
import type { LinkHost } from "../relay/host";
import { fromLocalApp, type RouteContext } from "../server/context";
import { liveLink, readLink, type LinkState } from "../state";
import { awaySessions } from "../moves/records";
import { holdsBack, permissionsHere } from "../bots/tell";
import { partsOf } from "../work/policy";
import { call } from "../engine/client";
import { matches } from "../../agents/rules";
import type { Target } from "../../shared/records";
import { performsMutation } from "./admission";

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
export function submitterOf(context: Pick<RouteContext, "request" | "phone">, link: LinkState | null, legacyShell = false): Submitter | undefined {
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

export function mutationSubmitter(lane: "ownerWrite" | "queuedSessionWork", link: LinkState | null, submitter: Submitter | undefined,
  host?: Pick<LinkHost, "send"> | null, live = false) {
  if (MUTATION_DECLARATIONS[lane].propose !== "current submitter") return "unknown";
  // The live Relay/addressed-SSH handshake has checked trust. Queued work must refresh it.
  if (live && submitter?.kind === "paired")
    return deviceConnectionsAllowed(link) && pairedPhone(link, submitter.key, submitter.pairing) ? "allowed" : "revoked";
  return submitterAuthority(link, submitter, host);
}

type LiveContext = Pick<RouteContext, "request" | "phone"> & Partial<Pick<RouteContext, "admitQueued">>;
export function relayedConsent({ request, phone }: LiveContext): RouteContext["request"]["consentGrant"] {
  return phone?.key
    ? { key: phone.key.toString("base64url"), pairing: phone.pairing ?? "", addressedSsh: request.addressedSsh }
    : undefined;
}

type Refusal = { status: number; value: { message: string }; notAdmitted?: true };

export function liveMutationRefusal(lane: "command" | "consent", { request, phone, admitQueued }: LiveContext, ...sessions: string[]): Refusal | null {
  const link = readLink();
  const held = request.consentGrant;
  const source = MUTATION_DECLARATIONS[lane].propose;
  const forwarded = source === "live client or admitted draft" && admitQueued;
  // The loopback server constructs served only after checking its own password.
  const local = request.served || request.fromHolder || request.internal || submitterOf({ request, phone }, link, true)?.kind === "computer";
  const ssh = !phone?.key && request.fromMac && submitterAuthority(link, submitterOf({ request, phone }, link)) === "allowed";
  if ((request.fromOwnerPeer && !forwarded) || (phone?.key && !pairedSubmitter({ phone }, link))
    || (held && (!deviceConnectionsAllowed(link) || !pairedPhone(link, held.key, held.pairing)))
    || (!phone?.key && !held && !local && !ssh && !forwarded)) {
    return { status: 403, value: { message: "This device no longer has access to act here." }, notAdmitted: true };
  }
  const addressed = request.addressedSsh ? phone?.key?.toString("base64url") : held?.addressedSsh ? held.key : undefined;
  const trust = addressed && phoneTrustWait(link, addressed, liveLink() ?? undefined);
  if (trust) return { status: trust.reason === "pairing-readd" ? 403 : 503, value: { message: trust.message }, notAdmitted: true };
  for (const id of sessions) {
    if (id === "global") continue;
    const ref = { kind: "conversations" as const, id };
    if (!performsMutation(lane, ref, undefined, link) || awaySessions().includes(id)) {
      return { status: 409, value: { message: "Open the current owner’s live conversation before acting." }, notAdmitted: true };
    }
  }
  return null;
}

/** Both reply surfaces validate the same live grant at the native request boundary. The
 * backend must also consume the pending ask atomically, since it can expire after a GET. */
export async function dispatchConsent<T>(context: LiveContext, sessions: string[], signal: AbortSignal | undefined,
  permission: { target: Target; route: string; id: string; decision: string; folder?: string; headers?: Record<string, string> } | undefined,
  send: () => Promise<T>): Promise<{ refusal: Refusal } | { result: T }> {
  signal?.throwIfAborted();
  const refusal = liveMutationRefusal("consent", context, ...sessions);
  if (refusal) return { refusal };
  if (permission) {
    const { target, route, id, decision, folder, headers } = permission;
    const veto = await permissionGrantRefusal(target, route, id, decision, signal, folder, headers);
    if (veto) return { refusal: veto };
    signal?.throwIfAborted();
    const changed = liveMutationRefusal("consent", context, ...sessions);
    if (changed) return { refusal: changed };
  }
  return { result: await send() };
}

const localDenials = () => (permissionsHere() ?? []).filter(holdsBack);

/** An Always reply can release other native asks. Refuse a potentially overlapping grant
 * even when the next covered ask hasn't appeared yet. */
async function permissionGrantRefusal(target: Target, route: string, id: string, decision: string,
  signal?: AbortSignal, folder?: string, headers?: Record<string, string>) {
  if (decision !== "once" && decision !== "always") return null;
  const wait = AbortSignal.timeout(5_000);
  const pending = await call(target, "GET", route + "?live=1", undefined, signal ? AbortSignal.any([signal, wait]) : wait, folder, headers).catch(() => null);
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
  const rules = (MUTATION_DECLARATIONS.consent.tighten === "executing computer" ? localDenials() : []).filter(rule => family.some(each => matches(rule.action, each)));
  if (rules.some(rule => (parts.length ? parts : ["*"]).some(resource => matches(rule.resource, resource)))) {
    return { status: 403, value: { message: "This computer’s permissions no longer allow that action." } };
  }
  if (decision === "always" && rules.length) {
    return { status: 403, value: { message: "An Always answer could override a local restriction for this tool. Use Allow Once for this request." } };
  }
  return null;
}

export async function sandboxProposer(session: string | undefined, calling: () => Promise<boolean>): Promise<boolean> {
  return MUTATION_DECLARATIONS.sandboxCommand.propose === "current session tool" && Boolean(session)
    && await calling() && performsMutation("sandboxCommand", { kind: "conversations", id: session });
}

/** Re-read the executing computer's Block rules at the point a held sandbox command would spawn. */
export function sandboxCommandAllowed(sessions: string[], command: string): boolean {
  if (sessions.some(id => !performsMutation("sandboxCommand", { kind: "conversations", id }) || awaySessions().includes(id))) return false;
  if (MUTATION_DECLARATIONS.sandboxCommand.tighten !== "executing computer") return true;
  const parts = [command, ...partsOf(command)];
  return !localDenials().some(rule => (matches(rule.action, "shell") || matches(rule.action, "bash"))
    && parts.some(part => matches(rule.resource, part)));
}
