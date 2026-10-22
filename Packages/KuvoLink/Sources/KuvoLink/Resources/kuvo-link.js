// Kuvo Code's link on a machine: lets the phones you've paired reach this machine's engine
// through the relay, so its sessions and bots work from the phone with the Mac closed. The
// engine's own runtime runs it, so the machine needs nothing else:
//
//   BUN_BE_BUN=1 opencode kuvo-link.js run             the bridge, started beside the engine, which
//                                                      also tells your phones what happens in the
//                                                      machine's sessions while no Mac does
//   BUN_BE_BUN=1 opencode kuvo-link.js setup           the Mac's phones and relay, as JSON on stdin
//   BUN_BE_BUN=1 opencode kuvo-link.js pair [relay]    a link to pair a phone with this machine
//   BUN_BE_BUN=1 opencode kuvo-link.js selftest        checks the runtime has what it needs
//
// It keeps its state in ~/.kuvo-code/run/link.json (mode 600), reads the engine's port and
// password from run/engine.json, and connects out to the relay; it never listens. The protocol
// is in PROTOCOL.md beside this file's source (kuvo-ai's Packages/KuvoLink), with the same in
// Swift for the Mac and the phone.

import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

// Goes up with every change, so `setup` replaces a running bridge that's older.
export const BRIDGE_VERSION = "0.9.8";
const K = process.env.KUVO_HOME || path.join(os.homedir(), ".kuvo-code");
const RUN = path.join(K, "run");
export const LINK_FILE = path.join(RUN, "link.json");
const ENGINE_FILE = path.join(RUN, "engine.json");
// The coding agents' bridge beside the engine (kuvo-agents.js): its port and password.
const BRIDGE_FILE = path.join(RUN, "bridge.json");
const PID_FILE = path.join(RUN, "link.pid");
// While it's there, the box's keeper keeps this bridge running (`bin/kuvo-start keep`).
const NEEDED_FILE = path.join(RUN, "link.needed");
const VERSION_FILE = path.join(RUN, "link.version");
const PAIRING_MINUTES = 10;

// ---- Keys: X25519 keys are kept as their raw 32 bytes ----

const PKCS8 = Buffer.from("302e020100300506032b656e04220420", "hex");
const SPKI = Buffer.from("302a300506032b656e032100", "hex");
const EMPTY = Buffer.alloc(0);

const privateKey = raw => crypto.createPrivateKey({ key: Buffer.concat([PKCS8, raw]), format: "der", type: "pkcs8" });
const publicKey = raw => crypto.createPublicKey({ key: Buffer.concat([SPKI, raw]), format: "der", type: "spki" });
export const newKey = () => crypto.generateKeyPairSync("x25519").privateKey.export({ format: "der", type: "pkcs8" }).subarray(PKCS8.length);
export const publicOf = raw => crypto.createPublicKey(privateKey(raw)).export({ format: "der", type: "spki" }).subarray(SPKI.length);
const dh = (raw, remote) => crypto.diffieHellman({ privateKey: privateKey(raw), publicKey: publicKey(remote) });
export const addressOf = secret => crypto.createHash("sha256").update("kuvo-link address v1").update(secret).digest().subarray(0, 16);

// ---- Noise: IK for sessions, K for mail, with X25519, AES-GCM and SHA-256 ----

const sha256 = data => crypto.createHash("sha256").update(data).digest();
const hmac = (key, data) => crypto.createHmac("sha256", key).update(data).digest();
function hkdf(chainingKey, input) {
  const temp = hmac(chainingKey, input);
  const first = hmac(temp, Buffer.from([1]));
  return [first, hmac(temp, Buffer.concat([first, Buffer.from([2])]))];
}

export class CipherState {
  constructor(key) {
    this.key = key;
    this.n = 0n;
  }

  nonce() {
    const nonce = Buffer.alloc(12);
    nonce.writeBigUInt64BE(this.n, 4);
    return nonce;
  }

  encrypt(ad, plaintext) {
    if (!this.key) return plaintext;
    const cipher = crypto.createCipheriv("aes-256-gcm", this.key, this.nonce());
    cipher.setAAD(ad);
    const sealed = Buffer.concat([cipher.update(plaintext), cipher.final(), cipher.getAuthTag()]);
    this.n += 1n;
    return sealed;
  }

  decrypt(ad, ciphertext) {
    if (!this.key) return ciphertext;
    if (ciphertext.length < 16) throw new Error("decrypt");
    const decipher = crypto.createDecipheriv("aes-256-gcm", this.key, this.nonce());
    decipher.setAAD(ad);
    decipher.setAuthTag(ciphertext.subarray(-16));
    let plaintext;
    try {
      plaintext = Buffer.concat([decipher.update(ciphertext.subarray(0, -16)), decipher.final()]);
    } catch {
      throw new Error("decrypt");
    }
    this.n += 1n;
    return plaintext;
  }
}

class SymmetricState {
  constructor(name, prologue) {
    const bytes = Buffer.from(name);
    this.h = bytes.length <= 32 ? Buffer.concat([bytes, Buffer.alloc(32 - bytes.length)]) : sha256(bytes);
    this.ck = this.h;
    this.cipher = new CipherState();
    this.mixHash(prologue);
  }

  mixKey(input) {
    const [chainingKey, key] = hkdf(this.ck, input);
    this.ck = chainingKey;
    this.cipher = new CipherState(key);
  }

  mixHash(data) {
    this.h = sha256(Buffer.concat([this.h, data]));
  }

  encryptAndHash(plaintext) {
    const ciphertext = this.cipher.encrypt(this.h, plaintext);
    this.mixHash(ciphertext);
    return ciphertext;
  }

  decryptAndHash(ciphertext) {
    const plaintext = this.cipher.decrypt(this.h, ciphertext);
    this.mixHash(ciphertext);
    return plaintext;
  }

  split() {
    const [first, second] = hkdf(this.ck, EMPTY);
    return [new CipherState(first), new CipherState(second)];
  }
}

const IK = "Noise_IK_25519_AESGCM_SHA256";
const ONE_WAY = "Noise_K_25519_AESGCM_SHA256";
export const PROLOGUE = Buffer.from("kuvo-link v1");
export const MAIL_PROLOGUE = Buffer.from("kuvo-link mail v1");

const transport = (sending, receiving, hash) => ({
  hash,
  seal: plaintext => sending.encrypt(EMPTY, plaintext),
  open: ciphertext => receiving.decrypt(EMPTY, ciphertext),
});

// The side that starts (the phone), knowing the other's static key. Here for tests.
export class IKInitiator {
  constructor(s, rs, prologue, e = newKey()) {
    Object.assign(this, { s, rs, e, state: new SymmetricState(IK, prologue) });
    this.state.mixHash(rs);
  }

  writeMessage1(payload) {
    const state = this.state;
    const ephemeral = publicOf(this.e);
    state.mixHash(ephemeral);
    state.mixKey(dh(this.e, this.rs));
    const s = state.encryptAndHash(publicOf(this.s));
    state.mixKey(dh(this.s, this.rs));
    return Buffer.concat([ephemeral, s, state.encryptAndHash(payload)]);
  }

  readMessage2(message) {
    if (message.length < 48) throw new Error("malformed");
    const state = this.state;
    const re = message.subarray(0, 32);
    state.mixHash(re);
    state.mixKey(dh(this.e, re));
    state.mixKey(dh(this.s, re));
    const payload = state.decryptAndHash(message.subarray(32));
    const [first, second] = state.split();
    return { payload, transport: transport(first, second, state.h) };
  }
}

// The side that answers (this machine), learning the phone's static key.
export class IKResponder {
  constructor(s, prologue, e = newKey()) {
    Object.assign(this, { s, e, state: new SymmetricState(IK, prologue) });
    this.state.mixHash(publicOf(s));
  }

  readMessage1(message) {
    if (message.length < 96) throw new Error("malformed");
    const state = this.state;
    this.re = message.subarray(0, 32);
    state.mixHash(this.re);
    state.mixKey(dh(this.s, this.re));
    this.rs = state.decryptAndHash(message.subarray(32, 80));
    state.mixKey(dh(this.s, this.rs));
    return state.decryptAndHash(message.subarray(80));
  }

  writeMessage2(payload) {
    const state = this.state;
    const ephemeral = publicOf(this.e);
    state.mixHash(ephemeral);
    state.mixKey(dh(this.e, this.re));
    state.mixKey(dh(this.e, this.rs));
    const message = Buffer.concat([ephemeral, state.encryptAndHash(payload)]);
    const [first, second] = state.split();
    return { message, transport: transport(second, first, state.h) };
  }
}

// One message to a device that knows the sender, for pushes.
export function sealOneWay(payload, s, rs, prologue = MAIL_PROLOGUE, e = newKey()) {
  const state = new SymmetricState(ONE_WAY, prologue);
  state.mixHash(publicOf(s));
  state.mixHash(rs);
  const ephemeral = publicOf(e);
  state.mixHash(ephemeral);
  state.mixKey(dh(e, rs));
  state.mixKey(dh(s, rs));
  return { message: Buffer.concat([ephemeral, state.encryptAndHash(payload)]), following: state.split()[0] };
}

export function openOneWay(message, s, rs, prologue = MAIL_PROLOGUE) {
  if (message.length < 48) throw new Error("malformed");
  const state = new SymmetricState(ONE_WAY, prologue);
  state.mixHash(rs);
  state.mixHash(publicOf(s));
  const re = message.subarray(0, 32);
  state.mixHash(re);
  state.mixKey(dh(s, re));
  state.mixKey(dh(s, rs));
  return { payload: state.decryptAndHash(message.subarray(32)), following: state.split()[0] };
}

// ---- Frames ----

export const Frame = { handshake1: 1, handshake2: 2, transport: 3, close: 4, mail: 5 };
const MAX_MESSAGE = 65535 - 16;

export const relayHello = secret => Buffer.concat([Buffer.from([1]), secret]);
export const relaySend = (to, payload) => Buffer.concat([Buffer.from([2]), to, payload]);
export const relayPush = (to, payload) => Buffer.concat([Buffer.from([5]), to, payload]);

// What the relay sent, or null.
export function readRelay(bytes) {
  const kind = bytes[0];
  const address = bytes.subarray(1, 17);
  if (kind === 1 && bytes.length === 17) return { kind: "welcome", address };
  if (kind === 2 && bytes.length >= 17) return { kind: "deliver", address, payload: bytes.subarray(17) };
  if (kind === 3 && bytes.length === 17) return { kind: "unreachable", address };
  if (kind === 6 && bytes.length === 18) return { kind: "pushed", address, status: bytes[17] };
  return null;
}

export function frame(kind, session, body = EMPTY) {
  const header = Buffer.alloc(5);
  header[0] = kind;
  header.writeUInt32BE(session, 1);
  return Buffer.concat([header, body]);
}

export function readFrame(bytes) {
  if (bytes.length < 5 || bytes[0] < 1 || bytes[0] > 5) return null;
  return { kind: bytes[0], session: bytes.readUInt32BE(1), body: bytes.subarray(5) };
}

