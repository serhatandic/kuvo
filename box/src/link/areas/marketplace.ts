// Marketplace clients read box-authored cards and submit explicit install, sign-in and Add actions.
import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { marketplaceScope } from "../../shared/marketplace";
import { listBots } from "../bots/list";
import { readPeerAgents } from "../peers/directory";
import type { Target } from "../../shared/records";
import { acquireEngine } from "../engine/lifetime";
import { workQueueClient } from "./work-queue";
import { answerSkills, githubSkill, isSkillFolder, localSkill, SKILLS_FOLDER, skillAbout, skillName, SkillFailure, type SkillPackage } from "./skills";
import { skillDocument, skillsHere, skillsSection } from "./settings-skills";
import { catalog, endpoint, itemID, refreshCatalog, sha256, type Agent, type CatalogSkill } from "../marketplace/catalog";
import { addBot, botOptions, projectBotScopes } from "../marketplace/bots";
import { cancelSignIn, cleanRemovedConnections, connectionState, connectToken, disconnect, finishSignIn, projectApps, signInStatus, startSignIn } from "../marketplace/connections";
import { installations, installedApp, installedSkill, no, projectSkills, saveInstallations, skillRevision,
  type Installations, type InstalledApp, type InstalledSkill } from "../marketplace/installations";
import { ownKey, peersOf, type Peer } from "../peers/known";
import { computerName } from "../peers/computer-name";
import { awaySince } from "../peers/reach";
import { readLink } from "../state";
import { primaryOf } from "../work-queue/numbers";
import { forwardAct, takesForwarded, type Act, type Acted } from "../work-queue/forward";
import { VaultRefusal } from "../vault/store";
import type { AnswerPiece, RouteContext } from "../server/context";
import type { SettingItem } from "../../shared/routes/settings-pages";

const ROOT = "kuvo/marketplace";
const success = (value: unknown): Acted => ({ status: 200, value });
const missing = () => no(404, "There’s no Marketplace item here.");
const methodError = () => no(405, "That action isn’t available.");
const detailsPath = /^\/(apps|agents|skills)\/([^/]+)(?:\/([a-z-]+))?$/;
type PeerCommand = Pick<Act, "method" | "item" | "body">;
const offeredApps = () => catalog().apps.filter(app => app.auth !== "broker");
const offeredAgents = () => catalog().agents.filter(agent => agent.apps.every(id => offeredApps().some(app => app.id === id)));

function offerSkill(item: CatalogSkill): InstalledSkill {
  return { name: item.id, origin: "catalog", version: item.version, files: item.files };
}

function cardSkill(skill: InstalledSkill) {
  const markdown = Buffer.from(skill.files.find(file => file.path === "SKILL.md")!.data, "base64").toString("utf8");
  return { ...skillAbout(markdown), id: skill.name, origin: skill.origin, version: skill.version,
    source: skill.source, revision: skillRevision(skill), installed: true };
}

function appCard(app: InstalledApp, installed: boolean, details = false) {
  const connection = installed ? connectionState(app) : undefined;
  const logo = app.origin === "catalog" && offeredApps().some(each => each.id === app.id)
    ? ROOT + "/apps/" + app.id + "/logo" : undefined;
  // What a row says in place of its button once it's set up, in words a client shows as they are.
  const status = !installed ? undefined : connection === "ready" ? { words: app.auth === "open" ? "Added" : "Connected", tone: "good" }
    : connection === "locked" ? { words: "Vault locked", tone: "warning" } : undefined;
  return { id: app.id, title: app.title, category: app.category, summary: app.summary, origin: app.origin,
    logo, installed, connection, ...(status ? { status } : {}), ...(details ? {
      url: app.url, auth: app.auth, computerSignIn: app.auth !== "open",
      ...(app.auth === "token" ? { form: { title: `Connect ${app.title}`, submit: "Connect",
        fields: [{ id: "token", title: "Personal access token", kind: "password" }],
        ...(app.origin === "catalog" && app.id === "github" ? { help: "https://github.com/settings/personal-access-tokens/new" } : {}),
      } } : {}),
    } : {}),
    actions: installed ? [
      ...(app.auth === "open" ? [] : [{ id: "connect", title: connection === "ready" ? "Reconnect" : "Connect", enabled: connection !== "locked" },
        ...(connection === "ready" ? [{ id: "disconnect", title: "Disconnect on This Computer" }] : [])]),
      { id: "remove", title: "Remove from All Computers" },
    ] : [{ id: "add", title: app.auth === "open" ? "Add" : "Connect" }] };
}

