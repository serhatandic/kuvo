import { createHash, randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { isObject, sameJSON, sortedJSON } from "../../shared/json";
import { listOf, locked, writeDurablePrivate } from "../files";
import { ownName } from "../letters/start";
import { BOX, CONFIG_LOCK, IS_MAC } from "../home";
import { keyNames } from "../keys";
import { KEY, namedBoxes, ownKey, peersOf, type MailSender } from "../peers/known";
import type { RouteContext } from "../server/context";
import { acceptedMembership } from "../setup/membership";
import { followedSetup, ledSetup, setupDoc } from "../setup/record";
import { localRemovalPending } from "../setup/removal-record";
import { readLink, type LinkState } from "../state";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { macOnlyServers, translateSettings, type ChangeMade } from "./apply";
import { APPLY_FOLDER, prepareApply, settleApply, writeApplyResult, type ApplyResult } from "./apply-journal";
import { CONFIG_FILE, ownsSettings, readSettings, settingAt, valueAt, writeSettings, type SettingChange, type Settings, type SettingsRecorder } from "./config";
import { CATALOG_FILE, catalogSettings } from "./mcp-catalog";

export const APPLY_FROM = "config.apply.from";
const RECEIPTS = path.join(APPLY_FOLDER, "receipts.json");
const RECEIPT_LIFE = 60 * 60_000;
const PLAN_LIFE = 10 * 60_000;
const MAX_CHANGES = 100;
const MAX_BATCH_BYTES = 512 * 1024;
const ID = /^[a-f0-9]{32}$/;
const id = () => randomBytes(16).toString("hex");
const digest = (value: unknown) => createHash("sha256").update(sortedJSON(value)).digest("hex");
const opaqueID = (value: unknown): value is string => typeof value === "string" && ID.test(value);

interface Scope { setup: string; epoch: number }
interface Receipt extends Scope { id: string; expires: number; changes: SavedChange[] }
interface SavedChange extends ChangeMade { catalog?: string }
interface Batch extends Scope { expires: number; changes: SavedChange[]; macOnly: string[] }
interface Plan extends Scope {
  source: string;
  destination: string;
  changeSet: string;
  batch: string;
  expires: number;
  revision: string;
  catalogRevision: string;
  approval: string;
  made: SettingChange[];
  keys: string[];
  someSkipped: boolean;
}

function read<T>(file: string): T | undefined {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; }
  return undefined;
}

function keep(file: string, value: unknown) {
  writeDurablePrivate(file, JSON.stringify(value), BOX);
}

const batchFile = (id: string) => path.join(APPLY_FOLDER, id + ".batch.json");
const planFile = (id: string) => path.join(APPLY_FOLDER, id + ".plan.json");
const resultFile = (id: string) => path.join(APPLY_FOLDER, id + ".result.json");

function pruneExpired(now: number) {
  for (const entry of listOf(APPLY_FOLDER)) {
    if (!/^[a-f0-9]{32}\.(plan|batch)\.json$/.test(entry)) continue;
    const file = path.join(APPLY_FOLDER, entry);
    const saved = read<{ expires: number }>(file);
    if (!saved || saved.expires + RECEIPT_LIFE >= now) continue;
    if (entry.endsWith(".plan.json") && fs.existsSync(resultFile(entry.slice(0, 32)))) {
      const plan = saved as Plan;
      if (plan.made?.length) keep(file, { approval: plan.approval, expires: plan.expires });
    } else fs.rmSync(file, { force: true });
  }
}

function receipts(now: number) {
  return (read<Receipt[]>(RECEIPTS) ?? []).filter(receipt => receipt.expires > now);
}

const under = (at: string[], parent: string[]) => parent.length <= at.length && parent.every((key, index) => key === at[index]);

