// `kuvo/mac` (PROTOCOL.md, While Agents Work), answered by a Mac's box in a home of its own, where
// its app answered it: whether Kuvo keeps running with no app open, and whether it keeps the Mac
// awake while agents work. Both are this box's own facts there. It runs under Kuvo Helper, which
// macOS starts at login unless you switched it off in Login Items (`helperAtLogin`), and it holds
// the Mac awake itself by its own setting (`work/awake.ts`, `kuvo/work/settings`).

import { isObject, parsed } from "../../shared/json";
import { HELPER_NAME, helperAtLogin } from "../helper";
import { ownName } from "../letters/start";
import type { AnswerPiece, RouteContext } from "../server/context";
import { awakeForWork } from "../work/awake";
import { KEEPS_AWAKE_REFUSED, keepsAwakeSetting, setKeepsAwake } from "../work/settings";

/** Background work, in the words a phone knows: `on` while Kuvo Helper starts at login;
 * `needsApproval` while you've switched it off in Login Items (only you turn it on again, on the
 * Mac); `off` where no helper keeps this box at login (a test's or a demo's copy). */
export const backgroundWork = (atLogin: boolean | null): "on" | "needsApproval" | "off" => (atLogin === true ? "on" : atLogin === false ? "needsApproval" : "off");

// What Login Items last said. `helperAtLogin` can't say both where no helper keeps this box and
// where launchd didn't answer in time: one that has said once isn't taken for "no helper" then.
let lastSaid: boolean | null = null;
function atLoginNow(): boolean | null {
  const said = helperAtLogin();
  if (said !== null) lastSaid = said;
  return said ?? lastSaid;
}

export async function* ownMacRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  const method = request.method.toUpperCase();
  if (method !== "GET" && method !== "PATCH") return yield* json(405, {});
  // (Asked once for a request: it's `launchctl`, and nothing here changes what it says.)
  const state = backgroundWork(atLoginNow());
  if (method === "PATCH") {
    const given = parsed(request.body);
    const body = isObject(given) ? given : {};
    // (A phone from before background work asks for Open at Login, which it replaced: the same.
    // As the Mac's app took it; it goes when no phone that old is left, with `openAtLogin` below.)
    const wanted = typeof body.backgroundWork === "boolean" ? body.backgroundWork : typeof body.openAtLogin === "boolean" ? body.openAtLogin : undefined;
    // Both checked before either is kept: when one can't be done, nothing changes.
    if ("keepsAwake" in body && typeof body.keepsAwake !== "boolean") return yield* json(400, { message: KEEPS_AWAKE_REFUSED });
    if (wanted === false && state !== "off") return yield* json(400, { message: "Background work is turned off in Login Items & Extensions, on the Mac." });
    // (Switched off in Login Items, asked for: it waits for you there, as the answer says.)
    if (wanted === true && state === "off") return yield* json(409, { message: `${HELPER_NAME} isn’t set up to start Kuvo Code when you log in to ${ownName()}. Open Kuvo Code there.` });
    if (typeof body.keepsAwake === "boolean") setKeepsAwake(body.keepsAwake);
  }
  return yield* json(200, { backgroundWork: state, openAtLogin: state === "on", keepsAwake: keepsAwakeSetting(), awakeNow: awakeForWork() });
}