function catalogApp(id: string): InstalledApp | undefined {
  const entry = offeredApps().find(app => app.id === id);
  return entry && { id, title: entry.title, category: entry.category, summary: entry.summary, url: entry.url, auth: entry.auth, origin: "catalog" };
}

function agentBots(): Map<string, { id: string; name: string; computer: string }[]> {
  const link = readLink();
  const told = readPeerAgents();
  const records = new Map<string, { id: string; name: string; computer: string; agent: string }>();
  for (const peer of peersOf(link)) {
    for (const bot of told[peer.key]?.list ?? []) {
      if (bot.kind === "bot" && bot.id && bot.marketplaceAgent)
        records.set(bot.id, { id: bot.id, name: bot.name, computer: peer.key, agent: bot.marketplaceAgent });
    }
  }
  const here = link && ownKey(link) || "here";
  for (const bot of listBots()) {
    const agent = bot.folder && marketplaceScope(path.join(bot.folder, "workspace"))?.agent;
    if (agent) records.set(bot.id, { id: bot.id, name: bot.name, computer: here, agent });
  }
  const byAgent = new Map<string, { id: string; name: string; computer: string }[]>();
  for (const { agent, ...bot } of records.values()) {
    const existing = byAgent.get(agent);
    if (existing) existing.push(bot);
    else byAgent.set(agent, [bot]);
  }
  return byAgent;
}

function computers(context: RouteContext) {
  const link = readLink();
  return [
    { key: link && ownKey(link) || "here", name: context.info.name, here: true, available: true },
    ...peersOf(link).map(peer => ({ key: peer.key, name: computerName(peer), available: !awaySince(peer.key) && takesForwarded(peer.key, "marketplace") })),
  ];
}

