// The relay: one WebSocket per device, binary messages whose first byte says what they are
// (kuvo-ai's protocol/PROTOCOL.md, "The relay"). It passes payloads between addresses
// and sends phones' pushes. It keeps push registrations and setup leases, and never reads
// the encrypted payloads it carries.
import { createHash } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import type { IncomingMessage } from 'node:http';
import { WebSocketServer, type WebSocket } from 'ws';
import type { SetupLeases } from './leases.ts';
import type { Registrations } from './registrations.ts';

export const SECRET_LENGTH = 32;
export const ADDRESS_LENGTH = 16;
export const MAX_PAYLOAD = 65_600;

export const Kind = { Hello: 1, Send: 2, Unreachable: 3, Register: 4, Push: 5, Pushed: 6, Left: 7 } as const;
// Why a device that was sent to left (`Kind.Left`): another connection took its address, or its
// connection closed with none after it.
export const Left = { Replaced: 0, Gone: 1 } as const;
export const PushStatus = { Sent: 0, NoPhone: 1, NotAllowed: 2, Refused: 3 } as const;
export const Close = { Malformed: 4000, BadSecret: 4001, Replaced: 4002, TooFast: 4003, TooMany: 4005 } as const;

// Sends one push to Apple. Returns whether Apple took it; whether the token is gone for good; whether
// Apple says this environment doesn't know the token (`wrongEnvironment`: it may be the other's);
// and, when it didn't take it, why (Apple's reason, like "BadDeviceToken").
export type PushSender = (registration: { token: string; environment: string; topic: string }, payload: object) => Promise<{ ok: boolean; gone?: boolean; wrongEnvironment?: boolean; reason?: string }>;

// How pushes went since they were last counted: by the relay's answer, and Apple's reasons for
// those it refused. No tokens or addresses.
export interface PushCounts {
  sent: number;
  noPhone: number;
  notAllowed: number;
  refused: number;
  reasons: Record<string, number>;
}

export interface RelayOptions {
  registrations: Registrations;
  leases?: SetupLeases;
  leaseRelayHostIPs?: string[];
  push?: PushSender;
  // Messages a connection may send per second, on average; past that it's held back.
  messagesPerSecond?: number;
  // Connections from one IP address at once.
  connectionsPerAddress?: number;
  // What may wait to go out to a device before the senders to it are held back, and before
  // it's cut off as too slow to keep up.
  holdBytes?: number;
  dropBytes?: number;
  // Take the client's address from X-Forwarded-For (set when a proxy on this host ends TLS).
  trustProxy?: boolean;
  // How long a new connection has to say hello.
  helloTimeoutMs?: number;
  // Who a connection's leaving is told to, at most: those that sent to it, and those it sent to.
  listenersPerConnection?: number;
  targetsPerConnection?: number;
  pingIntervalMs?: number;
  // Monotonic milliseconds for connection activity (the relay tests advance this clock).
  pingClock?: () => number;
  // Where it says what's worth a look on the host (Apple refusing pushes, a token from the other
  // environment): never a token, an address or anything sent.
  log?: (line: string) => void;
}

export function addressOf(secret: Buffer): Buffer {
  return createHash('sha256').update('kuvo-link address v1').update(secret).digest().subarray(0, ADDRESS_LENGTH);
}

export const base64url = (bytes: Buffer) => bytes.toString('base64url');

interface Device {
  socket: WebSocket;
  address: string;
  addressBytes: Buffer;
  tokens: number;
  refilled: number;
  // While held back: the check that lets it go on, and the device it waits to drain, if any.
  held?: NodeJS.Timeout;
  waitingOn?: Device;
  // On this connection: the devices that sent to it, told when it leaves, and those it sent to.
  heardFrom: Set<Device>;
  sentTo: Set<Device>;
}

// Where a connection comes from, for counting connections per address.
function clientAddress(request: IncomingMessage, trustProxy: boolean): string {
  const forwarded = trustProxy ? request.headers['x-forwarded-for'] : undefined;
  // The proxy appends the address it saw, so the last one is the one to believe.
  const last = (Array.isArray(forwarded) ? forwarded.at(-1) : forwarded)?.split(',').at(-1)?.trim();
  return last || request.socket.remoteAddress || '?';
}

