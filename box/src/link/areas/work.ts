import { PEER_REACH } from "../peers/reach";
import { askingNow } from "../work/tree";
// A bot's work, as every client shows it (PROTOCOL.md, "Work"): `GET kuvo/work?bot=<id>`, its
// sessions and where each stands, the asks it answered for them, what it started today and the
// limits you set; `GET kuvo/work/events`, when each bot's work last changed, a line now and one for
// each change; what a command a session left running printed (`kuvo/work/background/<session>/
// commands/<id>/output`, and its events); a conversation's subagents, at work and done
// (`kuvo/work/subagents/<session>`, and its events); `POST kuvo/bots/<id>/stop-all`, every session of its work stopped, and the bot's own
// turn with what it was still to hear of them. What a bot asks
// you before it does it (`kuvo/work/asks`, its events, and your answer, `POST kuvo/work/asks/<id>`),
// where new projects go (`kuvo/work/settings`), and making a project (`POST kuvo/projects/new`).

import path from "node:path";
import { listOf } from "../files";
import { readAsks } from "../glance/asked";
import { isObject, parsed } from "../../shared/json";
import { followed, followersOf } from "../follow";
import { botHere } from "../bots/here";
import { targetsOf } from "../letters/tools";
import { answersOn } from "../work/answers";
import { readWorkChanges, WORK_FOLDER, workFollowers } from "../work/changes";
import { stopBotWork } from "../work/stop";
import { oversightFollowers, readOversight } from "../work/oversight";
import { botLimits } from "../work/limits";
import { answerWork, askFollowers, workAsks } from "../work/asks";
import { madeWords, makeProject } from "../work/projects";
import { startOfDay } from "../work/clock";
import { patchWorkSettings, folderGiven, KEEPS_AWAKE_REFUSED, setKeepsAwake, setProjectsFolder, workSettings } from "../work/settings";
import { awakeForWork, keepsMacAwakeItself } from "../work/awake";
import { unansweringList, unansweringNow } from "../work/unanswering";
import { call } from "../engine/client";
import { readProjects } from "./projects";
import { BOTS, HERE } from "../home";
import { backgroundFollowers, readBackground, readSessionBackground, readStopped } from "../work/background";
import { bridgeCommand, commandOf, followOutput, outputReader, rememberCommands } from "../work/command-output";
import { subagentsOf } from "../work/subagents";
import type { Target } from "../../shared/records";
import { keptList, LIST_FILE, listFollowers, listNow } from "../lists/sessions";
import { describeWork, ofBot, workSessions } from "../work/tree";
import { activityFacts, activityOf } from "../work/activity";
import { ACTIVITY_FOLDER } from "../work/activity-peers";
import { workAccounts } from "../work/accounts";
import { workBot, workRow } from "../work/listed-work";
import type { BotWork, WorkAnswer } from "../../shared/routes/work";
import { fromAPhone, type AnswerPiece, type RouteContext } from "../server/context";

export const WORK_ROUTE = /^kuvo\/(?:work(?:\/events|\/oversight(?:\/events)?|\/background(?:\/events|\/([^/]+)(\/events)?)?)?|bots\/([^/]+)\/stop-all)$/;
export const COMMAND_OUTPUT_ROUTE = /^kuvo\/work\/background\/([^/]+)\/commands\/([^/]+)\/output(\/events)?$/;
export const SUBAGENTS_ROUTE = /^kuvo\/work\/subagents\/([^/]+)(\/events)?$/;
export const WORK_ASKS_ROUTE = /^kuvo\/work\/(?:settings|unanswering|asks(?:\/([^/]+))?)$/;

