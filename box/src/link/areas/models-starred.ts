// Your starred models (PROTOCOL.md, "The models offered", `models.starred`): kept by the box in
// `box/models.json`, so every client's picker shows the same stars, in the order you starred them.
// `kuvo/models/offered` says them with the models; `POST kuvo/models/starred` stars one or takes its
// star away.

import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { locked, textOf, writePrivate } from "../files";
import { BOX } from "../home";
import type { AnswerPiece, RouteContext } from "../server/context";

export const MODELS_STARRED_ROUTE = "kuvo/models/starred";
export const MODELS_FILE = path.join(BOX, "models.json");

/** The most kept: a list no one could star by hand stays small. */
const MOST = 200;

/** A starred model: its provider and its id. */
export type Starred = { providerID: string; id: string };

const isStarred = (value: unknown): value is Starred =>
  isObject(value) && typeof value.providerID === "string" && value.providerID !== "" && typeof value.id === "string" && value.id !== "";

const same = (a: Starred, b: Starred) => a.providerID === b.providerID && a.id === b.id;

/** Your starred models, in the order starred (none for a file that's missing or can't be read). */
export function starredModels(file = MODELS_FILE): Starred[] {
  try {
    const kept = JSON.parse(textOf(file) ?? "");
    if (!isObject(kept) || !Array.isArray(kept.starred)) return [];
    const out: Starred[] = [];
    for (const each of kept.starred) if (isStarred(each) && !out.some(other => same(other, each))) out.push({ providerID: each.providerID, id: each.id });
    return out;
  } catch {
    return [];
  }
}

/** Stars `model` (at the end, or where it was if it's starred) or takes its star away; what's
 * starred now. */
export function setStarred(model: Starred, starred: boolean, file = MODELS_FILE): Starred[] {
  return locked(`${file}.lock`, () => {
    let kept: Record<string, unknown> = {};
    try {
      const read = JSON.parse(textOf(file) ?? "");
      if (isObject(read)) kept = read;
    } catch {}
    const was = starredModels(file);
    const without = was.filter(each => !same(each, model));
    // (Starred again: where it was.)
    const now = !starred ? without : without.length < was.length ? was : [...was, { providerID: model.providerID, id: model.id }].slice(-MOST);
    if (now.length !== was.length || now.some((each, ix) => !same(each, was[ix]!))) writePrivate(file, JSON.stringify({ ...kept, starred: now }, null, 2));
    return now;
  });
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

// `GET kuvo/models/starred` → `{"starred": [{providerID, id}]}`; `POST` with `{providerID, id,
// starred}` stars it or takes its star away and answers the same; `400` for anything else sent.
export async function* modelsStarredRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  if (method === "GET") return yield* json(200, { starred: starredModels() });
  if (method !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isStarred(given) || typeof (given as { starred?: unknown }).starred !== "boolean") {
    return yield* json(400, { message: "Say which model (providerID, id) and whether it's starred (starred, true or false)." });
  }
  return yield* json(200, { starred: setStarred(given, (given as unknown as { starred: boolean }).starred) });
}