/** The settings writer calls this with its actual pre- and postimages under its lock. */
export function recordConfigChange(received: string[], catalog?: string): SettingsRecorder {
  return (before, after, changes) => {
    try {
      const scope = currentScope(readLink());
      if (!scope) return;
      const paths = changes.map(change => change.path).filter(pathFits).sort((a, b) => a.length - b.length);
      const selected: SavedChange[] = [];
      for (const path of paths) {
        if (selected.some(change => under(path, change.path))) continue;
        const was = valueAt(before, path) ?? null;
        const now = valueAt(after, path) ?? null;
        if (!sameJSON(was, now)) selected.push({ path, before: was, after: now, ...(catalog ? { catalog } : {}) });
      }
      if (!selected.length || Buffer.byteLength(JSON.stringify(selected)) > MAX_BATCH_BYTES) return;
      const now = Date.now();
      const saved = { ...scope, id: id(), expires: now + RECEIPT_LIFE, changes: selected };
      keep(RECEIPTS, [...receipts(now), saved].slice(-200));
      received.push(saved.id);
    } catch {
      // The settings write has already committed. A receipt is optional for legacy settings.
    }
  };
}

/** Merge in the order the source saved the writes. Refuse gaps and overlapping selections. */
export function netChanges(selected: SavedChange[][]): SavedChange[] | undefined {
  const net = new Map<string, SavedChange>();
  for (const change of selected.flat()) {
    const key = JSON.stringify(change.path);
    const previous = net.get(key);
    if (previous && !sameJSON(previous.after, change.before)) return;
    if (!previous && [...net.values()].some(each => under(each.path, change.path) || under(change.path, each.path))) return;
    net.set(key, previous ? { path: change.path, before: previous.before, after: change.after } : change);
  }
  return [...net.values()].filter(change => !sameJSON(change.before, change.after));
}

function currentScope(link: LinkState | null): Scope | undefined {
  const setup = followedSetup(link);
  const own = link && ownKey(link);
  if (!setup || !own || setup.conflict || link?.localWipe || link?.localUnlink || localRemovalPending(link)) return;
  const membership = acceptedMembership(link);
  if (setup.checkpointed || setup.role === "member") {
    const word = !setup.checkpointed && ledSetup(setupDoc(link));
    const fromPrimary = word && word.id === setup.id && word.epoch === setup.epoch && word.primary === setup.primary;
    const roster = membership?.boxes ?? (fromPrimary ? namedBoxes(link, setup.primary) : []);
    if (!roster.some(peer => peer.key === own)) return;
  }
  return { setup: setup.id, epoch: setup.epoch };
}

const inScope = (current: Scope | undefined, expected: unknown) => isObject(expected) &&
  current?.setup === expected.setup && current?.epoch === expected.epoch && current !== undefined;

function member(link: LinkState | null, key: string): ReturnType<typeof peersOf>[number] | undefined {
  const setup = followedSetup(link);
  if (!setup) return;
  const roster = acceptedMembership(link)?.boxes ?? (setup.role === "member" ? namedBoxes(link, setup.primary) : peersOf(link));
  return roster.find(peer => peer.key === key);
}

const reply = (status: number, value: unknown): Acted => ({ status, value });
const unavailable = () => reply(503, { status: "unavailable" });

export function saveChangeSet(given: unknown): Acted {
  const link = readLink();
  const scope = currentScope(link);
  if (!scope) return unavailable();
  if (!isObject(given) || !Array.isArray(given.receipts) || !given.receipts.length || given.receipts.length > MAX_CHANGES
    || !given.receipts.every(opaqueID)) return reply(400, { status: "invalidReceipts" });
  const ids = new Set(given.receipts);
  return locked(CONFIG_LOCK, () => {
    if (!inScope(currentScope(readLink()), scope)) return unavailable();
    const selected = receipts(Date.now()).filter(receipt => ids.has(receipt.id) && inScope(scope, receipt));
    if (selected.length !== ids.size) return reply(404, { status: "receiptsExpired" });
    const changes = netChanges(selected.map(receipt => receipt.changes));
    if (!changes || changes.length > MAX_CHANGES || Buffer.byteLength(JSON.stringify(changes)) > MAX_BATCH_BYTES)
      return reply(409, { status: "changesDoNotCombine" });
    pruneExpired(Date.now());
    const changeSet = id();
    const expires = Math.min(...selected.map(receipt => receipt.expires));
    keep(batchFile(changeSet), { ...scope, expires, changes, macOnly: [...macOnlyServers()] } satisfies Batch);
    return reply(201, { changeSet, expires });
  }, { required: true });
}

