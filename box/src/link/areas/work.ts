import { askingNow } from "../work/tree";
// A bot's work, as every client shows it (PROTOCOL.md, "Work"): `GET kuvo/work?bot=<id>`, its
// sessions and where each stands, the asks it answered for them, what it started today and the
// limits you set; `GET kuvo/work/events`, when each bot's work last changed, a line now and one for
// each change; what a command a session left running printed (`kuvo/work/background/<session>/
// commands/<id>/output`, and its events); a conversation's subagents, at work and done
// (`kuvo/work/subagents/<session>`, and its events); `POST kuvo/bots/<id>/stop-all`, every session of its work stopped, and the bot's own
// turn with what it was still to hear of them. What a bot asks
// you before it does it (`kuvo/work/asks`, its events, and your answer, `POST kuvo/work/asks/<id>`),
// where new projects go (`kuvo/work/settings`), a project made (`POST kuvo/projects/new`), and a
// bot's goal (`kuvo/work/goals`, its events, a goal asked for, `POST kuvo/work/goals/ask`, and your
// answer to one, `POST kuvo/work/goals/<id>`), and the icon of an app its steps are done in
// (`kuvo/work/apps/icon`).

import fs from "node:fs";
import { isObject, parsed } from "../../shared/json";
import { followed, followedLive, followersOf } from "../follow";
import { botHere } from "../bots/here";
import { listBots } from "../bots/list";
import { targetsOf } from "../letters/tools";
import { answersOn } from "../work/answers";
import { noteWorkChanged, readWorkChanges, WORK_FOLDER, workFollowers } from "../work/changes";
import { stopBotWork } from "../work/stop";
import { oversightFollowers, readOversight } from "../work/oversight";
import { botLimits } from "../work/limits";
import { answerWork, askFollowers, workAsks } from "../work/asks";
import { madeWords, makeProject } from "../work/projects";
import { startOfDay } from "../work/clock";
import { patchWorkSettings, folderGiven, KEEPS_AWAKE_REFUSED, setKeepsAwake, setProjectsFolder, workSettings } from "../work/settings";
import { awakeForWork, keepsMacAwakeItself } from "../work/awake";
import { unansweringList, unansweringNow } from "../work/unanswering";
import { answerGoal, askGoal, goalFollowers, goalPlans } from "../work/goals";
import { call } from "../engine/client";
import { appIcon, isAppID } from "../work/apps";
import { readProjects } from "./projects";
import { HERE } from "../home";
import { backgroundFollowers, readBackground, readSessionBackground, WORK_BACKGROUND } from "../work/background";
import { commandOf, followOutput, outputReader } from "../work/command-output";
import { subagentsOf, subagentsSaid } from "../work/subagents";
import type { Target } from "../../shared/records";
import { keptList, LIST_FILE, listFollowers, listNow } from "../lists/sessions";
import { botOfConversation, describeWork, ofBot, workSessions } from "../work/tree";
import { activityFacts, activityOf } from "../work/activity";
import { ACTIVITY_FOLDER } from "../work/activity-peers";
import { workAccounts } from "../work/accounts";
import type { BotWork, WorkAnswer } from "../../shared/routes/work";
import type { AnswerPiece, RouteContext } from "../server/context";

