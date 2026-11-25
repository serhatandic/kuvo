// What every section of `kuvo/settings` is made with (PROTOCOL.md, "A box's settings"): a row and
// its revision, the box's own routes asked as the client asks them, refusals in words, and what a
// section is. Each section is a module of its own beside settings-pages.ts (settings-mcp.ts), which
// lists them and answers the route.

import crypto from "node:crypto";
import fs from "node:fs";
import { sortedJSON } from "../../shared/json";
import type { SettingAction, SettingCan, SettingField, SettingFieldWords, SettingItem, SettingRow, SettingsChecked, SettingsPage } from "../../shared/routes/settings-pages";
import { CHANGED_SINCE } from "./config";
import type { AnswerPiece, RouteContext } from "../server/context";

/** A row's revision: a hash of what it is and what governs whether it can be changed (its value,
 * `can`, its choices' and actions' ids, a list's items' revisions, the values a form of its opens
 * with, a flow's state), never its display words (confirmation words do count), so a computer
 * renamed or a line reworded changes none; nor how it stands (`badge`, `notice`), which the engine
 * says from moment to moment. A `secret`'s, of whether one is set (it has no value). */
export function revised<Row extends Omit<SettingItem, "revision"> & { items?: readonly SettingItem[] }>(row: Row): Row & { revision: string } {
  // (Whether it can be, not why not: a why names the computer.)
  const can = (value: SettingCan | undefined) => (value === undefined ? undefined : value === true);
  const action = (each: SettingAction | undefined) => each && {
    id: each.id, can: can(each.can), opens: each.opens, confirm: each.confirm,
    form: each.form?.fields.map(safeField).map(field => ({ id: field.id, kind: field.kind, value: field.value, can: can(field.can) })),
  };
  const governs = {
    confirmBelow: row.confirmBelow, min: row.min, max: row.max,
    kind: row.kind, value: row.kind === "secret" ? undefined : row.value, can: can(row.can), set: row.set,
    choices: row.choices?.map(choice => choice.id), actions: row.actions?.map(action),
    start: action(row.start), section: row.section, address: row.address, name: row.name,
    items: row.items?.map(each => each.revision), add: action((row as { add?: SettingAction }).add),
    // (A flow's state, not its actions, which differ by who asks: Finish only where it's pasted back.)
    flow: row.flow && { id: row.flow.id, state: row.flow.state },
  };
  const revision = hashed(governs);
  const withForm = (each: SettingAction) => {
    if (!each.form) return each;
    const form = { ...each.form, fields: each.form.fields.map(safeField) };
    return { ...each, form: { ...form, revision: formRevision(form) } };
  };
  const add = (row as { add?: SettingAction }).add;
  return {
    ...row, revision,
    ...(row.actions ? { actions: row.actions.map(withForm) } : {}),
    ...(add ? { add: withForm(add) } : {}),
    ...(row.flow ? { flow: { ...row.flow, actions: row.flow.actions.map(withForm) } } : {}),
  };
}

/** Where a `secret` field's kept value may be read (Show): the vault's own values route, which
 * notes that it was shown, and nothing else. */
export const SHOWS_ROUTE = /^kuvo\/vault\/items\/v_[a-z0-9]{10}\/values$/;

/** A form's field as any section may send it: what's secret never in it, whatever the section put
 * there. A `secret` opens with no value; a `secret` pairs' values are empty (their names stay); a
 * `file` opens with its name alone; and `shows` names only the vault's values route. */
export function safeField(field: SettingField): SettingField {
  const { value, shows, ...rest } = field;
  if (field.kind === "secret") return { ...rest, ...(shows && SHOWS_ROUTE.test(shows.route) ? { shows } : {}) };
  if (field.kind === "pairs" && field.secret) {
    return { ...rest, value: Array.isArray(value) ? value.map(pair => ({ key: typeof pair?.key === "string" ? pair.key : "", value: "" })) : [] };
  }
  if (field.kind === "file") {
    const name = value !== null && typeof value === "object" && typeof (value as { name?: unknown }).name === "string" ? (value as { name: string }).name : undefined;
    return { ...rest, ...(name ? { value: { name } } : {}) };
  }
  return { ...rest, ...(value !== undefined ? { value } : {}) };
}

const hashed = (value: unknown): string => crypto.createHash("sha256").update(sortedJSON(value)).digest("base64url").slice(0, 16);

/** A form's own revision: of the values it opens with, so a form goes stale only when what it shows
 * changed (never because another thing in its list did). */
