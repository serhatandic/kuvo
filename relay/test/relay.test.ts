import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import type { AddressInfo } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { after, before, test, type TestContext } from 'node:test';
import { WebSocket as Client } from 'ws';
import { addressOf, createRelay, type PushSender } from '../src/relay.ts';
import { Registrations, parseRegistration } from '../src/registrations.ts';

// ---- A relay on a free port, with a push sender that records instead of calling Apple ----
const pushes: { registration: { token: string }; payload: any }[] = [];
const fakePush: PushSender = async (registration, payload) => {
  pushes.push({ registration, payload });
  return { ok: registration.token !== 'dead'.repeat(16), gone: registration.token === 'dead'.repeat(16) };
};
const relay = createRelay({ registrations: new Registrations(), push: fakePush, messagesPerSecond: 50, pingIntervalMs: 60_000 });
let url = '';
before(async () => {
  await new Promise<void>(resolve => relay.server.listen(0, '127.0.0.1', resolve));
  url = `ws://127.0.0.1:${(relay.server.address() as AddressInfo).port}`;
});
after(() => relay.close());

// A device: a WebSocket that's said hello, with what it received queued up.
interface Device { address: Buffer; send(bytes: Buffer): void; next(): Promise<Buffer>; closed: Promise<number>; socket: WebSocket }

async function device(secret = randomBytes(32), at = url): Promise<Device> {
  const socket = new WebSocket(at);
  socket.binaryType = 'arraybuffer';
  const queue: Buffer[] = [];
  const waiting: ((b: Buffer) => void)[] = [];
  socket.onmessage = event => {
    const bytes = Buffer.from(event.data as ArrayBuffer);
    const resolve = waiting.shift();
    if (resolve) resolve(bytes); else queue.push(bytes);
  };
  const closed = new Promise<number>(resolve => { socket.onclose = event => resolve(event.code); });
  await new Promise(resolve => { socket.onopen = resolve; });
  const next = () => new Promise<Buffer>((resolve, reject) => {
    const queued = queue.shift();
    if (queued) return resolve(queued);
    waiting.push(resolve);
    setTimeout(() => reject(new Error('nothing arrived')), 2000).unref();
  });
  socket.send(Buffer.concat([Buffer.from([1]), secret]));
  const welcome = await next();
  assert.equal(welcome[0], 1);
  assert.deepEqual(welcome.subarray(1), addressOf(secret));
  return { address: addressOf(secret), send: bytes => socket.send(bytes), next, closed, socket };
}

const bytes = (...parts: (number[] | Buffer | string)[]) => Buffer.concat(parts.map(p => typeof p === 'string' ? Buffer.from(p) : Buffer.isBuffer(p) ? p : Buffer.from(p)));

// Once the relay has answered something sent after it, it has taken in all a device sent before:
// it reads each connection's messages in order.
async function settled(device: Device) {
  const nobody = addressOf(randomBytes(32));
  device.send(bytes([2], nobody, 'ping'));
  assert.deepEqual(await device.next(), bytes([3], nobody));
}

test('the address matches the shared vectors', () => {
  const vectors = JSON.parse(readFileSync(new URL('../../protocol/vectors/link.json', import.meta.url), 'utf8'));
  for (const { secret, address } of vectors.addresses) {
    assert.equal(addressOf(Buffer.from(secret, 'hex')).toString('hex'), address);
  }
});

test('what one device sends reaches the other, marked with the sender', async () => {
  const phone = await device();
  const mac = await device();
  phone.send(bytes([2], mac.address, 'hello'));
  const delivered = await mac.next();
  assert.deepEqual(delivered, bytes([2], phone.address, 'hello'));
  mac.send(bytes([2], phone.address, 'back'));
  assert.deepEqual(await phone.next(), bytes([2], mac.address, 'back'));
  phone.socket.close();
  mac.socket.close();
});

test('sending to nobody says so', async () => {
  const phone = await device();
  const nobody = addressOf(randomBytes(32));
  phone.send(bytes([2], nobody, 'x'));
  assert.deepEqual(await phone.next(), bytes([3], nobody));
  phone.socket.close();
});