/** Invoked only by the authenticated peer-mail receiver. The envelope's sender is authoritative. */
export function configApplyAct(sender: MailSender, act: Act): Acted {
  const link = readLink();
  if (act.method !== "GET" || !opaqueID(act.item) || !ownsSettings()
    || !inScope(currentScope(link), act.body) || act.body?.destination !== sender.key
    || act.body?.source !== (link && ownKey(link)) || !member(link, sender.key)) return unavailable();
  const batch = read<Batch>(batchFile(act.item));
  if (!batch || batch.expires <= Date.now() || !inScope(batch, act.body)) return reply(404, { status: "changeSetExpired" });
  return reply(200, act.body?.verify === true ? { batch: digest(batch) } : batch);
}

async function sourceBatch(link: LinkState, source: string, destination: string, changeSet: string, scope: Scope, signal?: AbortSignal, verify = false) {
  const peer = member(link, source);
  if (!peer) return unavailable();
  if (!takesForwarded(source, APPLY_FROM)) return reply(409, { status: "unsupported", computer: source });
  const result = await forwardAct(peer, "Settings", { area: "configApply", method: "GET", item: changeSet,
    body: { setup: scope.setup, epoch: scope.epoch, source, destination, verify } }, signal, link, Date.now(), { backgroundRelay: true });
  return "refused" in result ? unavailable() : result;
}

const pathFits = (path: unknown): path is string[] => Array.isArray(path) && path.length > 0 && path.length <= 16 &&
  path.every(key => typeof key === "string" && key.length > 0 && key.length <= 200 && !["__proto__", "prototype", "constructor"].includes(key));

function batchFits(value: unknown, scope: Scope): value is Batch {
  return isObject(value) && inScope(scope, value) && typeof value.expires === "number" && value.expires > Date.now() &&
    Array.isArray(value.macOnly) && value.macOnly.every(name => typeof name === "string") &&
    Array.isArray(value.changes) && value.changes.length <= MAX_CHANGES && value.changes.every(change =>
      isObject(change) && pathFits(change.path) && (change.catalog === undefined || typeof change.catalog === "string"));
}

function referencedKeys(value: unknown): string[] {
  const keys = new Set<string>();
  const visit = (value: unknown) => {
    if (typeof value === "string") {
      for (const [, name] of value.matchAll(/\{(?:file:[^{}]*\/|env:)(KUVO_KEY_[A-Z0-9_]+)\}/g)) keys.add(name);
    } else if (Array.isArray(value)) value.forEach(visit);
    else if (isObject(value)) Object.values(value).forEach(visit);
  };
  visit(value);
  return [...keys].sort();
}

function catalogRevision(): string {
  try { return createHash("sha256").update(fs.readFileSync(CATALOG_FILE)).digest("hex"); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error; return ""; }
}

