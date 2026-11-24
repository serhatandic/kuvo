// One small record per setup. Only its holder can replace the authorized roster.
import { createPublicKey, verify } from 'node:crypto';
import { closeSync, fsyncSync, mkdirSync, openSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import type { IncomingMessage, ServerResponse } from 'node:http';
import { join } from 'node:path';

const KEY = /^[\w-]{43}$/;
const SETUP = /^set_[\w-]{22}$/;
const DOMAIN = 'kuvo-link setup lease v1\0';
const LEASE_MS = 60_000;
const RESTART_GRACE_MS = 120_000;
const MAX_REQUEST = 24_000;
const ED_SPKI = Buffer.from('302a300506032b6570032100', 'hex');

type Roster = Record<string, string>;
type Lease = { holder: string; term: number; starts: number; expires: number; high: number; roster: Roster; lastWrite: number; relayHost: boolean };
type ObjectValue = Record<string, unknown>;
const object = (value: unknown): value is ObjectValue => typeof value === 'object' && value !== null && !Array.isArray(value);
const key = (value: unknown): value is string => typeof value === 'string' && KEY.test(value);
const counter = (value: unknown): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;

function authorizedRoster(value: unknown, computer: string, sign: string): Roster | undefined {
  if (!object(value) || value[computer] !== sign) return undefined;
  const entries = Object.entries(value);
  if (entries.length <= 128 && entries.every(([computer, sign]) => key(computer) && key(sign))) return value as Roster;
  return undefined;
}

export class SetupLeases {
  private readonly graceUntil: number;

  constructor(privateFolder: string, now: () => number = Date.now) {
    this.folder = join(privateFolder, 'setup-leases');
    this.now = now;
    this.graceUntil = now() + RESTART_GRACE_MS;
    mkdirSync(this.folder, { recursive: true, mode: 0o700 });
  }

  private readonly folder: string;
  private readonly now: () => number;

  async handle(request: IncomingMessage, response: ServerResponse, relayHost: boolean) {
    function reply(status: number, value: unknown) {
      response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' }).end(JSON.stringify(value));
    }
    if (request.method !== 'POST') return reply(405, {});
    let envelope: unknown;
    try {
      const chunks: Buffer[] = [];
      let size = 0;
      for await (const chunk of request.iterator({ destroyOnReturn: false })) {
        size += chunk.length;
        if (size > MAX_REQUEST) return reply(413, {});
        chunks.push(chunk);
      }
      envelope = JSON.parse(Buffer.concat(chunks).toString());
    } catch { return reply(400, {}); }

    if (!object(envelope) || typeof envelope.data !== 'string' || typeof envelope.signature !== 'string') return reply(400, {});
    const bytes = Buffer.from(envelope.data, 'base64url');
    let body: unknown;
    try { body = JSON.parse(bytes.toString()); } catch { return reply(400, {}); }
    if (!object(body)) return reply(400, {});
    const { action, setup, computer, sign, at, term, high, target } = body;
    const now = this.now();
    if (typeof setup !== 'string' || !SETUP.test(setup) || !key(computer) || !key(sign) ||
        !counter(at) || Math.abs(at - now) > 30_000 ||
        (action !== 'read' && action !== 'claim' && action !== 'renew' && action !== 'transfer')) return reply(400, {});
    if (action !== 'read' && (!counter(term) || !counter(high))) return reply(400, {});
    const publicKey = createPublicKey({ key: Buffer.concat([ED_SPKI, Buffer.from(sign, 'base64url')]), format: 'der', type: 'spki' });
    if (!verify(null, Buffer.concat([Buffer.from(DOMAIN), bytes]), publicKey, Buffer.from(envelope.signature, 'base64url')))
      return reply(403, {});

    const file = join(this.folder, setup + '.json');
    let lease: Lease | undefined;
    try { lease = JSON.parse(readFileSync(file, 'utf8')); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
    }
    if (lease && lease.roster[computer] !== sign) return reply(403, {});
    const roster = authorizedRoster(body.roster, computer, sign);
    const takeoverAt = (extra: number) => Math.max(this.graceUntil, (lease?.expires ?? 0) + extra);
    let updated: Lease | undefined;
    if (!lease && action === 'claim' && term === 0 && body.bootstrap === true && roster && counter(high)) {
      updated = { holder: computer, term: 1, starts: now, expires: now + LEASE_MS, high, roster, lastWrite: at, relayHost };
    } else if (lease && lease.term === term && counter(high)) {
      if (action === 'claim' && now >= takeoverAt(body.alwaysOn === true ? 0 : LEASE_MS)) {
        updated = { ...lease, holder: computer, term: lease.term + 1, starts: now, expires: now + LEASE_MS,
          high: Math.max(lease.high, high), lastWrite: 0, relayHost };
      } else if (lease.holder === computer && at > lease.lastWrite && roster) {
        if (action === 'renew' && (now < lease.expires || now < this.graceUntil)) updated = { ...lease, expires: Math.max(now, lease.starts) + LEASE_MS, high: Math.max(lease.high, high), roster, lastWrite: at, relayHost };
        if (action === 'transfer' && key(target) && target !== computer && lease.roster[target] && roster[target] && now >= lease.starts && now < lease.expires) {
          // The departing holder may still have a cached grant in another process.
          updated = { holder: target, term: lease.term + 1, starts: lease.expires, expires: lease.expires + LEASE_MS,
            high: Math.max(lease.high, high), roster, lastWrite: 0, relayHost: false };
        }
      }
    }
    if (updated) {
      writeFileSync(file + '.tmp', JSON.stringify(updated), { mode: 0o600, flush: true });
      renameSync(file + '.tmp', file);
      const folder = openSync(this.folder, 'r');
      try { fsyncSync(folder); } finally { closeSync(folder); }
      lease = updated;
    }
    if (!lease) return reply(200, { lease: null });
    return reply(200, {
      lease: { holder: lease.holder, sign: lease.roster[lease.holder], term: lease.term, expires: lease.expires, high: lease.high },
      remaining: Math.max(0, lease.expires - now),
      startsIn: Math.max(0, lease.starts - now),
      takeoverIn: Math.max(0, takeoverAt(body.alwaysOn === true ? 0 : LEASE_MS) - now),
      relayHost: lease.relayHost,
    });
  }
}
