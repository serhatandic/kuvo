// The active session facts a peer publishes. A complete snapshot replaces the previous one.
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { writePrivate } from "../files";
import { RUN } from "../home";
import { KEY, mailFor, peersOf } from "../peers/known";
import type { ActivityFact } from "../../shared/records";
import type { LinkHost } from "../relay/host";
import type { LinkState } from "../state";

export const ACTIVITY_FOLDER = path.join(RUN, "activity");
const fileOf = (key: string) => path.join(ACTIVITY_FOLDER, `${key}.json`);
const sessionID = (value: unknown): value is string => typeof value === "string" && /^ses_[A-Za-z0-9]{1,60}$/.test(value);
const MAX_PARTS = 256;
const PER_PART = 20;

function rowOf(row: unknown): ActivityFact | undefined {
  if (!isObject(row) || !sessionID(row.session) || (row.state !== "working" && row.state !== "asking")) return;
  const bot = typeof row.bot === "string" && /^bot_[A-Za-z0-9]{1,60}$/.test(row.bot) ? row.bot : undefined;
  const line = Array.isArray(row.line) ? row.line.filter(sessionID).slice(0, 24) : [];
  return { session: row.session, state: row.state, title: plainName(row.title).slice(0, 200) || "New Session",
    line, ...(bot ? { bot } : {}) };
}

export function readActivity(key: string): { at: number; heard: number; rows: ActivityFact[] } | undefined {
  if (!KEY.test(key)) return undefined;
  try { return JSON.parse(fs.readFileSync(fileOf(key), "utf8")); } catch { return undefined; }
}

const pending = new Map<string, { at: number; total: number; parts: Map<number, ActivityFact[]> }>();

export function takeActivity(key: string, given: unknown): void {
  if (!KEY.test(key) || !isObject(given) || typeof given.at !== "number" || !Number.isFinite(given.at)
      || typeof given.part !== "number" || typeof given.total !== "number" || !Number.isInteger(given.part)
      || !Number.isInteger(given.total) || given.part < 0 || given.total > MAX_PARTS || given.part >= given.total
      || !Array.isArray(given.rows) || given.rows.length > PER_PART) return;
  const { at, part, total } = given;
  let snapshot = pending.get(key);
  if (snapshot && snapshot.at > at || at <= (readActivity(key)?.at ?? 0)) return;
  if (snapshot?.at !== at) {
    snapshot = { at, total, parts: new Map() };
    pending.set(key, snapshot);
  }
  if (snapshot.total !== total) return;
  snapshot.parts.set(part, given.rows.map(rowOf).filter((row): row is ActivityFact => row !== undefined));
  if (snapshot.parts.size !== total) return;
  const rows = Array.from({ length: total }, (_, i) => snapshot.parts.get(i)!).flat();
  fs.mkdirSync(ACTIVITY_FOLDER, { recursive: true, mode: 0o700 });
  writePrivate(fileOf(key), JSON.stringify({ at, heard: Date.now(), rows }));
  pending.delete(key);
}

const sent = new Map<string, { text: string; at: number }>();

export function pushActivity(host: Pick<LinkHost, "send">, link: LinkState | null, facts: ActivityFact[], now = Date.now()): void {
  if (!link?.key) return;
  const rows = facts.map(rowOf).filter((row): row is ActivityFact => row !== undefined);
  const text = JSON.stringify(rows);
  const total = Math.max(1, Math.ceil(rows.length / PER_PART));
  if (total > MAX_PARTS) return;
  for (const peer of peersOf(link)) {
    const last = sent.get(peer.key);
    if (last?.text === text && now - last.at < 60_000) continue;
    const at = Math.max(now, (last?.at ?? 0) + 1);
    for (let part = 0; part < total; part++) {
      const bytes = mailFor(peer, { at: now, activity: { at, total, part, rows: rows.slice(part * PER_PART, (part + 1) * PER_PART) } }, link);
      if (!bytes) break;
      host.send(bytes);
      if (part === total - 1) sent.set(peer.key, { text, at });
    }
  }
}