// JSON with sorted keys, as the Swift side writes it.
function sortedJSON(value) {
  if (Array.isArray(value)) return `[${value.map(sortedJSON).join(",")}]`;
  if (value && typeof value === "object") {
    return `{${Object.keys(value).sort().filter(k => value[k] !== undefined).map(k => `${JSON.stringify(k)}:${sortedJSON(value[k])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

export function encodeMessage(head, body = EMPTY) {
  const json = Buffer.from(sortedJSON(head));
  const length = Buffer.alloc(2);
  length.writeUInt16BE(json.length);
  return Buffer.concat([length, json, body]);
}

export function decodeMessage(bytes) {
  if (bytes.length < 2) return null;
  const length = bytes.readUInt16BE(0);
  if (bytes.length < 2 + length) return null;
  try {
    return { head: JSON.parse(bytes.subarray(2, 2 + length).toString("utf8")), body: bytes.subarray(2 + length) };
  } catch {
    return null;
  }
}

// `head` and `body` as messages that each fit: the first with `head`, the rest as `data`.
export function pieces(head, body, end) {
  if (Buffer.byteLength(sortedJSON(head)) > 60_000) throw new Error("head too long");
  const out = [];
  let current = { ...head, end: false };
  let offset = 0;
  do {
    const room = MAX_MESSAGE - 2 - Buffer.byteLength(sortedJSON(current)) - 8;
    out.push([current, body.subarray(offset, offset + room)]);
    offset += room;
    current = { t: "data", id: head.id, end: false };
  } while (offset < body.length);
  out[out.length - 1][0].end = end;
  return out.map(([h, b]) => encodeMessage(h, b));
}

// ---- The host: answers paired phones' sessions (the Mac app's LinkServer, in JavaScript) ----

export class LinkHost {
  // authorize(key, hello, address) → whether to let the phone in; handle(request, phone, signal) → an
  // async iterable of { head: { status, headers } } and { body }; connect(request, phone,
  // { open, text, binary, end }) → a socket's { send, close } (PROTOCOL.md, "Sockets");
  // send(bytes) → to the relay; confirmed(phone) → its session proved its own: its first message
  // opened with the handshake's keys, which a replayed first handshake message can't do.
  // mail(from, body) → a mail frame sent to this machine (its Mac's phone list).
  constructor({ key, hello, authorize, handle, connect, send, confirmed, mail }) {
    Object.assign(this, { key, hello, authorize, handle, connect, send, confirmed, mail });
    this.sessions = new Map();
  }

  // The relay connection dropped: what was under way is gone.
  reset() {
    for (const session of this.sessions.values()) this.end(session, false);
  }

  get active() { return this.sessions.size; }

  // A phone the relay can't reach any more (asleep, gone): what it asked for has nobody to go to.
  forget(address) {
    for (const session of this.sessions.values()) if (session.from.equals(address)) this.end(session, false);
  }

  // Phones no longer paired (removed on the Mac) lose the sessions they have open.
  keepOnly(keys) {
    for (const session of this.sessions.values()) if (!keys.has(session.phone.key.toString("base64url"))) this.end(session, true);
  }

  async receive(from, payload) {
    try {
      await this.receiveFrame(from, payload);
    } catch {
      // Nothing a phone sends gets to stop the bridge.
    }
  }

  async receiveFrame(from, payload) {
    const parsed = readFrame(payload);
    if (!parsed) return;
    const id = `${from.toString("base64url")}:${parsed.session}`;
    const session = this.sessions.get(id);
    switch (parsed.kind) {
      case Frame.handshake1:
        return this.handshake(from, parsed.session, id, parsed.body);
      case Frame.transport: {
        if (!session) return this.send(relaySend(from, frame(Frame.close, parsed.session)));
        let message;
        try {
          message = decodeMessage(session.transport.open(parsed.body));
        } catch {
          message = null;
        }
        if (!message || typeof message.head !== "object" || message.head === null) return this.end(session, true);
        if (!session.confirmed) {
          session.confirmed = true;
          this.confirmed?.(session.phone);
        }
        return this.message(session, message);
      }
      case Frame.close:
        if (session) this.end(session, false);
        return;
      case Frame.mail:
        return this.mail?.(from, parsed.body);
    }
  }

  async handshake(from, number, id, body) {
    const refuse = () => this.send(relaySend(from, frame(Frame.close, number)));
    const responder = new IKResponder(this.key, PROLOGUE);
    let hello;
    try {
      hello = JSON.parse(responder.readMessage1(body).toString("utf8"));
    } catch {
      return refuse();
    }
    if (hello?.v !== 1 || !(await this.authorize(responder.rs, hello, from))) return refuse();
    const { message, transport } = responder.writeMessage2(Buffer.from(JSON.stringify(this.hello)));
    // One session per phone: a new one replaces what's left of the last.
    for (const old of this.sessions.values()) if (old.from.equals(from)) this.end(old, false);
    const phone = { key: responder.rs, name: String(hello.name ?? ""), address: from };
    this.sessions.set(id, { id, from, number, phone, transport, incoming: new Map(), running: new Map(), sockets: new Map() });
    this.send(relaySend(from, frame(Frame.handshake2, number, message)));
  }

  message(session, { head, body }) {
    switch (head.t) {
      case "req": {
        const request = { method: head.m ?? "GET", path: head.p ?? "", headers: head.h ?? {}, body };
        if (String(request.headers.upgrade ?? "").toLowerCase() === "websocket") this.open(session, head.id, request);
        else if (head.end === false) session.incoming.set(head.id, request);
        else this.run(session, head.id, request);
        return;
      }
      case "data": {
        const socket = session.sockets.get(head.id);
        if (socket) {
          if (body.length) socket.handle?.send(body);
          if (head.end === true) {
            session.sockets.delete(head.id);
            socket.handle?.close();
          }
          return;
        }
        const request = session.incoming.get(head.id);
        if (!request) return;
        request.body = Buffer.concat([request.body, body]);
        if (head.end === true) {
          session.incoming.delete(head.id);
          this.run(session, head.id, request);
        }
        return;
      }
      case "cancel":
        session.running.get(head.id)?.abort();
        session.running.delete(head.id);
        session.incoming.delete(head.id);
        session.sockets.get(head.id)?.handle?.close();
        session.sockets.delete(head.id);
        return;
      case "ping":
        return this.seal(session, encodeMessage({ t: "pong" }));
    }
  }

  run(session, id, request) {
    const controller = new AbortController();
    session.running.set(id, controller);
    const live = () => !controller.signal.aborted && this.sessions.get(session.id) === session;
    const reply = (head, body, end) => {
      if (!live()) return;
      for (const piece of pieces(head, body, end)) this.seal(session, piece);
    };
    (async () => {
      let head = null;
      let started = false;
      try {
        for await (const part of this.handle(request, session.phone, controller.signal)) {
          if (!live()) return;
          if (part.head) {
            head = { t: "res", id, s: part.head.status, h: part.head.headers };
          } else if (part.body?.length) {
            // The head goes with the first piece of the body.
            reply(started ? { t: "data", id } : head ?? { t: "data", id }, part.body, false);
            started = true;
          }
        }
        reply(started ? { t: "data", id } : head ?? { t: "data", id }, EMPTY, true);
      } catch (error) {
        if (!live()) return;
        if (!started) reply({ t: "res", id, s: 502, h: { "content-type": "text/plain" } }, Buffer.from(String(error?.message ?? error)), true);
        else this.seal(session, encodeMessage({ t: "cancel", id }));
      } finally {
        if (session.running.get(id) === controller) session.running.delete(id);
      }
    })();
  }

  // A socket: open once the engine's is, then messages both ways until either side ends it.
  open(session, id, request) {
    const socket = { handle: null, opened: false };
    session.sockets.set(id, socket);
    const live = () => this.sessions.get(session.id) === session && session.sockets.get(id) === socket;
    const reply = (head, body, end) => {
      if (!live()) return;
      for (const piece of pieces(head, body, end)) this.seal(session, piece);
    };
    const end = error => {
      if (!live()) return;
      if (socket.opened) reply({ t: "data", id }, EMPTY, true);
      else reply({ t: "res", id, s: 502, h: { "content-type": "text/plain" } }, Buffer.from(String(error?.message ?? "It closed.")), true);
      session.sockets.delete(id);
    };
    if (!this.connect) return end(new Error("Sockets aren't available here."));
    socket.handle = this.connect(request, session.phone, {
      open: () => {
        socket.opened = true;
        reply({ t: "res", id, s: 101 }, EMPTY, false);
      },
      text: data => reply({ t: "data", id }, data, false),
      // Binary messages are small (the engine's notes on a terminal); one too long to go whole
      // doesn't go.
      binary: data => { if (data.length <= 60_000) reply({ t: "data", id, h: { type: "binary" } }, data, false); },
      end,
    });
  }

  seal(session, plaintext) {
    this.send(relaySend(session.from, frame(Frame.transport, session.number, session.transport.seal(plaintext))));
  }

  end(session, notify) {
    if (this.sessions.get(session.id) !== session) return;
    this.sessions.delete(session.id);
    for (const controller of session.running.values()) controller.abort();
    session.running.clear();
    for (const socket of session.sockets.values()) socket.handle?.close();
    session.sockets.clear();
    if (notify) this.send(relaySend(session.from, frame(Frame.close, session.number)));
  }
}

// ---- Forwarding to the engine ----

// The sockets a phone may open: a terminal's.
const SOCKETS = [/^api\/pty\/[^/?]+\/connect(\?|$)/];
const REQUEST_HEADERS = ["content-type", "accept", "x-opencode-directory", "last-event-id"];
const RESPONSE_HEADERS = ["content-type", "cache-control"];

// No way out of the project, written plainly or percent-encoded ("%2E%2E"): the engine decodes
// it, and lists a folder above the project if asked. Nor tabs, line breaks or other control
// characters, which the URL parser drops (".\t." is ".." to it), nor a fragment ("#…"), which
// isn't sent, nor a space at either end, which it drops too (".../update " is ".../update").
const leavesProject = path => {
  try {
    if (/[\u0000-\u001f\u007f#]/.test(path) || path.startsWith(" ") || path.endsWith(" ")) return true;
    const decoded = decodeURIComponent(path);
    return path.includes("..") || decoded.includes("..") || decoded.includes("\\") || joinsASign(path) || joinsASign(decoded);
  } catch {
    return true;
  }
};
// A sign other than a letter or digit joined into one character with a mark after it, as the Mac
// refuses it (its checks are made on characters, where "?\u0301" isn't "?").
const characters = new Intl.Segmenter(undefined, { granularity: "grapheme" });
const joinsASign = text => {
  for (const { segment } of characters.segment(text)) {
    if ([...segment].length > 1 && /[\u0000-\u002f\u003a-\u0040\u005b-\u0060\u007b-\u007f]/.test(segment)) return true;
  }
  return false;
};
// A paired phone is yours: the engine's whole API, as the app has it (sessions, files, terminals,
// commands, accounts and keys, settings, MCP servers). Only a path that isn't what it seems
// (`leavesProject`) is refused. As the Mac allows them (KuvoLink's EngineRoutes.allows).
export const allowed = (path, method = "GET") => !leavesProject(path) && /^api(\/|\?|$)/.test(path);
export const socketAllowed = path => !leavesProject(path) && SOCKETS.some(pattern => pattern.test(path));

// `path` as the coding agents' bridge routes it: split into parts, without empty ones or "."
// ones (which its URL parser drops, "%2e" too), each decoded ("%75pdate" is "update").
export const routedPath = path => {
  const at = path.indexOf("?");
  const parts = (at < 0 ? path : path.slice(0, at)).split("/")
    .filter(part => part !== "" && part !== "." && part.toLowerCase() !== "%2e")
    .map(part => {
      try {
        return decodeURIComponent(part);
      } catch {
        return part;
      }
    });
  return parts.join("/") + (at < 0 ? "" : path.slice(at));
};

// The same of the coding agents' bridge (`kuvo/agents/api/…`), but for the two routes only the
// app wires things with: where the engine is, and its group chats' own steps (moves), which a
// phone does through `kuvo/groups`. As the Mac allows them (EngineRoutes.allowsAgents; both are
// checked against Vectors/link.json).
export const agentsAllowed = (path, method = "GET") =>
  allowed(path, method) && !/^api\/kuvo\/(engine|groups)(\/|\?|$)/.test(routedPath(path));

// Where a sign-in's page sent the browser back to this machine, for it to load
// (`kuvo/sign-in/return`), as the Mac's EngineRoutes.signInReturn: only its own address
// (127.0.0.1, localhost, [::1]) over http, at a port, with what the page sent back (a query),
// written plainly, in the characters an address may hold as it is (so it's loaded exactly as
// written). The address without its fragment, which a browser doesn't send; null otherwise.
const RETURN = /^(http:\/\/(?:127\.0\.0\.1|localhost|\[::1\]):([1-9][0-9]{0,4})(?:\/(?:[A-Za-z0-9\-._~!$&'()*+,;=:@/]|%[0-9A-Fa-f]{2})*)?\?(?:[A-Za-z0-9\-._~!$&'()*+,;=:@/?]|%[0-9A-Fa-f]{2})+)(?:#.*)?$/;
export const signInReturn = url => {
  if (typeof url !== "string" || Buffer.byteLength(url) > 8192 || !/^[!-~]*$/.test(url) || url.includes("\\")) return null;
  const match = url.match(RETURN);
  return match && Number(match[2]) <= 65535 ? match[1] : null;
};

const readEngineFile = () => JSON.parse(fs.readFileSync(ENGINE_FILE, "utf8"));
// The coding agents' bridge beside the engine, when it runs: its port and password.
const readBridgeFile = () => JSON.parse(fs.readFileSync(BRIDGE_FILE, "utf8"));
const AGENTS = "kuvo/agents/";

// The block a prompt from a phone ends with, for the agent, in the Mac's words (FromPhone.swift):
// the user is on their iPhone, where a file is shown with show_file. Never shown.
export const FROM_PHONE = ["<from-phone>", "The user sent this from Kuvo Code on their iPhone.", "They read your reply on a small screen, and can't open file paths, links to files on the computer or Kuvo Code's side panel there. Keep your reply short and easy to read on a phone. To show them a file (a picture, a chart, a PDF, a document you made or found), use show_file: it puts the file in this conversation, where they see it.", "</from-phone>"].join("\n");

// A prompt's body with the block at the end of its text, once; any other body as it is.
export function fromPhone(body) {
  let prompt;
  try {
    prompt = JSON.parse(Buffer.from(body ?? "").toString());
  } catch {
    return body;
  }
  if (typeof prompt?.text !== "string" || prompt.text.trimEnd().endsWith("</from-phone>")) return body;
  return Buffer.from(JSON.stringify({ ...prompt, text: `${prompt.text}\n\n${FROM_PHONE}` }));
}

// Passes `path` to the engine or the agents' bridge on this machine, with its password, and
// streams back what it answers. What's sent is checked with `allows` as the URL parser leaves it
// too, so nothing it tidies up reaches what the path as written wouldn't.
async function* forward(target, path, request, signal, allows) {
  const method = request.method.toUpperCase();
  let url;
  try {
    url = new URL(`http://127.0.0.1:${target.port}/${path}`);
  } catch {
    url = null;
  }
  if (!url || !allows(url.pathname.slice(1) + url.search, method)) {
    yield { head: { status: 403, headers: { "content-type": "text/plain" } } };
    yield { body: Buffer.from("That isn't available over the link.") };
    return;
  }
  const headers = { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}` };
  const given = Object.fromEntries(Object.entries(request.headers ?? {}).map(([k, v]) => [k.toLowerCase(), v]));
  for (const name of REQUEST_HEADERS) if (given[name]) headers[name] = given[name];
  // A message the phone sends: that it's from the phone goes with it.
  const prompt = method === "POST" && /^\/api\/session\/[^/]+\/prompt$/.test(url.pathname);
  const response = await fetch(url, {
    method,
    headers,
    body: method === "GET" || method === "HEAD" ? undefined : prompt ? fromPhone(request.body) : request.body,
    signal,
  });
  const out = {};
  for (const name of RESPONSE_HEADERS) {
    const value = response.headers.get(name);
    if (value) out[name] = value;
  }
  yield { head: { status: response.status, headers: out } };
  if (!response.body) return;
  const reader = response.body.getReader();
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) return;
      yield { body: Buffer.from(value) };
    }
  } finally {
    reader.cancel().catch(() => {});
  }
}

// The handler LinkHost runs for each request.
export function engineHandler(readEngine = readEngineFile, info = {}, readBridge = readBridgeFile, phonesChanged = () => {}) {
  // The agents' bridge, while it runs: its record, while the process it names is still it.
  const bridge = () => {
    try {
      const found = readBridge();
      return found?.port && found?.password && runs(found.pid, "kuvo-agents") ? found : null;
    } catch {
      return null;
    }
  };
  return async function* (request, phone, signal) {
    const text = (status, words) => [{ head: { status, headers: { "content-type": "text/plain" } } }, { body: Buffer.from(words) }];
    // A phone using this machine, whatever it asks.
    phoneHere();
    if (request.path === "kuvo/info") {
      let engine = {};
      try { engine = readEngine(); } catch {}
      // When the engine running now came up, in seconds since 1970 (each start writes its record):
      // a restart changes it.
      let started = null;
      if (engine.pid) try { started = fs.statSync(ENGINE_FILE).mtimeMs / 1000; } catch {}
      // Whether it runs now, and whether it's stopped on purpose (until it's started, from anywhere).
      const running = Boolean(engine.pid && runs(engine.pid, `${K}/engine/`));
      const stopped = fs.existsSync(path.join(RUN, "stopped"));
      yield { head: { status: 200, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ kind: "machine", bridge: BRIDGE_VERSION, engine: engine.version ?? null, started, running, stopped, agents: Boolean(bridge()), ...info })) };
      return;
    }
    const json = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
    if (request.path.split("?")[0] === "kuvo/file") return yield* fileAt(request.path, json);
    // Its settings come from the Mac that started its engine, which puts them back as it
    // connects: changed there, they reach it.
    if (/^kuvo\/(config|mcp\/catalog)(\/|\?|$)/.test(request.path)) {
      return yield* json(409, { message: "This machine's settings come from your Mac. Change them there." });
    }
    // The phones paired with this machine, and removing some (a lost one, say), from any phone
    // paired with it: what a removal takes off stays off (see `removePhones`).
    if (request.path === "kuvo/phones") {
      const link = readLink();
      const asking = phone?.key?.toString("base64url");
      return yield* json(200, {
        phones: (link?.phones ?? []).map(each => ({ key: each.key, name: each.name, added: each.added ?? null, pairing: each.pairing ?? "", you: each.key === asking })),
      });
    }
    if (request.path === "kuvo/phones/remove") {
      if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
      const given = parsed(request.body);
      const changed = updateLink(link => link && removePhones(link, phone?.key?.toString("base64url"), given?.phones) ? link : null);
      if (!changed) return yield* json(400, { message: "Only phones paired here can be removed, and never your Mac." });
      // Once this answer is on its way: a phone that removed itself gets it too.
      setTimeout(() => phonesChanged(readLink() ?? changed), 200);
      return yield* json(200, {});
    }
    const control = request.path.match(/^kuvo\/engine\/(start|stop|restart)$/);
    if (control) {
      if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
      const { status, message } = await engineControl(control[1]);
      return yield* json(status, message ? { message } : {});
    }
    if (request.path === "kuvo/sign-in/return") {
      if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
      return yield* loadSignInReturn(request.body, signal);
    }
    // The coding agents' sessions, accounts and Auto, through their bridge, as the engine's go
    // to it; never where the engine is, nor an agent's update.
    if (request.path.startsWith(AGENTS)) {
      const path = request.path.slice(AGENTS.length);
      if (!agentsAllowed(path, request.method)) return yield* text(403, "That isn't available over the link.");
      const target = bridge();
      if (!target) {
        yield { head: { status: 503, headers: { "content-type": "application/json" } } };
        yield { body: Buffer.from(JSON.stringify({ message: "The coding agents aren't running on this machine." })) };
        return;
      }
      return yield* forward(target, path, request, signal, agentsAllowed);
    }
    // This machine's sign-ins, listed (`api/credential` has what each is signed in with too).
    if (request.path.split("?")[0] === "kuvo/accounts/credentials") {
      if (!["GET", "HEAD"].includes(request.method.toUpperCase())) return yield* text(405, "Read them with GET.");
      let engine;
      try {
        engine = readEngine();
      } catch {
        return yield* text(503, "The engine isn't running.");
      }
      let listed;
      try {
        const response = await fetch(`http://127.0.0.1:${engine.port}/api/credential`, {
          headers: { authorization: `Basic ${Buffer.from(`opencode:${engine.password}`).toString("base64")}` },
          signal,
        });
        if (!response.ok) return yield* text(502, "The engine didn't answer.");
        listed = ((await response.json())?.data ?? []).map(credential => ({
          id: String(credential.id), integrationID: String(credential.integrationID), label: String(credential.label ?? ""), active: Boolean(credential.active),
        }));
      } catch {
        return yield* text(502, "The engine didn't answer.");
      }
      yield { head: { status: 200, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ credentials: listed })) };
      return;
    }
    // New bots and group chats, made here as New Bot and New Group Chat make them on the Mac,
    // which takes them in when it next connects.
    const route = request.path.split("?")[0];
    if (route === "kuvo/bots/templates") {
      const kit = readKit();
      if (!kit) return yield* json(409, { message: "Connect to this machine from your Mac once, then try again." });
      return yield* json(200, botTemplates(kit, info.name));
    }
    if (request.method.toUpperCase() === "POST" && route === "kuvo/bots") {
      const { status, value } = await makeBot(parsed(request.body), readEngine, bridge, info.name, signal);
      return yield* json(status, value);
    }
    if (request.method.toUpperCase() === "POST" && route === "kuvo/groups") {
      const target = bridge();
      if (!target) return yield* json(503, { message: "Group chats run with the coding agents' bridge, which isn't running on this machine." });
      const { status, value } = await makeGroup(parsed(request.body), target, signal);
      return yield* json(status, value);
    }
    // The group chats that live here, as the Mac answers its own (`kuvo/groups`), from the
    // coding agents' bridge, which runs them.
    const group = request.path.split("?")[0].match(GROUP_ROUTE);
    if (group) {
      const target = bridge();
      if (!target) {
        yield { head: { status: 503, headers: { "content-type": "application/json" } } };
        yield { body: Buffer.from(JSON.stringify({ message: "Group chats run with the coding agents' bridge, which isn't running on this machine." })) };
        return;
      }
      return yield* answerGroups(target, group[1], group[2], request, signal);
    }
    // This machine's bots and wake-ups, from their folders.
    if (request.path.split("?")[0] === "kuvo/bots") {
      if (!["GET", "HEAD"].includes(request.method.toUpperCase())) return yield* text(405, "Read them with GET.");
      yield { head: { status: 200, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ bots: listBots(info.name) })) };
      return;
    }
    if (request.path.split("?")[0] === "kuvo/wakes") {
      if (!["GET", "HEAD"].includes(request.method.toUpperCase())) return yield* text(405, "Read them with GET.");
      yield { head: { status: 200, headers: { "content-type": "application/json" } } };
      yield { body: Buffer.from(JSON.stringify({ sessions: listWakes() })) };
      return;
    }
    const wakes = request.path.split("?")[0].match(/^kuvo\/wakes\/([^/]+)(?:\/(cancel|now))?$/);
    if (wakes) {
      let session;
      try {
        session = decodeURIComponent(wakes[1]);
      } catch {
        return yield* text(404, "No such session.");
      }
      return yield* answerWakes(session, wakes[2], request, readEngine, bridge, signal);
    }
    const stop = request.path.match(/^kuvo\/shell\/([^/?]+)\/stop$/);
    if (!stop && !allowed(request.path, request.method)) return yield* text(403, "That isn't available over the link.");
    let engine;
    try {
      engine = readEngine();
    } catch {
      return yield* text(503, "The engine isn't running.");
    }
    if (stop) {
      if (request.method.toUpperCase() !== "POST") return yield* text(405, "Stop it with POST.");
      const headers = { authorization: `Basic ${Buffer.from(`opencode:${engine.password}`).toString("base64")}` };
      const given = Object.fromEntries(Object.entries(request.headers).map(([k, v]) => [k.toLowerCase(), v]));
      for (const name of REQUEST_HEADERS) if (given[name]) headers[name] = given[name];
      return yield* stopShell(decodeURIComponent(stop[1]), engine, headers, signal);
    }
    return yield* forward(engine, request.path, request, signal, allowed);
  };
}

