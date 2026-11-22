// Setup-wide line topics. Every box publishes only its own rows and can draw the whole setup from
// its peers' last copies. A peer's settings or a row's action still go to its box.

import type { Infer } from "../../shared/shape";
import type { START_CATALOG } from "../../shared/routes/sessions";
import { offeredAnswer } from "../areas/models-offered";
import { bridgeTarget, readEngineFile } from "../engine/client";
import { queueFollowers, shownQueues } from "../engine/queue";
import { followedLive } from "../follow";
import { readAsks } from "../glance/asked";
import { frozen } from "../moves/records";
import { movedOf } from "../peers/known";
import { peerListFollowers, peerSetupLists, pushSetupLists, type SetupLists } from "../peers/lists";
import type { LinkHost } from "../relay/host";
import type { AnswerPiece, RouteContext } from "../server/context";
import { readLink } from "../state";
import { listsHere } from "../work/model-rule";
import { listComputer } from "./computer";
import { keepsList, keptList, listFollowers, listNow } from "./sessions";

export const SETUP_LIST_ROUTE = /^kuvo\/setup\/(sessions|waiting|queues|runs-on)\/events$/;

type Readers = Pick<RouteContext, "readEngine" | "bridge">;
type Catalog = Infer<typeof START_CATALOG>;

let catalog: Catalog | undefined;
let catalogAt = -Infinity;
let readingCatalog: Promise<Catalog> | undefined;

function startCatalog(readers: Readers): Promise<Catalog> {
  const now = performance.now();
  if (catalog && now - catalogAt < 60_000) return Promise.resolve(catalog);
  readingCatalog ??= (async () => {
    let engine = null;
    try { engine = readers.readEngine(); } catch {}
    // Background replication never wakes a parked engine. Use its last model list while parked.
    const lists = await listsHere(engine, readers.bridge());
    const { models, known, listing, starting, startChoice, providers } = offeredAnswer(lists, null);
    const prior = catalog?.offered;
    const selectable = models.filter(model => !model.hidden && !model.retiring);
    const offered = {
      models: [...(!known.engine ? prior?.models.filter(model => model.side === "engine") ?? [] : []), ...selectable],
      providers: [
        ...(!known.engine ? prior?.providers?.filter(provider => prior.models.some(model => model.side === "engine" && model.providerID === provider.id)) ?? [] : []),
        ...providers.filter(provider => selectable.some(model => model.providerID === provider.id)).map(({ id, name, label }) => ({ id, name, ...(label ? { label } : {}) })),
      ],
      known, listing, starting: known.engine ? starting : starting ?? prior?.starting, startChoice,
    };
    const agents = new Map<string, Catalog["agents"][number]>();
    for (const provider of providers) {
      if (provider.account && selectable.some(model => model.providerID === provider.id))
        agents.set(provider.account.agent, { id: provider.account.agent, name: provider.account.agentName });
    }
    catalogAt = now;
    return catalog = { offered, agents: [...agents.values()] };
  })().finally(() => { readingCatalog = undefined; });
  return readingCatalog;
}

type LocalRows = Omit<SetupLists, "catalog">;

