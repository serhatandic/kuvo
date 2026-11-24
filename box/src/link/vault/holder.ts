// Which process holds the vault: the box's long-running one (`run` on a machine, `serve`), which
// keeps a machine's store key in memory and is the only one that touches the vault. A `box route`
// (the Mac over SSH, kuvo-browser.js) passes `kuvo/vault…` to it over `run/vault.sock` (only you can
// reach it), one request a connection: first each says hello (a nonce each way, the client's with
// the box's socket key, which a command in the sandbox can't read, and the holder's proof that it
// holds the unlock key), then a line of JSON there, the answer's line back. The Mac's
// unlock key never crosses the socket while the holder can prove it has it: the request carries a
// proof of it for this connection instead, which nothing that reads the socket can use again.

import crypto from "node:crypto";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { supportsLegacyObjectVersion } from "../../shared/format-version";
import { isObject } from "../../shared/json";
import { LOCAL_SETUP_SAVE } from "../../shared/setup-local";
import { LABEL_RESTORE_ROUTE } from "../../shared/routes/work-queue";
import { readSubmitter } from "../ownership/authority";
import { isSocketKey, RUN, socketKey } from "../home";
import type { AnswerPiece, AnyPiece, LinkHandler, LinkRequest } from "../server/context";
import { runningServe } from "../serve-kept";
import { keepSshAgents } from "./ssh";
import { LOCKED, provesUnlock, unlockProof } from "./store";

export const VAULT_SOCKET = path.join(RUN, "vault.sock");

let holding = false;

/** Whether this process holds the vault. */
export const holdsVault = () => holding;

/** Share reads from short-lived SSH route processes with the server that owns the host caches. */
export async function* heldRead(request: LinkRequest, signal?: AbortSignal): AsyncGenerator<AnswerPiece, boolean> {
  if (!request.fromMac || request.served || holdsVault()) return false;
  const reply = await askHolder(request, signal);
  const body = reply && Buffer.from(reply.body, "base64");
  if (reply && body && !(reply.status === 404 && body.toString() === "{}")) {
    yield { head: { status: reply.status, headers: { "content-type": reply.type || "application/json" } } };
    yield { body };
    return true;
  }
  const server = runningServe();
  if (!server) return false;
  const clientVersion = request.headers["x-kuvo-client-version"];
  const result = await fetch(`http://127.0.0.1:${server.port}/${request.path}`, {
    headers: { authorization: `Basic ${Buffer.from("opencode:" + server.password).toString("base64")}`, "x-kuvo-from": "mac",
      ...(request.internal ? { "x-kuvo-internal": "1" } : {}),
      ...(typeof clientVersion === "string" ? { "x-kuvo-client-version": clientVersion } : {}) },
    redirect: "manual", signal,
  }).catch(() => null);
  if (!result || result.status === 401) return false;
  const served = Buffer.from(await result.arrayBuffer());
  yield { head: { status: result.status, headers: { "content-type": result.headers.get("content-type") || "application/json" } } };
  yield { body: served };
  return true;
}

/** Whether a route is one only the holding process answers: the vault's, and the sandbox's asks
 * and its agents' tool (fence/outside.ts), whose asks wait in it, what it stopped (which the
 * engine's plugin tells it), and Also Allowed, which an ask's Always adds to there (one process
 * changes it); a conversation's asks (`kuvo/sessions/<id>/asks…`), which list and answer those
 * asks with the engine's; and a change to a settings page (`kuvo/settings/<section>…`), so each is
 * checked against the page in turn in one process, whichever process it's sent to. The label
 * restore command also needs the active box's signing identity and relay connection. */
export const heldHere = (route: string) => route === LOCAL_SETUP_SAVE || route === LABEL_RESTORE_ROUTE || /^kuvo\/(vault|models\/offered|sandbox\/(asks|tools|allowed|stopped)|sessions\/[^/]+\/asks)(\/|$)|^kuvo\/settings\/[^/]+(?:\/[^/]+\/[^/]+)?$/.test(route.split("?", 1)[0]!);