// The engine here started, stopped or started over by the box's own tool (`bin/kuvo-start`, as
// the Mac has it do): `start` after a stop, `stop` until it's started again (from anywhere), and
// `restart`. This bridge stays up through each. A start or restart is answered at once (202), and
// the phone sees it come up as `kuvo/info` says (`running`, `started`); a stop once it's done.
export async function engineControl(command) {
  const start = path.join(K, "bin", "kuvo-start");
  if (!fs.existsSync(start)) return { status: 409, message: "Kuvo Code hasn't set this machine up yet." };
  // Started from a Mac once, which keeps how (`run/launch.env`) and what starts it (`bin/kuvo-launch`).
  if (command !== "stop" && ![path.join(K, "bin", "kuvo-launch"), path.join(RUN, "launch.env")].every(file => fs.existsSync(file))) {
    return { status: 409, message: "Start it once from Kuvo Code on your Mac." };
  }
  // Not with what runs this bridge (the engine's runtime as Bun), which would make `opencode
  // serve` a script to look for.
  const environment = { ...process.env };
  delete environment.BUN_BE_BUN;
  delete environment.BUN_RUNTIME_TRANSPILER_CACHE_PATH;
  const workspace = path.join(K, "workspace");
  const options = { cwd: fs.existsSync(workspace) ? workspace : K, env: environment };
  if (command === "stop") {
    // Answered once it's down; one that takes over a minute (a start under way, which it waits
    // for) goes on by itself, never cut off halfway.
    let child;
    try {
      child = spawn("sh", [start, "stop"], { ...options, detached: true, stdio: ["ignore", "ignore", "pipe"] });
    } catch {
      return { status: 503, message: "It couldn't be stopped." };
    }
    let said = "";
    child.stderr.on("data", data => { said += data; });
    const status = await new Promise(resolve => {
      const timer = setTimeout(() => resolve(null), 60_000);
      child.on("exit", code => { clearTimeout(timer); resolve(code); });
      child.on("error", () => { clearTimeout(timer); resolve(-1); });
    });
    if (status === null) {
      child.stderr.destroy();
      child.unref();
      return { status: 202 };
    }
    return status === 0 ? { status: 200 } : { status: 503, message: said.trim() || "It couldn't be stopped." };
  }
  try {
    const child = spawn("sh", [start, command], { ...options, detached: true, stdio: "ignore" });
    child.unref();
  } catch {
    return { status: 503, message: "It couldn't be started." };
  }
  return { status: 202 };
}

// `kuvo/sign-in/return` with {"url"}: where a sign-in's page sent the browser back to this
// machine (an MCP server's, or a coding agent's), which a phone can't load, loaded here as the
// browser would have: only this machine's own address (`signInReturn`), and no further than it
// answers (a redirect isn't followed). Answers with the status it got: 2xx or 3xx, it took it.
async function* loadSignInReturn(body, signal) {
  const json = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  let given = {};
  try {
    given = JSON.parse(Buffer.from(body ?? []).toString("utf8") || "{}");
  } catch {}
  const url = signInReturn(given?.url);
  if (!url) return yield* json(400, { message: "That isn't an address a sign-in returns to on this computer." });
  const stop = new AbortController();
  const timer = setTimeout(() => stop.abort(), 15_000);
  const cancel = () => stop.abort();
  signal?.addEventListener("abort", cancel);
  try {
    const response = await fetch(url, { redirect: "manual", signal: stop.signal });
    await response.arrayBuffer().catch(() => {});
    return yield* json(200, { status: response.status });
  } catch {
    return yield* json(502, { message: "Nothing on this computer took it. Start the sign-in again." });
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener("abort", cancel);
  }
}

