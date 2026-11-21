// Where each phone wants its pushes, and from which addresses it takes them: the only thing the
// relay keeps, in one small file, so pushes still reach a phone after the relay restarts.
import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

export interface Registration {
  token: string;
  environment: 'sandbox' | 'production';
  topic: string;
  senders: string[];
}

const ADDRESS = /^[A-Za-z0-9_-]{22}$/;
const TOKEN = /^[0-9a-f]{32,200}$/;
const TOPIC = /^[A-Za-z0-9.-]{1,200}$/;
const MAX_SENDERS = 64;
const DAY = 24 * 60 * 60 * 1000;

// A phone's registration as it sent it, checked; null if it isn't one.
export function parseRegistration(body: unknown): Registration | null {
  if (typeof body !== 'object' || body === null) return null;
  const { token, environment, topic, senders } = body as Record<string, unknown>;
  if (typeof token !== 'string' || !TOKEN.test(token)) return null;
  if (environment !== 'sandbox' && environment !== 'production') return null;
  if (typeof topic !== 'string' || !TOPIC.test(topic)) return null;
  if (!Array.isArray(senders) || senders.length > MAX_SENDERS || !senders.every(s => typeof s === 'string' && ADDRESS.test(s))) return null;
  return { token, environment, topic, senders: [...new Set(senders as string[])] };
}

export interface RegistrationOptions {
  // How many phones it keeps; past that, the one heard from longest ago goes.
  max?: number;
  // How long one lasts without the phone sending it again (it does whenever it connects).
  lastsMs?: number;
  now?: () => number;
}

type Entry = Registration & { updated: number };

export class Registrations {
  // Oldest first: each one set goes to the end.
  private readonly entries = new Map<string, Entry>();
  private readonly byToken = new Map<string, string>();
  private readonly file?: string;
  private readonly max: number;
  private readonly lastsMs: number;
  private readonly now: () => number;
  private saving?: NodeJS.Timeout;

  // Keeps them in `folder` when given; only in memory otherwise.
  constructor(folder?: string, options: RegistrationOptions = {}) {
    this.max = options.max ?? 200_000;
    this.lastsMs = options.lastsMs ?? 90 * DAY;
    this.now = options.now ?? Date.now;
    if (!folder) return;
    mkdirSync(folder, { recursive: true });
    this.file = join(folder, 'registrations.json');
    try {
      const saved = JSON.parse(readFileSync(this.file, 'utf8')) as Record<string, unknown>;
      const loaded = Object.entries(saved).flatMap(([address, body]) => {
        const registration = parseRegistration(body);
        const updated = Number((body as { updated?: unknown }).updated) || this.now();
        return registration && ADDRESS.test(address) ? [{ address, entry: { ...registration, updated } }] : [];
      });
      for (const { address, entry } of loaded.sort((a, b) => a.entry.updated - b.entry.updated)) this.put(address, entry);
      this.prune();
    } catch {
      // None yet.
    }
  }

  get size() { return this.entries.size; }

  get(address: string): Registration | undefined {
    const entry = this.entries.get(address);
    if (!entry || this.now() - entry.updated > this.lastsMs) return undefined;
    const { updated: _, ...registration } = entry;
    return registration;
  }

  // Returns false if `body` isn't a registration.
  set(address: string, body: unknown): boolean {
    const registration = parseRegistration(body);
    if (!registration) return false;
    this.put(address, { ...registration, updated: this.now() });
    this.prune();
    this.save();
    return true;
  }

  // The environment a phone's `token` turned out to be (Apple's other host took it), kept until the
  // phone sends its registration again; nothing if it registered another token meanwhile. Whether
  // it changed.
  setEnvironment(address: string, token: string, environment: Registration['environment']): boolean {
    const entry = this.entries.get(address);
    if (!entry || entry.token !== token || entry.environment === environment) return false;
    entry.environment = environment;
    this.save();
    return true;
  }

  // `token`, when given: only while it's still that one (a phone that registered another meanwhile
  // keeps it).
  delete(address: string, token?: string) {
    if (token !== undefined && this.entries.get(address)?.token !== token) return;
    if (this.remove(address)) this.save();
  }

  private put(address: string, entry: Entry) {
    // A token belongs to one phone: if it moved (the app reinstalled), the old entry goes.
    const other = this.byToken.get(entry.token);
    if (other !== undefined && other !== address) this.remove(other);
    this.remove(address);
    this.entries.set(address, entry);
    this.byToken.set(entry.token, address);
  }

  private remove(address: string): boolean {
    const entry = this.entries.get(address);
    if (!entry) return false;
    this.entries.delete(address);
    if (this.byToken.get(entry.token) === address) this.byToken.delete(entry.token);
    return true;
  }

  // Drops the ones not heard from in a long while, and the oldest past the most it keeps.
  private prune() {
    const oldest = this.now() - this.lastsMs;
    for (const [address, entry] of this.entries) {
      if (entry.updated >= oldest && this.entries.size <= this.max) break;
      this.remove(address);
    }
  }

  // Writes soon, once for a burst of changes.
  private save() {
    if (!this.file || this.saving) return;
    this.saving = setTimeout(() => this.flush(), 1000);
    this.saving.unref();
  }

  flush() {
    clearTimeout(this.saving);
    this.saving = undefined;
    if (!this.file) return;
    const temporary = `${this.file}.tmp`;
    writeFileSync(temporary, JSON.stringify(Object.fromEntries(this.entries)), { mode: 0o600 });
    renameSync(temporary, this.file);
  }
}