async function catalogView(context: RouteContext, state: Installations, query: URLSearchParams): Promise<Acted> {
  const appMap = new Map(state.apps.map(app => [app.id, app]));
  for (const app of offeredApps()) if (!appMap.has(app.id)) appMap.set(app.id, catalogApp(app.id)!);
  const apps = [...appMap.values()].map(app => appCard(app, state.apps.some(each => each.id === app.id)));
  const catalogNames = new Set(catalog().skills.map(skill => skill.id));
  const installed = new Map(state.skills.map(skill => [skill.name, skill]));
  const skills = catalog().skills.map(({ files, ...skill }) => ({
    ...skill, origin: "catalog", installed: installed.has(skill.id),
    ...(installed.has(skill.id) ? { installedVersion: installed.get(skill.id)!.version } : {}),
  }));
  const managed = state.skills.filter(skill => skill.origin !== "catalog" || !catalogNames.has(skill.name)).map(skill => {
    const shown = cardSkill(skill);
    return { ...shown, title: shown.name ?? shown.id, summary: shown.description ?? "", category: "Yours" };
  });
  const local = await localSkills();
  const addedBots = agentBots();
  const here = await skillsHere(context);
  // (Whether each installed skill is on here; a skill only in the catalog has no switch.)
  const withHere = <T extends { id: string; installed: boolean }>(items: T[]) => here.problem !== undefined ? items
    : items.map(item => item.installed ? { ...item, enabled: here.enabled(item.id), ...(here.canSwitch ? {} : { switchWhy: "Permissions are set as something other than a list. Change them in the settings file." }) } : item);
  return success({
    title: "Marketplace", computers: computers(context), searchPlaceholder: "Search, or paste a GitHub link",
    areas: [
      area("apps", "Apps", apps, query, [{ id: "custom", title: "Add Custom", form: {
        title: "Add Custom App", submit: "Add",
        fields: [{ id: "title", title: "Name" }, { id: "url", title: "MCP server address", placeholder: "https://example.com/mcp" },
          { id: "auth", title: "Sign-in", choices: [{ id: "open", title: "No sign-in" }, { id: "token", title: "Personal access token" }, { id: "oauth", title: "Sign in with the service" }] }],
      } }]),
      area("agents", "Agents", offeredAgents().map(({ instructions, ...agent }) => {
        const botCount = addedBots.get(agent.id)?.length ?? 0;
        const needs = agent.apps.map(id => catalog().apps.find(app => app.id === id)?.title ?? id);
        const detail = [...needs, plural(agent.skills.length, "skill"), ...(botCount ? [`${plural(botCount, "bot")} added`] : [])].join(" · ");
        return { ...agent, botCount, detail };
      }), query),
      { ...area("skills", "Skills", withHere([...skills, ...managed, ...local.filter(skill => !installed.has(skill.id)),
        ...(here.problem !== undefined ? [] : here.found.filter(skill => !installed.has(skill.id) && !catalogNames.has(skill.id) && !local.some(each => each.id === skill.id))
          .map(skill => ({ ...skill, category: "Yours", origin: "found", installed: true })))]), query,
        [{ id: "preview", title: "Preview Skill" }, { id: "create", title: "New Skill" }]),
        // This computer's own: whether each skill is on here, Recently Deleted and Other Places.
        here: here.problem !== undefined ? { problem: here.problem } : {
          computer: context.info.name, switchTitle: `Use on ${context.info.name}`,
          deleted: { title: "Recently Deleted", note: SKILL_DELETION, items: here.deleted, action: { id: "restore", title: "Restore" } },
          places: { title: "Other Places", note: "A folder, or the address of a skill catalog with an index.json, where agents also find skills.",
            items: here.places.map((place, index) => ({ id: String(index), title: place })),
            add: { id: "add", title: "Add Place", form: { title: "Add a Place", submit: "Add", fields: [{ id: "place", title: "A folder, or the address of a skill catalog" }] } },
            remove: { id: "remove", title: "Remove" } },
        } },
    ],
  });
}

const SKILL_DELETION = "Deleted skills are kept for 30 days, then removed for good.";
const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;

function area<T extends { title: string; summary: string; category: string }>(id: string, title: string, items: T[], query: URLSearchParams, actions: unknown[] = []) {
  const categories = [...new Set(items.map(item => item.category))].map(title => ({ title, count: items.filter(item => item.category === title).length }));
  const search = (query.get("q") ?? "").trim().toLowerCase();
  const category = query.get("category");
  const origin = query.get("origin");
  const results = items.filter(item => (!category || item.category === category)
    && (!origin || id === "agents" || ("origin" in item && (origin === "kuvo") === (item.origin === "catalog")))
    && (!search || (item.title + "\n" + item.summary).toLowerCase().includes(search)));
  return { id, title, count: items.length, categories, items: results, actions,
    ...(id === "agents" ? {} : { filters: [{ id: "kuvo", title: "Kuvo" }, { id: "yours", title: "Yours" }] }),
    empty: search ? "No items match your search." : "There’s nothing here yet." };
}

async function localSkills() {
  const listed = await answerSkills("GET", "kuvo/skills", undefined, null);
  const value = listed.value;
  if (!("folders" in value)) return [];
  const rows = [];
  for (const id of value.folders.filter(isSkillFolder)) {
    if (fs.existsSync(path.join(SKILLS_FOLDER, id, ".kuvo-marketplace.json"))) continue;
    const got = await answerSkills("GET", `kuvo/skills/${id}`, undefined, null);
    if (!("text" in got.value)) continue;
    const about = skillAbout(got.value.text);
    rows.push({ id, title: about.name ?? id, summary: about.description ?? "", category: "Yours", origin: "local",
      installed: true, source: value.github[id] });
  }
  return rows;
}