test('a reconnecting device replaces its old connection', async () => {
  const secret = randomBytes(32);
  const first = await device(secret);
  const second = await device(secret);
  assert.equal(await first.closed, 4002);
  const other = await device();
  other.send(bytes([2], second.address, 'to the new one'));
  assert.deepEqual(await second.next(), bytes([2], other.address, 'to the new one'));
  second.socket.close();
  other.socket.close();
});

test('a host replaced by a new connection is said so to each device that sent to it, first', async () => {
  const secret = randomBytes(32);
  const host = await device(secret);
  const phone = await device();
  const other = await device();
  phone.send(bytes([2], host.address, 'hi'));
  other.send(bytes([2], host.address, 'hi too'));
  await host.next();
  await host.next();

  const again = await device(secret);
  again.send(bytes([2], phone.address, 'back'));
  // Told it was replaced before anything from the new connection.
  assert.deepEqual(await phone.next(), bytes([7], host.address, [0]));
  assert.deepEqual(await phone.next(), bytes([2], host.address, 'back'));
  assert.deepEqual(await other.next(), bytes([7], host.address, [0]));
  assert.equal(await host.closed, 4002);
  for (const each of [again, phone, other]) each.socket.close();
});

test('a host whose connection closes is said gone, once, to those that sent to it', async () => {
  const host = await device();
  const phone = await device();
  const quiet = await device();
  phone.send(bytes([2], host.address, 'hi'));
  await host.next();
  host.socket.close();
  assert.deepEqual(await phone.next(), bytes([7], host.address, [1]));
  // Not told twice, and one that never sent to it isn't told at all.
  await settled(phone);
  await settled(quiet);
  for (const each of [phone, quiet]) each.socket.close();
});

test('a device that reconnected isn\'t told about what its old connection sent to', async () => {
  const secret = randomBytes(32);
  const host = await device();
  const phone = await device(secret);
  const witness = await device();
  phone.send(bytes([2], host.address, 'hi'));
  witness.send(bytes([2], host.address, 'hi'));
  await host.next();
  await host.next();
  const phoneAgain = await device(secret);
  await phone.closed;
  host.socket.close();
  // The relay has dealt with the host leaving once the witness hears of it.
  assert.deepEqual(await witness.next(), bytes([7], host.address, [1]));
  await settled(phoneAgain);
  for (const each of [phoneAgain, witness]) each.socket.close();
});

test('sending to itself notes nothing, and past its limits a connection notes no more', async () => {
  const { relay: small, url: at } = await smallRelay({ messagesPerSecond: 50, listenersPerConnection: 1, targetsPerConnection: 1 });
  try {
    const phone = await device(undefined, at);
    const host = await device(undefined, at);
    const another = await device(undefined, at);
    // To itself first: it doesn't take the one place it has.
    phone.send(bytes([2], phone.address, 'me'));
    await phone.next();
    phone.send(bytes([2], host.address, 'hi'));
    await host.next();
    // The host's one listener taken: another sending to it isn't noted.
    const extra = await device(undefined, at);
    extra.send(bytes([2], host.address, 'me too'));
    await host.next();
    // Its one place taken: sending to another notes nothing.
    phone.send(bytes([2], another.address, 'hi'));
    await another.next();
    another.socket.close();
    await another.closed;
    host.socket.close();
    assert.deepEqual(await phone.next(), bytes([7], host.address, [1]));
    await settled(phone);
    await settled(extra);
    phone.socket.close();
    extra.socket.close();
  } finally {
    await small.close();
  }
});

test('anything before hello, or a short secret, closes the connection', async () => {
  for (const [first, code] of [[bytes([2], randomBytes(16), 'x'), 4000], [bytes([1], randomBytes(8)), 4001]] as const) {
    const socket = new WebSocket(url);
    const closed = new Promise<number>(resolve => { socket.onclose = event => resolve(event.code); });
    await new Promise(resolve => { socket.onopen = resolve; });
    socket.send(first);
    assert.equal(await closed, code);
  }
});

test('a device a little over its rate is held back, and loses nothing', async () => {
  const sender = await device();
  const receiver = await device();
  // The bucket holds 100; 130 at once goes 30 over, which waits rather than closing.
  for (let i = 0; i < 130; i++) sender.send(bytes([2], receiver.address, `m${i}`));
  for (let i = 0; i < 130; i++) assert.deepEqual(await receiver.next(), bytes([2], sender.address, `m${i}`));
  sender.send(bytes([2], receiver.address, 'after'));
  assert.deepEqual(await receiver.next(), bytes([2], sender.address, 'after'));
  sender.socket.close();
  receiver.socket.close();
});

