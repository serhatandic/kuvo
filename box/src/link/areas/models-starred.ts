// Your starred models (PROTOCOL.md, "The models offered", `models.starred`): kept setup-wide, by the
// setup's main computer (its shared `setupStarredModels`, published to every member), so every
// computer's picker, from every client, shows the same stars, in the order you starred them.
// `kuvo/models/offered` says them with the models; `POST kuvo/models/starred` stars one or takes its
// star away. On a member, that save goes on to the main computer (`ownership/write.ts`), as the
// setup's time zone does.
// A box whose setup doesn't save setup-wide values yet (or that is in none) keeps them itself, in
// `box/models.json`, as before; that list is also what a box offers to seed the setup's value with,
// the first time there's none (`setup/seed.ts`), so no one's stars are lost.

import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { locked, textOf, writePrivate } from "../files";
import { settingsDisk } from "../server/disk-worker";
import { BOX } from "../home";
import { followedSetup, usesAcceptedSaves } from "../setup/record";
import { changeShared, sharedValue } from "../setup/shared";
import { readLink } from "../state";
import type { AnswerPiece, RouteContext } from "../server/context";

export const MODELS_STARRED_ROUTE = "kuvo/models/starred";
export const MODELS_FILE = path.join(BOX, "models.json");
/** The setup-wide value: `{starred: [{providerID, id}]}`. */
export const STARRED_KIND = "setupStarredModels";

/** The most kept: a list no one could star by hand stays small. */
const MOST = 200;

/** A starred model: its provider and its id. */
export type Starred = { providerID: string; id: string };

/** The longest id kept (a provider's or a model's). */
const LONGEST = 300;

const isStarred = (value: unknown): value is Starred =>
  isObject(value) && typeof value.providerID === "string" && value.providerID !== "" && value.providerID.length <= LONGEST && typeof value.id === "string" && value.id !== "" && value.id.length <= LONGEST;

const same = (a: Starred, b: Starred) => a.providerID === b.providerID && a.id === b.id;

/** A list as kept (a file's or the setup's `starred`): each valid star once, in its order. */
function listOf(kept: unknown): Starred[] {
  if (!isObject(kept) || !Array.isArray(kept.starred)) return [];
  const out: Starred[] = [];
  for (const each of kept.starred) if (isStarred(each) && !out.some(other => same(other, each))) out.push({ providerID: each.providerID, id: each.id });
  return out.slice(-MOST);
}

/** What this box keeps itself, in `box/models.json` (none for a file that's missing or can't be read). */
function localStarred(file = MODELS_FILE): Starred[] {
  try {
    return listOf(JSON.parse(textOf(file) ?? ""));
  } catch {
    return [];
  }
}

/** Whether this box's stars are the setup's: it follows a setup that saves setup-wide values. */
const setupWide = () => {
  const link = readLink();
  return usesAcceptedSaves(followedSetup(link)) ? link : null;
};

/** Your starred models, in the order starred: the setup's, once it has them; until then (or with no
 * setup saving them), this box's own. */
export function starredModels(file = MODELS_FILE): Starred[] {
  const link = setupWide();
  const shared = link ? sharedValue(link, STARRED_KIND) : undefined;
  return shared !== undefined ? listOf(shared) : localStarred(file);
}

/** `list` with `model` starred (at the end, or where it was if it's starred) or its star taken away. */
function withStar(list: Starred[], model: Starred, starred: boolean): Starred[] {
  const without = list.filter(each => !same(each, model));
  // (Starred again: where it was.)
  return !starred ? without : without.length < list.length ? list : [...list, { providerID: model.providerID, id: model.id }].slice(-MOST);
}

const sameList = (a: Starred[], b: Starred[]) => a.length === b.length && a.every((each, ix) => same(each, b[ix]!));

/** Stars `model` in this box's own file (at the end, or where it was if it's starred) or takes its
 * star away; what's starred now. (For a box whose stars aren't setup-wide.) */
export function setStarred(model: Starred, starred: boolean, file = MODELS_FILE): Starred[] {
  return locked(`${file}.lock`, () => {
    let kept: Record<string, unknown> = {};
    try {
      const read = JSON.parse(textOf(file) ?? "");
      if (isObject(read)) kept = read;
    } catch {}
    const was = localStarred(file);
    const now = withStar(was, model, starred);
    if (!sameList(now, was)) writePrivate(file, JSON.stringify({ ...kept, starred: now }, null, 2));
    return now;
  });
}