interface Preview { skill: InstalledSkill; expires: number }
const previews = new Map<string, Preview>();
const previewID = /^[0-9a-f]{64}$/;
function stagePreview(skill: InstalledSkill) {
  const now = Date.now();
  for (const [key, value] of previews) if (value.expires <= now) previews.delete(key);
  if (previews.size >= 12) previews.delete(previews.keys().next().value!);
  const id = skillRevision(skill);
  previews.set(id, { skill, expires: now + 15 * 60_000 });
  return success({ preview: id, skill: { ...cardSkill(skill), installed: false }, installed: false, files: skill.files, action: { id: "install", title: "Install Skill" } });
}
const findPreview = (id: unknown) => {
  if (typeof id !== "string" || !previewID.test(id)) return undefined;
  const found = previews.get(id);
  return found && found.expires > Date.now() ? found.skill : undefined;
};
const candidate = (skill: SkillPackage, origin: "github" | "folder"): InstalledSkill => ({ ...skill, origin });
function writtenSkill(body: Record<string, unknown>, name?: string, existing?: string): InstalledSkill | undefined {
  if (typeof body.description !== "string" || !body.description.trim() || typeof body.instructions !== "string") return;
  const id = typeof body.name === "string" ? skillName(body.name) : "";
  const resolved = name ?? id;
  if (!itemID.test(resolved)) return;
  const bytes = Buffer.from(skillDocument(resolved, body.description, body.instructions, existing));
  if (bytes.length > 64 * 1024) return;
  return { name: resolved, origin: "created", files: [{ path: "SKILL.md", data: bytes.toString("base64"), sha256: sha256(bytes) }] };
}

function filesOnThisBox(id: string): boolean {
  return fs.existsSync(path.join(SKILLS_FOLDER, id)) && !fs.existsSync(path.join(SKILLS_FOLDER, id, ".kuvo-marketplace.json"));
}

// Explicitly allowlisted normalized mutations received by the setup's primary. No credentials are sent.
function sharedMutation(item: string, body: Record<string, unknown>): Acted {
  return saveInstallations(state => {
    if (item === "save-app" && installedApp(body.app)) {
      const { id, title, summary, category, origin, auth, url } = body.app;
      const before = state.apps.find(app => app.id === id);
      if (before) return before.url === url && before.auth === auth && before.origin === origin
        ? success({ installed: true }) : no(409, "An app already uses that name.");
      if (state.apps.length >= 60) return no(413, "Remove an app before adding another.");
      return { ...state, apps: [...state.apps, { id, title, summary, category, origin, auth, url }] };
    }
    if (item === "save-skill" && installedSkill(body.skill)) {
      const { name, files, origin, source, version } = body.skill;
      const old = state.skills.find(skill => skill.name === name);
      const next = { name, files: files.map(({ path, sha256, data }) => ({ path, sha256, data })), origin,
        ...(source ? { source: { repository: source.repository, path: source.path, commit: source.commit, ...(source.ref ? { ref: source.ref } : {}) } } : {}),
        ...(version ? { version } : {}) };
      if (old && skillRevision(old) === skillRevision(next)) return success({ installed: true });
      if ((old ? skillRevision(old) : undefined) !== body.was) return no(409, "The installed skill changed. Read it again first.");
      if (filesOnThisBox(name)) return no(409, "A skill with that name already exists on the main computer.");
      return { ...state, skills: [...state.skills.filter(skill => skill.name !== name), next] };
    }
    if (item === "agent-skills" && typeof body.id === "string") {
      const agent = offeredAgents().find(each => each.id === body.id && each.version === body.version);
      if (!agent) return no(409, "This agent changed. Read it again before adding it.");
      const additions = agent.skills.filter(id => !state.skills.some(each => each.name === id)).map(id => offerSkill(catalog().skills.find(skill => skill.id === id)!));
      if (additions.some(skill => filesOnThisBox(skill.name))) return no(409, "A skill required by this agent already exists as a local skill on the main computer.");
      return additions.length ? { ...state, skills: [...state.skills, ...additions] } : success({ installed: true });
    }
    if (typeof body.id !== "string" || !itemID.test(body.id)) return missing();
    if (item === "remove-app") return state.apps.some(app => app.id === body.id)
      ? { ...state, apps: state.apps.filter(app => app.id !== body.id) } : success({ installed: false });
    if (item === "remove-skill") {
      const old = state.skills.find(skill => skill.name === body.id);
      if (!old) return success({ installed: false });
      if (body.was !== skillRevision(old)) return no(409, "The installed skill changed. Read it again first.");
      return { ...state, skills: state.skills.filter(skill => skill.name !== body.id) };
    }
    return missing();
  });
}