// A bot's work as clients show it.
async function botWork(bot: string, context: RouteContext): Promise<BotWork> {
  const { readEngine, bridge, signal } = context;
  const targets = targetsOf({ readEngine, bridge });
  const work = ofBot(bot, await workSessions(targets, signal ?? undefined));
  const list = await listNow(context, signal ?? undefined);
  const activity = activityOf(activityFacts(list), botHere(bot)?.session, bot);
  const active = new Map(activity.sessions.map(row => [row.session, row]));
  const working = new Set(activity.sessions.filter(row => row.state === "working").map(row => row.session));
  // A few at a time: each asks the engine what it asks and said.
  const sessions: Awaited<ReturnType<typeof describeWork>>[] = [];
  let recent = 0;
  const shown = [];
  for (let at = 0; at < work.length && !signal?.aborted; at += 8) {
    const checked = await Promise.all(work.slice(at, at + 8).map(async listed => ({ listed, ask: active.get(listed.data.id)?.state === "asking" ? await askingNow(listed, signal ?? undefined) : null })));
    for (const row of checked) {
      if (active.has(row.listed.data.id)) shown.push(row);
      else if (recent < 60) {
        shown.push(row);
        recent++;
      }
    }
  }
  const asked = workAccounts(shown.map(({ listed }) => ({ id: listed.data.id, target: listed.target, provider: listed.data.model?.providerID })), signal ?? undefined);
  for (let at = 0; at < shown.length; at += 8) {
    sessions.push(...await Promise.all(shown.slice(at, at + 8).map(({ listed, ask }) => describeWork(listed, working, { replies: true, ask: active.get(listed.data.id)?.state === "asking" ? ask ?? { id: "", what: "Waiting for you" } : null, signal: signal ?? undefined }))));
  }
  const describedIDs = new Set(sessions.map(row => row.session));
  for (const row of activity.sessions) {
    if (describedIDs.has(row.session)) continue;
    sessions.push({ ...row, folder: list?.sessions[row.session]?.facts.directory ?? "", startedBy: "", depth: 1, started: 0, tokens: 0 });
  }
  const allAnswered: WorkAnswer[] = work.flatMap(each => answersOn(each.data.metadata).map(answer => ({ ...answer, session: each.data.id }))).sort((a, b) => b.at - a.at);
  const answered = allAnswered.slice(0, 100);
  const accounts = await asked;
  const midnight = startOfDay(Date.now());
  const today = work.filter(each => (each.data.time?.created ?? 0) >= midnight);
  const cost = today.reduce((sum, each) => sum + (typeof each.data.cost === "number" ? each.data.cost : 0), 0);
  return {
    bot,
    activity,
    // (A reply from before it was stopped at rest isn't how the stopped work ended: a row can't say so.)
    sessions: sessions.map(({ askID: _, replyBeforeStop, lastReply, ...described }) => {
      const account = accounts.get(described.session);
      const shown = !replyBeforeStop && lastReply ? { ...described, lastReply } : described;
      return account ? { ...shown, account } : shown;
    }),
    answered,
    today: {
      started: today.length,
      // (The asks it answered since midnight here, as the bar over its composer says them.)
      answered: allAnswered.filter(answer => answer.at >= midnight).length,
      working: activity.working,
      tokens: today.reduce((sum, each) => sum + (each.data.tokens?.input ?? 0) + (each.data.tokens?.output ?? 0) + (each.data.tokens?.reasoning ?? 0), 0),
      ...(cost > 0 ? { cost } : {}),
    },
    limits: botLimits(bot),
    asks: workAsks().filter(ask => ask.bot === bot),
    projects: readProjects().filter(project => !project.removed && project.madeBy?.bot === bot),
  };
}

