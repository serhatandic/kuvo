// The speed you picked for each provider (PROTOCOL.md, "The models offered", `models.speed`): kept
// by the box in `box/models.json` beside your starred models, so every client's picker shows the
// same, and the agents bridge runs each turn at it with no client there (shared/model-speeds.ts).
// `kuvo/models/offered` says it with each provider; `POST kuvo/models/speed` picks one, or Standard.

import { isObject, parsed } from "../../shared/json";
import { isSpeedID, isSpeedProvider, pickedSpeeds } from "../../shared/model-speeds";
import { locked, textOf, writePrivate } from "../files";
import { settingsDisk } from "../server/disk-worker";
import { MODELS_FILE } from "./models-starred";
import type { AnswerPiece, RouteContext } from "../server/context";

export const MODELS_SPEED_ROUTE = "kuvo/models/speed";

/** The most providers kept: a list no one could pick for by hand stays small. */
const MOST = 200;

/** The speed picked for each provider (a speed's id, or null for Standard picked yourself). */
export function modelSpeeds(file = MODELS_FILE): Record<string, string | null> {
  return pickedSpeeds(file);
}

/** Picks `speed` for `providerID` (null: Standard); what's picked for it now. What else the file
 * keeps (your starred models) stays. */
export function setSpeed(providerID: string, speed: string | null, file = MODELS_FILE): { providerID: string; speed: string | null } {
  return locked(`${file}.lock`, () => {
    let kept: Record<string, unknown> = {};
    try {
      const read = JSON.parse(textOf(file) ?? "");
      if (isObject(read)) kept = read;
    } catch {}
    const was = pickedSpeeds(file);
    if (!(Object.hasOwn(was, providerID) && was[providerID] === speed)) {
      const others = Object.entries(was).filter(([provider]) => provider !== providerID).slice(-(MOST - 1));
      writePrivate(file, JSON.stringify({ ...kept, speeds: Object.fromEntries([...others, [providerID, speed]]) }, null, 2));
    }
    return { providerID, speed };
  });
}

// `POST kuvo/models/speed` with `{providerID, speed}` (a speed's id, or null for Standard) picks it
// and answers `{providerID, speed}`; `400` for anything else sent.
export async function* modelsSpeedRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isObject(given) || !isSpeedProvider(given.providerID) || !(given.speed === null || isSpeedID(given.speed))) {
    return yield* json(400, { message: "Say which provider (providerID) and its speed (speed: a speed's id, or null for Standard)." });
  }
  return yield* json(200, await settingsDisk.run("setSpeed", given.providerID, given.speed));
}
