import { clientTransportsLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { IKResponder } from "../crypto/noise";
import { IS_MAC, K } from "../home";
import { keptDeviceConnections, macKeys, ownMacConfig, savedMacConfig } from "../mac/link";
import { KEY, ownKey } from "../peers/known";
import { deviceConnectionsAllowed, pairedPhone } from "../phones";
import { macLink, readLink } from "../state";
import { answerOnce } from "../once";
import { ROUTE_VERSION, ROUTE_VERSION_HEADER } from "../../shared/route-version";
import { routeRequestHeaders } from "./format";
import type { Asker, LinkHandler } from "./context";
import type { Writable } from "node:stream";

const PROLOGUE = Buffer.from("kuvo-ssh v1");
type Opening = { computer: string; given: Record<string, unknown>; actor: Asker; authorized: () => boolean } | { status: number; message: string };

/** The same paired device identity as on the relay, bound to this connection and this box. */
export async function acceptSshRoute(read: () => Promise<Record<string, unknown>>, say: (value: unknown) => Promise<void>): Promise<Opening> {
  const opening = await read();
  const computer = opening?.computer;
  if (typeof computer !== "string" || !KEY.test(computer) || typeof opening.hello !== "string" || opening.hello.length > 4000) {
    return { status: 400, message: "Send the computer’s public key and a device handshake." };
  }
  if (!clientTransportsLayoutReady(K)) return { status: 503, message: "Wait for the Kuvo Code update to finish before connecting over SSH." };
  if (IS_MAC) {
    macLink.config ??= savedMacConfig() ?? ownMacConfig();
    if (!keptDeviceConnections()) return { status: 403, message: "“Let my devices connect” is off on this Mac." };
    if (!await macKeys({ needed: true })) return { status: 503, message: "Kuvo couldn’t verify this Mac’s identity." };
  }
  const here = () => {
    try {
      const link = readLink();
      return link?.key && ownKey(link) === computer && deviceConnectionsAllowed(link, !IS_MAC || keptDeviceConnections()) ? link : null;
    } catch { return null; }
  };
  const box = here();
  if (!box?.key) return { status: 409, message: "SSH reached a different computer or one being removed." };
  try {
    const responder = new IKResponder(Buffer.from(box.key, "base64url"), PROLOGUE);
    const hello: unknown = JSON.parse(responder.readMessage1(Buffer.from(opening.hello, "base64url")).toString("utf8"));
    if (!isObject(hello) || hello.v !== 1) return { status: 400, message: "The SSH handshake is not supported." };
    // SSH already proved access to the OS account. The application's current pairing still
    // has to be present here, so the command can't stand in for a paired client by itself.
    const key = responder.rs.toString("base64url");
    const pairing = pairedPhone(box, key);
    const { message, transport } = responder.writeMessage2(Buffer.from(JSON.stringify({ allowed: !!pairing })));
    await say({ hello: message.toString("base64url") });
    if (!pairing) return { status: 403, message: "Pair this device before reaching this computer over SSH." };
    const sealed = (await read())?.request;
    if (typeof sealed !== "string") return { status: 400, message: "Send the authenticated SSH request." };
    const given: unknown = JSON.parse(transport.open(Buffer.from(sealed, "base64url")).toString("utf8"));
    if (!isObject(given)) return { status: 400, message: "Send a routed request." };
    const currentPairing = () => pairedPhone(here(), key, pairing.pairing ?? "");
    const current = currentPairing();
    if (!current) return { status: 403, message: "This device’s pairing changed. Connect again." };
    return { computer, given, authorized: () => !!currentPairing(),
      actor: { key: responder.rs, pairing: current.pairing, name: current.name, ...(hello.client === "desktop" ? { kind: "desktop" } : {}) } };
  } catch {
    return { status: 403, message: "Kuvo couldn’t authenticate the SSH request." };
  }
}

export interface SshCommand {
  addressed: boolean;
  internal: boolean;
  internalPath?: string;
}

export function writeSshBytes(to: Writable, bytes: Buffer): Promise<void> {
  return new Promise((resolve, reject) => to.write(bytes, error => error ? reject(error) : resolve()));
}

export const sshLine = (value: unknown): Buffer => Buffer.from(JSON.stringify(value) + "\n");

function sshInput(source: AsyncIterable<Buffer>) {
  const chunks = source[Symbol.asyncIterator]();
  let rest: Buffer | undefined;
  const take = async () => {
    if (rest) {
      const bytes = rest;
      rest = undefined;
      return bytes;
    }
    const part = await chunks.next();
    return part.done ? undefined : part.value;
  };
  return {
    async line(): Promise<Record<string, unknown>> {
      const parts: Buffer[] = [];
      for (let bytes; (bytes = await take());) {
        const end = bytes.indexOf(10);
        if (end === -1) {
          parts.push(bytes);
          continue;
        }
        parts.push(bytes.subarray(0, end));
        if (end + 1 < bytes.length) rest = bytes.subarray(end + 1);
        try {
          const value: unknown = JSON.parse(Buffer.concat(parts).toString("utf8"));
          return isObject(value) ? value : {};
        } catch { return {}; }
      }
      return {};
    },
    async all(): Promise<unknown> {
      const parts: Buffer[] = [];
      for (let bytes; (bytes = await take());) parts.push(bytes);
      return JSON.parse(Buffer.concat(parts).toString("utf8"));
    },
    async drain(): Promise<void> {
      while (await take()) { /* Only EOF matters once a streaming request has arrived. */ }
    },
  };
}

function isUTF8(bytes: Buffer): boolean {
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return true;
  } catch { return false; }
}