// A relay of its own, with small limits, for the tests that need them.
async function smallRelay(options: Partial<Parameters<typeof createRelay>[0]>) {
  const small = createRelay({ registrations: new Registrations(), pingIntervalMs: 60_000, ...options });
  await new Promise<void>(resolve => small.server.listen(0, '127.0.0.1', resolve));
  return { relay: small, url: `ws://127.0.0.1:${(small.server.address() as AddressInfo).port}` };
}

test('one address gets only so many connections', async () => {
  const { relay: small, url } = await smallRelay({ connectionsPerAddress: 3 });
  try {
    const sockets = await Promise.all([0, 1, 2].map(() => new Promise<Client>(resolve => {
      const socket = new Client(url);
      socket.on('open', () => resolve(socket));
    })));
    const fourth = new Client(url);
    assert.equal(await new Promise(resolve => fourth.on('close', resolve)), 4005);
    // One goes, and there's room again.
    sockets[0].close();
    await new Promise(resolve => sockets[0].on('close', resolve));
    const again = new Client(url);
    await new Promise(resolve => again.on('open', resolve));
    for (const socket of [...sockets, again]) socket.close();
  } finally {
    await small.close();
  }
});

test('a device that takes nothing in is cut off before it piles up', async () => {
  const { relay: small, url } = await smallRelay({ holdBytes: 16 * 1024, dropBytes: 96 * 1024, messagesPerSecond: 1000 });
  try {
    const hello = (socket: Client, secret: Buffer) => new Promise<void>(resolve => {
      socket.once('message', () => resolve());
      socket.on('open', () => socket.send(Buffer.concat([Buffer.from([1]), secret])));
    });
    const stuckSecret = randomBytes(32);
    const stuck = new Client(url);
    await hello(stuck, stuckSecret);
    const stuckClosed = new Promise(resolve => stuck.on('close', resolve));
    // It stops reading: everything for it waits on the relay.
    stuck.pause();
    const senders = await Promise.all(Array.from({ length: 8 }, async () => {
      const socket = new Client(url);
      await hello(socket, randomBytes(32));
      return socket;
    }));
    const chunk = bytes([2], addressOf(stuckSecret), randomBytes(60_000));
    for (let i = 0; i < 40; i++) for (const socket of senders) socket.send(chunk);
    for (let i = 0; i < 100 && small.connections() === 9; i++) await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(small.connections(), 8);
    stuck.resume();
    await stuckClosed;
    for (const socket of senders) socket.terminate();
  } finally {
    await small.close();
  }
});

test('a device that sends too fast is cut off', async () => {
  const flooder = await device();
  for (let i = 0; i < 200; i++) flooder.send(bytes([2], flooder.address, 'x'));
  assert.equal(await flooder.closed, 4003);
});

test('pushes reach only registered phones, from their own hosts', async () => {
  const phone = await device();
  const mac = await device();
  const stranger = await device();
  const mail = bytes([5, 0, 0, 0, 0], randomBytes(80));

  mac.send(bytes([5], phone.address, mail));
  assert.deepEqual(await mac.next(), bytes([6], phone.address, [1]));

  const token = 'ab'.repeat(32);
  phone.send(bytes([4], JSON.stringify({ token, environment: 'sandbox', topic: 'run.kuvo.code.ios', senders: [mac.address.toString('base64url')] })));
  await settled(phone);

  stranger.send(bytes([5], phone.address, mail));
  assert.deepEqual(await stranger.next(), bytes([6], phone.address, [2]));

  mac.send(bytes([5], phone.address, mail));
  assert.deepEqual(await mac.next(), bytes([6], phone.address, [0]));
  const sent = pushes.at(-1)!;
  assert.equal(sent.registration.token, token);
  assert.equal(sent.payload.f, mac.address.toString('base64url'));
  assert.deepEqual(Buffer.from(sent.payload.k, 'base64'), mail);
  assert.equal(sent.payload.aps['mutable-content'], 1);
  assert.equal(sent.payload.aps.alert.body, 'New activity');

  for (const d of [phone, mac, stranger]) d.socket.close();
});