function reconcile(state = installations()): string[] {
  return [...projectSkills(state), ...projectApps(state.apps), ...projectBotScopes(state)];
}
function applied(result: Acted): Acted {
  if (result.status !== 200) return result;
  const value = isObject(result.value) ? result.value : {};
  const state = isObject(value.installations) ? value.installations as unknown as Installations : undefined;
  return { status: 200, value: { ...value, ...(state ? { installations: undefined } : {}), problems: reconcile(state) } };
}

async function send(peer: Peer, command: PeerCommand, context: RouteContext): Promise<Acted> {
  if (!takesForwarded(peer.key, "marketplace")) return no(409, `Update Kuvo Code on ${computerName(peer)} to use Marketplace there.`);
  const result = await forwardAct({ ...peer, name: computerName(peer) }, "Marketplace", { ...command, area: "marketplace" }, context.signal);
  return "refused" in result ? no(result.status, result.refused) : result;
}
async function save(item: string, body: Record<string, unknown>, context: RouteContext): Promise<Acted> {
  const primary = primaryOf();
  if ("peer" in primary) {
    const result = await send(primary.peer, { method: "POST", item, body }, context);
    // The accepted replica may still be in flight. Only project the signed value once it arrives.
    return result;
  }
  if (!("self" in primary)) return no(503, "Marketplace changes are waiting for the main computer.");
  return applied(sharedMutation(item, body));
}

function replyError(error: unknown): Acted {
  if (error instanceof SkillFailure || error instanceof VaultRefusal) return no(error.status, error.message);
  return no(502, "Marketplace couldn’t complete that action. Try again.");
}

type Targets = { readEngine: () => Target; bridge: () => Target | null };
async function agentAction(agent: Agent, action: string, body: Record<string, unknown>, targets: Targets): Promise<Acted> {
  if (action === "options") {
    let engine: Target | null = null;
    const release = await acquireEngine();
    try {
      try { engine = targets.readEngine(); } catch {}
      return await botOptions(agent, engine, targets.bridge());
    } finally { release(); }
  }
  const problems = reconcile();
  if (problems.length) return no(409, problems[0]!);
  return addBot(agent, body, targets.readEngine, targets.bridge);
}

/** Work Queue peer mail: install only on the primary or create an explicitly addressed local bot. */
export async function marketplaceForPeer(act: Act, targets: Targets): Promise<Acted> {
  try {
    if (act.method === "POST" && /^(?:save-app|save-skill|remove-app|remove-skill|agent-skills)$/.test(act.item)) {
      if (readLink()?.setup?.role !== "primary") return no(409, "Marketplace changes are waiting for the main computer.");
      return applied(sharedMutation(act.item, act.body ?? {}));
    }
    const match = /^(options|add-agent)\/([a-z0-9-]+)$/.exec(act.item);
    if (!match || (match[1] === "options" ? act.method !== "GET" : act.method !== "POST")) return missing();
    const agent = offeredAgents().find(each => each.id === match[2]);
    return agent ? agentAction(agent, match[1]!, act.body ?? {}, targets) : missing();
  } catch (error) { return replyError(error); }
}