// Stops a command the engine runs for an agent: SIGTERM to its process group, here on the
// machine, so the engine sees it end and tells the agent, with what it printed. The engine's own
// remove deletes the output too, and the agent hears the command was never found, so it's only
// for when the signal can't be sent.
async function* stopShell(id, engine, headers, signal) {
  const json = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const url = `http://127.0.0.1:${engine.port}/api/shell/${encodeURIComponent(id)}`;
  let job;
  try {
    const response = await fetch(url, { headers, signal });
    if (response.ok) job = (await response.json()).data;
  } catch {}
  if (!job || job.status !== "running") return yield* json(404, {});
  if (Number.isInteger(job.pid) && job.pid > 1) {
    try {
      process.kill(-job.pid, "SIGTERM");
      return yield* json(200, { stopped: "signal" });
    } catch {}
  }
  try {
    const response = await fetch(url, { method: "DELETE", headers, signal });
    if (response.ok) return yield* json(200, { stopped: "removed" });
  } catch {}
  return yield* json(404, {});
}

// What LinkHost runs to open a socket: the engine's WebSocket at that path, with its password.
export function engineConnect(readEngine = readEngineFile) {
  return (request, phone, events) => {
    let ended = false;
    const end = error => {
      if (ended) return;
      ended = true;
      events.end(error);
    };
    const refuse = words => {
      queueMicrotask(() => end(new Error(words)));
      return { send() {}, close() {} };
    };
    if (!socketAllowed(request.path)) return refuse("That isn't available over the link.");
    let engine;
    try {
      engine = readEngine();
    } catch {
      return refuse("The engine isn't running.");
    }
    phoneHere();
    const headers = { authorization: `Basic ${Buffer.from(`opencode:${engine.password}`).toString("base64")}` };
    const directory = Object.entries(request.headers ?? {}).find(([name]) => name.toLowerCase() === "x-opencode-directory")?.[1];
    if (directory) headers["x-opencode-directory"] = directory;
    const socket = new WebSocket(`ws://127.0.0.1:${engine.port}/${request.path}`, { headers });
    socket.binaryType = "arraybuffer";
    socket.onopen = () => events.open();
    socket.onmessage = event => typeof event.data === "string" ? events.text(Buffer.from(event.data)) : events.binary(Buffer.from(event.data));
    socket.onerror = () => end(new Error("The engine closed it."));
    socket.onclose = () => end();
    return {
      send: data => { if (socket.readyState === 1) socket.send(data.toString("utf8")); },
      close: () => {
        ended = true;
        try { socket.close(); } catch {}
      },
    };
  };
}

// The engine's watchdog stops it after a day without the app; a phone counts as the app.
let touched = 0;
function touchAttached() {
  if (Date.now() - touched < 5 * 60_000) return;
  touched = Date.now();
  try {
    const now = new Date();
    fs.utimesSync(path.join(RUN, "attached"), now, now);
  } catch {}
}

// When a phone last used this machine, in `run/link.seen`: while one has in the last week, the
// engine stays up for it (the bridge's timer, and the Mac leaving it running as it quits), so the
// machine's sessions and bots are there whenever it asks; a phone lost or unpaired elsewhere
// stops keeping it after that.
export const SEEN_FILE = path.join(RUN, "link.seen");
export const KEEP_FOR = 7 * 24 * 60 * 60_000;
let seen = 0;
function phoneHere() {
  touchAttached();
  if (Date.now() - seen < 5 * 60_000) return;
  seen = Date.now();
  try {
    const now = new Date();
    if (fs.existsSync(SEEN_FILE)) {
      fs.utimesSync(SEEN_FILE, now, now);
    } else {
      fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
      fs.writeFileSync(SEEN_FILE, "", { mode: 0o600 });
    }
  } catch {}
}

// No phone is paired any more: none keeps the engine up.
export function forgetPhones() {
  seen = 0;
  try {
    fs.rmSync(SEEN_FILE, { force: true });
  } catch {}
}

export function phoneSeenLately(file = SEEN_FILE, now = Date.now()) {
  try {
    return now - fs.statSync(file).mtimeMs < KEEP_FOR;
  } catch {
    return false;
  }
}

// ---- This machine's bots and wake-ups, from their folders ----

// What the Mac answers for them (PROTOCOL.md, `kuvo/bots` and `kuvo/wakes`), answered here from
// the folders the app keeps on the machine, so a phone has them with the Mac closed: a bot's
// description (`bot.json`, which the app writes from its list), its session, and the wake-ups,
// watches and pause that `kuvo-wake` keeps beside them.
const BOTS = path.join(K, "bots");
const SESSION_WAKES = path.join(K, "wakes");
// The version of `bot.json` this reads (the app's `Bot.Descriptor`).
const DESCRIPTION = 1;
const WAKE_MARKER = "[Wake-up] ";

const textOf = file => {
  try {
    return fs.readFileSync(file, "utf8");
  } catch {
    return null;
  }
};
const listOf = folder => {
  try {
    return fs.readdirSync(folder);
  } catch {
    return [];
  }
};
// On one line, cut to `limit` characters, as the Mac lists a machine's.
const oneLine = (text, limit) => Array.from(String(text).replace(/[\t\r\n]/g, " ")).slice(0, limit).join("");

// How often a wake-up comes back, as its name says (the app's Wakes.Repeat): 30m, 2h, 1d, 1w or
// weekdays, within a year; null for anything else.
export function repeatText(text) {
  if (text === "weekdays") return text;
  const match = /^([1-9][0-9]{0,5})([mhdw])$/.exec(text ?? "");
  if (!match) return null;
  const count = Number(match[1]);
  const fits = { m: count >= 5 && count <= 525_600, h: count <= 8_760, d: count <= 366, w: count <= 52 }[match[2]];
  return fits ? text : null;
}

// A wake-up from its file's name (the app's Wakes.Pending): when it's due, in seconds since 1970,
// the letters that name it for good, and how often it comes back; null for another file.
export function wakeOf(name, note = "") {
  const [due, letters = "", ...more] = name.split("-");
  if (!/^[0-9]+(\.[0-9]+)?$/.test(due ?? "")) return null;
  const repeating = more.join("-");
  return { id: name, due: Number(due), note, every: repeating ? repeatText(repeating.split("@")[0]) : null, letters };
}

// The folder keeping a session's wake-ups: its bot's, or its own in `wakes`; null for none.
export function wakeFolder(session) {
  if (!/^[A-Za-z0-9_-]{1,100}$/.test(session)) return null;
  for (const id of listOf(BOTS)) {
    if ((textOf(path.join(BOTS, id, "session")) ?? "").trim() === session) return path.join(BOTS, id);
  }
  const own = path.join(SESSION_WAKES, session);
  return textOf(path.join(own, "session")) === null ? null : own;
}

// Each session here with a wake-up or a watch, as the Mac lists them in `kuvo/wakes`: its soonest
// wake-up and whether it watches. Bots' come with `kuvo/bots`.
export function listWakes() {
  const sessions = [];
  for (const name of listOf(SESSION_WAKES)) {
    const folder = path.join(SESSION_WAKES, name);
    const session = (textOf(path.join(folder, "session")) ?? "").trim();
    if (!session) continue;
    const { wakes, watches } = readWakes(folder);
    if (!wakes.length && !watches.length) continue;
    sessions.push({ session, ...(wakes.length ? { next: wakes[0].due } : {}), watching: watches.length > 0 });
  }
  return sessions;
}

// A folder's waiting wake-ups (soonest first), its watches, and whether you paused them.
export function readWakes(folder) {
  const wakes = listOf(path.join(folder, "wakes"))
    .map(name => wakeOf(name, oneLine(textOf(path.join(folder, "wakes", name)) ?? "", 400)))
    .filter(Boolean)
    .sort((a, b) => a.due - b.due);
  const watches = listOf(path.join(folder, "watches")).sort().map(letters => {
    const at = path.join(folder, "watches", letters);
    const check = textOf(path.join(at, "check")) ?? "";
    if (!check) return null;
    return { letters, every: (textOf(path.join(at, "every")) ?? "").trim(), check: oneLine(check, 300), note: oneLine(textOf(path.join(at, "note")) ?? "", 400) };
  }).filter(Boolean);
  return { wakes, watches, paused: fs.existsSync(path.join(folder, "paused")) };
}

// ---- Group chats that live here ----

// What a phone asks of them, as of a Mac's: the list and one chat (read, or followed as they
// change), and what it does in one (writes to it, stops it, lets it keep going, looks at it, takes
// a message back, starts the work a wrap-up split up).
const GROUP_ROUTE = /^kuvo\/groups(?:\/(grp_[a-z0-9]+))?(?:\/(events|stop|keep-going|viewed|undo|start))?$/;

// Times as the Mac sends them to a phone: seconds since 1970.
const seconds = text => {
  const time = Date.parse(text ?? "");
  return Number.isFinite(time) ? time / 1000 : undefined;
};

// A message as the Mac sends it: its times in seconds, without the block naming agents (the
// machine's own record of who you named) nor, in a list, what each bot did on its turn.
export function phoneMessage(message, withSteps = true) {
  if (!message) return undefined;
  const { named: _, steps, time, tasks, ...rest } = message;
  const out = { ...rest, time: seconds(time) };
  if (withSteps && steps) out.steps = steps;
  if (tasks) out.tasks = tasks.map(({ started, finished, ...task }) => ({ ...task, ...(started ? { started: seconds(started) } : {}), ...(finished ? { finished: seconds(finished) } : {}) }));
  return out;
}

// A chat's bot as the Mac lists it: who it is, how it looks, its own conversation and folder.
function groupMember(id) {
  if (!/^bot_[A-Za-z0-9]+$/.test(id)) return null;
  const folder = path.join(BOTS, id);
  let described;
  try {
    described = JSON.parse(textOf(path.join(folder, "bot.json")) ?? "");
  } catch {
    return null;
  }
  const session = (textOf(path.join(folder, "session")) ?? "").trim();
  if (!described?.name || !session) return null;
  return {
    id, name: described.name, look: described.look, session,
    directory: (textOf(path.join(folder, "directory")) ?? "").trim() || path.join(folder, "workspace"),
    agents: (textOf(path.join(folder, "via")) ?? "").trim() === "bridge",
  };
}