test('a token Apple says is gone is forgotten', async () => {
  const phone = await device();
  const mac = await device();
  phone.send(bytes([4], JSON.stringify({ token: 'dead'.repeat(16), environment: 'production', topic: 't', senders: [mac.address.toString('base64url')] })));
  await settled(phone);
  mac.send(bytes([5], phone.address, [5, 0, 0, 0, 0, 1]));
  assert.deepEqual(await mac.next(), bytes([6], phone.address, [3]));
  mac.send(bytes([5], phone.address, [5, 0, 0, 0, 0, 1]));
  assert.deepEqual(await mac.next(), bytes([6], phone.address, [1]));
  phone.socket.close();
  mac.socket.close();
});

test('a token from the other environment goes there, the registration keeps it, and one neither knows is forgotten', async () => {
  // Apple as a build signed for development sees it: its token is the sandbox's alone.
  const asked: string[] = [];
  const sandboxOnly: PushSender = async registration => {
    asked.push(`${registration.token.slice(0, 4)} ${registration.environment}`);
    return registration.token.startsWith('cafe') && registration.environment === 'sandbox' ? { ok: true } : { ok: false, wrongEnvironment: true, reason: 'BadDeviceToken' };
  };
  const logged: string[] = [];
  const registrations = new Registrations();
  const own = createRelay({ registrations, push: sandboxOnly, log: line => logged.push(line), pingIntervalMs: 60_000 });
  await new Promise<void>(resolve => own.server.listen(0, '127.0.0.1', resolve));
  const at = `ws://127.0.0.1:${(own.server.address() as AddressInfo).port}`;
  try {
    const phone = await device(randomBytes(32), at);
    const mac = await device(randomBytes(32), at);
    const mail = bytes([5, 0, 0, 0, 0], randomBytes(40));
    // The phone took itself for the App Store's build.
    phone.send(bytes([4], JSON.stringify({ token: 'cafe'.repeat(16), environment: 'production', topic: 'run.kuvo.code.ios', senders: [mac.address.toString('base64url')] })));
    await settled(phone);
    mac.send(bytes([5], phone.address, mail));
    assert.deepEqual(await mac.next(), bytes([6], phone.address, [0]), 'it reaches the phone');
    assert.deepEqual(asked, ['cafe production', 'cafe sandbox']);
    assert.equal(registrations.get(phone.address.toString('base64url'))?.environment, 'sandbox');
    // The next goes straight there.
    mac.send(bytes([5], phone.address, mail));
    assert.deepEqual(await mac.next(), bytes([6], phone.address, [0]));
    assert.deepEqual(asked.slice(2), ['cafe sandbox']);
    assert.ok(logged.some(line => line.includes("sandbox's, not production's")));

    // A token neither environment knows: refused, said once, and forgotten.
    const other = await device(randomBytes(32), at);
    other.send(bytes([4], JSON.stringify({ token: 'beef'.repeat(16), environment: 'sandbox', topic: 'run.kuvo.code.ios', senders: [mac.address.toString('base64url')] })));
    await settled(other);
    mac.send(bytes([5], other.address, mail));
    assert.deepEqual(await mac.next(), bytes([6], other.address, [3]));
    mac.send(bytes([5], other.address, mail));
    assert.deepEqual(await mac.next(), bytes([6], other.address, [1]));
    // (Said for the environment that answered last: the other one, tried after the sandbox.)
    assert.deepEqual(logged.filter(line => line.startsWith('Apple refused')), ['Apple refused a push (production): BadDeviceToken']);
    // Counted, with no token or address in the log.
    assert.deepEqual(own.pushCounts(), { sent: 2, noPhone: 1, notAllowed: 0, refused: 1, reasons: { BadDeviceToken: 1 } });
    assert.deepEqual(own.pushCounts(), { sent: 0, noPhone: 0, notAllowed: 0, refused: 0, reasons: {} });
    for (const line of logged) assert.ok(!line.includes('cafe') && !line.includes('beef') && !line.includes(phone.address.toString('base64url')));
    for (const d of [phone, mac, other]) d.socket.close();
  } finally {
    await own.close();
  }
});

