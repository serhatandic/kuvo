// Setup-wide line topics. Every box publishes only its own rows and can draw the whole setup from
// its peers' last copies. A peer's settings or a row's action still go to its box.

import fs from "node:fs";
import path from "node:path";
import { needsYouIn } from "../areas/waiting";
import { allItems, queueFollowers as workQueueFollowers } from "../work-queue/store";
import { QUEUES_FEATURE, itemsElsewhere } from "../work-queue/across";
import { botsWithNow } from "../areas/bots";
import type { Infer } from "../../shared/shape";
import { START_CATALOG, type SessionFacts, type SessionList } from "../../shared/routes/sessions";
import { writePrivate } from "../../shared/files";
import { parsed } from "../../shared/json";
import { fitsShape } from "../../shared/shape";
import { RUN } from "../home";
import { offeredAnswer } from "../areas/models-offered";
import { bridgeTarget, readEngineFile } from "../engine/client";
import { queueFollowers, shownQueues } from "../engine/queue";
import { followedLive } from "../follow";
import { readAsks } from "../glance/asked";
import { frozen } from "../moves/records";
import { movedOf } from "../peers/known";
import { SETUP_LIST_FEATURE, PEER_BOT_ACTIVITY, peerListFollowers, peerSetupLists, pushSetupLists, type SetupLists } from "../peers/lists";
import type { LinkHost } from "../relay/host";
import type { AnswerPiece, RouteContext } from "../server/context";
import { readLink } from "../state";
import { listsHere } from "../work/model-rule";
import { pendingComputers } from "../peers/pending";
import { listComputer } from "./computer";
import { listedProjects, projectPage, shownOfCursor, sessionWaits } from "./rule";
import { answerFrom, archivedGroups, keepsList, keptList, listFollowers, listNow } from "./sessions";

export const PAGE_ROUTE = /^kuvo\/sessions\/page$/;
export const SETUP_SESSION_LIST_ROUTE = /^kuvo\/setup\/sessions\/list\/events$/;
export const SETUP_WAIT_COUNTS = /^kuvo\/setup\/waiting\/counts(\/events)?$/;
export const SETUP_LIST_ROUTE = /^kuvo\/setup\/(sessions|waiting|queues|runs-on)\/events$/;

type Readers = Pick<RouteContext, "readEngine" | "bridge">;
type Catalog = Infer<typeof START_CATALOG>;

let catalog: Catalog | undefined;
let catalogAt = -Infinity;
let readingCatalog: Promise<Catalog> | undefined;
/** Whether this process has read its own catalog yet (before: the copy it saved last, `pending`). */
let catalogRead = false;
const catalogFollowers = new Set<() => void>();

/** This computer's catalog as last read, kept so Runs on lists it at once after the box starts
 * again (only none the very first time). */
export const CATALOG_FILE = path.join(RUN, "lists", "runs-on.json");
let catalogSaved = "";

function savedCatalog(): Catalog | undefined {
  try {
    const text = fs.readFileSync(CATALOG_FILE, "utf8");
    const value = parsed(text);
    if (!fitsShape(START_CATALOG, value)) return undefined;
    catalogSaved = text;
    return value as Catalog;
  } catch { return undefined; }
}

function saveCatalog(value: Catalog): void {
  const text = JSON.stringify(value);
  if (text === catalogSaved) return;
  try {
    writePrivate(CATALOG_FILE, text);
    catalogSaved = text;
  } catch {}
}

function startCatalog(readers: Readers): Promise<Catalog> {
  const now = performance.now();
  if (catalog && now - catalogAt < 60_000) return Promise.resolve(catalog);
  // (Just started: the copy saved last is what a side that can't be read yet keeps.)
  if (!catalogRead) catalog ??= savedCatalog();
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
    catalog = { offered, agents: [...agents.values()] };
    if (known.engine || known.agents) saveCatalog(catalog);
    if (!catalogRead) {
      catalogRead = true;
      for (const tell of [...catalogFollowers]) try { tell(); } catch {}
    }
    return catalog;
  })().finally(() => { readingCatalog = undefined; });
  return readingCatalog;
}

