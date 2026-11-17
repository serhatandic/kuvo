// `kuvo/box/rehome` (PROTOCOL.md, "Moving this Mac's box"): how moving this Mac's box into a home
// of its own stands, for a client to show (the box decides when, and keeps count); Move Now or
// Move Back, asked by Kuvo Code itself, which the keeper acts on as it next asks `rehome --ready`;
// and an earlier Kuvo Code's saved sign-ins brought into the vault (`{signIns: true}`), asked by
// Kuvo on this Mac when you ask it to, since macOS asks you there: started in a process of its own
// (`rehome --sign-ins`), which nothing waits for.

import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import type { RehomeStatus } from "../../shared/routes/box";
import { APP_FOLDER, K, OWN_HOME_MAC, OWN_RUNTIME, SCRIPT, homeFolder, located } from "../home";
import { fromLocalApp, type AnswerPiece, type RouteContext } from "../server/context";
import { askRehome } from "./move";
import { movedHere } from "./run";
import { rehomeFolder, rehomeStatus, signInsRunning, SIGN_INS_PID } from "./state";

/** Where this box's move is kept: in its own home (a box in a home of its own, moved or never in an
 * app's folder), or for the app's box in its folder, the home it moves to: the account's
 * `~/.kuvo-code`, or the folder a copy names (`KUVO_MACHINE_ROOT`, a name in your home folder). A
 * copy that isn't the real app's names none: it never reads or asks the real one's. */
export function rehomeHome(): string | null {
  if (!APP_FOLDER) return K;
  const root = process.env.KUVO_MACHINE_ROOT;
  if (root && !root.includes("/") && root !== "." && root !== "..") return path.join(homeFolder(), root);
  // (The real app's folder alone, by its own name: a demo or test copy's is `run.kuvo.code.<…>`.)
  return path.basename(APP_FOLDER) === "run.kuvo.code" ? path.join(homeFolder(), ".kuvo-code") : null;
}

const NOTHING_YET: RehomeStatus = {
  state: null, way: null, step: null, tries: 0, nextTry: null, waitingSince: null, overdue: false, failed: null,
  movedAt: null, keptUntil: null, owed: [], asked: null, from: null, to: null,
};

export async function* rehomeRoute({ request, json }: RouteContext): AsyncGenerator<AnswerPiece> {
  const method = request.method.toUpperCase();
  const home = rehomeHome();
  if (method === "GET") return yield* json(200, home ? rehomeStatus(home) : NOTHING_YET);
  if (method !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (isObject(given) && given.signIns === true) return yield* signIns(fromLocalApp(request), home, json);
  // (Kuvo Code asks, having asked you: never a phone, which shows it.)
  if (!request.fromApp && !request.fromMac) return yield* json(403, { message: "Only Kuvo Code on this Mac moves its data." });
  const ask = isObject(given) ? (given.now === true ? "now" : given.back === true ? "back" : null) : null;
  if (!ask) return yield* json(400, { message: "Say Move Now (now) or Move Back (back)." });
  if (!home) return yield* json(409, { message: "This copy of Kuvo Code has no home to move its data to." });
  const refused = askRehome(home, ask);
  return yield* (refused ? json(409, { message: refused }) : json(200, rehomeStatus(home)));
}

/** As this Mac's box starts in a home of its own: what a move into it that's done still owes
 * that asks nothing (its start, the Keychain's originals, the old keeper), settled in a process of
 * its own (`rehome --settle`), which nothing waits for. Never the saved sign-ins: you start those. */
export function settleOwedAtStart(): void {
  const home = rehomeHome();
  if (!OWN_HOME_MAC || !home || !ownHome(home) || !movedHere(home)) return;
  if (!rehomeStatus(home).owed.some(what => what !== "passwords")) return;
  start(home, "--settle");
}

/** Whether `home` is this box's and a folder in your home folder, as `rehome <root>` names it. */
const ownHome = (home: string): boolean => located(home) === located(K) && located(path.dirname(home)) === located(homeFolder());

/** `rehome <root> <verb>` in a process of its own, what it says for support in the move's log;
 * its process ID, where it started. */
function start(home: string, verb: "--settle" | "--sign-ins"): number | undefined {
  fs.mkdirSync(rehomeFolder(home), { recursive: true, mode: 0o700 });
  const log = fs.openSync(path.join(rehomeFolder(home), "run.log"), "a", 0o600);
  try {
    const child = spawn(process.execPath, [...OWN_RUNTIME, SCRIPT, "rehome", path.basename(home), verb], { detached: true, stdio: ["ignore", "ignore", log], cwd: home, env: { ...process.env, KUVO_HOME: home } });
    child.once("error", () => {});
    child.unref();
    return child.pid;
  } finally {
    fs.closeSync(log);
  }
}

/** The saved sign-ins' hand-over started, for Kuvo on this Mac alone (macOS asks you there, never
 * on a phone); one that's running already is left to run. */
async function* signIns(local: boolean, home: string | null, json: RouteContext["json"]): AsyncGenerator<AnswerPiece> {
  if (!local) return yield* json(403, { message: "Your saved sign-ins come over from Kuvo on the Mac they were saved on." });
  if (!OWN_HOME_MAC || !home || !ownHome(home)) return yield* json(409, { message: "There are no saved sign-ins to bring over on this Mac." });
  if (signInsRunning(home)) return yield* json(200, rehomeStatus(home));
  if (!movedHere(home) || !rehomeStatus(home).owed.includes("passwords")) return yield* json(409, { message: "There are no saved sign-ins left to bring over." });
  const pid = start(home, "--sign-ins");
  if (pid) fs.writeFileSync(SIGN_INS_PID(home), String(pid), { mode: 0o600 });
  return yield* json(202, rehomeStatus(home));
}