/** Translate in source order, keeping changes whose destination can use them. */
function planChanges(batch: Pick<Batch, "changes" | "macOnly">, settings: Settings, existingKeys: string[], mac = IS_MAC) {
  const macOnly = new Set([...batch.macOnly, ...macOnlyServers()]);
  const made: SettingChange[] = [];
  const leftOut: string[][] = [], kept: string[][] = [], stayed: string[] = [];
  const skipped: { path: string[]; reason: string; keys?: string[] }[] = [];
  const usedKeys = new Set<string>();
  const blocked: string[][] = [];
  const add = <T>(to: T[], values: T[]) => { for (const value of values) if (!to.some(each => sameJSON(each, value))) to.push(value); };
  let here = settings;
  for (const change of batch.changes) {
    const at = change.path;
    if (blocked.some(parent => under(at, parent))) {
      skipped.push({ path: at, reason: "missingParent" });
      continue;
    }
    const before = valueAt(here, at);
    let translated = change;
    if (change.catalog) {
      if (!mac && macOnly.has(change.catalog)) { add(stayed, [change.catalog]); continue; }
      if (before !== undefined) { add(kept, [at]); continue; }
      const catalog = catalogSettings(change.catalog);
      if (!catalog) {
        skipped.push({ path: at, reason: "catalogUnavailable" });
        blocked.push(at);
        continue;
      }
      translated = { ...change, after: catalog };
    }
    const result = translateSettings([translated], here, { mac, macOnly });
    add(leftOut, result.leftOut);
    add(kept, result.kept);
    add(stayed, result.macOnly);
    const next = result.made[0];
    if (!next || sameJSON(before ?? null, next.value ?? null)) continue;
    if (at.slice(0, -1).some((_, index) => {
      const parent = valueAt(here, at.slice(0, index + 1));
      return parent !== undefined && !isObject(parent);
    })) {
      skipped.push({ path: at, reason: "incompatibleParent" });
      continue;
    }
    const keys = referencedKeys(next.value);
    const missing = keys.filter(key => !existingKeys.includes(key));
    if (missing.length) {
      skipped.push({ path: at, reason: "missingKeys", keys: missing });
      blocked.push(at);
      continue;
    }
    keys.forEach(key => usedKeys.add(key));
    made.push(next);
    here = settingAt(here, at, next.value);
  }
  // Paths and change kinds only. Settings can contain arbitrary literal credentials.
  const changes = made.map(({ path, value }) => ({ path, action: value == null ? "remove" : valueAt(settings, path) === undefined ? "add" : "set" }));
  return { made, changes, leftOut, kept, macOnly: stayed, skipped, keys: [...usedKeys].sort() };
}

async function preview(given: Record<string, unknown>, context: RouteContext): Promise<Acted> {
  const { from } = given;
  if (given.dry !== true || !isObject(from) || typeof from.computer !== "string" || !KEY.test(from.computer)
    || !opaqueID(from.changeSet)) return reply(400, { status: "invalidSource" });
  const link = readLink();
  const scope = currentScope(link);
  const destination = link && ownKey(link);
  if (!link || !scope || !destination || destination === from.computer) return unavailable();
  const source = from.computer, changeSet = from.changeSet;
  const fetched = await sourceBatch(link, source, destination, changeSet, scope, context.signal);
  if (fetched.status !== 200) return fetched;
  const batch = fetched.value;
  if (!batchFits(batch, scope)) return unavailable();
  const existingKeys = await keyNames().catch(() => []);
  return locked(CONFIG_LOCK, () => {
    const current = readLink();
    const peer = member(current, source);
    if (!current || ownKey(current) !== destination || !inScope(currentScope(current), scope) || !peer) return unavailable();
    const { settings } = readSettings();
    if (!settings) return unavailable();
    pruneExpired(Date.now());
    const plan = id();
    const revision = digest(settings);
    const expires = Math.min(Date.now() + PLAN_LIFE, batch.expires);
    const catalog = catalogRevision();
    const { made, ...shown } = planChanges(batch, settings, existingKeys);
    if (catalog !== catalogRevision() || expires <= Date.now()) return unavailable();
    const details = { plan, expires, source: { computer: source, name: peer.name },
      destination: { computer: destination, name: context.info.name || ownName() }, ...shown };
    const approval = digest(details);
    keep(planFile(plan), { ...scope, source, destination, changeSet, batch: digest(batch), expires,
      revision, catalogRevision: catalog, approval, made, keys: shown.keys,
      someSkipped: Boolean(shown.leftOut.length || shown.kept.length || shown.macOnly.length || shown.skipped.length) } satisfies Plan);
    return reply(200, { ...details, approval });
  }, { required: true });
}