/** A request as it goes over the socket, and its answer back. */
interface Sent {
  formatVersion?: number;
  method: string;
  path: string;
  headers: Record<string, string>;
  body: string;
  consent?: LinkRequest["consentGrant"];
}
interface Answered {
  formatVersion?: number;
  status: number;
  type: string;
  body: string;
}

const incompatibleHolder: Answered = {
  status: 409, type: "application/json", formatVersion: 1,
  body: Buffer.from(JSON.stringify({ code: "unsupportedFormatVersion", message: "This Vault holder uses an unsupported socket format." })).toString("base64"),
};
const MOST = 8 * 1024 * 1024;
const KEY = "x-kuvo-vault-key";
const AUTH = "x-kuvo-vault-auth";
const nonce = () => crypto.randomBytes(18).toString("base64url");
/** What a client's proof covers: the holder's nonce for this connection, and the request itself. */
const covered = (theirs: string, method: string, route: string, body: string) =>
  `${theirs}\0${method.toUpperCase()}\0${route}\0${crypto.createHash("sha256").update(body).digest("base64url")}`;

/** This process holds the vault from now on, answering the socket with `handle` (as a local request:
 * neither a phone nor this Mac's app, whatever it says). `keepAgents`: what keeps each
 * conversation's SSH agent (a test's keeps none). */