export const formRevision = (form: NonNullable<SettingAction["form"]>): string =>
  hashed(form.fields.map(field => ({ id: field.id, kind: field.kind, value: field.value, can: field.can === undefined ? undefined : field.can === true })));

/** What a request about `action` on `at` must have read: its form's revision, or the row's. */
export const expectedRevision = (at: SettingItem, action: SettingAction | undefined): string => action?.form?.revision ?? at.revision;

export const item = (row: Omit<SettingItem, "revision">): SettingItem => revised(row);
export const row = (fields: Omit<SettingRow, "revision">): SettingRow => revised(fields);

/** Each row on a page, a list's items too. */
export const rowsOf = (page: SettingsPage): SettingItem[] => page.groups.flatMap(group => group.rows.flatMap(each => [each, ...(each.items ?? [])]));

/** What a row offers: its actions, a list's `add`, and its flow's. */
export const actionsOf = (at: SettingItem): SettingAction[] => [...(at.actions ?? []), ...((at as SettingRow).add ? [(at as SettingRow).add!] : []), ...(at.flow?.actions ?? [])];

/** What one of the box's own routes answers, asked as this request's client asks (the same `fromApp`,
 * the same phone): its status and its JSON. */
export async function asked(context: RouteContext, answer: (context: RouteContext) => AsyncIterable<AnswerPiece>, method: string, path: string, body?: unknown): Promise<{ status: number; value: any }> {
  const request = {
    method, path, headers: body === undefined ? {} : { "content-type": "application/json" },
    ...(body === undefined ? {} : { body: Buffer.from(JSON.stringify(body)) }),
    ...(context.request.fromApp ? { fromApp: true } : {}),
    ...(context.request.fromMac ? { fromMac: true } : {}),
    // (Who it is to the vault, as the holder proved it, and what kind of client: the same.)
    ...(context.request.vaultClient ? { vaultClient: true } : {}),
    ...(context.request.desktop ? { desktop: true } : {}),
  };
  let status = 500;
  const chunks: Buffer[] = [];
  for await (const piece of answer({ ...context, request, route: path.split("?")[0]! })) {
    if ("head" in piece) status = piece.head.status;
    if ("body" in piece) chunks.push(Buffer.from(piece.body));
  }
  let value: any = {};
  try { value = JSON.parse(Buffer.concat(chunks).toString() || "{}"); } catch {}
  return { status, value };
}

/** Why one of them didn't answer, in words. */
export const said = (answer: { status: number; value: any }, fallback: string): string =>
  typeof answer.value?.message === "string" && answer.value.message ? answer.value.message : fallback;

/** A change or an action refused, in words: its status, and why; `stale` where the row changed since
 * it was read (answered with the page now); `fields`, a form's words under its fields. */
export type Refused = { status: number; message: string; notAdmitted?: true; stale?: true; fields?: SettingFieldWords[] };
export type SettingChange = Refused | { save: import("../../shared/routes/setup-save").SaveReply };
export const refusal = (answer: { status: number; value: any }): Refused | null =>
  answer.status >= 200 && answer.status < 300 ? null : { status: answer.status, message: said(answer, `It couldn't be changed (${answer.status}).`) };
export const STALE: Refused = { status: 409, message: CHANGED_SINCE, stale: true };

export const unreadPage = (id: string, title: string, answer: { status: number; value: any }): SettingsPage => ({ id, title, groups: [], notes: [{ words: said(answer, "This computer's settings couldn't be read."), tone: "warning" }] });

export const staleOrRefusal = (answer: { status: number; value: any }): Refused | null => answer.status === 409 && answer.value.message === CHANGED_SINCE ? STALE : refusal(answer);

export async function noActions(): Promise<Refused | null> {
  return { status: 404, message: "That isn't something it does." };
}

/** A section: its page; a change to a row; what a row offers, with its form's values (or a flow's
 * code); a form's values checked as they're typed; and what's looked at for a change made elsewhere. */
export interface Section {
  /** (`quiet`: read only, following no flow, for a form's check.) */
  read: (context: RouteContext, quiet?: boolean) => Promise<SettingsPage>;
  change: (context: RouteContext, at: SettingItem, value: unknown, confirmed?: unknown) => Promise<SettingChange | null>;
  act: (context: RouteContext, at: SettingItem, action: SettingAction, values?: Record<string, unknown>) => Promise<Refused | null>;
  check?: (context: RouteContext, at: SettingItem, action: SettingAction, values: Record<string, unknown>) => Promise<SettingsChecked>;
  /** What's looked at each second for a change another process made (cheap). */
  look: () => string;
}

export const mtime = (file: string): number => { try { return fs.statSync(file).mtimeMs; } catch { return 0; } };