async function appAction(context: RouteContext, id: string, action: string | undefined, method: string,
  body: Record<string, unknown>, app: InstalledApp | undefined): Promise<Acted> {
  const listed = app ?? catalogApp(id);
  if (!listed) return missing();
  if (!action && method === "GET") return success(appCard(listed, !!app, true));
  if (!action && method === "DELETE") {
    const result = await save("remove-app", { id }, context);
    if (result.status === 200) void cleanRemovedConnections().catch(() => {});
    return result;
  }
  if (action === "add" && method === "POST") return save("save-app", { app: listed }, context);
  if (!app) return no(409, "Add the app first. Wait for the setup to deliver the change if you added it on another computer.");
  if (action === "connect" && method === "POST") {
    if (app.auth === "oauth") return { status: 202, value: await startSignIn(app) };
    if (app.auth === "token") await connectToken(app, body.token);
    return success({ connection: connectionState(app), problems: projectApps() });
  }
  if (action === "disconnect" && method === "POST") {
    await disconnect(app);
    return success({ connection: connectionState(app), problems: projectApps() });
  }
  if (action === "status" && method === "GET") {
    return success(signInStatus(app, new URLSearchParams(context.request.path.split("?")[1]).get("attempt")));
  }
  if (action === "cancel" && method === "POST") {
    cancelSignIn(app, body.attempt);
    return success({ state: "cancelled" });
  }
  if (action === "finish" && method === "POST") {
    await finishSignIn(app, body.attempt, body.address);
    return success({ connection: connectionState(app), problems: projectApps() });
  }
  return methodError();
}

async function skillAction(context: RouteContext, id: string, action: string | undefined, verb: string,
  body: Record<string, unknown>, skill: InstalledSkill | undefined): Promise<Acted> {
  const listed = catalog().skills.find(each => each.id === id);
  if (action === "update-preview" && verb === "POST" && skill?.source) return stagePreview(candidate(await githubSkill({
    ...skill.source, ref: typeof body.ref === "string" ? body.ref : skill.source.ref,
  }), "github"));
  if (action === "add" && verb === "POST" && listed) {
    if (body.version !== listed.version || body.revision !== skillRevision(offerSkill(listed))) return no(409, "Review the current version before installing it.");
    if (filesOnThisBox(id)) return no(409, "A local skill already uses that name.");
    return save("save-skill", { skill: offerSkill(listed), was: body.was }, context);
  }
  if (action) return missing();
  if (verb === "GET" && (skill || listed)) {
    const info = skill ?? offerSkill(listed!);
    return success({ ...cardSkill(info), title: listed?.title ?? cardSkill(info).name ?? id,
      summary: listed?.summary ?? cardSkill(info).description, category: listed?.category ?? "Yours", files: info.files, installed: !!skill,
      available: listed ? { version: listed.version, revision: skillRevision(offerSkill(listed)), files: listed.files } : undefined,
      actions: skill ? [{ id: "remove", title: "Remove" }, ...(skill.source ? [{ id: "update-preview", title: "Check for Update" }] : []),
        ...(!skill.source && skill.origin !== "catalog" ? [{ id: "edit", title: "Edit" }] : [])] : [{ id: "add", title: "Install Skill" }] });
  }
  if (skill && verb === "DELETE") return save("remove-skill", { id, was: body.was }, context);
  if (skill && verb === "PATCH") {
    if (skill.source || skill.origin === "catalog") return no(409, "Edit a copy of this skill or install an updated version.");
    const markdown = skill.files.find(file => file.path === "SKILL.md")!;
    const edit = writtenSkill(body, id, Buffer.from(markdown.data, "base64").toString("utf8"));
    if (!edit) return no(400, "Give the skill a description and instructions.");
    return save("save-skill", { skill: { ...skill, files: skill.files.map(file => file.path === "SKILL.md" ? edit.files[0] : file) }, was: body.was }, context);
  }
  if (!skill && filesOnThisBox(id)) {
    const legacy = `kuvo/skills/${encodeURIComponent(id)}`;
    if (verb === "GET" || verb === "DELETE") return answerSkills(verb, legacy, body, null);
    if (verb === "PATCH") {
      const old = await answerSkills("GET", legacy, undefined, null);
      if (!("text" in old.value)) return old;
      if (typeof body.description !== "string" || !body.description.trim() || typeof body.instructions !== "string")
        return no(400, "Give the skill a description and instructions.");
      const text = skillDocument(skillAbout(old.value.text).name ?? id, body.description, body.instructions, old.value.text);
      return answerSkills("PUT", legacy, { was: body.was, text }, null);
    }
  }
  return missing();
}

