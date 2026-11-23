import { OUTPUT_COPY, SETUP_OUTPUTS } from "../../shared/routes/artifacts";
import { fitsShape, object, type Infer } from "../../shared/shape";
import { parsed } from "../../shared/json";
import { readServices } from "../../shared/box-services";
import { followedLive } from "../follow";
import { RUN } from "../home";
import { listComputer } from "../lists/computer";
import { readList } from "../lists/setup";
import { peersOf } from "../peers/known";
import { peerListFollowers, peerSetupLists, peerSnapshots } from "../peers/lists";
import { pendingComputers } from "../peers/pending";
import { readReach } from "../peers/reach";
import type { LinkHost } from "../relay/host";
import type { AnswerPiece, RouteContext } from "../server/context";
import { readLink } from "../state";
import { copyStanding } from "../peers/lists";

const outputCopy = object(OUTPUT_COPY);
type OutputCopy = Infer<typeof outputCopy>;
type Output = Infer<typeof SETUP_OUTPUTS>["artifacts"][number];
const FEATURE = "line.merged-outputs";
export const outputCopies = peerSnapshots<OutputCopy>("outputCopy", "outputs-v1.json", FEATURE, OUTPUT_COPY);

async function localOutputs(sessions?: Set<string>, signal?: AbortSignal): Promise<OutputCopy | null> {
  const target = readServices(RUN);
  if (!target) return null;
  const query = new URLSearchParams();
  for (const session of sessions ?? []) query.append("session", session);
  try {
    const response = await fetch(`http://127.0.0.1:${target.port}/api/kuvo/link/artifacts/peer-copy?${query}`, {
      headers: { authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}` },
      signal,
    });
    const value: unknown = response.ok ? await response.json() : null;
    if (!response.ok) await response.body?.cancel();
    return fitsShape(outputCopy, value) ? value as OutputCopy : null;
  } catch { return null; }
}

export const shareOutputs = (host: Pick<LinkHost, "send">) => outputCopies.push(host, () => localOutputs());

type Order = [number, number, string, string];
const order = (row: Output): Order => [Number(row.pinned), row.updated ?? 0, row.id, row.computer.id];
const compare = (a: Order, b: Order) => b[0] - a[0] || b[1] - a[1] || b[2].localeCompare(a[2]) || b[3].localeCompare(a[3]);

function beforeOf(value: string | null): Order | null {
  if (!value) return null;
  const key = parsed(Buffer.from(value, "base64url"));
  if (!Array.isArray(key) || key.length !== 4 || typeof key[0] !== "number" || typeof key[1] !== "number" ||
      typeof key[2] !== "string" || typeof key[3] !== "string") return null;
  return [key[0], key[1], key[2], key[3]];
}

async function setupOutputs(context: RouteContext, root: string, before: Order | null) {
  const link = readLink();
  const [local, known] = await Promise.all([readList(context, context.signal), peerSetupLists(link)]);
  const parents = new Map<string, Set<string>>();
  const entries = [
    ...Object.entries(local?.sessions ?? {}).map(([id, entry]) => ({ id, facts: entry.facts })),
    ...known.flatMap(({ saved }) => saved.sessions),
  ];
  for (const { id, facts } of entries) {
    for (const parent of new Set([facts.parent, ...(facts.line ?? [])])) {
      if (!parent) continue;
      let children = parents.get(parent);
      if (!children) parents.set(parent, children = new Set());
      children.add(id);
    }
  }
  const sessions = new Set([root]);
  for (const session of sessions) for (const child of parents.get(session) ?? []) sessions.add(child);

  const reach = readReach();
  const now = Date.now();
  const [here, ...peers] = await Promise.all([
    localOutputs(sessions, context.signal),
    ...peersOf(link).map(async peer => ({ peer, copy: await outputCopies.read(peer.key) })),
  ]);
  const pending = pendingComputers(FEATURE, key => peers.some(({ peer, copy }) => peer.key === key && copy !== null), link);
  if (!here && !peers.some(({ copy }) => copy) && !pending.length) return;
  const rows: Output[] = [];
  const add = ({ sessions: _, ...row }: OutputCopy["artifacts"][number], computer: Output["computer"], away: boolean, seen?: number) => {
    rows.push({ ...row, id: `${computer.id}:${row.id}`, artifactId: row.id, computer, away, ...(away ? { lastSeen: seen } : {}) });
  };
  const localComputer = { ...listComputer(link), here: true as const };
  for (const row of here?.artifacts ?? []) add(row, localComputer, false);
  for (const { peer, copy } of peers) {
    if (!copy) continue;
    // (Away by the one rule, `standingOf`.)
    const { away, lastSeen } = copyStanding(peer.key, copy.seen, reach, undefined, now);
    const computer = { id: peer.key, name: peer.name };
    for (const row of copy.artifacts) {
      if (row.sessions.some(session => sessions.has(session))) add(row, computer, away, lastSeen);
    }
  }
  const sorted = rows.filter(row => !before || compare(order(row), before) > 0).sort((a, b) => compare(order(a), order(b)));
  const artifacts = sorted.slice(0, 200);
  const next = sorted.length > artifacts.length ? Buffer.from(JSON.stringify(order(artifacts.at(-1)!))).toString("base64url") : null;
  return { artifacts, next, pendingComputers: pending };
}

export async function* setupOutputsRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, json, signal } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const query = new URLSearchParams(request.path.split("?")[1]);
  const session = query.get("session");
  const before = beforeOf(query.get("before"));
  if (!session || !/^ses_[A-Za-z0-9_-]{1,95}$/.test(session)) return yield* json(400, { message: "Choose a conversation." });
  const read = () => setupOutputs(context, session, before);
  if (route === "kuvo/setup/outputs") {
    const state = await read();
    return state ? yield* json(200, state) : yield* json(503, { message: "The outputs aren't ready yet." });
  }
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  yield* followedLive(read, tell => {
    peerListFollowers.add(tell);
    outputCopies.followers.add(tell);
    return () => { peerListFollowers.delete(tell); outputCopies.followers.delete(tell); };
  }, signal, { every: 3_000 });
}
