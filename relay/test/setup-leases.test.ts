import assert from 'node:assert/strict';
import { generateKeyPairSync, randomBytes, sign, type KeyObject } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test, type TestContext } from 'node:test';
import { SetupLeases } from '../src/leases.ts';
import { createRelay } from '../src/relay.ts';
import { Registrations } from '../src/registrations.ts';

const id = () => randomBytes(32).toString('base64url');
function identity() {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  return { computer: id(), sign: publicKey.export({ type: 'spki', format: 'der' }).subarray(-32).toString('base64url'), privateKey };
}

async function fixture(t: TestContext) {
  const folder = mkdtempSync(join(tmpdir(), 'kuvo-lease-test-'));
  const a = identity(), b = identity(), c = identity();
  const roster = Object.fromEntries([a, b, c].map(each => [each.computer, each.sign]));
  const setup = 'set_' + randomBytes(16).toString('base64url');
  let now = Date.now();
  let relay: ReturnType<typeof createRelay> | undefined;
  let url: string;
  async function start() {
    await relay?.close();
    relay = createRelay({ registrations: new Registrations(), leases: new SetupLeases(folder, () => now),
      leaseRelayHostIPs: ['127.0.0.1'] });
    const server = relay.server;
    await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('No relay port');
    url = 'http://127.0.0.1:' + address.port + '/setup-lease';
  }
  t.after(async () => {
    await relay?.close();
    rmSync(folder, { recursive: true, force: true });
  });
  await start();
  async function request(who: typeof a, action: string, options: Record<string, unknown> = {}, signer: KeyObject = who.privateKey) {
    const bytes = Buffer.from(JSON.stringify({ action, setup, at: now, computer: who.computer, sign: who.sign,
      term: 1, high: 0, ...options }));
    const response = await fetch(url, { method: 'POST', body: JSON.stringify({
      data: bytes.toString('base64url'),
      signature: sign(null, Buffer.concat([Buffer.from('kuvo-link setup lease v1\0'), bytes]), signer).toString('base64url'),
    }) });
    const body = await response.json().catch(() => ({}));
    return { status: response.status, ...body };
  }
  return { a, b, c, roster, request, start, advance: (ms: number) => { now += ms; },
    bootstrap: (high = 0) => request(a, 'claim', { term: 0, bootstrap: true, roster, high }) };
}

test('setup lease authenticates the setup roster and renews only for the current holder', async t => {
  const f = await fixture(t);
  assert.deepEqual(await f.request(f.b, 'read'), { status: 200, lease: null });
  assert.deepEqual(await f.request(f.b, 'claim', { term: 0 }), { status: 200, lease: null });
  const first = await f.bootstrap(300);
  assert.equal(first.lease.holder, f.a.computer);
  assert.equal(first.lease.term, 1);
  assert.equal(first.relayHost, true);
  assert.equal((await f.request(identity(), 'read')).status, 403);
  assert.equal((await f.request(f.a, 'renew', { roster: f.roster }, f.b.privateKey)).status, 403);
  f.advance(10_000);
  assert.deepEqual((await f.request(f.b, 'renew', { roster: f.roster, high: 900 })).lease, first.lease);
  const { [f.c.computer]: _, ...updatedRoster } = f.roster;
  const next = await f.request(f.a, 'renew', { roster: updatedRoster, high: 400 });
  assert.equal(next.lease.high, 400);
  assert.equal(next.lease.expires, first.lease.expires + 10_000);
  assert.equal((await f.request(f.c, 'read')).status, 403);
  assert.equal((await f.request(f.a, 'renew', { roster: f.roster, high: 900 })).lease.high, 400, 'replaying a write cannot extend the lease');
});

test('setup lease permits one claim after expiry and gives always-on computers a minute first', async t => {
  const f = await fixture(t);
  f.advance(120_000);
  const first = (await f.bootstrap(700)).lease;
  f.advance(59_999);
  assert.equal((await f.request(f.b, 'claim', { alwaysOn: true })).lease.holder, f.a.computer);
  f.advance(1);
  const laptop = await f.request(f.c, 'claim');
  assert.equal(laptop.takeoverIn, 60_000);
  assert.equal(laptop.lease.term, 1);
  const claims = await Promise.all([f.request(f.b, 'claim', { alwaysOn: true }), f.request(f.c, 'claim', { alwaysOn: true })]);
  assert.equal(claims[0].lease.term, 2);
  assert.deepEqual(claims[0].lease, claims[1].lease);
  assert.equal(claims[0].lease.high, first.high);
  assert.deepEqual((await f.request(f.a, 'renew', { roster: f.roster, high: 900 })).lease, claims[0].lease);
  f.advance(119_999);
  assert.equal((await f.request(f.a, 'claim', { term: 2 })).lease.term, 2);
  f.advance(1);
  assert.equal((await f.request(f.a, 'claim', { term: 2 })).lease.term, 3);
});

test('setup lease survives relay restart and leaves two minutes for the holder to renew', async t => {
  const f = await fixture(t);
  const first = (await f.bootstrap(600)).lease;
  f.advance(70_000);
  await f.start();
  assert.deepEqual((await f.request(f.b, 'read')).lease, first);
  f.advance(119_999);
  assert.equal((await f.request(f.b, 'claim', { alwaysOn: true })).lease.term, 1);
  const renewed = await f.request(f.a, 'renew', { roster: f.roster, high: 500 });
  assert.equal(renewed.lease.term, 1);
  assert.equal(renewed.lease.high, 600);
  f.advance(1);
  assert.equal((await f.request(f.b, 'claim', { alwaysOn: true })).lease.holder, f.a.computer);
  f.advance(59_999);
  assert.equal((await f.request(f.b, 'claim', { alwaysOn: true })).lease.term, 2);
});

test('setup lease handover fences the old term and delays writes until its grant expires', async t => {
  const f = await fixture(t);
  const first = (await f.bootstrap(30)).lease;
  f.advance(10_000);
  const moved = await f.request(f.a, 'transfer', { roster: f.roster, high: 40, target: f.b.computer });
  assert.equal(moved.lease.holder, f.b.computer);
  assert.equal(moved.lease.term, 2);
  assert.equal(moved.startsIn, 50_000);
  assert.equal(moved.lease.expires, first.expires + 60_000);
  assert.deepEqual((await f.request(f.a, 'renew', { roster: f.roster, high: 99 })).lease, moved.lease);
  f.advance(50_000);
  const renewed = await f.request(f.b, 'renew', { term: 2, roster: f.roster });
  assert.equal(renewed.startsIn, 0);
  assert.equal(renewed.lease.high, 40);
});