export async function* marketplaceRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, json } = context;
  if (!workQueueClient(context)) return yield* json(403, { message: "Open this from a paired Kuvo Code client." });
  const verb = request.method.toUpperCase();
  const given = parsed(request.body);
  const body = isObject(given) ? given : {};
  const action = route.slice(ROOT.length);
  const query = new URLSearchParams(request.path.split("?")[1]);
  try {
    if (action === "/refresh" && verb === "POST") {
      let refreshed = false;
      try { refreshed = await refreshCatalog(); } catch {
        return yield* json(200, { refreshed, message: "The catalog couldn’t be refreshed. Showing the last available copy." });
      }
      return yield* json(200, { refreshed });
    }
    const state = installations();
    if (!action && verb === "GET") {
      const view = await catalogView(context, state, query);
      return yield* json(view.status, view.value);
    }
    if (action === "/apps/custom" && verb === "POST") {
      const id = typeof body.id === "string" ? body.id : skillName(String(body.title ?? ""));
      const app = { id, title: body.title, category: "Yours", summary: body.summary ?? "Custom MCP server.",
        auth: body.auth ?? "open", url: body.url, origin: "custom" };
      if (!endpoint(body.url) || !installedApp(app)) return yield* json(400, { message: "Enter a name and an HTTPS MCP address without credentials or query parameters." });
      if (catalog().apps.some(item => item.id === id)) return yield* json(409, { message: "A catalog app already uses that name." });
      const result = await save("save-app", { app }, context);
      return yield* json(result.status, { ...(isObject(result.value) ? result.value : {}), id });
    }
    if (action === "/skills/preview" && verb === "POST") {
      const skill = typeof body.folder === "string" ? candidate(localSkill(body.folder), "folder") : candidate(await githubSkill(body), "github");
      const preview = stagePreview(skill);
      return yield* json(preview.status, preview.value);
    }
    if ((action === "/skills/install" || action === "/skills/create") && verb === "POST") {
      const skill = action.endsWith("/create") ? writtenSkill(body) : findPreview(body.preview);
      if (!skill) return yield* json(400, { message: action.endsWith("/create") ? "Fill in the name, description and instructions." : "Preview the skill again first." });
      if (filesOnThisBox(skill.name)) return yield* json(409, { message: "A local skill already uses that name." });
      const result = await save("save-skill", { skill, was: body.was }, context);
      return yield* json(result.status, { ...(isObject(result.value) ? result.value : {}), id: skill.name });
    }
    // This computer's own skills, as Settings › Skills kept them: Recently Deleted and Other Places.
    if ((action === "/skills/restore" && verb === "POST") || (action === "/skills/places" && (verb === "POST" || verb === "DELETE"))) {
      const at = action === "/skills/restore" ? `trash-${String(body.id ?? "")}` : verb === "POST" ? "places" : `place-${Number(body.index)}`;
      if (action === "/skills/places" && verb === "DELETE" && !(Number.isInteger(body.index) && (body.index as number) >= 0)) return yield* json(400, { message: "Choose a place to remove." });
      const refused = await skillsSection.act(context, { id: at } as SettingItem, { id: verb === "DELETE" ? "remove" : action === "/skills/restore" ? "restore" : "add", title: "" }, body);
      return yield* json(refused ? refused.status : 200, refused ? { message: refused.message } : {});
    }
    const match = detailsPath.exec(action);
    if (!match) return yield* json(404, {});
    const [, area, segment, command] = match as unknown as [string, string, string, string | undefined];
    let id: string;
    try { id = decodeURIComponent(segment); } catch { return yield* json(400, { message: "That item name isn’t valid." }); }
    if (area === "skills" ? !isSkillFolder(id) : !itemID.test(id)) return yield* json(404, {});
    if (area === "skills" && command === "enabled" && verb === "POST") {
      if (typeof body.on !== "boolean") return yield* json(400, { message: "Send {\"on\": true} or false." });
      const refused = await skillsSection.change(context, { id: `skill-${id}` } as SettingItem, body.on);
      return yield* json(refused && "status" in refused ? refused.status : 200, refused && "message" in refused ? { message: refused.message } : { enabled: body.on });
    }
    if (area === "apps" && command === "logo" && verb === "GET") {
      const logo = offeredApps().find(app => app.id === id)?.logo;
      if (!logo) return yield* json(404, {});
      yield { head: { status: 200, headers: { "content-type": "image/png", "x-content-type-options": "nosniff" } } };
      yield { body: Buffer.from(logo, "base64") };
      return;
    }
    let result: Acted;
    if (area === "apps") result = await appAction(context, id, command, verb, body, state.apps.find(each => each.id === id));
    else if (area === "skills") result = await skillAction(context, id, command, verb, body, state.skills.find(each => each.name === id));
    else {
      const agent = offeredAgents().find(each => each.id === id);
      if (!agent) result = missing();
      else if (!command && verb === "GET") result = success({ ...agent, bots: agentBots().get(id) ?? [],
        actions: [{ id: "add", title: "Add Agent" }], computers: computers(context),
        labels: { computer: "Runs on", model: "Account and model", routine: "Enable scheduled routines", name: "Name",
          apps: "Needed apps", routines: "Routines", skills: agent.skills.length ? `Includes ${plural(agent.skills.length, "skill")}: ${agent.skills.join(", ")}` : undefined,
          off: "Routines stay off until you turn them on." } });
      else if ((command === "options" && verb === "GET") || (command === "add" && verb === "POST")) {
        const target = command === "options" ? query.get("computer") : body.computer;
        const local = computers(context)[0]!;
        const peer = peersOf(readLink()).find(each => each.key === target);
        if (target !== local.key && !peer) return yield* json(400, { message: "Choose a computer in your setup." });
        if (command === "add") {
          if (body.version !== agent.version) return yield* json(409, { message: "Review the current version before adding this agent." });
          const saved = await save("agent-skills", { id, version: agent.version }, context);
          if (saved.status !== 200) return yield* json(saved.status, saved.value);
        }
        const run = command === "options" ? "options" : "add-agent";
        result = peer ? await send(peer, { method: verb as Act["method"], item: run + "/" + id, body }, context)
          : await agentAction(agent, run, body, context);
        // An app still to connect is connected from here only on this computer; elsewhere, said so.
        if (command === "options" && result.status === 200 && isObject(result.value) && Array.isArray(result.value.missing)) {
          const name = peer ? computerName(peer) : local.name;
          result = success({ ...result.value, missing: result.value.missing.map((app: Record<string, unknown>) => ({ ...app,
            ...(peer ? { connect: false, words: `Connect it on ${name} first` } : { connect: true, words: "Not connected" }) })),
            blocked: result.value.missing.length ? `${plural(result.value.missing.length, "app")} still to connect` : undefined });
        }
      } else result = methodError();
    }
    return yield* json(result.status, result.value);
  } catch (error) {
    const result = replyError(error);
    return yield* json(result.status, result.value);
  }
}

export function keepMarketplaceProjection() {
  const refresh = () => {
    try { reconcile(); } catch { /* The next pass retries a transient disk or config problem. */ }
    void cleanRemovedConnections().catch(() => {});
  };
  refresh();
  const timer = setInterval(refresh, 5_000);
  timer.unref?.();
  return timer;
}