// The desktop's recent sessions and activity come from the same kept list as its sidebar.
function botWorkSummary(bot: string, root: string): BotWork {
  const list = keptList();
  const activity = activityOf(activityFacts(list), root, bot);
  const active = new Map(activity.sessions.map(row => [row.session, row]));
  const listed = list?.sessions ?? {};
  // A list written before the work fields were added still has ownership and ancestry.
  const ids = list?.workByBot?.[bot] ?? (list?.workByBot ? [] : Object.keys(listed).filter(id => id !== root && workBot(listed[id]!.facts) === bot));
  const work = ids.flatMap(id => listed[id] && id !== root ? [{ id, ...listed[id] }] : [])
    .sort((a, b) => b.facts.created - a.facts.created || b.id.localeCompare(a.id));
  const asks = new Map<string, string>();
  for (const ask of readAsks().sort((a, b) => a.at - b.at)) {
    if (ask.asks && !asks.has(ask.session)) asks.set(ask.session, ask.asks.summary);
  }
  const stopped = readStopped();
  const sessions: BotWork["sessions"][number][] = [];
  const shown = new Set<string>();
  const midnight = startOfDay(Date.now());
  const today: BotWork["today"] = { started: 0, answered: 0, working: activity.working, tokens: 0 };
  let cost = 0;
  let recent = 0;
  const answered: WorkAnswer[] = [];
  for (const { id, facts, work: detail } of work) {
    if (facts.created >= midnight) {
      today.started++;
      today.tokens += detail?.tokens ?? 0;
      cost += detail?.cost ?? 0;
    }
    for (const answer of detail?.answered ?? []) {
      answered.push({ ...answer, session: id });
      if (answer.at >= midnight) today.answered!++;
    }
    const state = active.get(id)?.state;
    if (!state && recent++ >= 60) continue;
    sessions.push(workRow(id, facts, detail, list?.projects[facts.project ?? facts.directory]?.name, {
      state, stopped: stopped[id], ...(state === "asking" ? { asks: asks.get(id) ?? "Waiting for you" } : {}),
    }));
    shown.add(id);
  }
  for (const row of activity.sessions) {
    if (shown.has(row.session)) continue;
    sessions.push({ ...row, folder: listed[row.session]?.facts.directory ?? "", startedBy: "", depth: 1, started: 0, tokens: 0 });
  }
  if (cost > 0) today.cost = cost;
  return {
    bot, activity, sessions, today,
    answered: answered.sort((a, b) => b.at - a.at).slice(0, 100),
    limits: botLimits(bot),
    asks: workAsks().filter(ask => ask.bot === bot),
    projects: readProjects().filter(project => !project.removed && project.madeBy?.bot === bot),
  };
}