export function holdVault(handle: LinkHandler, keepAgents: () => () => void = keepSshAgents): net.Server {
  holding = true;
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  // Copies a box that ended mid-command left (it was killed): gone.
  try { fs.rmSync(path.join(RUN, "vault-run"), { recursive: true, force: true }); } catch {}
  try { fs.rmSync(VAULT_SOCKET, { force: true }); } catch {}
  // The key made now, before anyone asks (one that can't be made yet is made by the first who does).
  try { socketKey(); } catch {}
  const server = net.createServer(socket => {
    let given = "";
    let hello: string | null = null;
    const mine = nonce();
    const stop = new AbortController();
    // A few seconds to say hello in.
    const unheard = setTimeout(() => { if (hello === null) socket.destroy(); }, 10_000);
    socket.on("close", () => { clearTimeout(unheard); stop.abort(); });
    socket.on("error", () => stop.abort());
    socket.setEncoding("utf8");
    socket.on("data", async (chunk): Promise<void> => {
      given += chunk;
      if (given.length > MOST) return void socket.destroy();
      // Hello first: its nonce, answered with this one's and, while the unlock key is here, the proof of it.
      if (hello === null) {
        const end = given.indexOf("\n");
        if (end < 0) return;
        let said: { hello?: unknown; key?: unknown } | null = null;
        try { said = JSON.parse(given.slice(0, end)); } catch {}
        // Only from Kuvo Code's own processes: nothing at all for one without the key.
        if (!isSocketKey(said?.key)) return void socket.destroy();
        if (!isObject(said) || !supportsLegacyObjectVersion(said))
          return void socket.end(JSON.stringify(incompatibleHolder) + "\n");
        given = given.slice(end + 1);
        hello = typeof said?.hello === "string" ? said.hello.slice(0, 100) : "";
        const proof = hello ? unlockProof("holder", hello) : null;
        socket.write(JSON.stringify({ formatVersion: 1, nonce: mine, ...(proof ? { proof } : {}) }) + "\n");
      }
      const end = given.indexOf("\n");
      if (end < 0) return;
      socket.removeAllListeners("data");
      let sent: Partial<Sent> | null = null;
      try { sent = JSON.parse(given.slice(0, end)); } catch {}
      if (!isObject(sent)) return void socket.destroy();
      if (!supportsLegacyObjectVersion(sent)) return void socket.end(JSON.stringify(incompatibleHolder) + "\n");
      const headers: Record<string, string> = typeof sent.headers === "object" && sent.headers ? { ...sent.headers } : {};
      // A client proves it holds the unlock key for this connection and this request alone.
      const proved = provesUnlock("client", covered(mine, String(sent?.method ?? "GET"), String(sent?.path ?? ""), String(sent?.body ?? "")), headers[AUTH]);
      delete headers[AUTH];
      const consent = sent?.consent && readSubmitter({ ...sent.consent, kind: "paired" });
      if (sent?.consent && consent?.kind !== "paired") return void socket.destroy();
      const request: LinkRequest = {
        method: String(sent?.method ?? "GET"),
        path: String(sent?.path ?? ""),
        headers,
        body: Buffer.from(String(sent?.body ?? ""), "base64"),
        fromMac: true,
        fromHolder: true,
        ...(consent?.kind === "paired" ? { consentGrant: { ...consent, ...(sent?.consent?.addressedSsh === true ? { addressedSsh: true as const } : {}) } } : {}),
        ...(proved ? { vaultClient: true } : {}),
      };
      // Only the routes of what this process alone keeps answer here: the vault's, and the sandbox's
      // asks and its agents' tool, which wait in it.
      if (!heldHere(request.path)) return void socket.end(JSON.stringify({ status: 404, type: "application/json", body: Buffer.from("{}").toString("base64") }) + "\n");
      const reply = request.method === "POST" && /^kuvo\/sessions\/[^/]+\/asks\//.test(request.path) ? new AbortController() : null;
      if (reply) socket.once("close", () => reply.abort());
      const signal = reply ? AbortSignal.any([stop.signal, reply.signal]) : stop.signal;
      let status = 502;
      let type = "";
      const chunks: Buffer[] = [];
      try {
        for await (const part of handle(request, undefined, signal) as AsyncIterable<AnyPiece>) {
          if (part.head) {
            status = part.head.status;
            type = String(part.head.headers?.["content-type"] ?? "");
          }
          if (part.body) chunks.push(Buffer.from(part.body));
          // A stream's first line is what it says now.
          if (type.startsWith("application/x-ndjson") && Buffer.concat(chunks).includes(10)) {
            stop.abort();
            break;
          }
        }
      } catch (error) {
        status = 500;
        type = "application/json";
        chunks.splice(0, chunks.length, Buffer.from(JSON.stringify({ message: String((error as Error)?.message ?? error) })));
      }
      let body = Buffer.concat(chunks);
      if (type.startsWith("application/x-ndjson") && body.includes(10)) body = body.subarray(0, body.indexOf(10) + 1);
      if (!socket.destroyed) socket.end(JSON.stringify({ formatVersion: 1, status, type, body: body.toString("base64") } satisfies Answered) + "\n");
    });
  });
  server.once("listening", () => {
    // The resident exits explicitly after draining. Remove only the socket this listener bound.
    let owned: fs.BigIntStats;
    try { owned = fs.lstatSync(VAULT_SOCKET, { bigint: true }); } catch { return; }
    if (!owned.isSocket()) return;
    const withdraw = () => {
      try {
        const current = fs.lstatSync(VAULT_SOCKET, { bigint: true });
        if (current.isSocket() && current.dev === owned.dev && current.ino === owned.ino) {
          fs.unlinkSync(VAULT_SOCKET);
        }
      } catch {}
    };
    process.once("exit", withdraw);
    server.once("close", () => process.off("exit", withdraw));
  });
  const old = process.umask(0o177);
  try {
    server.listen(VAULT_SOCKET);
  } finally {
    process.umask(old);
  }
  // Without its socket, `box route` (the Mac over SSH, the browser here) can't reach the vault: said where it's seen.
  server.on("error", error => console.error(`The vault's socket (${VAULT_SOCKET}) couldn't listen: ${error.message}`));
  server.unref();
  // Each conversation's SSH agent, from the vault's SSH keys, while its commands are in the sandbox.
  const stopAgents = keepAgents();
  server.on("close", stopAgents);
  // (Closed, it holds nothing: what a test's holder does as it ends.)
  server.on("close", () => { holding = false; });
  return server;
}

/** Whether the socket is one only this user could have made, as the holder makes it. */
function socketLooksHeld(): boolean {
  try {
    const stat = fs.lstatSync(VAULT_SOCKET);
    return stat.isSocket() && (process.getuid ? stat.uid === process.getuid() : true) && (stat.mode & 0o077) === 0;
  } catch {
    return false;
  }
}