function operation(id: string): ApplyResult | undefined {
  settleApply(CONFIG_FILE);
  return read<ApplyResult>(resultFile(id));
}

async function confirm(id: string, approval: unknown, context: RouteContext): Promise<Acted> {
  const plan = locked(CONFIG_LOCK, () => read<Plan>(planFile(id)), { required: true });
  if (!plan || plan.approval !== approval) return reply(plan ? 409 : 404, { status: plan ? "freshPreview" : "planUnknown" });
  const previous = locked(CONFIG_LOCK, () => operation(id), { required: true });
  if (previous) return reply(200, previous);
  if (plan.expires <= Date.now()) return reply(409, { status: "freshPreview" });
  const link = readLink();
  if (!link || !inScope(currentScope(link), plan) || ownKey(link) !== plan.destination) return unavailable();
  const fetched = await sourceBatch(link, plan.source, plan.destination, plan.changeSet, plan, context.signal, true);
  if (fetched.status === 404) return reply(409, { status: "freshPreview" });
  if (fetched.status !== 200) return fetched;
  if (!isObject(fetched.value) || fetched.value.batch !== plan.batch) return reply(409, { status: "freshPreview" });
  const existingKeys = plan.keys.length ? await keyNames().catch(() => []) : [];
  return locked(CONFIG_LOCK, () => {
    const previous = operation(id);
    if (previous) return reply(200, previous);
    const current = readLink();
    if (!current || !inScope(currentScope(current), plan) || ownKey(current) !== plan.destination || !member(current, plan.source))
      return unavailable();
    const { settings } = readSettings();
    if (!settings) return unavailable();
    const finish = (result: ApplyResult) => {
      writeApplyResult(result);
      return reply(200, result);
    };
    if (plan.expires <= Date.now() || digest(settings) !== plan.revision || catalogRevision() !== plan.catalogRevision ||
      plan.keys.some(key => !existingKeys.includes(key)))
      return finish({ plan: id, status: "freshPreview" });
    if (!plan.made.length) return finish({ plan: id, status: plan.someSkipped ? "nothingToApply" : "alreadySet", revision: plan.revision });
    const next = plan.made.reduce((at, change) => settingAt(at, change.path, change.value), settings);
    const result: ApplyResult = { plan: id, status: "applied", revision: digest(next) };
    const written = writeSettings(settings, plan.made, CONFIG_FILE, (target, staged) => {
      const current = readLink();
      if (!current || !inScope(currentScope(current), plan) || ownKey(current) !== plan.destination || !member(current, plan.source))
        throw new Error("The setup changed before applying settings");
      prepareApply(CONFIG_FILE, target, staged, result);
    });
    const saved = operation(id);
    if (saved) return reply(200, saved);
    return written.status === 200 ? reply(202, { plan: id, status: "recoveryPending" }) : unavailable();
  }, { required: true });
}

export async function applyFrom(given: Record<string, unknown>, context: RouteContext): Promise<Acted> {
  try {
    if ("from" in given) return await preview(given, context);
    if (!opaqueID(given.plan)) return reply(400, { status: "invalidPlan" });
    return await confirm(given.plan, given.approval, context);
  } catch { return unavailable(); }
}

export function applyResult(id: string): Acted {
  if (!opaqueID(id)) return reply(404, { status: "planUnknown" });
  try {
    return locked(CONFIG_LOCK, () => {
      const saved = operation(id);
      if (saved) return reply(200, saved);
      const plan = read<Plan>(planFile(id));
      return plan && plan.expires > Date.now()
        ? reply(200, { plan: id, status: "awaitingApproval" }) : reply(404, { status: "planUnknown" });
    }, { required: true });
  } catch { return reply(503, { plan: id, status: "recoveryPending" }); }
}