type LocalRows = Omit<SetupLists, "catalog" | "bots">;

export async function readList(readers: Readers, signal?: AbortSignal) {
  return (keepsList() ? await listNow(readers, signal) : null) ?? keptList() ?? await listNow(readers, signal);
}

async function ownRows(readers: Readers): Promise<LocalRows | null> {
  const list = await readList(readers);
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
      if (!rows) return null;
      const bots = Object.fromEntries(botsWithNow().map(({ id, session, now, paused, wakes }) => [
        id, { session, now, ...(paused ? { paused } : {}), ...(wakes !== undefined ? { wakes } : {}) },
      ]));
      return { ...rows, bots, catalog: await startCatalog(readers) };
    });
  }
  finally { publishing = false; }
}

type Topic = "session-list" | "sessions" | "waiting" | "queues" | "runs-on";

async function mergedSessionList(context: RouteContext, link = readLink(), saved = peerSetupLists(link)): Promise<SessionList | undefined> {
  const computer = listComputer(link);
  const [peers, here] = await Promise.all([saved, readList(context, context.signal)]);
  const pending = pendingComputers(SETUP_LIST_FEATURE, key => peers.some(({ origin }) => origin.computer.id === key), link);
  if (!here && !peers.length && !pending.length) return;
  const listing: SessionList = here ? answerFrom(here, null) : {
    at: "peers", full: true, sessions: {}, projects: {}, projectOrder: [], startProject: null,
    needsYou: [], archivedGroups: [],
  };
  for (const [id, facts] of Object.entries(listing.sessions)) listing.sessions[id] = { ...facts, computer };
  for (const [directory, facts] of Object.entries(listing.projects)) listing.projects[directory] = { ...facts, computer };
  const archived = [...listing.archivedGroups];
  const waiting = new Map(listing.needsYou?.map(row => [row.session, row]));
  const moved = movedOf(link);
  for (const { saved, origin } of peers) {
    const stamp = (actions: readonly ("open" | "manage")[] = []) => ({
      computer: origin.computer,
      on: { ...origin.computer, away: origin.away, lastSeen: origin.lastSeen, updating: origin.updating, actions: origin.away ? [] : actions },
    });
    const keyOf = (directory: string) => `${origin.computer.id}:${directory}`;
    const peerRows: Record<string, SessionFacts> = {};
    for (const { id, facts, actions } of saved.sessions) {
      const before = listing.sessions[id];
      if (before && !(before.away && !before.on && moved.get(id) === origin.computer.id) ||
          moved.has(id) && moved.get(id) !== origin.computer.id) continue;
      const row = { ...facts, ...stamp(actions) };
      if (facts.kind === "project") {
        const directory = facts.project ?? facts.directory;
        row.project = keyOf(directory);
      }
      peerRows[id] = row;
      listing.sessions[id] = row;
    }
    for (const { session, asks } of saved.waiting) {
      const kind = peerRows[session]?.kind;
      if (kind === "chat" || kind === "project") waiting.set(session, { session, asks });
    }
    const projects = saved.projects.map(({ directory, facts }) => ({
      directory: keyOf(directory), name: facts.name ?? (path.posix.basename(directory) || directory),
    }));
    const peerProjects = listedProjects(peerRows, { projects });
    for (const [directory, facts] of Object.entries(peerProjects)) listing.projects[directory] = { ...facts, ...stamp() };
    archived.push(...archivedGroups(peerRows, peerProjects));
  }
  const needsYou = [...waiting.values()].sort((a, b) =>
    (listing.sessions[b.session]?.asked ?? 0) - (listing.sessions[a.session]?.asked ?? 0) || a.session.localeCompare(b.session));
  return { ...listing, needsYou, archivedGroups: archived, pendingComputers: pending };
}

