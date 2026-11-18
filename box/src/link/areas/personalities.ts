// Your personalities (PROTOCOL.md, "Personalities"): this box's copy of the library, followed and
// changed, and another computer's copy met with it.

import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import type { Body } from "../../shared/json";
import { followed } from "../follow";
import { PENDING_SAVE, changeShared, waitShared } from "../setup/shared";
import { requiresStandby } from "../setup/record";
import { readLink } from "../state";
import { mergePersonalities, fitsLibrary, FULL_LIBRARY, PERSONALITIES_FILE, personalitiesDocument, personalityFollowers, readPersonalities } from "../personalities/library";
import { answerPersonalities, retellBots, takePersonalitiesDocument } from "../personalities/take";
import type { AnswerPiece, RouteContext } from "../server/context";
import type { Personalities } from "../../shared/routes/personalities";

// The library followed: a line now, and again as it changes, from a route here, a peer's copy
// met, or another process (a Mac's `box personalities` over SSH, the bridge beside this box).
export async function* personalityEventsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  return yield* followed((): Personalities => ({ personalities: readPersonalities().personalities }), personalityFollowers, signal, { folders: [path.dirname(PERSONALITIES_FILE)], recursive: false });
}

// The library whole, and another computer's copy met with it: for the app beside this box only
// (it signs the library for your other computers), never a phone.
export async function* personalitiesDocumentRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, readEngine, readBridge, json } = context;
  if (!request.fromApp) return yield* json(403, { message: "Only Kuvo Code on this computer can do that." });
  const method = request.method.toUpperCase();
  if (route === "kuvo/personalities/document") return yield* method === "GET" ? json(200, personalitiesDocument()) : json(405, {});
  if (method !== "POST") return yield* json(405, {});
  const setup = readLink()?.setup;
  if (requiresStandby(setup)) {
    if (setup?.role !== "primary") return yield* json(409, { message: "Change your setup's personalities on its primary." });
    if (!setup.standby) return yield* json(503, { message: PENDING_SAVE });
    const given = parsed(request.body) as Body<"document">;
    if (!isObject(given?.document) || !Array.isArray(given.document.personalities)
      || (given.document.removed !== undefined && !Array.isArray(given.document.removed))) {
      return yield* json(400, { message: "Send a library of personalities: {document: {personalities, removed}}." });
    }
    const document = given.document;
    let changed: string[] = [];
    let full = false;
    const save = changeShared("setupPersonalities", value => {
      const before = isObject(value) ? mergePersonalities(value) : readPersonalities();
      const after = mergePersonalities(before, document);
      if (!fitsLibrary(after)) { full = true; return undefined; }
      const was = new Map([...before.personalities, ...before.removed].map(entry => [entry.id, JSON.stringify(entry)]));
      changed = [...after.personalities, ...after.removed].filter(entry => was.get(entry.id) !== JSON.stringify(entry)).map(entry => entry.id);
      return after;
    });
    if (full) return yield* json(400, { message: FULL_LIBRARY });
    if (!save) return yield* json(409, { message: "A personality change is already waiting to be saved." });
    if (await waitShared("setupPersonalities", save.id, save.epoch, save.sequence) === "pending") return yield* json(202, { pending: true, message: PENDING_SAVE });
    return yield* json(200, { changed });
  }
  const [status, value] = takePersonalitiesDocument(parsed(request.body) as Body<"document">, readEngine, readBridge);
  return yield* json(status, value);
}

// The personalities you make, this machine's copy of the library.
export async function* personalityRoute(context: RouteContext, personality: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, readBridge, json } = context;
  let id: string | undefined;
  try { id = personality[1] === undefined ? undefined : decodeURIComponent(personality[1]); } catch { return yield* json(404, {}); }
  const method = request.method.toUpperCase();
  const body = parsed(request.body) as Body<"name" | "description">;
  const setup = readLink()?.setup;
  if (requiresStandby(setup) && method !== "GET") {
    if (setup?.role !== "primary") return yield* json(409, { message: "Change your setup's personalities on its primary." });
    if (!setup.standby) return yield* json(503, { message: PENDING_SAVE });
    let answer: ReturnType<typeof answerPersonalities> = [409, { message: "The setup changed." }];
    const save = changeShared("setupPersonalities", value => {
      const before = isObject(value) ? mergePersonalities(value) : readPersonalities();
      let after = before;
      answer = answerPersonalities(method, id, body, undefined, undefined, change => {
        after = mergePersonalities(change(before));
        return after;
      });
      return answer[0] === 200 ? after : undefined;
    });
    if (answer[0] !== 200) return yield* json(...answer);
    if (!save) return yield* json(409, { message: "A personality change is already waiting to be saved." });
    if (await waitShared("setupPersonalities", save.id, save.epoch, save.sequence) === "pending") return yield* json(202, { pending: true, message: PENDING_SAVE });
    return yield* json(...answer);
  }
  const [status, value] = answerPersonalities(method, id, body);
  // Its bots with it are told what it is now, or that it's gone.
  if (status === 200 && id !== undefined) retellBots([id], readEngine, readBridge).catch(() => {});
  return yield* json(status, value);
}
