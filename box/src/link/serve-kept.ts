// The box's server for clients, kept running by its keeper (#333 piece 3; PROTOCOL.md, "Kept for
// clients"): while a client wants one (`run/serve.needed`, `kuvo/box/serve`), `bin/kuvo-start`
// keeps `kuvo-link.js serve --kept` running on a box whose app doesn't run its own `serve` (a
// machine's; a Mac's own box has its app's keeper, or its helper, run that). It answers clients'
// routes as `serve` does, streams included, where `run/serve.json` says (`{pid, port, password,
// kept}`), and nothing else: what the box does in the background stays with what does it there
// (its link, `run`, and the keeper), so nothing is done twice. It goes once it isn't wanted, or
// another took its place.

import fs from "node:fs";
import path from "node:path";
import { UnsupportedFormatVersion } from "../shared/format-version";
import { readRendezvous, serveRecordFormat } from "../shared/records";
import { runsAs } from "../shared/platform";
import { isObject, parsed } from "../shared/json";
import { writePrivate } from "../shared/files";
import { IS_MAC, RUN } from "./home";
import { keepKeeper } from "./keeper";
import type { AnswerPiece, RouteContext } from "./server/context";

export const SERVE_NEEDED = () => path.join(RUN, "serve.needed");
export const SERVE_FILE = () => path.join(RUN, "serve.json");

export const readServeRecord = () => readRendezvous<Partial<ServeRecord>>(serveRecordFormat, SERVE_FILE());

export function writeServeRecord(record: ServeRecord): void {
  try { readServeRecord(); } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
  }
  writePrivate(SERVE_FILE(), JSON.stringify(serveRecordFormat.write(record)));
}

interface ServeRecord { pid: number; port: number; kept: boolean; password: string }
/** The command line of `serve`, and of one kept for clients. */
export const SERVE_ROLE = /kuvo-link\.js serve(\s|$)/;
export const KEPT_ROLE = /kuvo-link\.js serve --kept(\s|$)/;
/** Where `serve` answers now, as its record says, when that process runs as `serve` (its number
 * taken since by another process isn't believed). */
export function runningServe(): ServeRecord | null {
  try {
    const record = readServeRecord();
    if (isObject(record) && Number.isInteger(record.pid) && typeof record.password === "string" && runsAs(record.pid, record.kept === true ? KEPT_ROLE : SERVE_ROLE)) {
      return { pid: record.pid as number, port: Number(record.port), kept: record.kept === true, password: record.password };
    }
  } catch (error) {
    if (error instanceof UnsupportedFormatVersion) throw error;
  }
  return null;
}

/** `kuvo/box/serve`: whether a client wants the box's server kept running here (`GET`, for any client), and asking
 * for it or letting it go (`POST {needed}`, from Kuvo Code itself). Asked for, the keeper's started
 * should it have gone; it starts the server within its next round (15 s at most). A Mac's own box
 * answers 409: its app's keeper, or its helper, runs its server. */
export async function* serveRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  if (method !== "GET" && method !== "POST") return yield* json(405, {});
  if (IS_MAC) return yield* json(409, { message: "Kuvo Code itself runs the server on this Mac." });
  // Whether it serves as it was asked: looked at before it's let go, which it may notice (and go)
  // before a look after would end.
  const running = runningServe();
  if (method === "POST") {
    if (!request.fromApp && !request.fromMac) return yield* json(403, { message: "Only Kuvo Code asks for Kuvo's server on this computer." });
    const given = parsed(request.body);
    if (!isObject(given) || typeof given.needed !== "boolean") return yield* json(400, { message: "Say whether it's needed (needed: true or false)." });
    if (given.needed) {
      writePrivate(SERVE_NEEDED(), "");
      keepKeeper();
    } else {
      fs.rmSync(SERVE_NEEDED(), { force: true });
    }
  }
  return yield* json(200, { needed: fs.existsSync(SERVE_NEEDED()), serving: running !== null });
}