export const WORK_ROUTE = /^kuvo\/(?:work(?:\/events|\/oversight(?:\/events)?|\/background(?:\/events|\/([^/]+)(\/events)?)?)?|bots\/([^/]+)\/stop-all)$/;
export const COMMAND_OUTPUT_ROUTE = /^kuvo\/work\/background\/([^/]+)\/commands\/([^/]+)\/output(\/events)?$/;
export const SUBAGENTS_ROUTE = /^kuvo\/work\/subagents\/([^/]+)(\/events)?$/;
export const WORK_ASKS_ROUTE = /^kuvo\/work\/(?:settings|unanswering|asks(?:\/([^/]+))?)$/;
export const GOALS_ROUTE = /^kuvo\/work\/goals(?:\/([^/]+))?$/;
export const APP_ICON_ROUTE = "kuvo/work/apps/icon";

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
      const rows = activityFacts();
      for (const bot of listBots()) {
        const key = JSON.stringify(activityOf(rows, bot.session, bot.id));
        const old = seen.get(bot.id);
        const at = old?.key === key ? old.changed : Math.max(Date.now(), (old?.changed ?? 0) + 1);
        seen.set(bot.id, { key, changed: at });
        changed[bot.id] = Math.max(changed[bot.id] ?? 0, at);
      }
      return { changed };
    };
    return yield* followed(now, followersOf(workFollowers, listFollowers), signal ?? undefined,
      { folders: [WORK_FOLDER, ACTIVITY_FOLDER], recursive: false, coalesce: true });
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
    if (!keptList()) await listNow(context, signal ?? undefined);
    const now = () => {
      const { commands, subagents } = readSessionBackground(session);
      const list = keptList();
      const listed = list?.sessions ?? {};
      const activity = activityOf(activityFacts(list), session, botOfConversation(session)?.bot);
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
    return yield* followed(now, followersOf(backgroundFollowers, listFollowers), signal ?? undefined, { folders: [WORK_FOLDER, ACTIVITY_FOLDER], recursive: false, coalesce: true });
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
    const bot = new URLSearchParams(request.path.split("?")[1] ?? "").get("bot") ?? "";
    if (!bot) return yield* json(400, { message: "Name the bot: kuvo/work?bot=<id>." });
    if (!botHere(bot)) return yield* json(404, { message: "There's no such bot here." });
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
  const command = commandOf(decodeURIComponent(match[1]!), decodeURIComponent(match[2]!));
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
  const { request, readEngine, bridge, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const session = decodeURIComponent(match[1]!);
  // (Their titles and asks, as the session lists say them.)
  if (!keptList()) await listNow(context, signal ?? undefined).catch(() => null);
  // (Where they run found again each time: its engine started again on another port.)
  const places = () => {
    let engine: Target | null = null;
    try { engine = readEngine(); } catch {}
    return { engine, bridge: bridge() };
  };
  if (!match[2]) {
    const said = await subagentsOf(session, places(), signal ?? undefined);
    return yield* said ? json(200, said) : json(503, { message: "Its engine isn't answering now. Try again in a moment." });
  }
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  // Read again as what the box knows of them changes here (one starting or ending, a title, an
  // ask), as another process changes it, and every quarter minute; a place not answering leaves
  // what was said last.
  const listen = (told: () => void) => {
    let was = subagentsSaid(session);
    const changed = () => {
      const now = subagentsSaid(session);
      if (now === was) return;
      was = now;
      told();
    };
    backgroundFollowers.add(changed);
    listFollowers.add(changed);
    return () => { backgroundFollowers.delete(changed); listFollowers.delete(changed); };
  };
  const mtime = (file: string) => { try { return String(fs.statSync(file).mtimeMs); } catch { return ""; } };
  const look = () => [WORK_BACKGROUND, LIST_FILE].map(mtime).join(" ");
  return yield* followedLive(async () => (await subagentsOf(session, places(), signal ?? undefined)) ?? undefined, listen, signal ?? undefined, { look });
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

// The goals here (each bot's latest), their events, a goal asked for in a bot's chat (/goal), and
// your answer to one: start (with your switches and limits), decline or stop.
export async function* goalsRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, readEngine, bridge, signal, json } = context;
  const method = request.method.toUpperCase();
  if (!match[1] || match[1] === "events") {
    if (method !== "GET") return yield* json(405, {});
    if (!match[1]) return yield* json(200, { goals: goalPlans() });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followed(() => ({ goals: goalPlans() }), goalFollowers, signal ?? undefined, { folders: [WORK_FOLDER], recursive: false });
  }
  if (method !== "POST") return yield* json(405, {});
  const given = parsed(request.body);
  const targets = () => targetsOf({ readEngine, bridge });
  if (match[1] === "ask") {
    if (!isObject(given) || typeof given.session !== "string" || !botOfConversation(given.session)) return yield* json(400, { message: "Send {session, goal}: a bot's conversation, and the goal in your words." });
    const outcome = await askGoal(given.session, typeof given.goal === "string" ? given.goal : "", targets);
    if ("refused" in outcome) return yield* json(outcome.refused.status, { message: outcome.refused.message });
    return yield* json(200, outcome);
  }
  const outcome = await answerGoal(decodeURIComponent(match[1]), isObject(given) ? given : {}, targets);
  if ("refused" in outcome) {
    const { status, ...refusal } = outcome.refused;
    return yield* json(status, refusal);
  }
  return yield* json(200, outcome);
}

// `kuvo/work/apps/icon?app=<id>`: the icon of an app on this computer that a goal's step or what
// waits names, by the id the box gave it: a PNG; 404 for one it has none of.
export async function* appIconRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  const id = new URLSearchParams(request.path.split("?")[1] ?? "").get("app");
  if (!isAppID(id)) return yield* json(400, { message: "Name the app by its id: kuvo/work/apps/icon?app=<id>." });
  const file = appIcon(id);
  let bytes: Buffer | null = null;
  try { bytes = file ? fs.readFileSync(file) : null; } catch {}
  if (!bytes) return yield* json(404, { message: "There's no icon for that app here." });
  yield { head: { status: 200, headers: { "content-type": "image/png", "content-length": String(bytes.length) } } };
  yield { body: bytes };
}