test('a phone that registers a new token while Apple turns down its old one keeps the new one', async () => {
  // Apple answers the old token only once the test lets it, and doesn't know it anywhere.
  let letAnswer!: () => void;
  const answered = new Promise<void>(resolve => { letAnswer = resolve; });
  let reachedApple!: () => void;
  const asked = new Promise<void>(resolve => { reachedApple = resolve; });
  const slow: PushSender = async registration => {
    if (registration.token.startsWith('01d0')) {
      reachedApple();
      await answered;
      return { ok: false, wrongEnvironment: true, reason: 'BadDeviceToken' };
    }
    return { ok: true };
  };
  const registrations = new Registrations();
  const own = createRelay({ registrations, push: slow, pingIntervalMs: 60_000 });
  await new Promise<void>(resolve => own.server.listen(0, '127.0.0.1', resolve));
  const at = `ws://127.0.0.1:${(own.server.address() as AddressInfo).port}`;
  try {
    const phone = await device(randomBytes(32), at);
    const mac = await device(randomBytes(32), at);
    const register = (token: string) => phone.send(bytes([4], JSON.stringify({ token, environment: 'production', topic: 't', senders: [mac.address.toString('base64url')] })));
    register('01d0'.repeat(16));
    await settled(phone);
    mac.send(bytes([5], phone.address, [5, 0, 0, 0, 0, 1]));
    // Reinstalled meanwhile: a new token at the same address.
    await asked;
    register('7e57'.repeat(16));
    await settled(phone);
    letAnswer();
    assert.deepEqual(await mac.next(), bytes([6], phone.address, [3]));
    assert.equal(registrations.get(phone.address.toString('base64url'))?.token, '7e57'.repeat(16), 'the new token stays');
    assert.equal(registrations.get(phone.address.toString('base64url'))?.environment, 'production');
    mac.send(bytes([5], phone.address, [5, 0, 0, 0, 0, 1]));
    assert.deepEqual(await mac.next(), bytes([6], phone.address, [0]));
    phone.socket.close();
    mac.socket.close();
  } finally {
    await own.close();
  }
});