async function ownRows(readers: Readers, signal?: AbortSignal): Promise<LocalRows | null> {
  const list = (keepsList() ? await listNow(readers, signal) : null) ?? keptList() ?? await listNow(readers, signal);
  if (!list) return null;
  const moving = frozen();
  const sessions: SetupLists["sessions"] = Object.entries(list.sessions).flatMap(([id, { facts }]) => {
    if (facts.away) return [];
    const actions: SetupLists["sessions"][number]["actions"] = moving.has(id) ? [] :
      ["subagent", "hidden", "seat"].includes(facts.kind) ? ["open"] : ["open", "manage"];
    return [{ id, facts, actions }];
  });
  const projects = Object.entries(list.projects).map(([directory, facts]) => ({
    directory, facts: { ...facts, asking: facts.asking ?? 0, working: facts.working ?? 0 },
  }));

  // The list's existing summary for chats and projects, with the oldest ask for bots as well.
  // Full cards and their choices are read on the owning computer, with the client's authority.
  const asks = new Map(list.needsYou?.map(row => [row.session, row.asks]));
  const bots = new Map<string, ReturnType<typeof readAsks>[number]>();
  for (const ask of readAsks()) {
    let root = ask.session;
    for (let step = 0; step < 10 && list.sessions[root]?.facts.parent; step++) root = list.sessions[root]!.facts.parent!;
    if (list.sessions[root]?.facts.kind === "bot" && ask.at < (bots.get(root)?.at ?? Infinity)) bots.set(root, ask);
  }
  const waiting: SetupLists["waiting"] = sessions.flatMap(({ id, facts }) => {
    if (facts.asked === undefined || facts.archived || !["project", "chat", "bot"].includes(facts.kind)) return [];
    return [{
      session: id, since: facts.asked, title: facts.title,
      asks: asks.get(id) ?? bots.get(id)?.asks ?? { kind: "other", summary: "waits on you" },
      actions: moving.has(id) ? [] : ["review"],
    }];
  });
  const owned = new Set(sessions.map(row => row.id));
  const queues: SetupLists["queues"] = Object.entries(shownQueues()).flatMap(([session, queue]) => {
    if (!owned.has(session)) return [];
    return [{ session, queue, actions: moving.has(session) ? [] : ["open", queue.paused ? "resume" : "pause", "messages"] }];
  });
  return { sessions, projects, waiting, queues };
}

let publishing = false;
/** The existing three-second peer round calls here on both operating systems. */
export async function shareSetupLists(host: Pick<LinkHost, "send">, readers: Readers = { readEngine: readEngineFile, bridge: bridgeTarget }): Promise<void> {
  if (publishing) return;
  publishing = true;
  try {
    await pushSetupLists(host, async () => {
      const rows = await ownRows(readers);
      return rows && { ...rows, catalog: await startCatalog(readers) };
    });
  }
  finally { publishing = false; }
}

type Topic = "sessions" | "waiting" | "queues" | "runs-on";

async function merged(topic: Topic, context: RouteContext) {
  const link = readLink();
  const computer = listComputer(link);
  const local: { computer: typeof computer; away: boolean; lastSeen?: number; updating?: true; here?: true } = { computer, away: false, here: true };
  if (topic === "runs-on") {
    const [peers, here] = await Promise.all([peerSetupLists(link), startCatalog(context)]);
    return {
      // (One that's away stays listed, said to be away since it was last seen: never picked.)
      computers: [{ computer, here: true, ...here }, ...peers.map(({ saved, origin }) => ({
        computer: origin.computer, ...saved.catalog,
        ...(origin.away ? { away: true, lastSeen: origin.lastSeen, ...(origin.updating ? { updating: true } : {}) } : {}),
      }))],
    };
  }

  const [peers, here] = await Promise.all([peerSetupLists(link), ownRows(context, context.signal)]);
  const boxes = here ? [{ saved: here, origin: local }, ...peers] : peers;
  const moved = movedOf(link);
  const owns = (id: string, owner: string) => !moved.has(id) || moved.get(id) === owner;
  const rows = <T extends { actions: readonly string[] }>(items: readonly T[], origin: typeof local, id: (row: T) => string) =>
    items.filter(row => owns(id(row), origin.computer.id)).map(row => ({ ...row, ...origin, actions: origin.away ? [] : row.actions }));
  switch (topic) {
    case "sessions":
      return {
        sessions: boxes.flatMap(({ saved, origin }) => rows(saved.sessions, origin, row => row.id))
          .sort((a, b) => b.facts.updated - a.facts.updated || a.id.localeCompare(b.id)),
        projects: boxes.flatMap(({ saved, origin }) => saved.projects.map(project => ({ ...project, ...origin }))),
      };
    case "waiting":
      return { waiting: boxes.flatMap(({ saved, origin }) => rows(saved.waiting, origin, row => row.session)).sort((a, b) => b.since - a.since) };
    case "queues":
      return { queues: boxes.flatMap(({ saved, origin }) => rows(saved.queues, origin, row => row.session)) };
  }
}

export async function* setupListsRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  const topic = match[1] as Topic;
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  yield* followedLive(() => merged(topic, context), tell => {
    const followers = [listFollowers, queueFollowers, peerListFollowers];
    for (const set of followers) set.add(tell);
    return () => { for (const set of followers) set.delete(tell); };
  }, signal, { every: 3_000 });
}