/** Why a request wasn't passed on: the process at the socket couldn't prove it holds the vault. */
export const NOT_THE_HOLDER = "That isn't this vault's key, or what answers for it couldn't prove it's Kuvo Code's own: nothing was sent to it.";

/** `request` asked of the process that holds the vault; null when none does (nothing listens). A
 * request with the unlock key sends a proof of it instead, once the holder proved it has it too;
 * to one that can't (it's locked), the key itself only to unlock it (`kuvo/vault/unlock`, which a
 * client sends when you ask): anything else is told it's locked, and nothing is sent. */
export function askHolder(request: LinkRequest, signal?: AbortSignal, consent?: LinkRequest["consentGrant"]): Promise<Answered | null> {
  return new Promise(resolve => {
    if (!socketLooksHeld()) return resolve(null);
    // Only Kuvo Code's own processes can read the key; without it nothing answers.
    let key: string;
    try { key = socketKey({ create: request.method !== "GET" || request.path !== LABEL_RESTORE_ROUTE }); } catch { return resolve(null); }
    const socket = net.createConnection(VAULT_SOCKET);
    const ours = nonce();
    let theirs: { nonce?: unknown; proof?: unknown } | null = null;
    let answer = "";
    let done = false;
    const finish = (value: Answered | null) => {
      if (done) return;
      done = true;
      socket.destroy();
      resolve(value);
    };
    signal?.addEventListener("abort", () => finish(null), { once: true });
    socket.setEncoding("utf8");
    socket.on("connect", () => socket.write(JSON.stringify({ formatVersion: 1, hello: ours, key }) + "\n"));
    const body = Buffer.from(request.body ?? Buffer.alloc(0)).toString("base64");
    const send = () => {
      const headers: Record<string, string> = {};
      for (const [name, value] of Object.entries(request.headers ?? {})) if (typeof value === "string" && name !== AUTH) headers[name] = value;
      const key = headers[KEY];
      if (key && typeof theirs?.proof !== "string" && !/^kuvo\/vault\/unlock(\?|$)/.test(request.path)) {
        finish({ status: 423, type: "application/json", body: Buffer.from(JSON.stringify({ message: LOCKED })).toString("base64") });
        return;
      }
      if (key) {
        const unlock = Buffer.from(key, "base64url");
        if (typeof theirs?.proof === "string") {
          // It holds the key: proved, and the key stays here.
          // It doesn't: the key isn't this vault's any more (made again), or what answers isn't the
          // holder. Either way nothing is sent, and the client hears its key isn't this vault's.
          if (theirs.proof !== unlockProof("holder", ours, unlock)) {
            finish({ status: 403, type: "application/json", body: Buffer.from(JSON.stringify({ message: NOT_THE_HOLDER })).toString("base64") });
            return;
          }
          delete headers[KEY];
          headers[AUTH] = unlockProof("client", covered(String(theirs.nonce ?? ""), request.method, request.path, body), unlock)!;
        }
      }
      socket.write(JSON.stringify({ formatVersion: 1, method: request.method, path: request.path, headers, body, consent } satisfies Sent) + "\n");
    };
    socket.on("data", chunk => {
      answer += chunk;
      if (answer.length > MOST * 2) return finish(null);
      let end = answer.indexOf("\n");
      if (end < 0) return;
      if (!theirs) {
        try {
          const hello: unknown = JSON.parse(answer.slice(0, end));
          if (!isObject(hello) || !supportsLegacyObjectVersion(hello) || typeof hello.nonce !== "string") return finish(incompatibleHolder);
          theirs = hello;
        } catch { return finish(incompatibleHolder); }
        answer = answer.slice(end + 1);
        send();
        end = answer.indexOf("\n");
        if (end < 0) return;
      }
      try {
        const reply: unknown = JSON.parse(answer.slice(0, end));
        if (!isObject(reply) || !supportsLegacyObjectVersion(reply) || typeof reply.status !== "number"
          || typeof reply.type !== "string" || typeof reply.body !== "string") return finish(incompatibleHolder);
        finish({ status: reply.status, type: reply.type, body: reply.body });
      } catch { finish(incompatibleHolder); }
    });
    socket.on("error", () => finish(null));
    socket.on("close", () => finish(null));
  });
}