test('registrations are checked and kept across restarts', () => {
  const sender = randomBytes(16).toString('base64url');
  assert.equal(parseRegistration({ token: 'zz', environment: 'sandbox', topic: 't', senders: [] }), null);
  assert.equal(parseRegistration({ token: 'ab'.repeat(16), environment: 'beta', topic: 't', senders: [] }), null);
  assert.equal(parseRegistration({ token: 'ab'.repeat(16), environment: 'sandbox', topic: 't', senders: ['short'] }), null);

  const folder = mkdtempSync(join(tmpdir(), 'kuvo-relay-test-'));
  try {
    const kept = new Registrations(folder);
    const phone = randomBytes(16).toString('base64url');
    assert.ok(kept.set(phone, { token: 'ab'.repeat(16), environment: 'sandbox', topic: 't', senders: [sender, sender] }));
    kept.flush();
    const reloaded = new Registrations(folder);
    assert.deepEqual(reloaded.get(phone)?.senders, [sender]);

    // The same token from another address (the app reinstalled) replaces the old entry.
    const again = randomBytes(16).toString('base64url');
    reloaded.set(again, { token: 'ab'.repeat(16), environment: 'sandbox', topic: 't', senders: [] });
    assert.equal(reloaded.get(phone), undefined);
    reloaded.flush();
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
});

test('registrations not sent again in a while go, and so do the oldest past the most kept', () => {
  let now = 1_000_000;
  const kept = new Registrations(undefined, { max: 2, lastsMs: 1000, now: () => now });
  const phones = [0, 1, 2].map(() => randomBytes(16).toString('base64url'));
  const body = (i: number) => ({ token: `${i}`.repeat(32), environment: 'sandbox', topic: 't', senders: [] });

  kept.set(phones[0], body(0));
  now += 10;
  kept.set(phones[1], body(1));
  now += 10;
  kept.set(phones[2], body(2));
  assert.equal(kept.size, 2);
  assert.equal(kept.get(phones[0]), undefined);

  // Sending it again keeps one going; the other runs out.
  now += 900;
  kept.set(phones[2], body(2));
  now += 200;
  assert.equal(kept.get(phones[1]), undefined);
  assert.ok(kept.get(phones[2]));
});

// ---- What a connection may and mayn't do ----

// A ws client that's said hello, for relays of their own.
function hello(socket: Client, secret = randomBytes(32)) {
  return new Promise<Buffer>(resolve => {
    socket.once('message', data => resolve(data as Buffer));
    socket.on('open', () => socket.send(Buffer.concat([Buffer.from([1]), secret])));
  });
}
const closeCode = (socket: Client) => new Promise<number>(resolve => socket.on('close', code => resolve(code)));

test('a connection that never says hello is closed', async () => {
  const { relay: small, url } = await smallRelay({ helloTimeoutMs: 100 });
  try {
    const silent = new Client(url);
    assert.equal(await closeCode(silent), 4000);
    assert.equal(small.connections(), 0);
  } finally {
    await small.close();
  }
});

test('text, an unknown kind or a bad registration closes the connection', async () => {
  for (const send of [
    (socket: WebSocket) => socket.send('hello as text'),
    (socket: WebSocket) => socket.send(bytes([9], 'what is this')),
    (socket: WebSocket) => socket.send(bytes([4], 'not json')),
  ]) {
    const phone = await device();
    send(phone.socket);
    assert.equal(await phone.closed, 4000);
  }
});

test('a device that left is unreachable, and stops counting', async () => {
  const phone = await device();
  const mac = await device();
  const before = relay.connections();
  mac.socket.close();
  await mac.closed;
  for (let i = 0; i < 50 && relay.connections() === before; i++) await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(relay.connections(), before - 1);
  phone.send(bytes([2], mac.address, 'still there?'));
  assert.deepEqual(await phone.next(), bytes([3], mac.address));
  phone.socket.close();
});

// The relay's clock is advanced explicitly; the WebSocket traffic still goes over loopback.
async function pingAfter(t: TestContext, socket: Client, closed: Promise<number>, elapsed = 100) {
  const pinged = once(socket, 'ping').then(() => true);
  t.mock.timers.tick(elapsed);
  assert.equal(await Promise.race([pinged, closed.then(() => false)]), true, 'the relay closed before its grace expired');
}

async function selfEcho(socket: Client, address: Buffer, closed: Promise<number>) {
  const message = once(socket, 'message').then(([data]) => data);
  socket.send(bytes([2], address));
  assert.deepEqual(await Promise.race([message, closed.then(() => null)]), bytes([2], address));
}

test('a pong arriving after one missed ping window keeps the connection', { timeout: 4000 }, async t => {
  t.mock.timers.enable({ apis: ['Date', 'setInterval'], now: 0 });
  const { relay: small, url } = await smallRelay({ pingIntervalMs: 100, pingClock: Date.now });
  try {
    const client = new Client(url, { autoPong: false });
    const closed = closeCode(client);
    await hello(client);
    await pingAfter(t, client, closed); // No pong in this window.
    await pingAfter(t, client, closed);
    // A late pong is activity. Fence it with a client ping and its automatic server pong.
    const acknowledged = once(client, 'pong');
    client.pong();
    client.ping();
    await acknowledged;
    await pingAfter(t, client, closed);
    await pingAfter(t, client, closed);
    assert.equal(small.connections(), 1);
  } finally {
    await small.close();
  }
});

test('a device that stops answering pings is dropped after three quiet windows', { timeout: 4000 }, async t => {
  t.mock.timers.enable({ apis: ['Date', 'setInterval'], now: 0 });
  const { relay: small, url } = await smallRelay({ pingIntervalMs: 100, pingClock: Date.now });
  try {
    const gone = new Client(url, { autoPong: false });
    const closed = closeCode(gone);
    await hello(gone);
    await pingAfter(t, gone, closed);
    await pingAfter(t, gone, closed);
    t.mock.timers.tick(100);
    assert.equal(await closed, 1006);
    assert.equal(small.connections(), 0);
  } finally {
    await small.close();
  }
});

test('the relay does not expire a connection while it holds back its reading', { timeout: 4000 }, async t => {
  t.mock.timers.enable({ apis: ['Date', 'setInterval'], now: 0 });
  const { relay: small, url } = await smallRelay({ pingIntervalMs: 100, pingClock: Date.now, messagesPerSecond: 1 });
  try {
    const client = new Client(url, { autoPong: false });
    const closed = closeCode(client);
    const secret = randomBytes(32);
    await hello(client, secret);
    const address = addressOf(secret);
    // The token bucket starts with two messages. The second one pauses the relay's reads.
    await selfEcho(client, address, closed);
    await selfEcho(client, address, closed);
    for (let window = 0; window < 6; window++) await pingAfter(t, client, closed);
    assert.equal(small.connections(), 1);
    // The refill at one second lets the queued request pass, on the same socket.
    const resumed = selfEcho(client, address, closed);
    t.mock.timers.tick(450);
    await resumed;
    assert.equal(small.connections(), 1);
  } finally {
    await small.close();
  }
});

test('both computer heartbeat cadences keep their connections even without pongs', { timeout: 4000 }, async t => {
  t.mock.timers.enable({ apis: ['Date', 'setInterval'], now: 0 });
  const { relay: small, url } = await smallRelay({ pingIntervalMs: 100, pingClock: Date.now });
  try {
    const clients = await Promise.all([[99, 201, 301, 401], [33, 66, 99, 132, 165, 198, 231, 264, 297, 330, 363, 396]].map(async times => {
      const socket = new Client(url, { autoPong: false });
      const closed = closeCode(socket);
      const secret = randomBytes(32);
      await hello(socket, secret);
      return { socket, closed, address: addressOf(secret), times };
    }));
    // The first client echoes every relay interval, with a little timing variation around a
    // watchdog tick. The second echoes three times per interval. Neither needs a timely pong.
    let previous = 0;
    for (const at of [...new Set(clients.flatMap(client => client.times))].sort((a, b) => a - b)) {
      t.mock.timers.tick(at - previous);
      previous = at;
      for (const client of clients) if (client.times.includes(at)) await selfEcho(client.socket, client.address, client.closed);
    }
    assert.equal(small.connections(), 2);
  } finally {
    await small.close();
  }
});

test('a device still sending is there, though its pongs are held up behind what it sends (#144)', async () => {
  const { relay: small, url } = await smallRelay({ pingIntervalMs: 100 });
  try {
    // A phone sending a picture on a slow connection: each pong waits behind it, so none comes
    // in time, but its messages keep coming.
    const sending = new Client(url, { autoPong: false });
    const secret = randomBytes(32);
    await hello(sending, secret);
    let pings = 0;
    const closed = closeCode(sending).then(() => 'closed');
    const pinged = new Promise<string>(resolve => sending.on('ping', () => {
      sending.send(bytes([2], addressOf(secret), 'more of the picture'));
      if (++pings === 4) resolve('carried on');
    }));
    assert.equal(await Promise.race([closed, pinged]), 'carried on');
    sending.close();
  } finally {
    await small.close();
  }
});

test('the health check answers, and nothing else does', async () => {
  const base = url.replace('ws://', 'http://');
  const health = await fetch(`${base}/health`);
  assert.equal(health.status, 200);
  assert.equal(await health.text(), 'ok');
  assert.equal((await fetch(`${base}/anything`)).status, 404);
});

test('a message over the largest allowed closes the connection', async () => {
  const phone = await device();
  const mac = await device();
  phone.send(bytes([2], mac.address, randomBytes(66_000)));
  assert.equal(await phone.closed, 1009);
  mac.socket.close();
});

test('behind the proxy, connections count by the address it saw', async () => {
  const { relay: small, url } = await smallRelay({ connectionsPerAddress: 1, trustProxy: true });
  try {
    const from = (forwarded: string) => new Client(url, { headers: { 'x-forwarded-for': forwarded } });
    const first = from('203.0.113.1');
    const second = from('203.0.113.2');
    await Promise.all([hello(first), hello(second)]);
    // The proxy adds the address it saw last: what comes before it could be anyone's.
    const third = from('198.51.100.9, 203.0.113.1');
    assert.equal(await closeCode(third), 4005);
    first.close();
    second.close();
  } finally {
    await small.close();
  }
  // Not behind a proxy: the header is anyone's to send, and isn't believed.
  const { relay: direct, url: directURL } = await smallRelay({ connectionsPerAddress: 1 });
  try {
    const first = new Client(directURL, { headers: { 'x-forwarded-for': '203.0.113.1' } });
    await hello(first);
    const second = new Client(directURL, { headers: { 'x-forwarded-for': '203.0.113.2' } });
    assert.equal(await closeCode(second), 4005);
    first.close();
  } finally {
    await direct.close();
  }
});