export function createRelay(options: RelayOptions): { server: Server; connections: () => number; pushCounts: () => PushCounts; close: () => Promise<void> } {
  const { registrations, push } = options;
  const log = options.log ?? (() => {});
  const fresh = (): PushCounts => ({ sent: 0, noPhone: 0, notAllowed: 0, refused: 0, reasons: {} });
  let counts = fresh();
  // Apple's reasons said once already, so each is logged the first time only (the counts carry the rest).
  const said = new Set<string>();
  // A phone sending a 10 MB file is ~220 messages at once, so the bucket holds a few such bursts.
  const rate = options.messagesPerSecond ?? 500;
  const perAddress = options.connectionsPerAddress ?? 64;
  const holdBytes = options.holdBytes ?? 2 * 1024 * 1024;
  const dropBytes = options.dropBytes ?? 8 * 1024 * 1024;
  const devices = new Map<string, Device>();
  const fromAddress = new Map<string, number>();
  const listenersAtMost = options.listenersPerConnection ?? 1024;
  const targetsAtMost = options.targetsPerConnection ?? 256;

  // A device that sent to another, to be told when that one leaves; past either limit, nothing
  // more is noted, and those devices just aren't told.
  function heard(from: Device, target: Device) {
    if (target.heardFrom.size >= listenersAtMost || from.sentTo.size >= targetsAtMost) return;
    target.heardFrom.add(from);
    from.sentTo.add(target);
  }

  // A connection leaving, once: each device that sent to it on it, still connected, is told why
  // (`Kind.Left`), and it's forgotten on both sides. A device comes back to the list only by
  // sending again.
  function leave(device: Device, why: number) {
    if (device.heardFrom.size === 0 && device.sentTo.size === 0) return;
    const notice = Buffer.concat([Buffer.from([Kind.Left]), device.addressBytes, Buffer.from([why])]);
    for (const listener of device.heardFrom) {
      listener.sentTo.delete(device);
      if (listener.socket.readyState === listener.socket.OPEN && devices.get(listener.address) === listener) listener.socket.send(notice);
    }
    for (const target of device.sentTo) target.heardFrom.delete(device);
    device.heardFrom.clear();
    device.sentTo.clear();
  }

  const server = createServer((request, response) => {
    if (request.url === '/health') {
      response.writeHead(200, { 'content-type': 'text/plain' }).end('ok');
    } else if (request.url === '/setup-lease' && options.leases) {
      const address = clientAddress(request, options.trustProxy === true).replace(/^::ffff:/, '');
      const relayHost = options.leaseRelayHostIPs?.includes(address) ?? false;
      void options.leases.handle(request, response, relayHost).catch(() => {
        log('Could not read or save a setup lease.');
        if (!response.headersSent) response.writeHead(503).end();
      });
    } else {
      response.writeHead(404).end();
    }
  });
  const sockets = new WebSocketServer({ server, maxPayload: MAX_PAYLOAD + ADDRESS_LENGTH + 1 });

  // ---- Keeping connections honest: pings, and a token bucket per device ----
  // A connection is there while it answers pings, or sends: a phone sending a picture on a slow
  // connection has its pong behind it, later than the next ping (#144).
  const pingInterval = options.pingIntervalMs ?? 30_000;
  const pingClock = options.pingClock ?? (() => performance.now());
  const lastActivity = new WeakMap<WebSocket, number>();
  const noteActivity = (socket: WebSocket) => lastActivity.set(socket, pingClock());
  const pinger = setInterval(() => {
    const now = pingClock();
    for (const socket of sockets.clients) {
      // A single late pong or binary heartbeat is not a lost connection. Reading we have
      // paused ourselves is not missing activity, either.
      if (!socket.isPaused && now - (lastActivity.get(socket) ?? now) >= 3 * pingInterval) {
        socket.terminate();
        continue;
      }
      socket.ping();
    }
  }, pingInterval);
  pinger.unref();

  function refill(device: Device) {
    const now = Date.now();
    device.tokens = Math.min(rate * 2, device.tokens + ((now - device.refilled) / 1000) * rate);
    device.refilled = now;
  }

  // A device sending faster than it may, or faster than the one it sends to takes it in, stops
  // being read until that passes: TCP then slows it down, and nothing piles up here.
  function blocked(device: Device): boolean {
    refill(device);
    const target = device.waitingOn;
    const waiting = target !== undefined && devices.get(target.address) === target && target.socket.bufferedAmount > holdBytes / 2;
    if (!waiting) device.waitingOn = undefined;
    return device.tokens < 1 || waiting;
  }

  function holdBack(device: Device) {
    if (device.held) return;
    device.socket.pause();
    const check = () => {
      if (device.socket.readyState !== device.socket.OPEN) return;
      if (blocked(device)) { device.held = setTimeout(check, 20); return; }
      device.held = undefined;
      // Give the socket a full activity window after we start reading it again.
      noteActivity(device.socket);
      device.socket.resume();
    };
    device.held = setTimeout(check, 20);
  }

  sockets.on('connection', (socket, request) => {
    const client = clientAddress(request, options.trustProxy ?? false);
    const count = (fromAddress.get(client) ?? 0) + 1;
    fromAddress.set(client, count);
    let counted = true;
    const uncount = () => {
      if (!counted) return;
      counted = false;
      const left = (fromAddress.get(client) ?? 1) - 1;
      if (left > 0) fromAddress.set(client, left); else fromAddress.delete(client);
    };
    if (count > perAddress) {
      uncount();
      return socket.close(Close.TooMany, 'too many connections');
    }
    noteActivity(socket);
    socket.on('pong', () => noteActivity(socket));
    let device: Device | undefined;
    const helloTimer = setTimeout(() => socket.close(Close.Malformed, 'hello first'), options.helloTimeoutMs ?? 10_000);

    socket.on('message', (data, isBinary) => {
      noteActivity(socket);
      const message = Buffer.isBuffer(data) ? data : Buffer.concat(data as Buffer[]);
      if (!isBinary || message.length === 0) return socket.close(Close.Malformed, 'binary only');
      const kind = message[0];

      if (!device) {
        if (kind !== Kind.Hello) return socket.close(Close.Malformed, 'hello first');
        if (message.length !== 1 + SECRET_LENGTH) return socket.close(Close.BadSecret, 'bad secret');
        clearTimeout(helloTimer);
        const addressBytes = addressOf(message.subarray(1));
        const address = base64url(addressBytes);
        // A device that reconnects before its old connection is noticed gone takes over. Those
        // that sent to the old one are told first, in this same pass: before anything from the
        // new one can reach them.
        const replaced = devices.get(address);
        if (replaced) {
          leave(replaced, Left.Replaced);
          replaced.socket.close(Close.Replaced, 'replaced');
        }
        device = { socket, address, addressBytes, tokens: rate * 2, refilled: Date.now(), heardFrom: new Set(), sentTo: new Set() };
        devices.set(address, device);
        socket.send(Buffer.concat([Buffer.from([Kind.Hello]), addressBytes]));
        return;
      }

      // A connection another took over, closing: what it still sends is from before, and goes
      // nowhere (those it sent to were told it was replaced, and were forgotten).
      if (devices.get(device.address) !== device) return;

      // Messages already read when it was held back still come; one that keeps on regardless
      // is cut off.
      refill(device);
      device.tokens -= 1;
      if (device.tokens < -rate) {
        // Held back, it isn't read, so its answer to the close never comes: it's dropped a
        // moment after being told why, rather than keeping its place for ws's 30 seconds.
        socket.close(Close.TooFast, 'too fast');
        setTimeout(() => socket.terminate(), 1000).unref();
        return;
      }
      if (device.tokens < 1) holdBack(device);
      const from = device;
      switch (kind) {
        case Kind.Send: {
          if (message.length < 1 + ADDRESS_LENGTH) return socket.close(Close.Malformed, 'short');
          const to = message.subarray(1, 1 + ADDRESS_LENGTH);
          const target = devices.get(base64url(to));
          if (target) {
            target.socket.send(Buffer.concat([Buffer.from([Kind.Send]), from.addressBytes, message.subarray(1 + ADDRESS_LENGTH)]));
            // Not to itself: a bridge sends itself one now and then to see it's still connected.
            if (target !== from) heard(from, target);
            const waiting = target.socket.bufferedAmount;
            if (waiting > dropBytes) {
              // What's waiting for it goes too: closing politely would keep it until sent.
              target.socket.terminate();
            } else if (waiting > holdBytes) {
              from.waitingOn = target;
              holdBack(from);
            }
          } else {
            socket.send(Buffer.concat([Buffer.from([Kind.Unreachable]), to]));
          }
          return;
        }
        case Kind.Register: {
          let body: unknown;
          try { body = JSON.parse(message.subarray(1).toString('utf8')); } catch { return socket.close(Close.Malformed, 'bad registration'); }
          if (!registrations.set(from.address, body)) return socket.close(Close.Malformed, 'bad registration');
          return;
        }
        case Kind.Push: {
          if (message.length < 1 + ADDRESS_LENGTH + 1) return socket.close(Close.Malformed, 'short');
          const to = message.subarray(1, 1 + ADDRESS_LENGTH);
          const payload = message.subarray(1 + ADDRESS_LENGTH);
          void sendPush(from, to, payload);
          return;
        }
        default:
          return socket.close(Close.Malformed, 'unknown kind');
      }
    });

    socket.on('close', () => {
      clearTimeout(helloTimer);
      clearTimeout(device?.held);
      uncount();
      // Still its address's connection (one replaced was dealt with as it was): gone.
      if (device && devices.get(device.address) === device) {
        devices.delete(device.address);
        leave(device, Left.Gone);
      }
    });
    socket.on('error', () => socket.terminate());
  });

  async function sendPush(from: Device, to: Buffer, payload: Buffer) {
    const answer = (status: number) => {
      counts[(['sent', 'noPhone', 'notAllowed', 'refused'] as const)[status]!] += 1;
      if (from.socket.readyState === from.socket.OPEN) from.socket.send(Buffer.concat([Buffer.from([Kind.Pushed]), to, Buffer.from([status])]));
    };
    const refused = (reason: string, environment: string) => {
      counts.reasons[reason] = (counts.reasons[reason] ?? 0) + 1;
      if (said.has(reason)) return;
      said.add(reason);
      log(`Apple refused a push (${environment}): ${reason}`);
    };
    const address = base64url(to);
    const registration = registrations.get(address);
    if (!registration || !push) return answer(PushStatus.NoPhone);
    if (!registration.senders.includes(from.address)) return answer(PushStatus.NotAllowed);
    // What Apple and anyone between see: that something happened. The phone decrypts the rest.
    const notification = {
      aps: { alert: { title: 'Kuvo Code', body: 'New activity' }, 'mutable-content': 1, sound: 'default' },
      f: from.address,
      k: payload.toString('base64'),
    };
    // Which environment answered last, for the log.
    let environment: string = registration.environment;
    try {
      let result = await push(registration, notification);
      // A token the phone said was the other environment's (a build signed for development that
      // took itself for the App Store's, or the other way): Apple's other host takes it, and the
      // registration keeps the one that did. Neither knowing it, it's gone. (Only that token's:
      // a phone that registered another meanwhile keeps its own.)
      if (!result.ok && result.wrongEnvironment) {
        const other = registration.environment === 'production' ? 'sandbox' : 'production';
        environment = other;
        const retried = await push({ ...registration, environment: other }, notification);
        if (retried.ok && registrations.setEnvironment(address, registration.token, other)) {
          log(`A phone's push token was ${other}'s, not ${registration.environment}'s: sent there, and kept so`);
        }
        result = retried.ok ? retried : { ...retried, gone: retried.gone || retried.wrongEnvironment };
      }
      if (!result.ok) refused(result.reason ?? 'unknown', environment);
      if (result.gone) registrations.delete(address, registration.token);
      answer(result.ok ? PushStatus.Sent : PushStatus.Refused);
    } catch {
      refused('failed', environment);
      answer(PushStatus.Refused);
    }
  }

  return {
    server,
    connections: () => devices.size,
    // How pushes went since the last time this was asked.
    pushCounts: () => {
      const taken = counts;
      counts = fresh();
      return taken;
    },
    close: async () => {
      clearInterval(pinger);
      for (const socket of sockets.clients) socket.terminate();
      await new Promise<void>(resolve => sockets.close(() => resolve()));
      await new Promise<void>(resolve => server.close(() => resolve()));
    },
  };
}