async function merged(topic: Topic, context: RouteContext) {
  if (topic === "session-list") return mergedSessionList(context);
  const link = readLink();
  const computer = listComputer(link);
  const local: { computer: typeof computer; away: boolean; lastSeen?: number; updating?: true; here?: true } = { computer, away: false, here: true };
  if (topic === "runs-on") {
    // Just started, its own catalog not read yet: the copy it saved last, at once, said to be as
    // last read (`pending`). The live one follows on the same stream.
    if (!catalogRead) catalog ??= savedCatalog();
    const saved = !catalogRead && catalog ? catalog : undefined;
    const reading = startCatalog(context);
    if (saved) reading.catch(() => {});
    const [peers, here] = await Promise.all([peerSetupLists(link), saved ? { ...saved, pending: true as const } : reading]);
    return {
      pendingComputers: pendingComputers(SETUP_LIST_FEATURE, key => peers.some(({ origin }) => origin.computer.id === key), link),
      // (One that's away stays listed, said to be away since it was last seen: never picked.)
      computers: [{ computer, here: true, ...here }, ...peers.map(({ saved, origin }) => ({
        computer: origin.computer, ...saved.catalog,
        // (Nor one switching to a new release, said to be updating.)
        ...(origin.away || origin.updating ? { away: true, ...(origin.lastSeen !== undefined ? { lastSeen: origin.lastSeen } : {}), ...(origin.updating ? { updating: true } : {}) } : {}),
      }))],
    };
  }

  const [peers, here] = await Promise.all([peerSetupLists(link), ownRows(context)]);
  const pending = pendingComputers(SETUP_LIST_FEATURE, key => peers.some(({ origin }) => origin.computer.id === key), link);
  const boxes = here ? [{ saved: here, origin: local }, ...peers] : peers;
  const moved = movedOf(link);
  const owns = (id: string, owner: string) => !moved.has(id) || moved.get(id) === owner;
  const rows = <T extends { actions: readonly string[] }>(items: readonly T[], origin: typeof local, id: (row: T) => string) =>
    items.filter(row => owns(id(row), origin.computer.id)).map(row => ({ ...row, ...origin, actions: origin.away ? [] : row.actions }));
  switch (topic) {
    case "sessions":
      return {
        pendingComputers: pending,
        sessions: boxes.flatMap(({ saved, origin }) => rows(saved.sessions, origin, row => row.id))
          .sort((a, b) => b.facts.updated - a.facts.updated || a.id.localeCompare(b.id)),
        projects: boxes.flatMap(({ saved, origin }) => saved.projects.map(project => ({ ...project, ...origin }))),
      };
    case "waiting":
      return { pendingComputers: pending, waiting: boxes.flatMap(({ saved, origin }) => rows(saved.waiting, origin, row => row.session)).sort((a, b) => b.since - a.since) };
    case "queues":
      return { pendingComputers: pending, queues: boxes.flatMap(({ saved, origin }) => rows(saved.queues, origin, row => row.session)) };
  }
}

export async function* setupListsRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  const topic = (match[1] ?? "session-list") as Topic;
  // A new client can use the same 503 and retry behavior as the single-box list.
  let first = topic === "session-list" ? await merged(topic, context) : undefined;
  if (topic === "session-list" && !first) return yield* json(503, { message: "The engine isn’t running." });
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  yield* followedLive(async () => {
    const initial = first;
    first = undefined;
    return initial ?? merged(topic, context);
  }, tell => {
    const followers = [listFollowers, queueFollowers, peerListFollowers, catalogFollowers];
    for (const set of followers) set.add(tell);
    return () => { for (const set of followers) set.delete(tell); };
  }, signal, { every: 3_000 });
}