/** Both SSH verbs use this exchange, whether it is read over stdio or the resident's socket. */
export async function answerSshRoute({ addressed, internal, internalPath }: SshCommand, source: AsyncIterable<Buffer>,
  write: (bytes: Buffer) => Promise<void>, signal: AbortSignal, handle: LinkHandler, transient = false): Promise<void> {
  const read = sshInput(source);
  const say = (value: unknown) => write(sshLine(value));
  let given: Record<string, unknown> = {};
  let actor: Asker = {};
  let computer: string | undefined;
  let authorized: (() => boolean) | undefined;
  if (addressed) {
    const opened = await acceptSshRoute(read.line, say);
    if ("status" in opened) {
      const { status, ...refusal } = opened;
      return say({ status, routeVersion: ROUTE_VERSION, body: JSON.stringify(refusal) });
    }
    ({ given, actor, computer, authorized } = opened);
  } else if (internal && internalPath) given = { path: internalPath };
  else {
    try {
      const value = await read.all();
      if (isObject(value)) given = value;
    } catch {}
  }

  const stream = addressed && given.stream === true;
  if (transient && !stream) answerOnce();
  const { headers: givenHeaders, error } = routeRequestHeaders(given.headers);
  if (error) {
    const head = { status: 400, routeVersion: ROUTE_VERSION, ...(computer ? { computer, type: "application/json" } : {}) };
    if (!stream) return say({ ...head, body: JSON.stringify(error) });
    await say(head);
    return write(Buffer.from(JSON.stringify(error)));
  }
  const headers = addressed ? givenHeaders : { "content-type": "application/json" } as typeof givenHeaders;
  if (!addressed) {
    for (const name of ["x-opencode-directory", "x-kuvo-sign-in-client", "x-kuvo-vault-key", "x-kuvo-vault-proof", "x-kuvo-client-version", "x-kuvo-client-id", ROUTE_VERSION_HEADER]) {
      const value = givenHeaders[name];
      if (typeof value === "string") headers[name] = value;
    }
  }
  const sentBody = typeof given.bodyText === "string" ? given.bodyText : given.body === undefined ? "" : JSON.stringify(given.body);
  const body = addressed && typeof given.bodyBase64 === "string" ? Buffer.from(given.bodyBase64, "base64") : Buffer.from(sentBody);
  const request = { method: String(given.method ?? "GET"), path: String(given.path ?? ""), headers, body,
    fromMac: !addressed, ...(addressed ? { addressedSsh: true as const } : {}),
    ...(!transient ? { residentSsh: true as const } : {}), internal };

  let status = 502;
  let type = "";
  let routeVersion: string | undefined;
  const chunks: Buffer[] = [];
  const stop = new AbortController();
  const running = AbortSignal.any([signal, stop.signal]);
  const paired = authorized && setInterval(() => { if (!authorized()) stop.abort(); }, 2000);
  // A finite request can end its input while still waiting for an answer.
  if (stream) void read.drain().then(() => stop.abort(), () => stop.abort());
  else if (!transient) void read.drain().catch(() => stop.abort());
  try {
    if (running.aborted) return;
    for await (const part of handle(request, actor, running)) {
      if (running.aborted) return;
      if ("head" in part) {
        status = part.head.status;
        type = part.head.headers["content-type"] ?? "";
        routeVersion = part.head.headers[ROUTE_VERSION_HEADER];
        if (stream) await say({ status, routeVersion, computer, type });
      } else if (stream) {
        await write(part.body);
      } else {
        chunks.push(part.body);
        if (type.startsWith("application/x-ndjson") && part.body.includes(10)) {
          const all = Buffer.concat(chunks);
          stop.abort();
          return say({ status, routeVersion, body: all.subarray(0, all.indexOf(10) + 1).toString("utf8"), ...(computer ? { computer, type } : {}) });
        }
      }
    }
    if (stream || running.aborted) return;
    const reply = Buffer.concat(chunks);
    const text = (!type || type.startsWith("text/") || type.includes("json")) && isUTF8(reply);
    const route = { status, ...(internalPath && internal ? { internal: true } : {}), routeVersion, ...(computer ? { computer, type } : {}) };
    await say(text ? { ...route, body: reply.toString("utf8") } : { ...route, bodyBase64: reply.toString("base64"), type });
  } catch (error) {
    if (!running.aborted) throw error;
  } finally {
    clearInterval(paired);
  }
}