export async function* workRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  const method = request.method.toUpperCase();
  const route = request.path.split("?")[0];
  if (route === "kuvo/work/events") {
    if (method !== "GET") return yield* json(405, {});
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    const seen = new Map<string, { key: string; changed: number }>();
    const now = () => {
      const changed = readWorkChanges();
      const list = keptList();
      const rows = activityFacts(list);
      for (const id of listOf(BOTS)) {
        const bot = botHere(id);
        if (!bot) continue;
        const work = list?.workByBot?.[id] ?? [];
        const seq = work.reduce((latest, session) => Math.max(latest, list?.sessions[session]?.workSeq ?? 0), 0);
        const key = JSON.stringify([activityOf(rows, bot.session, id), Boolean(list?.workByBot), work.length, seq]);
        const old = seen.get(id);
        const at = old?.key === key ? old.changed : Math.max(Date.now(), (old?.changed ?? 0) + 1);
        seen.set(id, { key, changed: at });
        changed[id] = Math.max(changed[id] ?? 0, at);
      }
      return { changed };
    };
    return yield* followed(now, followersOf(workFollowers, listFollowers), signal ?? undefined,
      { folders: [WORK_FOLDER, ACTIVITY_FOLDER, path.dirname(LIST_FILE), path.dirname(PEER_REACH)], recursive: false, coalesce: true, beforeWatch: true });
  }
  // What still runs for each session besides its turn: a list now, and again as it changes.
  if (route === "kuvo/work/background" || route === "kuvo/work/background/events") {
    if (method !== "GET") return yield* json(405, {});
    if (route === "kuvo/work/background") return yield* json(200, { sessions: readBackground() });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(() => ({ sessions: readBackground() }), backgroundFollowers, signal ?? undefined, { folders: [WORK_FOLDER], recursive: false });
  }
  // One session's, each one: the bars over a client's composer (commands left running, subagents
  // at work), each subagent with its title and whether it waits on you, as the session lists say.
  // (Followed, a line now and one as it changes: a command past the counts' limit included.)
  if (match[1] !== undefined) {
    if (method !== "GET") return yield* json(405, {});
    const session = decodeURIComponent(match[1]);
    const now = () => {
      const { commands, subagents } = readSessionBackground(session);
      rememberCommands(session, commands);
      const list = keptList();
      const listed = list?.sessions ?? {};
      const facts = listed[session]?.facts;
      const bot = facts?.bot && botHere(facts.bot)?.session === session ? facts.bot : undefined;
      const activity = activityOf(activityFacts(list), session, bot, true);
      return {
        commands,
        activity,
        subagents: subagents.map(each => {
          const facts = listed[each.id]?.facts;
          return { session: each.id, since: each.since, ...(facts?.title ? { title: facts.title } : {}), ...(facts?.asked !== undefined ? { asking: true } : {}) };
        }),
      };
    };
    if (!match[2]) return yield* json(200, now());
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(now, followersOf(backgroundFollowers, listFollowers), signal ?? undefined, { folders: [WORK_FOLDER, ACTIVITY_FOLDER, path.dirname(LIST_FILE), path.dirname(PEER_REACH)], recursive: false, coalesce: true, beforeWatch: true });
  }
  // What bots just did to their sessions: a list now, and again with each mark.
  if (route === "kuvo/work/oversight" || route === "kuvo/work/oversight/events") {
    if (method !== "GET") return yield* json(405, {});
    if (route === "kuvo/work/oversight") return yield* json(200, { marks: readOversight() });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(() => ({ marks: readOversight() }), oversightFollowers, signal ?? undefined, { folders: [WORK_FOLDER], recursive: false });
  }
  if (route === "kuvo/work") {
    if (method !== "GET") return yield* json(405, {});
    const query = new URLSearchParams(request.path.split("?")[1] ?? "");
    const bot = query.get("bot") ?? "";
    if (!bot) return yield* json(400, { message: "Name the bot: kuvo/work?bot=<id>." });
    const here = botHere(bot);
    if (!here) return yield* json(404, { message: "There's no such bot here." });
    if (!fromAPhone(request)) return yield* json(200, botWorkSummary(bot, here.session));
    try {
      readEngine();
    } catch {
      return yield* json(503, { message: "The engine isn't running." });
    }
    return yield* json(200, await botWork(bot, context));
  }
  // Every session of a bot's work stopped.
  if (method !== "POST") return yield* json(405, {});
  const bot = decodeURIComponent(match[3] ?? "");
  if (!botHere(bot)) return yield* json(404, { message: "There's no such bot here." });
  const targets = targetsOf({ readEngine, bridge });
  if (!targets.length) return yield* json(503, { message: "The engine isn't running." });
  const stopped = await stopBotWork(bot, targets, signal ?? undefined);
  return yield* json(200, { stopped });
}

// What a command a session left running printed, as a client's Output shows it: the end of it now,
// and, followed, again about once a second while it runs, its last line once it ended.
export async function* commandOutputRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const session = decodeURIComponent(match[1]!);
  const id = decodeURIComponent(match[2]!);
  const command = commandOf(session, id) ?? await bridgeCommand(session, id, bridge(), signal ?? undefined);
  // (Where it runs found again each time: its engine started again on another port.)
  const places = () => {
    let engine: Target | null = null;
    try { engine = readEngine(); } catch {}
    return { engine, bridge: bridge() };
  };
  const read = outputReader(command, places, signal ?? undefined);
  if (!match[3]) return yield* json(200, await read());
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  return yield* followOutput(read, signal ?? undefined);
}

// A conversation's subagents, at work and done, as a side panel lists them: now, and as they change.
export async function* subagentsRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const session = decodeURIComponent(match[1]!);
  const read = () => subagentsOf(session);
  if (!match[2]) return yield* json(200, read());
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  return yield* followed(read, followersOf(backgroundFollowers, listFollowers), signal,
    { folders: [WORK_FOLDER, path.dirname(LIST_FILE)], recursive: false, coalesce: true, beforeWatch: true });
}