/** This box's own stars as its copy for seeding the setup's value (`setup/seed.ts`); none for none. */
export function starredForSeed(file = MODELS_FILE): { starred: Starred[] } | undefined {
  const list = localStarred(file);
  return list.length ? { starred: list } : undefined;
}

/** Whether a copy or a value has no stars. */
export const noStars = (value: unknown): boolean => !listOf(value).length;

/** A star's field, for merging copies: its provider and id. */
const fieldOf = (model: Starred) => JSON.stringify([model.providerID, model.id]);

/** A computer's copy (`incoming`) merged into the setup's value (`prior`): the setup's stars in their
 * order, then the copy's others, each that `take` keeps (a star the copy had when it was taken before,
 * and the setup no longer has, stays unstarred). */
export function mergeStarred(prior: unknown, incoming: unknown, take: (field: string, accepted: true | undefined, proposed: true) => true | undefined):
  { value: { starred: Starred[] } | undefined; fields: Record<string, true> } {
  const list = listOf(prior);
  for (const each of listOf(incoming)) {
    if (list.some(other => same(other, each))) continue;
    if (take(fieldOf(each), undefined, true) === true) list.push(each);
  }
  const value = prior !== undefined || list.length ? { starred: list.slice(-MOST) } : undefined;
  return { value, fields: Object.fromEntries((value?.starred ?? []).map(each => [fieldOf(each), true as const])) };
}

/** Your starred models as the earlier Mac app kept them (its `favoriteModels` preference: keys
 * `provider/model`, comma separated), for the move to this box. */
export function starredFromKeys(keys: string): Starred[] {
  const out: Starred[] = [];
  for (const key of keys.split(",")) {
    const at = key.indexOf("/");
    if (at <= 0 || at === key.length - 1) continue;
    const each = { providerID: key.slice(0, at), id: key.slice(at + 1) };
    if (!out.some(other => same(other, each))) out.push(each);
  }
  return out.slice(-MOST);
}

/** A star saved setup-wide, by the main computer: seeded first where the setup has no value yet.
 * A member says to save it on the main (the router sends it there, as the time zone's, when it can). */
async function starSetupWide(context: RouteContext, model: Starred, starred: boolean): Promise<{ status: number; value: unknown }> {
  const refused = (status: number, message: string) => ({ status, value: { message } });
  if (followedSetup(readLink())?.role !== "primary") return refused(409, "Your setup's main computer keeps your starred models, and this computer couldn't reach it. Try again in a moment.");
  const { setupSeedRefusal } = await import("../setup/seed");
  const seed = await setupSeedRefusal(STARRED_KIND);
  if (seed) return refused(seed.status, seed.message);
  const veto = context.admitQueued?.();
  if (veto) return { status: veto.status, value: isObject(veto.value) && typeof veto.value.message === "string" ? { message: veto.value.message } : { message: "The change could not be admitted." } };
  const made: { now?: Starred[] } = {};
  const save = changeShared(STARRED_KIND, value => {
    const was = listOf(value);
    const now = made.now = withStar(was, model, starred);
    // (Nothing changed: nothing to save.)
    if (sameList(now, was) && value !== undefined) return undefined;
    return { starred: now };
  });
  if (save && made.now) return { status: 200, value: { starred: made.now } };
  // (Already as asked: what's starred, as the setup has it.)
  const after = sharedValue(readLink(), STARRED_KIND);
  if (made.now && after !== undefined && sameList(listOf(after), made.now)) return { status: 200, value: { starred: listOf(after) } };
  return refused(409, "The main computer couldn't save your starred models just now. Try again in a moment.");
}

// `GET kuvo/models/starred` → `{"starred": [{providerID, id}]}`; `POST` with `{providerID, id,
// starred}` stars it or takes its star away and answers the same; `400` for anything else sent.
export async function* modelsStarredRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  const method = request.method.toUpperCase();
  if (method === "GET") return yield* json(200, { starred: starredModels() });
  if (method !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isStarred(given) || typeof (given as { starred?: unknown }).starred !== "boolean") {
    return yield* json(400, { message: "Say which model (providerID, id) and whether it's starred (starred, true or false)." });
  }
  const starred = (given as unknown as { starred: boolean }).starred;
  const model = { providerID: given.providerID, id: given.id };
  if (setupWide()) {
    const answer = await starSetupWide(context, model, starred);
    return yield* json(answer.status, answer.value);
  }
  return yield* json(200, { starred: await settingsDisk.run("setStarred", model, starred) });
}