/** A single setup-wide count for the workspace rail and notification client. */
export async function* setupWaitCountsRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  const counts = async () => {
    const link = readLink();
    const reading = peerSetupLists(link);
    const [listing, peers] = await Promise.all([mergedSessionList(context, link, reading), reading]);
    if (!listing) return undefined;
    const asking = new Set(Object.entries(listing.sessions)
      .filter(([, facts]) => facts.asked !== undefined && !facts.archived && !facts.away).map(([id]) => id));
    const bots = new Set(botsWithNow().filter(bot => bot.now.asking > 0).map(bot => bot.id));
    for (const { saved, origin } of peers) {
      for (const [id, bot] of Object.entries(saved.bots ?? {})) {
        if (bot.now.asking > 0 && listing.sessions[bot.session]?.computer?.id === origin.computer.id) bots.add(id);
      }
    }
    const waiting = Object.values(listing.sessions).filter(sessionWaits);
    const countedBots = new Set(waiting.flatMap(facts => [facts.bot, ...(facts.line ?? []).map(id => listing.sessions[id]?.bot)]));
    const dock = waiting.length + [...bots].filter(bot => !countedBots.has(bot)).length;
    const queue = itemsElsewhere("all", link);
    const pending = [
      ...(listing.pendingComputers ?? []),
      ...pendingComputers(PEER_BOT_ACTIVITY, key => peers.some(({ saved, origin }) => origin.computer.id === key && saved.bots !== undefined), link),
      ...pendingComputers(QUEUES_FEATURE, key => queue.computers.some(computer => computer.key === key), link),
    ];
    return {
      pendingComputers: [...new Map(pending.map(computer => [computer.id, computer])).values()].sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id)),
      sessions: waiting.length,
      dock,
      bots: bots.size,
      queue: needsYouIn([...allItems(), ...queue.items], asking),
    };
  };
  if (!match[1]) {
    const counted = await counts();
    return counted ? yield* json(200, counted) : yield* json(503, { message: "The session list isn’t ready." });
  }
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  yield* followedLive(counts, tell => {
    const followers = [listFollowers, peerListFollowers, workQueueFollowers];
    for (const set of followers) set.add(tell);
    return () => { for (const set of followers) set.delete(tell); };
  }, signal, { every: 3_000 });
}

/** `GET kuvo/sessions/page?project=<key>&cursor=<its more's>`: a project's list as far as "Show
 * more" has opened it, in the box's order (PROTOCOL.md, "Session lists"); without a cursor, as far
 * as it shows first. A client asks again with the same cursor as the list changes. */
export async function* pageRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, { message: "Only GET." });
  const query = new URLSearchParams(request.path.split("?")[1] ?? "");
  const project = query.get("project");
  if (!project) return yield* json(400, { message: "Name the project: project=<its key>." });
  // A project key selects existing inventory rows. It never resolves a path.
  const name = project.replace(/^[\w-]{43}:/, "");
  if (name.split(/[/\\]/).some(part => part.replace(/[^\x00-\x7f]/g, "") === "..")) {
    return yield* json(400, { message: "Name the project: project=<its key>." });
  }
  const cursor = query.get("cursor");
  const at = cursor ? shownOfCursor(cursor) : undefined;
  if (at === null) return yield* json(400, { message: "That cursor isn't one this box gave. Read the list again." });
  // Peer projects use the same rows as the merged sidebar, including the last copy of an away box.
  if (/^[\w-]{43}:/.test(project)) {
    const merged = await mergedSessionList(context);
    if (!merged) return yield* json(503, { message: "The session list isn’t ready." });
    return yield* json(200, { project, ...projectPage(merged.sessions, project, at) });
  }
  const list = await listNow(context, signal ?? undefined);
  if (!list) return yield* json(503, { message: "The engine isn’t running." });
  const facts = Object.fromEntries(Object.entries(list.sessions).map(([id, entry]) => [id, entry.facts]));
  return yield* json(200, { project, ...projectPage(facts, project, at) });
}