async function askGroups(target, method, route, body, signal) {
  const response = await fetch(`http://127.0.0.1:${target.port}/api/kuvo/groups${route}`, {
    method,
    headers: { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`, ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal,
  });
  const value = await response.json().catch(() => ({}));
  return { status: response.status, value };
}

// The list, as the Mac's `kuvo/groups` has it; null when none run here. Throws when the bridge
// couldn't be asked (it's there, but not answering now).
export async function groupsList(target, signal) {
  const { status, value } = await askGroups(target, "GET", "", undefined, signal);
  if (status === 404) return null;
  if (status !== 200) throw new Error(`The coding agents' bridge answered ${status}.`);
  const groups = (value.data ?? []).filter(chat => chat.status === "active").map(({ group, state, said, fromBot }) => {
    const meeting = group.meeting ? { ...group.meeting, next: seconds(group.meeting.next) } : undefined;
    return {
      id: group.id, name: group.name, purpose: group.purpose, members: group.members.map(groupMember).filter(Boolean),
      speaking: state?.speaking, discussing: Boolean(state?.discussing), paused: Boolean(state?.paused),
      unread: Boolean(fromBot && (!group.viewed || Date.parse(fromBot.time) > Date.parse(group.viewed))),
      last: phoneMessage(said, false), meeting,
    };
  });
  return { groups };
}

// One chat, as the Mac's `kuvo/groups/<id>/events` sends it; null once it's gone (or moving).
// Throws when the bridge couldn't be asked.
export async function groupChat(target, id, signal) {
  const { status, value } = await askGroups(target, "GET", `/${id}`, undefined, signal);
  if (status === 404) return null;
  if (status !== 200) throw new Error(`The coding agents' bridge answered ${status}.`);
  if (value.data?.status !== "active") return null;
  const { messages, state } = value.data;
  return {
    messages: messages.slice(-300).map(message => phoneMessage(message)), speaking: state?.speaking, discussing: Boolean(state?.discussing), paused: Boolean(state?.paused),
    ...(state?.activity?.length ? { activity: state.activity } : {}),
  };
}

async function* answerGroups(target, id, action, request, signal) {
  const method = request.method.toUpperCase();
  const json = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  // Read, or followed: a line of JSON each time it changes, as a Mac's.
  if (method === "GET" && (!action || action === "events")) {
    const read = () => (id ? groupChat(target, id, signal) : groupsList(target, signal));
    let now;
    try {
      now = await read();
    } catch {
      // There, but not answering now: asked again later, not taken as having none.
      return yield* json(503, {});
    }
    if (!now) return yield* json(404, {});
    if (!action) return yield* json(200, now);
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    // One chat's lines leave out its messages when they're as they were (who's talking, or
    // what they're doing, changed): the phone keeps the ones it has.
    let last = "";
    let lastMessages = null;
    while (!signal?.aborted && now) {
      const line = JSON.stringify(now);
      if (line !== last) {
        const messages = id ? JSON.stringify(now.messages) : null;
        const { messages: _, ...state } = now;
        yield { body: Buffer.from((id && messages === lastMessages ? JSON.stringify(state) : line) + "\n") };
        last = line;
        lastMessages = messages;
      }
      await new Promise(done => setTimeout(done, 1000));
      if (signal?.aborted) break;
      now = await read().catch(() => now);
    }
    return;
  }
  if (method !== "POST" || !id || action === "events") return yield* json(405, {});
  let given = {};
  try {
    given = JSON.parse(Buffer.from(request.body ?? []).toString("utf8") || "{}");
  } catch {}
  let asked;
  switch (action) {
    case undefined:
      if (typeof given.text !== "string" || !given.text.trim()) return yield* json(400, {});
      asked = await askGroups(target, "POST", `/${id}/send`, { text: given.text }, signal);
      break;
    case "stop":
    case "keep-going":
      asked = await askGroups(target, "POST", `/${id}/${action}`, {}, signal);
      break;
    case "viewed":
      asked = await askGroups(target, "PATCH", `/${id}`, { viewed: new Date().toISOString().replace(/\.\d{3}Z$/, "Z") }, signal);
      break;
    case "undo": {
      if (typeof given.message !== "string") return yield* json(404, {});
      asked = await askGroups(target, "POST", `/${id}/undo`, { message: given.message }, signal);
      // Moving: 409, as for what you write.
      if (asked.status !== 200) return yield* json(failed(asked.status), {});
      return yield* json(200, { text: asked.value.data?.text ?? "" });
    }
    case "start":
      if (typeof given.message !== "string") return yield* json(404, {});
      asked = await askGroups(target, "POST", `/${id}/start-work`, { wrapUp: given.message }, signal);
      break;
  }
  return yield* json(asked.status === 200 ? 200 : failed(asked.status), {});
}

// What a phone hears of a group chat's action the coding agents' bridge didn't take: moving (409),
// there but not answering now (503: ask again), or no such chat or message (404).
const failed = status => (status === 409 ? 409 : status >= 500 ? 503 : 404);

// The biggest file `kuvo/file` sends.
export const FILE_LIMIT = 25 * 1024 * 1024;

const TYPES = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", heic: "image/heic",
  svg: "image/svg+xml", pdf: "application/pdf", mp4: "video/mp4", mov: "video/quicktime", mp3: "audio/mpeg",
  wav: "audio/wav", txt: "text/plain", md: "text/markdown", csv: "text/csv", json: "application/json",
  html: "text/html", css: "text/css", js: "text/javascript",
};

// `kuvo/file?uri=<file:// URI>`: a file on this machine, as an agent's reply or tool names it:
// its bytes with its type; 404 with a message for one that isn't there (or isn't a file), 413
// over FILE_LIMIT.
async function* fileAt(requestPath, json) {
  const query = requestPath.includes("?") ? requestPath.slice(requestPath.indexOf("?") + 1) : "";
  const uri = new URLSearchParams(query).get("uri");
  let file;
  try {
    file = uri ? fileURLToPath(uri) : undefined;
  } catch {}
  if (!file) return yield* json(400, { message: "That isn’t a file:// address." });
  let stat;
  try {
    stat = fs.statSync(file);
  } catch {}
  if (!stat?.isFile()) return yield* json(404, { message: "That file isn’t there anymore." });
  if (stat.size > FILE_LIMIT) return yield* json(413, { message: "That file is too big to open here." });
  const type = TYPES[path.extname(file).slice(1).toLowerCase()] ?? "application/octet-stream";
  yield { head: { status: 200, headers: { "content-type": type, "content-length": String(stat.size) } } };
  // In pieces, as the engine's bodies go.
  for await (const piece of fs.createReadStream(file, { highWaterMark: 32 * 1024 })) yield { body: piece };
}

// This machine's bots, as the Mac lists them in `kuvo/bots`: each by its session, with its name,
// look and job as the app last described it, and what its folder says now (paused, its next
// wake-up, how many things it watches, whether it runs with the coding agents). One without a
// description (made before them, until the Mac connects) or a session isn't listed.
export function listBots(machine) {
  const bots = [];
  for (const id of listOf(BOTS)) {
    if (!id.startsWith("bot_")) continue;
    const folder = path.join(BOTS, id);
    let described;
    try {
      described = JSON.parse(textOf(path.join(folder, "bot.json")) ?? "");
    } catch {
      continue;
    }
    const session = (textOf(path.join(folder, "session")) ?? "").trim();
    if (described?.kuvo !== DESCRIPTION || described.id !== id || typeof described.name !== "string" || !session) continue;
    const { wakes, watches, paused } = readWakes(folder);
    bots.push({
      created: String(described.created ?? ""),
      bot: {
        id,
        session,
        name: described.name,
        look: described.look,
        machine: machine || undefined,
        job: typeof described.job === "string" ? described.job : "",
        paused,
        wakes: wakes.length ? Math.min(...wakes.map(wake => wake.due)) : undefined,
        watching: watches.length,
        agents: (textOf(path.join(folder, "via")) ?? "").trim() === "bridge",
      },
    });
  }
  // In the order they were made, as the Mac's list is.
  return bots.sort((a, b) => a.created.localeCompare(b.created)).map(entry => entry.bot);
}

// ---- Bots and group chats made here ----

// What New Bot needs here, which the Mac gives the machine as it connects (Bots.MachineKit).
const KIT_FILE = path.join(BOTS, "kit.json");
// The coding agents' providers (the Mac's OutsideAgents): a bot on one of theirs runs with them.
const AGENT_PROVIDERS = ["claude-code", "codex-cli", "cursor", "grok-cli", "antigravity"];
const runsWithAgents = model => model?.providerID === "auto" || AGENT_PROVIDERS.includes(String(model?.providerID ?? "").split("@")[0]);

export function readKit() {
  try {
    const kit = JSON.parse(textOf(KIT_FILE) ?? "");
    return kit?.kuvo === 1 && Array.isArray(kit.templates) && Array.isArray(kit.personalities) && typeof kit.instructions === "string" ? kit : null;
  } catch {
    return null;
  }
}

const parsed = body => {
  try {
    return JSON.parse(Buffer.from(body ?? "").toString() || "{}");
  } catch {
    return undefined;
  }
};

// As Swift writes dates: to the second.
const stamp = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
const newID = prefix => prefix + Array.from(crypto.randomBytes(10), byte => "abcdefghijklmnopqrstuvwxyz0123456789"[byte % 36]).join("");
// A name as one line of plain text, as the Mac's name field has it.
const plainName = text => String(text ?? "").replace(/[\u0000-\u001f\u007f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, " ").split(/\s+/).filter(Boolean).join(" ");

// `kuvo/bots/templates`, as the Mac's: what New Bot offers, the model a bot starts on here (by
// this machine's name, as its bots' `machine` has it), and its projects.
export function botTemplates(kit, machine) {
  return {
    templates: kit.templates.map(({ firstMessage: _, ...template }) => template),
    personalities: kit.personalities.map(({ instruction: _, ...personality }) => personality),
    looks: kit.looks,
    startingModel: { [machine || "machine"]: kit.startingModel ?? null },
    projects: (kit.projects ?? []).map(project => ({ ...project, machine: machine || undefined })),
  };
}

async function call(target, method, route, body, signal) {
  const response = await fetch(`http://127.0.0.1:${target.port}${route}`, {
    method,
    headers: { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`, ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal,
  });
  const value = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(value?.message || `It answered ${response.status}.`);
  return value?.data ?? value;
}

// What a bot is told, from the kit's words with its own name, folder, personality and project.
export function botInstructions(kit, name, folder, personality, project) {
  let text = kit.instructions.replaceAll("⟦name⟧", name).replaceAll("⟦folder⟧", folder);
  text = personality ? text.replace("⟦personality⟧", personality) : text.replace("\n⟦personality⟧\n", "");
  if (project) text = text.replace(`\n\n${kit.identityNote}`, `\n\n${kit.projectNote}\n\n${kit.identityNote}`);
  return text;
}

// `POST kuvo/bots`, as the Mac's: the bot made here, its folder, conversation and instructions,
// and the template's job for your composer, or else its greeting, so it writes first.
export async function makeBot(given, readEngine, readBridge, machine, signal) {
  const no = (status, message) => ({ status, value: { message } });
  const kit = readKit();
  if (!kit) return no(409, "Connect to this machine from your Mac once, then try again.");
  if (!given || typeof given !== "object") return no(400, "That isn’t a bot to make.");
  const name = plainName(given.name);
  if (!name) return no(400, "A bot needs a name.");
  let template;
  if (given.template !== undefined && given.template !== null) {
    template = kit.templates.find(each => each.id === given.template);
    if (!template) return no(400, `There’s no template “${given.template}”.`);
  }
  const personalityID = given.personality ?? template?.personality;
  const personality = personalityID ? kit.personalities.find(each => each.id === personalityID) : undefined;
  if (personalityID && !personality) return no(400, `There’s no personality “${personalityID}”.`);
  const project = given.project ?? undefined;
  if (project !== undefined && !(kit.projects ?? []).some(each => each.directory === project)) return no(400, "That project isn’t in Kuvo Code.");
  const looks = kit.looks;
  const pick = list => list[crypto.randomInt(list.length)];
  const fits = look => look && looks.colors.includes(look.color) && looks.shapes.includes(look.shape) && looks.faces.includes(look.face);
  const look = fits(given.look) ? given.look : template?.look ?? { color: pick(looks.colors), shape: pick(looks.shapes), face: pick(looks.faces) };
  const model = given.model && typeof given.model.id === "string" && typeof given.model.providerID === "string" ? given.model : kit.startingModel;
  if (!model) return no(422, "There’s no model to start it on here: sign in to an account on this machine first.");
  const agents = runsWithAgents(model);
  let target;
  try {
    target = agents ? readBridge() : readEngine();
  } catch {}
  if (!target) return no(503, agents ? "The coding agents aren’t running on this machine." : "The engine isn’t running.");
  // A model of the engine's whose account isn't signed in here: said now, as the engine would drop it.
  if (!agents) {
    const ready = await call(target, "GET", "/api/provider", undefined, signal).catch(() => undefined);
    const ids = Array.isArray(ready) ? ready.map(each => each?.id) : [];
    if (ids.length && !ids.includes(model.providerID)) return no(422, `${model.id} needs a sign-in on this machine: sign in to it in Settings › Accounts, or choose another model.`);
  }

  const id = newID("bot_");
  const folder = path.join(BOTS, id);
  const directory = project ?? path.join(folder, "workspace");
  const discard = () => fs.rmSync(folder, { recursive: true, force: true });
  let session;
  try {
    fs.mkdirSync(path.join(folder, "workspace"), { recursive: true });
    fs.mkdirSync(path.join(folder, "wakes"), { recursive: true });
    fs.writeFileSync(path.join(folder, "MEMORY.md"), kit.memory.replaceAll("⟦name⟧", name));
    const rules = [...(kit.rules ?? []), { action: "external_directory", resource: path.join(folder, "*"), effect: "allow" }];
    session = await call(target, "POST", "/api/session", { title: name, location: { directory }, model, permissions: rules }, signal);
    if (!session?.id) throw new Error("No conversation was made.");
    await call(target, "PATCH", `/api/session/${encodeURIComponent(session.id)}`, { metadata: { kuvo: { bot: id } } }, signal).catch(() => {});
    const told = botInstructions(kit, name, folder, personality?.instruction, project);
    await call(target, "PUT", `/api/experimental/session/${encodeURIComponent(session.id)}/instructions/entries/kuvo.bot`, { value: told }, signal);
    fs.writeFileSync(path.join(folder, "session"), session.id);
    fs.writeFileSync(path.join(folder, "directory"), directory);
    if (kit.timeZone) fs.writeFileSync(path.join(folder, "timezone"), kit.timeZone);
    if (kit.zoneinfo) fs.writeFileSync(path.join(folder, "zoneinfo"), Buffer.from(kit.zoneinfo, "base64"));
    if (agents) fs.writeFileSync(path.join(folder, "via"), "bridge");
    const now = stamp();
    const described = { created: now, id, job: "", kuvo: DESCRIPTION, look, model, name, ...(personality ? { personality: personality.id } : {}), ...(project ? { project } : {}), updated: now };
    fs.writeFileSync(path.join(folder, "bot.json"), JSON.stringify(described, null, 2));
  } catch (error) {
    if (session?.id) await call(target, "DELETE", `/api/session/${encodeURIComponent(session.id)}`, undefined, signal).catch(() => {});
    discard();
    return no(503, `The bot couldn’t be made: ${error?.message ?? error}`);
  }
  // Without a template's job for you to send, it writes first.
  if (!template) await call(target, "POST", `/api/session/${encodeURIComponent(session.id)}/synthetic`, { text: kit.greeting, description: "A new bot's first message" }, signal).catch(() => {});
  const bot = listBots(machine).find(each => each.id === id);
  return { status: 201, value: { bot, firstMessage: template?.firstMessage ?? null } };
}

// `POST kuvo/groups`, as the Mac's: a group chat of this machine's bots, which lives here and runs
// in the coding agents' bridge, as the others here do.
export async function makeGroup(given, target, signal) {
  const no = (status, message) => ({ status, value: { message } });
  if (!given || typeof given !== "object") return no(400, "That isn’t a group chat to make.");
  const name = plainName(given.name);
  if (!name) return no(400, "A group chat needs a name.");
  const members = [...new Set(Array.isArray(given.members) ? given.members.filter(each => typeof each === "string") : [])];
  if (members.length < 2) return no(400, "A group chat needs at least two bots.");
  const here = new Set(listBots().map(bot => bot.id));
  const missing = members.find(member => !here.has(member));
  if (missing) return no(404, `There’s no bot “${missing}”.`);
  const purpose = typeof given.purpose === "string" && given.purpose.trim() ? given.purpose.trim() : undefined;
  const id = newID("grp_");
  const now = stamp();
  const group = { id, name, ...(purpose ? { purpose } : {}), members, seats: {}, heard: {}, created: now, viewed: now };
  const { status, value } = await askGroups(target, "PUT", `/${id}`, { group, messages: [], status: "active" }, signal).catch(error => ({ status: 503, value: { message: String(error?.message ?? error) } }));
  if (status !== 200) return no(status === 409 ? 409 : 503, value?.message ?? "The group chat couldn’t be made.");
  const listed = await groupsList(target, signal).catch(() => null);
  return { status: 201, value: { group: listed?.groups.find(each => each.id === id) ?? null } };
}

const LETTERS = /^[A-Za-z0-9]+$/;

// Takes a wake-up off the schedule by its letters, which a repeating one keeps as its time moves on.
function cancelWake(folder, letters) {
  if (!LETTERS.test(letters)) return;
  for (const name of listOf(path.join(folder, "wakes"))) {
    if (wakeOf(name)?.letters === letters) fs.rmSync(path.join(folder, "wakes", name), { force: true });
  }
}

// A message ID that sorts after earlier ones, as the engine's own do.
let lastMessage = { time: 0, counter: 0 };
export function messageID(now = Date.now()) {
  lastMessage = lastMessage.time === now ? { time: now, counter: lastMessage.counter + 1 } : { time: now, counter: 1 };
  const time = ((BigInt(now) * 0x1000n + BigInt(lastMessage.counter)) & 0xFFFFFFFFFFFFn).toString(16).padStart(12, "0");
  const alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
  return `msg_${time}${Array.from(crypto.randomBytes(14), byte => alphabet[byte % alphabet.length]).join("")}`;
}

// Until the MCP servers of the folder a conversation works in are connected, twenty seconds at
// most, as the app and the engine's watchdog wait before a wake-up: one sent sooner reaches a
// model without their tools.
async function waitForTools(port, authorization, directory, signal) {
  const started = Date.now();
  let waited = false;
  while (Date.now() - started < 20_000) {
    let listed;
    try {
      const response = await fetch(`http://127.0.0.1:${port}/api/mcp`, { headers: { authorization, "x-opencode-directory": encodeURIComponent(directory) }, signal });
      if (!response.ok) return;
      listed = await response.text();
    } catch {
      return;
    }
    // A folder it has only just opened lists none for a moment before they're pending.
    const none = !listed.includes('"status"');
    if (!listed.includes('"pending"') && !(none && Date.now() - started < 3_000)) {
      if (waited && !none) await sleep(1_500);
      return;
    }
    waited = true;
    await sleep(none ? 100 : 250);
  }
}

// Wake-ups being sent with Wake Up Now, so a second tap doesn't send one twice.
const wakingNow = new Set();

// `kuvo/wakes/<session>`: its wake-ups, watches and pause; `…/cancel` with {"wake"} or {"watch"};
// `…/now` with {"wake"}, sent at once (one that repeats still comes back at its time).
async function* answerWakes(session, action, request, readEngine, bridge, signal) {
  const json = (status, value) => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const method = request.method.toUpperCase();
  const folder = wakeFolder(session);
  if (!action) {
    if (method !== "GET" && method !== "HEAD") return yield* json(405, {});
    if (!folder) return yield* json(200, { wakes: [], watches: [], paused: false });
    const { wakes, watches, paused } = readWakes(folder);
    return yield* json(200, { wakes: wakes.map(({ id, due, note, every }) => ({ id, due, note, every: every ?? undefined })), watches, paused });
  }
  if (method !== "POST") return yield* json(405, {});
  if (!folder) return yield* json(404, {});
  // On its way to another computer (a move holds its wake-ups, as the Mac's Moves.hold): nothing
  // reaches it or changes here until it's there, as on the Mac.
  if ((textOf(path.join(folder, "paused")) ?? "").trim() === "moving") {
    return yield* json(409, { message: "It's moving to another computer. Try again once it's there." });
  }
  let given = {};
  try {
    given = JSON.parse(Buffer.from(request.body ?? []).toString("utf8") || "{}") ?? {};
  } catch {}
  const { wakes, watches } = readWakes(folder);
  const wake = typeof given.wake === "string" ? wakes.find(each => each.id === given.wake || each.letters === given.wake) : undefined;
  if (action === "cancel") {
    if (wake && LETTERS.test(wake.letters)) {
      cancelWake(folder, wake.letters);
      return yield* json(200, {});
    }
    const watch = typeof given.watch === "string" ? watches.find(each => each.letters === given.watch) : undefined;
    if (watch && LETTERS.test(watch.letters)) {
      fs.rmSync(path.join(folder, "watches", watch.letters), { recursive: true, force: true });
      return yield* json(200, {});
    }
    return yield* json(404, {});
  }
  if (!wake) return yield* json(404, {});
  const key = `${folder}/${wake.id}`;
  if (wakingNow.has(key)) return yield* json(200, {});
  wakingNow.add(key);
  try {
    // The whole note, as its time would send it, not the line it's listed by.
    const note = textOf(path.join(folder, "wakes", wake.id));
    if (note === null) return yield* json(404, {});
    const agents = (textOf(path.join(folder, "via")) ?? "").trim() === "bridge";
    let target = null;
    try {
      target = agents ? bridge() : readEngine();
    } catch {}
    if (!target) return yield* json(503, { message: agents ? "The coding agents aren't running on this machine." : "The engine isn't running." });
    const authorization = `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`;
    const directory = textOf(path.join(folder, "directory"));
    if (!agents && directory) await waitForTools(target.port, authorization, directory, signal);
    let response;
    try {
      response = await fetch(`http://127.0.0.1:${target.port}/api/session/${encodeURIComponent(session)}/prompt`, {
        method: "POST",
        headers: { authorization, "content-type": "application/json" },
        body: JSON.stringify({ id: messageID(), text: WAKE_MARKER + note, delivery: "queue" }),
        signal,
      });
    } catch {
      return yield* json(503, { message: "The wake-up couldn't be sent." });
    }
    if (!response.ok) return yield* json(503, { message: `The wake-up couldn't be sent (${response.status}).` });
    if (!wake.every) cancelWake(folder, wake.letters);
    return yield* json(200, {});
  } finally {
    wakingNow.delete(key);
  }
}

// ---- Notifications ----

// While no Mac is telling your phones what happens in this machine's sessions, the bridge does:
// a permission or a question asked, a run finished or failed, a bot's news (PROTOCOL.md,
// "Notifications"). A Mac connected to the engine touches a file of its own in run/present
// each minute, and tells your phones itself for two minutes after; the bridge waits longer.
const PRESENT = path.join(RUN, "present");
const PRESENT_MS = 150_000;
export const NOTICE_LIMIT = 2500;

export function macPresent(folder = PRESENT, now = Date.now()) {
  let names;
  try {
    names = fs.readdirSync(folder);
  } catch {
    return false;
  }
  return names.some(name => {
    try {
      return now - fs.statSync(path.join(folder, name)).mtimeMs < PRESENT_MS;
    } catch {
      return false;
    }
  });
}

// The characters of `text` that fit in `bytes` of UTF-8.
function prefixBytes(text, bytes) {
  let out = "", count = 0;
  for (const character of text) {
    count += Buffer.byteLength(character);
    if (count > bytes) break;
    out += character;
  }
  return out;
}

const cut = (text, count) => text.length <= count ? text : text.slice(0, count - 1) + "…";

// A notification's JSON, under NOTICE_LIMIT bytes: what doesn't fit of the body ends in "…".
export function noticeJSON(notice) {
  const value = { t: "notify", title: cut(String(notice.title ?? ""), 200), category: notice.category, thread: String(notice.thread ?? ""), data: notice.data ?? {} };
  if (notice.subtitle) value.subtitle = cut(String(notice.subtitle), 200);
  let body = String(notice.body ?? "");
  while (true) {
    const json = Buffer.from(sortedJSON({ ...value, body }));
    if (json.length <= NOTICE_LIMIT || !body) return json;
    const room = Buffer.byteLength(body) - (json.length - NOTICE_LIMIT) - Buffer.byteLength("…");
    body = room <= 0 ? "" : prefixBytes(body, room).trimEnd() + "…";
  }
}

// The relay's push of `json` to a phone ({key, address}, base64url), sealed for it alone.
export function pushTo(phone, json, hostKey) {
  const mail = frame(Frame.mail, 0, sealOneWay(json, hostKey, Buffer.from(phone.key, "base64url")).message);
  return relayPush(Buffer.from(phone.address, "base64url"), mail);
}

// Markdown's marks taken off, not the words (as ReplyPreview does on the Mac): headings and
// quotes at a line's start, emphasis around words, code ticks, a link's address.
const plain = text => String(text ?? "")
  .replace(/^[ \t]{0,3}(#{1,6}|>+)[ \t]+/gm, "")
  .replace(/!?\[([^\]]*)\]\([^)\s]*\)/g, "$1")
  .replace(/(?<![\w*_])[*_]{1,3}(?=\S)|(?<=\S)[*_]{1,3}(?![\w*_])/g, "")
  .replace(/`/g, "")
  .replace(/\s+/g, " ").trim();

// "Wants to run a command: npm test", as the Mac says it.
export function describePermission(request) {
  const what = { edit: "edit files", write: "edit files", patch: "edit files", shell: "run a command", bash: "run a command", read: "read a file", external_directory: "work outside the project", webfetch: "open a web page" }[request.action] ?? request.action;
  const resource = request.resources?.[0];
  return !resource || resource === "*" ? `Wants to ${what}.` : `Wants to ${what}: ${resource}`;
}

// What a form asks, as the Mac's notification says it.
export function describeForm(form) {
  const kind = form.metadata?.kind;
  if (kind === "mcp-elicitation") return form.metadata?.message || "Needs your answer.";
  if (kind !== "question") return "Needs your answer.";
  const fields = (form.fields ?? []).filter(field => !field.hidden && !field.when?.length);
  if (fields.length === 1 && fields[0].type === "string") return fields[0].description || fields[0].title || form.title || "Has a question for you.";
  return "Has a question for you.";
}

const isBot = session => Boolean(session.metadata?.kuvo?.bot) || /\/bots\/bot_[^/]+\//.test(`${session.location?.directory ?? ""}/`);

// Asks the engine for `route` (a path under its API), as JSON.
function engineGet(engine, route) {
  return fetch(`http://127.0.0.1:${engine.port}/${route}`, { headers: { authorization: `Basic ${Buffer.from(`opencode:${engine.password}`).toString("base64")}` }, signal: AbortSignal.timeout(10_000) })
    .then(response => response.ok ? response.json() : Promise.reject(new Error(`${response.status}`)));
}

// The notification for an engine event, or null for one that isn't news. `get(route)` asks the
// engine.
export async function noticeFor(event, get) {
  const data = event.data ?? {};
  const session = async id => (await get(`api/session/${encodeURIComponent(id)}`)).data;
  // A subagent's request shows in the session that started it.
  const shown = async id => {
    let current = await session(id);
    for (let step = 0; step < 5 && current.parentID; step++) current = await session(current.parentID);
    return current;
  };
  // A form an MCP server asks for is filed under no session ("global"): it's the running session
  // of its folder with a tool of that server under way, as the Mac finds it; null for none.
  const waitingOn = async (server, folder) => {
    if (!server) return null;
    const active = (await get("api/session/active").catch(() => null))?.data ?? {};
    const name = String(server).replace(/[^\p{L}\p{N}_-]/gu, "_");
    for (const id of Array.isArray(active) ? active : Object.keys(active)) {
      const candidate = await session(id).catch(() => null);
      if (!candidate || (folder && candidate.location?.directory !== folder)) continue;
      const page = await get(`api/session/${encodeURIComponent(id)}/message?limit=8&order=desc`).catch(() => null);
      const steps = (page?.data ?? []).filter(message => message.type === "assistant").flatMap(message => message.content ?? []);
      if (steps.some(block => block.type === "tool" && block.state?.status === "running"
        && (block.name?.startsWith(name + "_") || (block.name === "execute" && String(block.state.input?.code ?? "").includes(server))))) return id;
    }
    return null;
  };
  const about = (shown, category, body, extra = {}) => {
    const bot = isBot(shown);
    const folder = shown.location?.directory ?? "";
    return {
      title: shown.title || "New Session",
      subtitle: bot ? undefined : path.basename(folder) || undefined,
      body,
      category,
      thread: shown.id,
      data: { session: shown.id, ...extra },
    };
  };
  switch (event.type) {
    case "permission.asked": {
      const where = await shown(data.sessionID);
      return about(where, "permission", describePermission(data), { request: data.id, ...(where.id !== data.sessionID ? { requestSession: data.sessionID } : {}) });
    }
    case "form.created": {
      // The form comes under `form`, with its session.
      const form = data.form ?? data;
      const asker = form.sessionID === "global" ? await waitingOn(form.metadata?.server, event.location?.directory) : form.sessionID;
      if (!asker) return null;
      const where = await shown(asker);
      return about(where, "question", describeForm(form), { request: form.id, ...(where.id !== form.sessionID ? { requestSession: form.sessionID } : {}) });
    }
    case "session.execution.succeeded":
    case "session.execution.failed": {
      const id = data.sessionID;
      const ended = await session(id);
      // A subagent's run is part of its parent's.
      if (ended.parentID) return null;
      if (event.type === "session.execution.failed") return about(ended, "failed", "Stopped with an error.");
      let reply = "";
      try {
        const page = await get(`api/session/${encodeURIComponent(id)}/message?limit=6&order=desc`);
        for (const message of page.data ?? []) {
          if (message.type !== "assistant") continue;
          reply = plain((message.content ?? []).filter(part => part.type === "text").map(part => part.text).join(" "));
          if (reply) break;
        }
      } catch {}
      // What it said last, as long as the Mac shows it.
      return about(ended, isBot(ended) ? "report" : "finished", reply ? cut(reply, 220) : "Finished.");
    }
    default:
      return null;
  }
}

// The events in a `text/event-stream` body, parsed.
async function* serverEvents(body) {
  const decoder = new TextDecoder();
  let buffer = "";
  let data = [];
  for await (const chunk of body) {
    buffer += decoder.decode(chunk, { stream: true });
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end).replace(/\r$/, "");
      buffer = buffer.slice(end + 1);
      if (line === "") {
        if (data.length) {
          try { yield JSON.parse(data.join("\n")); } catch {}
        }
        data = [];
      } else if (line.startsWith("data:")) {
        data.push(line.slice(5).replace(/^ /, ""));
      }
    }
  }
}

// The events that can be news.
const NEWS = new Set(["permission.asked", "form.created", "session.execution.succeeded", "session.execution.failed"]);

// Follows the engine's events for as long as the bridge runs, handing each one's notification to
// `tell`, while no Mac is present.
async function watchEngine(tell) {
  let attempt = 0;
  while (true) {
    try {
      const engine = readEngineFile();
      const response = await fetch(`http://127.0.0.1:${engine.port}/api/event`, {
        headers: { authorization: `Basic ${Buffer.from(`opencode:${engine.password}`).toString("base64")}`, accept: "text/event-stream" },
      });
      if (!response.ok || !response.body) throw new Error(`${response.status}`);
      attempt = 0;
      for await (const event of serverEvents(response.body)) {
        if (!NEWS.has(event.type) || macPresent()) continue;
        noticeFor(event, route => engineGet(engine, route)).then(notice => notice && tell(notice)).catch(() => {});
      }
    } catch {}
    attempt += 1;
    await sleep(Math.min(30_000, 1000 * 2 ** Math.min(attempt, 5)));
  }
}

// ---- State ----

export function readLink() {
  try {
    return JSON.parse(fs.readFileSync(LINK_FILE, "utf8"));
  } catch {
    return null;
  }
}

export function writeLink(link) {
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  const temporary = `${LINK_FILE}.${process.pid}.tmp`;
  fs.writeFileSync(temporary, JSON.stringify(link, null, 2), { mode: 0o600 });
  fs.renameSync(temporary, LINK_FILE);
  // For the box's keeper, which keeps this bridge running while it's needed.
  if (needed(link)) fs.writeFileSync(NEEDED_FILE, "", { mode: 0o600 });
  else fs.rmSync(NEEDED_FILE, { force: true });
}

// One change to link.json at a time, here and from `setup` and `pair` (other processes): read,
// changed and written under a lock (run/.link.lock), so none writes over another's (a removal,
// say). `change` returns the link to write, or nothing to leave it. A lock left by one that was
// cut off goes after ten seconds; one not had in three goes ahead all the same.
const LOCK = path.join(RUN, ".link.lock");
export function updateLink(change) {
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  const nap = new Int32Array(new SharedArrayBuffer(4));
  let held = false;
  for (let i = 0; i < 300 && !held; i++) {
    try {
      fs.mkdirSync(LOCK);
      held = true;
    } catch {
      try { if (Date.now() - fs.statSync(LOCK).mtimeMs > 10_000) fs.rmdirSync(LOCK); } catch {}
      Atomics.wait(nap, 0, 0, 10);
    }
  }
  try {
    const changed = change(readLink());
    if (changed) writeLink(changed);
    return changed ?? null;
  } finally {
    if (held) try { fs.rmdirSync(LOCK); } catch {}
  }
}

// Nothing to answer any more (the last phone gone, a pairing run out, no link.json): the keeper
// starts this bridge no more, unless a pairing began meanwhile.
export function noLongerNeeded() {
  try { fs.rmSync(NEEDED_FILE, { force: true }); } catch {}
  if (needed(readLink())) try { fs.writeFileSync(NEEDED_FILE, "", { mode: 0o600 }); } catch {}
}

// Lets in phones in link.json, and a new one showing the pairing token (once, in time), noting
// where it is on the relay, for its notifications. Where a paired one is changes once its session
// proves its own (confirmPhone): the first handshake message can be replayed from elsewhere.
export function authorizePhone(key, hello, address) {
  const encoded = key.toString("base64url");
  if (!hello?.pair) return Boolean(readLink()?.phones?.some(phone => phone.key === encoded));
  // Showing the token: a pairing of its own, paired before or not (one removed comes back).
  let paired = false;
  updateLink(link => {
    const pairing = link?.pairing;
    if (!pairing || Date.now() > pairing.expires) return null;
    const given = Buffer.from(String(hello.pair), "base64url");
    const expected = Buffer.from(pairing.token, "base64url");
    if (given.length !== expected.length || !crypto.timingSafeEqual(given, expected)) return null;
    pairPhone(link, { key: encoded, name: String(hello.name ?? "").slice(0, 100), added: Math.floor(Date.now() / 1000), from: "pair", address: address?.toString("base64url"), pairing: newPairing() });
    delete link.pairing;
    paired = true;
    return link;
  });
  return paired || Boolean(readLink()?.phones?.some(phone => phone.key === encoded));
}

// ---- Paired phones (PROTOCOL.md, "Paired phones") ----
// A pairing adds a phone; a removal, from a phone on the list or a Mac that set this machine up,
// takes one off for good (`removed`, with the pairing it ended): only a pairing of its own, with a
// new pairing id, lets it back in, and nobody removes a Mac. Each Mac's list adds and removes only
// the phones it listed. The Mac's are the same rules (PairedPhones in KuvoLink), and both are held
// to Vectors/link.json.

const KEY = /^[A-Za-z0-9_-]{43}$/;
const PAIRING = /^[A-Za-z0-9_-]{0,32}$/;
// At most so many in one removal, and so many removals kept (the oldest go first).
const MOST_AT_ONCE = 16;
const MOST_KEPT = 256;

// The Macs that set this machine up (`setup`), whose lists and mail it takes.
export const macsOf = link => [...new Set([...(Array.isArray(link?.macs) ? link.macs : []), ...(typeof link?.mac === "string" ? [link.mac] : [])])];

// When a Mac's last list came (its own clock, milliseconds).
const lastFrom = (link, mac) => link.macAts?.[mac] ?? (mac === link.mac ? link.macAt ?? 0 : 0);

// A Mac this machine hasn't heard from in 30 days, by its own clock (gone, or its key made anew):
// it no longer sets the list, and the phones it listed are as ones from before Macs were named,
// which another Mac's list takes off, not for good. One back sets the machine up again. `keep`,
// heard from now.
const MAC_GONE_AFTER = 30 * 24 * 3600 * 1000;
export function forgetOldMacs(link, keep, now = Date.now()) {
  link.macSeen ??= {};
  for (const mac of macsOf(link)) link.macSeen[mac] ??= now;
  if (keep) link.macSeen[keep] = now;
  const gone = macsOf(link).filter(mac => mac !== keep && now - link.macSeen[mac] > MAC_GONE_AFTER);
  if (!gone.length) return;
  link.macs = macsOf(link).filter(mac => !gone.includes(mac));
  if (gone.includes(link.mac)) link.mac = keep;
  for (const mac of gone) {
    delete link.macSeen[mac];
    if (link.macAts) delete link.macAts[mac];
  }
  for (const phone of link.phones ?? []) if (gone.includes(phone.mac)) delete phone.mac;
}

const removedAlready = (link, key, pairing) => (link.removed ?? []).some(r => r.key === key && r.pairing === pairing);

function noteRemoved(link, key, pairing) {
  link.removed ??= [];
  if (!removedAlready(link, key, pairing)) link.removed = [...link.removed, { key, pairing }].slice(-MOST_KEPT);
}

// A phone's key, and its pairing if given, as a removal or a list names it: never a Mac's.
const fits = (entry, macs) => typeof entry?.key === "string" && KEY.test(entry.key) && !macs.includes(entry.key)
  && (entry.pairing == null || (typeof entry.pairing === "string" && PAIRING.test(entry.pairing)));

// `key`'s pairing `pairing` (the one it has here, when not given) taken off: one paired again
// since, with another pairing, stays.
function takeOff(link, key, pairing) {
  const current = (link.phones ?? []).find(phone => phone.key === key);
  const which = pairing == null ? (current?.pairing ?? "") : String(pairing);
  noteRemoved(link, key, which);
  if (current && (current.pairing ?? "") === which) link.phones = link.phones.filter(phone => phone.key !== key);
}

// A new pairing id, made where a phone pairs.
export const newPairing = () => crypto.randomBytes(12).toString("base64url");

// A phone paired here (its pairing token): in, with a pairing of its own, which nothing removed.
// What removed its others still keeps them out.
export function pairPhone(link, phone) {
  link.phones = [...(link.phones ?? []).filter(each => each.key !== phone.key), phone];
}

// Phones `by` removes ({key, pairing}): only a phone on the list, or a Mac, removes, and only
// phones, a few at a time. False, changing nothing, otherwise.
export function removePhones(link, by, entries) {
  const macs = macsOf(link);
  const allowed = new Set([...(link.phones ?? []).map(phone => phone.key), ...macs]);
  if (!allowed.has(by) || !Array.isArray(entries) || !entries.length || entries.length > MOST_AT_ONCE) return false;
  if (!entries.every(entry => fits(entry, macs))) return false;
  for (const entry of entries) takeOff(link, entry.key, entry.pairing);
  return true;
}

// The phones the Mac `by` has paired as it lists them now (from: "mac"), and the removals it heard
// of, beside the ones paired with this machine itself (from: "pair") and other Macs', keeping when
// each was added and where it is. One of its own it no longer lists is removed; one removed here
// stays out, unless the Mac paired it again (another pairing). One from before Macs were named goes
// off the list, not for good: another Mac may list it.
export function takeMacList(link, given, removed, by = link.mac) {
  const macs = macsOf(link);
  const known = new Map((link.phones ?? []).map(phone => [phone.key, phone]));
  const ADDRESS = /^[A-Za-z0-9_-]{22}$/;
  const listed = [];
  for (const phone of Array.isArray(given) ? given : []) {
    if (!fits(phone, macs) || listed.some(each => each.key === phone.key)) continue;
    const pairing = phone.pairing ?? "";
    if (removedAlready(link, phone.key, pairing)) continue;
    listed.push({
      key: phone.key,
      name: String(phone.name ?? "").slice(0, 100),
      added: known.get(phone.key)?.added ?? Math.floor(Date.now() / 1000),
      from: "mac",
      mac: by,
      address: ADDRESS.test(phone.address ?? "") ? phone.address : known.get(phone.key)?.address,
      pairing,
    });
  }
  const keys = new Set(listed.map(phone => phone.key));
  const kept = [];
  for (const phone of link.phones ?? []) {
    if (keys.has(phone.key)) continue;
    if (phone.from === "pair" || (phone.mac != null && phone.mac !== by)) kept.push(phone);
    else if (by != null && phone.mac === by) noteRemoved(link, phone.key, phone.pairing ?? "");
  }
  link.phones = [...listed, ...kept];
  for (const entry of Array.isArray(removed) ? removed : []) {
    if (fits(entry, macs)) takeOff(link, entry.key, entry.pairing ?? "");
  }
}

// A Mac's phone list, mailed through the relay (sealed with its key, which its setup gave): one
// you removed there loses its way in here at once, though the Mac can't reach this machine
// itself. Only a newer list from that Mac than its last one counts.
export function phonesFromMac(body, host) {
  const found = readLink();
  if (!found?.key) return;
  let given, by;
  for (const mac of macsOf(found)) {
    try {
      given = JSON.parse(openOneWay(body, Buffer.from(found.key, "base64url"), Buffer.from(mac, "base64url")).payload.toString("utf8"));
      by = mac;
      break;
    } catch {}
  }
  if (!by || !Array.isArray(given?.phones) || typeof given.at !== "number") return;
  const changed = updateLink(link => {
    if (!link) return null;
    if (given.at <= lastFrom(link, by)) return null;
    takeMacList(link, given.phones, given.removed, by);
    link.macAts = { ...(link.macAts ?? {}), [by]: given.at };
    forgetOldMacs(link, by);
    return link;
  });
  if (changed) host?.keepOnly(new Set(changed.phones.map(phone => phone.key)));
}

// A phone's session proved its own: its notifications go where it is now.
export function confirmPhone(phone) {
  const where = phone.address?.toString("base64url");
  const known = readLink()?.phones?.find(each => each.key === phone.key.toString("base64url"));
  if (!known || !where || known.address === where) return;
  updateLink(link => {
    const known = link?.phones?.find(each => each.key === phone.key.toString("base64url"));
    if (!known || !where || known.address === where) return null;
    known.address = where;
    return link;
  });
}

// ---- The relay connection ----

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

// Connects once, serves until the connection ends. Resolves whether it got as far as welcome.
export function connectOnce(relay, secret, host) {
  return new Promise(resolve => {
    const self = addressOf(secret);
    let welcomed = false;
    let lastHeard = Date.now();
    const socket = new WebSocket(relay);
    socket.binaryType = "arraybuffer";
    // Something sent to ourselves comes back through the relay: that shows the connection's
    // alive, where WebSocket pings can't be seen from here.
    const check = setInterval(() => {
      if (Date.now() - lastHeard > 90_000) return finish();
      if (welcomed) socket.send(relaySend(self, EMPTY));
    }, 30_000);
    // Once, however it ends: Node's WebSocket reports a refused connection as an error alone,
    // with no close to follow, and one that never opens has to be given up on.
    let finished = false;
    const finish = () => {
      if (finished) return;
      finished = true;
      clearInterval(check);
      clearTimeout(opening);
      try { socket.close(); } catch {}
      host.reset();
      host.send = () => {};
      resolve(welcomed);
    };
    const opening = setTimeout(() => { if (!welcomed) finish(); }, 15_000);
    socket.onopen = () => socket.send(relayHello(secret));
    socket.onmessage = event => {
      lastHeard = Date.now();
      const message = readRelay(Buffer.from(event.data));
      if (!message) return;
      if (!welcomed) {
        if (message.kind !== "welcome" || !message.address.equals(self)) return socket.close();
        welcomed = true;
        host.send = bytes => { if (socket.readyState === 1) socket.send(bytes); };
        return;
      }
      if (message.kind === "deliver" && !message.address.equals(self)) host.receive(message.address, message.payload);
      if (message.kind === "unreachable" && !message.address.equals(self)) host.forget(message.address);
    };
    socket.onerror = finish;
    socket.onclose = finish;
  });
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

// Whether `pid` runs `name`, not whatever took its number after a crash: on Linux its command
// line says so (elsewhere, where there's no /proc, that it's alive).
function runs(pid, name) {
  if (!Number.isInteger(pid) || pid <= 1 || !alive(pid)) return false;
  try {
    return fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").includes(name);
  } catch {
    return !fs.existsSync("/proc");
  }
}

// Another bridge.
const isBridge = pid => pid !== process.pid && runs(pid, "kuvo-link.js");

// The box's keeper (`bin/kuvo-start`), started again if it went: it keeps the engine and this
// bridge running.
export function keepKeeper() {
  const start = path.join(K, "bin", "kuvo-start");
  if (!fs.existsSync(start)) return;
  let keeper = null;
  try { keeper = Number(fs.readFileSync(path.join(RUN, "box.pid"), "utf8")); } catch {}
  if (keeper && runs(keeper, "kuvo-start")) return;
  const environment = { ...process.env };
  delete environment.BUN_BE_BUN;
  delete environment.BUN_RUNTIME_TRANSPILER_CACHE_PATH;
  try {
    spawn("sh", [start], { cwd: K, env: environment, detached: true, stdio: "ignore" }).unref();
  } catch {}
}

// Something to answer: a paired phone, or a pairing code waiting.
const needed = link => Boolean(link && ((link.phones ?? []).length || (link.pairing && Date.now() < link.pairing.expires)));

async function run() {
  // One bridge at a time: a newer one (the engine restarted) takes over.
  try {
    const old = Number(fs.readFileSync(PID_FILE, "utf8"));
    if (isBridge(old)) process.kill(old, "SIGTERM");
  } catch {}
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  fs.writeFileSync(PID_FILE, String(process.pid), { mode: 0o600 });
  fs.writeFileSync(VERSION_FILE, BRIDGE_VERSION, { mode: 0o600 });
  const cleanUp = () => {
    try { if (fs.readFileSync(PID_FILE, "utf8") === String(process.pid)) fs.unlinkSync(PID_FILE); } catch {}
    process.exit(0);
  };
  process.on("SIGTERM", cleanUp);
  process.on("SIGINT", cleanUp);

  // Stops when nothing's paired any more, or when another bridge took over; up while the engine
  // is down too, so a phone can start it. Phones removed since lose what they have open, and the
  // box's keeper, if it went, starts again (it keeps this bridge, and the engine, running).
  let current = null;
  setInterval(() => {
    let owner = null;
    try { owner = Number(fs.readFileSync(PID_FILE, "utf8")); } catch {}
    const link = readLink();
    if (!needed(link)) {
      forgetPhones();
      noLongerNeeded();
    }
    if (!needed(link) || owner !== process.pid) return cleanUp();
    current?.keepOnly(new Set((link.phones ?? []).map(phone => phone.key)));
    keepKeeper();
  }, 30_000);

  // Your phones' notifications, on whichever relay connection is up.
  watchEngine(notice => {
    const link = readLink();
    if (!current || !link?.key) return;
    const json = noticeJSON(notice);
    const key = Buffer.from(link.key, "base64url");
    for (const phone of link.phones ?? []) if (phone.address) current.send(pushTo(phone, json, key));
  });

  let attempt = 0;
  while (true) {
    const link = readLink();
    if (!needed(link)) {
      forgetPhones();
      noLongerNeeded();
      return cleanUp();
    }
    let engine = {};
    try { engine = readEngineFile(); } catch {}
    const host = new LinkHost({
      key: Buffer.from(link.key, "base64url"),
      hello: { v: 1, kind: "machine", name: link.name || os.hostname(), app: BRIDGE_VERSION, engine: engine.version ?? null },
      authorize: authorizePhone,
      confirmed: confirmPhone,
      mail: (_from, body) => phonesFromMac(body, host),
      handle: engineHandler(readEngineFile, { name: link.name || os.hostname() }, readBridgeFile, changed => host.keepOnly(new Set((changed.phones ?? []).map(phone => phone.key)))),
      connect: engineConnect(readEngineFile),
      send: () => {},
    });
    current = host;
    const welcomed = await connectOnce(link.relay, Buffer.from(link.secret, "base64url"), host);
    attempt = welcomed ? 0 : attempt + 1;
    await sleep(Math.min(60_000, 1000 * 2 ** Math.min(attempt, 6)));
  }
}

function runningBridge() {
  try {
    const pid = Number(fs.readFileSync(PID_FILE, "utf8"));
    return isBridge(pid) ? pid : null;
  } catch {
    return null;
  }
}

// Starts the bridge on its own, outliving this command, from the machine's workspace.
function startBridge() {
  const workspace = path.join(K, "workspace");
  const child = spawn(process.execPath, [fileURLToPath(import.meta.url), "run"], {
    detached: true,
    stdio: "ignore",
    cwd: fs.existsSync(workspace) ? workspace : K,
    env: process.env,
  });
  child.unref();
}

// What the Mac sends: the relay, this machine's name, and the phones you've paired with the Mac,
// which may reach it directly. Keeps the machine's own keys (made the first time, and never
// leaving it) and phones paired with the machine itself, starts the bridge or restarts it (on
// another relay, or a newer one), and prints the machine's address and public key for the Mac
// to tell your phones. Nothing left to answer: the bridge stops, and the keys stay.
async function setup() {
  let input;
  try {
    const chunks = [];
    for await (const chunk of process.stdin) chunks.push(chunk);
    input = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    console.error("setup takes JSON on stdin: {relay, name, phones: [{key, name, address}]}");
    process.exit(2);
  }
  const phones = Array.isArray(input.phones) ? input.phones : [];
  const mac = typeof input.mac === "string" ? input.mac : undefined;
  const running = runningBridge();
  const old = readLink();
  let link = null;
  updateLink(current => {
    const next = current ?? { secret: crypto.randomBytes(32).toString("base64url"), key: newKey().toString("base64url"), phones: [] };
    // The Mac's key, which a phone list it mails later must come from, and its list, unless one it
    // sent later came first (by mail, as this was on its way).
    const at = typeof input.at === "number" ? input.at : 0;
    const by = mac ?? next.mac;
    if (!by || at > lastFrom(next, by)) {
      takeMacList(next, phones, input.removed, by);
      if (by) next.macAts = { ...(next.macAts ?? {}), [by]: at };
    }
    if (mac) {
      next.mac = mac;
      next.macs = [...new Set([...macsOf(next), mac])];
      forgetOldMacs(next, mac);
    }
    if (!next.phones.length) return current ? next : null;
    next.relay = String(input.relay);
    next.name = String(input.name || next.name || os.hostname()).slice(0, 100);
    link = next;
    return next;
  });
  // None paired any more (each removed for good, as one at a time would be): no bridge.
  if (!link) {
    if (running) process.kill(running, "SIGTERM");
    forgetPhones();
    console.log("{}");
    return;
  }
  // A new relay or name needs a new connection, and a newer bridge replaces an older one.
  let runningVersion = null;
  try { runningVersion = fs.readFileSync(VERSION_FILE, "utf8").trim(); } catch {}
  const restart = old?.relay !== link.relay || old?.name !== link.name || runningVersion !== BRIDGE_VERSION;
  if (running && restart) process.kill(running, "SIGTERM");
  if (!running || restart) startBridge();
  console.log(JSON.stringify({
    address: addressOf(Buffer.from(link.secret, "base64url")).toString("base64url"),
    key: publicOf(Buffer.from(link.key, "base64url")).toString("base64url"),
  }));
}

function pair(relay) {
  const link = updateLink(current => {
    const next = current ?? { secret: crypto.randomBytes(32).toString("base64url"), key: newKey().toString("base64url"), name: os.hostname(), phones: [] };
    if (relay) next.relay = relay;
    if (!next.relay) return null;
    next.pairing = { token: crypto.randomBytes(16).toString("base64url"), expires: Date.now() + PAIRING_MINUTES * 60_000 };
    return next;
  });
  if (!link) {
    console.error("Give the relay's address: kuvo-link.js pair wss://…");
    process.exit(1);
  }
  const query = new URLSearchParams({
    v: "1",
    relay: link.relay,
    host: addressOf(Buffer.from(link.secret, "base64url")).toString("base64url"),
    key: publicOf(Buffer.from(link.key, "base64url")).toString("base64url"),
    token: link.pairing.token,
    name: link.name,
  });
  console.log(`kuvo-link://pair?${query}`);
  console.error(`Open this link on your phone within ${PAIRING_MINUTES} minutes. It works once.`);
  if (!runningBridge()) startBridge();
}

// Checks this runtime has what the link needs, both ends of a session and a mail.
export function selftest() {
  const phone = newKey(), host = newKey();
  const initiator = new IKInitiator(phone, publicOf(host), PROLOGUE);
  const responder = new IKResponder(host, PROLOGUE);
  const said = responder.readMessage1(initiator.writeMessage1(Buffer.from("hello")));
  const { message, transport: hostSide } = responder.writeMessage2(Buffer.from("welcome"));
  const { payload, transport: phoneSide } = initiator.readMessage2(message);
  const echoed = hostSide.open(phoneSide.seal(Buffer.from("ping")));
  const mail = openOneWay(sealOneWay(Buffer.from("mail"), host, publicOf(phone)).message, phone, publicOf(host)).payload;
  const ok = said.toString() === "hello" && payload.toString() === "welcome" && echoed.toString() === "ping" && mail.toString() === "mail"
    && responder.rs.equals(publicOf(phone)) && typeof WebSocket === "function" && typeof fetch === "function";
  return ok;
}

const main = process.argv[1] && fs.realpathSync(process.argv[1]) === fs.realpathSync(fileURLToPath(import.meta.url));
if (main) {
  const [mode, argument] = process.argv.slice(2);
  if (mode === "run") await run();
  else if (mode === "setup") await setup();
  else if (mode === "pair") pair(argument);
  else if (mode === "selftest") {
    const ok = selftest();
    console.log(ok ? "ok" : "failed");
    process.exit(ok ? 0 : 1);
  } else {
    console.error("Usage: kuvo-link.js run | setup | pair [relay] | selftest");
    process.exit(2);
  }
}