// What's asked of you, your answers, and where new projects go.
export async function* workAsksRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  const method = request.method.toUpperCase();
  const route = request.path.split("?")[0];
  // What can't answer now, as bots are told: for a client's model picker to say so too.
  if (route === "kuvo/work/unanswering") {
    if (method !== "GET") return yield* json(405, {});
    const held = unansweringNow();
    // (Which of an account's models a hold on its paid or free ones covers: as its engine prices them.)
    const listed = held.some(each => each.paid || each.free) ? await Promise.resolve().then(() => call(readEngine(), "GET", "/api/model", undefined, signal)).catch(() => null) : null;
    return yield* json(200, unansweringList(held, Array.isArray(listed) ? listed : null));
  }
  if (route === "kuvo/work/settings") {
    // (Keep This Mac Awake only where the box keeps its Mac awake itself: the process that does,
    // `serve`, holds or lets go by its next look, ten seconds at most.)
    const awake = () => keepsMacAwakeItself() ? { now: awakeForWork() } : undefined;
    if (method === "GET") return yield* json(200, workSettings(undefined, awake()));
    if (method !== "PATCH") return yield* json(405, {});
    const given = parsed(request.body);
    const keepsAwake = keepsMacAwakeItself() && isObject(given) && "keepsAwake" in given;
    if (!isObject(given) || (!("projectsFolder" in given) && !keepsAwake)) {
      return yield* json(400, { message: keepsMacAwakeItself() ? "Send {projectsFolder}: a full path, or null; or {keepsAwake}: true or false." : "Send {projectsFolder}: a full path, or null." });
    }
    // Both checked before either is kept.
    if (keepsAwake && typeof given.keepsAwake !== "boolean") return yield* json(400, { message: KEEPS_AWAKE_REFUSED });
    if ("projectsFolder" in given && given.projectsFolder !== null) {
      const checked = folderGiven(given.projectsFolder);
      if ("refused" in checked) return yield* json(400, { message: checked.refused });
    }
    if ("was" in given) {
      const changed = patchWorkSettings({ ...(keepsAwake ? { keepsAwake: given.keepsAwake } : {}), ...("projectsFolder" in given ? { projectsFolder: given.projectsFolder } : {}), was: given.was });
      return yield* changed.status === 200 ? json(200, workSettings(undefined, awake())) : json(changed.status, { message: changed.message });
    }
    if (keepsAwake) setKeepsAwake(given.keepsAwake);
    if ("projectsFolder" in given) {
      const set = setProjectsFolder(given.projectsFolder);
      if ("refused" in set) return yield* json(400, { message: set.refused });
    }
    return yield* json(200, workSettings(undefined, awake()));
  }
  if (route === "kuvo/work/asks" || match[1] === "events") {
    if (method !== "GET") return yield* json(405, {});
    if (route === "kuvo/work/asks") return yield* json(200, { asks: workAsks() });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(() => ({ asks: workAsks() }), askFollowers, signal ?? undefined, { folders: [WORK_FOLDER], recursive: false });
  }
  if (method !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  const outcome = await answerWork(decodeURIComponent(match[1] ?? ""), isObject(given) ? given : {}, () => targetsOf({ readEngine, bridge }));
  if ("refused" in outcome) return yield* json(outcome.refused.status, { message: outcome.refused.message });
  return yield* json(200, { answer: outcome.answered.answer, text: outcome.answered.text, isError: outcome.answered.isError });
}

// A project made by a client, as New Project would, in the projects folder (`{name, git?, clone?}`).
export async function* newProjectRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  if (!isObject(given)) return yield* json(400, { message: "Send {name}, and git or clone." });
  const outcome = await makeProject(given, null);
  if ("refused" in outcome) {
    const { status, ...refusal } = outcome.refused;
    return yield* json(status, refusal);
  }
  return yield* json(201, { project: outcome.made.project, text: madeWords(outcome.made, HERE) });
}
