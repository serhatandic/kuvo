import { writePrivate } from "../files";
import { compactionChoice, compactionChoices, compactionFile, isCompactionChoice } from "../../shared/context";
import { agentsUpdateThemselves, agentUpdatesFile } from "../../shared/agent-updates";
import { queuedRefusal } from "../server/context";
// `kuvo/settings`: this box's settings as it describes them (PROTOCOL.md, "A box's settings"), so
// every client draws each of them by its kind, with the box's words: its sections, each section's
// page, a change to one row (checked against that row's own revision, so two rows never conflict),
// what a row offers to do, and the page again each time it changes.
//
// Each page is made from the box's existing routes, and each change is made through them, as a
// client of them would (`kuvo/approval`, `kuvo/sandbox…`, `kuvo/config`, `kuvo/browser/settings`):
// what they check, keep and do follows, the same for every client.
// Platform: the same on a Mac's box and a machine's. What only opens on the box's own computer (a
// Mac's Software Update) is offered only to a client there (`fromApp`: the app beside its box,
// over its own server), and said plainly elsewhere.

import { usesAcceptedSaves } from "../setup/record";
import { sharedSaveReply, submitShared } from "../setup/shared";
import { setupSeedRefusal } from "../setup/seed";
import fs from "node:fs";
import path from "node:path";
import { listRoute, LIST_ROUTE } from "../lists/sessions";
import { sessionActionRoute, SESSION_ACTION_ROUTE, deletesRoute } from "./session-actions";
import { artifactsRoute } from "./artifacts";
import { homeFolder } from "../home";
import { workAsksRoute, WORK_ASKS_ROUTE } from "./work";
import { PROJECTS_FILE, readProjects } from "./projects";
import { WORK_SETTINGS_FILE } from "../work/settings";
import { WORKTREES_FILE, readWorktrees } from "./worktree-records";
import { worktreesRoute } from "./worktrees";
import { isObject, parsed, sortedJSON } from "../../shared/json";
import { actionsOf, asked, expectedRevision, item, mtime, noActions, refusal, revised, row, rowsOf, said, STALE, staleOrRefusal, unreadPage } from "./settings-kit";
import { listsHere } from "../work/model-rule";
import type { Refused, Section, SettingChange } from "./settings-kit";
import { skillsSection } from "./settings-skills";
import { customAutoSection } from "./settings-custom-auto";
import { mcpSection } from "./settings-mcp";
import { agentsSection } from "./settings-agents";
import { ACTIONS, choiceIn, EFFECTS, engineDefault, isRule } from "./permission-rules";
import { vaultSection } from "./settings-vault";
import { SETTING_GROUP, SETTINGS_ACT, SETTINGS_CHECK, SETTINGS_PAGE_ROUTES } from "../../shared/routes/settings-pages";
import { budgetConfirmation, fileSize, fileSizeUnit } from "../../shared/artifact-budget";
import { fitsShape, list } from "../../shared/shape";
import type { SettingAction, SettingCan, SettingGroup, SettingItem, SettingRow, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import { SEARCH_ENGINES } from "../../shared/routes/browser";
import { approvalRoute } from "./approval";
import { CHANGED_SINCE, CONFIG_FILE, readSettings } from "./config";
import { engineControl } from "./engine";
import { sandboxRoute } from "./sandbox";
import { GIT_SETTINGS, DEFAULT_BRANCH_PREFIX, cleanBranchPrefix } from "./git-words";
import { settingsRoute } from "./settings";
import { BROWSER_DATA_ROUTE, browserDataRoute } from "../browser/routes";
import { browserSettingsFiles, changeNeverSite } from "../browser/settings";
import { ALLOWED_FILE } from "../fence/allowed";
import { fenceState, SOFTWARE_UPDATE } from "../fence/state";
import { followedLive } from "../follow";
import { APPROVAL_FILE } from "../work/approval";
import { APP_FOLDER, IS_MAC, K, RUN } from "../home";
import { isMeasuring, measureProblem, measureSizes, readSizes, SIZES_FILE } from "./storage-sizes";
import { HELPER_NAME, helperAtLogin } from "../helper";
import { MAC_LINK_FILE } from "../mac/link";
import { letDevicesConnect } from "../mac/routes";
import { freshBoxPermissions } from "../permissions";
import type { BoxPermissions } from "../../shared/routes/box";
import { bridgeTarget, readBridgeFile, readEngineFile } from "../engine/client";
import { listComputer } from "../lists/computer";
import { ownerWrite, sendOwnerWrite } from "../ownership/write";
import { movedOf, peersOf } from "../peers/known";
import { peerSnapshots } from "../peers/lists";
import { pendingComputers } from "../peers/pending";
import { readReach } from "../peers/reach";
import type { LinkHost } from "../relay/host";
import { macLink, readLink } from "../state";
import { isUpdateMain } from "../updates/fleet";
import { agentsWorking, updateRoute, updateStatus } from "../updates/update";
import { boxAnswers } from "../mac/keeper-runs";
import { askHolder, heldRead, holdsVault } from "../vault/holder";
import { hasZoneRules, readZoneSetting, setZone, zoneChoice, ZONE_FILE, zoneFilesFor, zoneHere } from "../zone/setting";
import { ownClock } from "../../shared/zone";
import type { SaveReply } from "../../shared/routes/setup-save";
import type { AnswerPiece, RouteContext } from "../server/context";
import { copyStanding } from "../peers/lists";

export const SETTINGS_PAGE_ROUTE = /^kuvo\/settings(?:\/([^/]+)(?:\/(events)|\/([^/]+)\/([^/]+)(?:\/(check))?)?)?$/;

/** This computer's own section, titled for who asks (`computerTitle`). */
const COMPUTER: SettingsSection = {
  id: "computer", title: "This Computer", group: "System", icon: "computer",
  line: "Whether Kuvo runs here, its version and updates, and what may connect.",
  keywords: ["running", "restart", "version", "update", "devices", "phone", "connect", "login items", "kuvo helper", "screen recording", "accessibility", "macos"],
};

const SECTIONS: SettingsSection[] = [
  {
    id: "clock", title: "Clock", group: "General", icon: "clock",
    line: "Your time zone, which bots' wake-ups and times of day go by on all your computers.",
    keywords: ["time zone", "timezone", "clock", "wake-ups", "schedule", "travel"],
  },
  {
    id: "conversations", title: "Conversations", group: "Agents & Tools", icon: "message",
    line: "When supported agents summarize long conversations on this computer.",
    keywords: ["context", "tokens", "compact", "summarize"],
  },
  {
    id: "permissions", title: "Permissions", group: "Agents & Tools", icon: "shield",
    line: "What agents may do without asking, and the sandbox their commands run in.",
    keywords: ["approval", "full access", "auto", "ask first", "allow", "don't allow", "sandbox", "also allowed", "other computers", "rules"],
  },
  {
    id: "browser", title: "Browser", group: "System", icon: "globe",
    line: "The browser's search engine, and offering to save sign-ins.",
    keywords: ["search engine", "google", "duckduckgo", "sign-ins", "passwords", "tabs", "never offered"],
  },
  COMPUTER,
  { id: "git", title: "Git", group: "Projects", icon: "branch",
    line: "How sessions' branches are named, and commit messages and pull requests are written.",
    keywords: ["branch prefix", "worktree", "commit", "pull request", "instructions"],
  },
  { id: "worktrees", title: "Worktrees", group: "Projects", icon: "folder", line: "Where new projects and worktrees go, and sessions’ own copies of a project.", keywords: ["projects", "folders", "branches", "remove"] },
  { id: "storage", title: "Storage", group: "System", icon: "drive", line: "How much Kuvo keeps on this computer, and how much older versions of what agents made keep.", keywords: ["copies", "older versions", "budget", "disk", "clear", "space", "size", "folder"] },
  { id: "archived", title: "Archived Sessions", group: "System", icon: "archive", line: "Sessions out of the sidebar, still searchable; bring them back or delete them.", keywords: ["archive", "unarchive", "delete", "sessions"] },
  agentsSection.listed,
  vaultSection.listed,
  mcpSection.listed,
  customAutoSection.listed,
  // (Skills are in Marketplace › Skills now: `kuvo/settings/skills` still answers, unlisted.)
];

export { revised };


// MARK: The routes a page is made from

const browserAnswer = (context: RouteContext) => browserDataRoute(context, context.route.match(BROWSER_DATA_ROUTE)!);


// MARK: Permissions

const MODE_NAMES: Record<string, string> = { askFirst: "Ask First", acceptEdits: "Accept Edits", auto: "Auto", fullAccess: "Full Access" };

// (The kinds of action Settings › Permissions has a choice for, each a rule for everything ("*") at
// the front of the engine's `permissions`: permission-rules.ts, which Settings › Agents shares.)
const NOT_A_LIST = "The engine's permissions here aren't a list of rules: change them in its settings file.";

async function permissionsPage(context: RouteContext): Promise<SettingsPage> {
  const name = context.info.name || "this computer";
  const here = context.request.fromApp === true;
  const [approval, sandbox, allowed, config] = [
    await asked(context, approvalRoute, "GET", "kuvo/approval"),
    await asked(context, sandboxRoute, "GET", "kuvo/sandbox"),
    await asked(context, sandboxRoute, "GET", "kuvo/sandbox/allowed"),
    await asked(context, settingsRoute, "GET", "kuvo/config"),
  ];
  const groups: SettingGroup[] = [];
  const notes: { words: string; tone: "quiet" | "warning" }[] = [];

  if (approval.status === 200) {
    const { fullAccess, default: mode, auto, otherComputers } = approval.value;
    const rows: SettingRow[] = [
      row({ id: "default-mode", title: "New sessions start in", line: "The mode you picked last on this computer. Change it in a conversation's mode menu.", kind: "status", value: MODE_NAMES[mode] ?? String(mode) }),
      row({
        id: "full-access", title: "Full Access", kind: "switch", value: fullAccess === true,
        line: fullAccess ? "Conversations set to Full Access work here without asking." : "Asked the first time you pick it in a conversation on this computer.",
        can: fullAccess ? true : { why: "Turn it on in a conversation: pick Full Access in its mode menu, and confirm it there." },
        ...(fullAccess ? { confirm: { title: "Turn Off Full Access?", body: `Conversations on ${name} in Full Access go on in Auto. You're asked again the next time you pick it.`, button: "Turn Off" } } : {}),
      }),
    ];
    for (const [side, words] of [["engine", "the engine"], ["agents", "the coding agents"]] as const) {
      const held = auto?.[side]?.held;
      if (held) rows.push(row({ id: `auto-held-${side}`, title: `Auto, for ${words}`, line: held.why ?? held.reason, tone: "warning", kind: "status", value: "Held" }));
    }
    if (typeof otherComputers === "boolean") {
      rows.push(row({ id: "other-computers", title: "Let your other computers start work here", line: `Their bots and agents can start sessions on ${name}.`, kind: "switch", value: otherComputers, can: true }));
    }
    groups.push({ id: "approval", title: "Approval", rows });
  } else {
    notes.push({ words: `Approval couldn't be read: ${said(approval, "it didn't answer.")}`, tone: "warning" });
  }

  if (config.status === 200) {
    const rules = Array.isArray(config.value.permissions) ? config.value.permissions : [];
    // (Written by hand as something else than a list of rules: left as it is, shown as it is not
    // known here, and said.)
    const listed = config.value.permissions === undefined || Array.isArray(config.value.permissions);
    const can = listed ? true as const : { why: NOT_A_LIST };
    const rows = ACTIONS.map(action => {
      const chosen = choiceIn(rules, action.id);
      // What it does with no rule of yours is marked: picking that takes your rule away, so the
      // engine's own rules (Plan mode doesn't edit) stay.
      // A rule of yours that says what applies anyway still counts in every mode (Plan mode edits
      // under an Allow of yours): shown apart, with the default to take it away.
      const ownDefault = chosen === engineDefault(action.id);
      const choices = [
        ...EFFECTS.map(effect => effect.id !== engineDefault(action.id) ? effect
          : ownDefault ? { ...effect, title: `${effect.title} (Your Rule)`, line: "Applies in every mode, Plan mode too." }
          : { ...effect, title: `${effect.title} (Default)` }),
        // (One a newer engine set, shown as it is, and kept unless another is picked.)
        ...(chosen && !EFFECTS.some(effect => effect.id === chosen) ? [{ id: chosen, title: "Set by a Newer Engine" }] : []),
        ...(ownDefault ? [{ id: "default", title: `${EFFECTS.find(effect => effect.id === chosen)!.title} (Default)` }] : []),
      ];
      if (!listed) return row({ id: `when-${action.id}`, title: action.title, ...("line" in action ? { line: action.line } : {}), kind: "status", value: "Set in Its Settings File", can });
      return row({ id: `when-${action.id}`, title: action.title, ...("line" in action ? { line: action.line } : {}), kind: "choice", value: chosen ?? engineDefault(action.id), choices, can });
    });
    groups.push({ id: "when", title: "When the Agent Wants To", note: listed ? "For every session and mode. Don't Allow holds even in Full Access. Codex, Cursor, Grok and Antigravity may read files without asking." : NOT_A_LIST, rows });
  } else {
    notes.push({ words: said(config, "The engine's settings couldn't be read."), tone: "warning" });
  }

  if (sandbox.status === 200) {
    groups.push({ id: "sandbox", title: "Sandbox", rows: [row(sandboxRow(sandbox.value, name, here))] });
  } else {
    notes.push({ words: `The sandbox couldn't be read: ${said(sandbox, "it didn't answer.")}`, tone: "warning" });
  }

  if (allowed.status === 200) {
    const items = (Array.isArray(allowed.value.allowed) ? allowed.value.allowed : []).map((rule: any) => {
      const whose = rule.owner?.name ? ` · for ${rule.owner.name}` : "";
      return item({
        id: `allowed-${rule.id}`, title: rule.path, line: rule.gone ? `Gone from this computer${whose}` : `Can write${whose}`,
        kind: "actions", actions: [{ id: "remove", title: "Remove" }],
      });
    });
    groups.push({
      id: "allowed", rows: [row({
        id: "allowed", title: "Also Allowed", line: "Folders agents' commands may write in besides their own. Choosing Always on a stopped command's card adds one.",
        kind: "list", items, empty: "Nothing else is allowed yet.",
      })],
    });
  } else {
    notes.push({ words: `What's also allowed couldn't be read: ${said(allowed, "it didn't answer.")}`, tone: "warning" });
  }

  notes.push({ words: "Rules for particular commands or files, approvals saved with Always, and blocks aren't on this page yet.", tone: "quiet" });
  return { id: "permissions", title: "Permissions", groups, notes };
}

const CHECK = "POST kuvo/sandbox/check";
const RESTART = "POST kuvo/engine/restart";

/** Whether this box restarts its engine itself: a machine's always; a Mac's while its keeper runs
 * it (`boxAnswers`), else the app that runs it does. */
const restartsItself = () => !IS_MAC || boxAnswers("kuvo/engine/restart");

/** The sandbox's row, as `kuvo/sandbox` says it: a quiet line, or while it needs you (`needsYou`),
 * why (a warning) and its button. */
export function sandboxRow(state: any, name: string, here: boolean) {
  const needsYou = state.needsYou === true;
  const action = needsYou ? sandboxAction(state, name, here) : null;
  return {
    id: "sandbox", title: "Sandbox", line: needsYou ? String(state.why ?? "") : "Agents' commands here run in it.",
    ...(needsYou ? { tone: "warning" as const } : {}),
    kind: action ? "actions" as const : "status" as const, value: state.on ? "On" : "Off",
    ...(action ? { actions: [action] } : {}),
  };
}

/** The sandbox's button while it needs you, as the box names it (`kuvo/sandbox`'s `action`): one that
 * runs a route (checking again, restarting the engine), one that opens a page, or this computer's
 * own Software Update, only from a client on it. */
export function sandboxAction(state: any, name: string, here: boolean, restarts = restartsItself()): SettingAction | null {
  if (!state.action || !state.actionLabel) return null;
  const title = String(state.actionLabel);
  if (state.route === RESTART && !restarts) return { id: String(state.action), title, can: { why: `Kuvo Code on ${name} restarts its engine: restart it there.` } };
  if (state.route) {
    const confirm = state.action === "restartEngine" && title.endsWith("…")
      ? { confirm: { title: `Restart ${name}'s engine?`, body: "What agents are doing there now stops. Commands run in the sandbox once it's back.", button: "Restart Engine" } }
      : {};
    return { id: String(state.action), title, ...confirm };
  }
  if (typeof state.opens !== "string") return null;
  if (state.opensHere && state.opens === SOFTWARE_UPDATE && !here) return { id: String(state.action), title, can: { why: `Open Software Update on ${name} itself.` } };
  return { id: String(state.action), title, opens: state.opens };
}

async function permissionsChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  if (at.id === "full-access") {
    if (value !== false) return { status: 400, message: "Full Access is only turned off here." };
    return refusal(await asked(context, approvalRoute, "PATCH", "kuvo/approval", { fullAccess: false }));
  }
  if (at.id === "other-computers") return refusal(await asked(context, approvalRoute, "PATCH", "kuvo/approval", { otherComputers: value }));
  const action = at.id.startsWith("when-") ? at.id.slice(5) : null;
  if (action) {
    // Only this one's rule; the rest, those written by hand too, stay as they are. Written from
    // what's there just before (`was`): one changed meanwhile is read again, and this one's put to it.
    for (let tries = 0; tries < 3; tries++) {
      const read = await asked(context, settingsRoute, "GET", "kuvo/config");
      if (read.status !== 200) return refusal(read);
      const was = read.value.permissions ?? null;
      if (was !== null && !Array.isArray(was)) return { status: 403, message: NOT_A_LIST };
      // (This one changed since it was read, by hand or another client: refused, the page now.)
      if ((choiceIn(was ?? [], action) ?? engineDefault(action)) !== at.value) return STALE;
      // Picking what applies already writes nothing (a rule of yours that says it stays); the
      // default takes yours away, never writing one for it.
      if (value === at.value) return null;
      const items = (Array.isArray(was) ? was : []).filter((each: unknown) => !(isRule(each) && each.action === action && each.resource === "*"));
      if (value !== "default" && value !== engineDefault(action)) items.unshift({ action, resource: "*", effect: value });
      if (sortedJSON(items) === sortedJSON(was ?? [])) return null;
      const written = await asked(context, settingsRoute, "PATCH", "kuvo/config", { changes: [{ path: ["permissions"], value: items.length ? items : null, was }] });
      if (written.status === 409 && written.value?.problem === undefined && written.value?.message === CHANGED_SINCE) continue;
      return refusal(written);
    }
    return { ...STALE, message: "The engine's settings keep changing: try again in a moment." };
  }
  return { status: 400, message: "That isn't something to change." };
}

async function permissionsAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  if (at.id === "sandbox") {
    // As the sandbox names it now (the row's revision says it's the one that was read).
    const state = await asked(context, sandboxRoute, "GET", "kuvo/sandbox");
    if (state.status !== 200) return refusal(state);
    if (String(state.value.action) !== action.id) return STALE;
    return sandboxDoes(context, state.value.route);
  }
  if (at.id.startsWith("allowed-") && action.id === "remove") {
    return refusal(await asked(context, sandboxRoute, "DELETE", `kuvo/sandbox/allowed/${encodeURIComponent(at.id.slice(8))}?all`));
  }
  return { status: 404, message: "That isn't something it does." };
}

/** Runs the sandbox's button's route: checking it again, or restarting the engine where this box
 * does that itself (begun: `202`, done). */
export async function sandboxDoes(context: RouteContext, route: unknown, restarts = restartsItself()): Promise<Refused | null> {
  if (route === CHECK) return refusal(await asked(context, sandboxRoute, "POST", "kuvo/sandbox/check", {}));
  if (route === RESTART) {
    if (!restarts) return { status: 403, message: `Kuvo Code on ${context.info.name || "this Mac"} restarts its engine: restart it there.` };
    const { status, message } = await engineControl("restart");
    return refusal({ status, value: message ? { message } : {} });
  }
  return { status: 404, message: "That isn't something it does." };
}

// MARK: Browser

async function browserPage(context: RouteContext): Promise<SettingsPage> {
  const read = await asked(context, browserAnswer, "GET", "kuvo/browser/settings");
  if (read.status !== 200) {
    return { id: "browser", title: "Browser", groups: [], notes: [{ words: `The browser's settings couldn't be read: ${said(read, "they didn't answer.")}`, tone: "warning" }] };
  }
  const settings = read.value;
  const engines: { id: string; title: string }[] = Array.isArray(settings.searchEngines) ? settings.searchEngines : SEARCH_ENGINES.map(({ id, title }) => ({ id, title }));
  const never: string[] = Array.isArray(settings.never) ? settings.never : [];
  return {
    id: "browser", title: "Browser",
    groups: [
      {
        id: "browser", rows: [
          row({ id: "search-engine", title: "Search engine", line: "For anything typed in the address field that isn't an address.", kind: "choice", value: settings.searchEngine, choices: engines.map(({ id, title }) => ({ id, title })), can: true }),
          row({ id: "tidies", title: "Close agents' tabs you haven't looked at", line: "An hour after an agent last used one, so tabs don't pile up.", kind: "switch", value: settings.tidiesAgentTabs === true, can: true }),
          row({ id: "offers", title: "Offer to save sign-ins", line: "When you sign in to a site in the browser.", kind: "switch", value: settings.offersToSave === true, can: true }),
        ],
      },
      {
        id: "never", rows: [row({
          id: "never", title: "Never Offered", line: "Sites you said Never to when it offered to save a sign-in.", kind: "list", empty: "None yet.",
          items: never.map(host => item({ id: `never-${host}`, title: host, kind: "actions", actions: [{ id: "offer-again", title: "Offer Again" }] })),
        })],
      },
    ],
  };
}

const BROWSER_FIELDS: Record<string, string> = { "search-engine": "searchEngine", tidies: "tidiesAgentTabs", offers: "offersToSave" };

async function browserChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  const field = BROWSER_FIELDS[at.id];
  if (!field) return { status: 400, message: "That isn't something to change." };
  return refusal(await asked(context, browserAnswer, "PATCH", "kuvo/browser/settings", { [field]: value }));
}

async function browserAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  if (!at.id.startsWith("never-") || action.id !== "offer-again") return { status: 404, message: "That isn't something it does." };
  // (That one alone: a site said Never to meanwhile stays.)
  const problem = changeNeverSite(at.id.slice(6), false);
  return problem ? { status: 400, message: problem } : null;
}

// MARK: Git

// One card of single lines (the branch prefix), then each set of instructions as a group of its own:
// its heading and line, and the text written full width under them (a `text` row with `lines`
// alone in its group).
const GIT_ROWS = [
  { id: "branch-prefix", key: "branchPrefix", group: undefined, groupId: "branches", title: "Branch prefix", line: "How a session’s own branch in a worktree starts.", placeholder: "None" },
  { id: "commit-instructions", key: "commitInstructions", group: "Commit Instructions", groupId: "commit-messages", title: "Commit instructions", line: "Added when Suggest a Message in Changes asks the model for one.", placeholder: "For example: Start with the area changed, like “ui: …”, and never mention tickets." },
  { id: "pull-request-instructions", key: "pullRequestInstructions", group: "Pull Request Instructions", groupId: "pull-requests", title: "Pull request instructions", line: "Added when Create Pull Request writes the title and description.", placeholder: "For example: Add a Testing section, and link the issue if the branch names one." },
] as const;

async function gitPage(context: RouteContext): Promise<SettingsPage> {
  const read = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (read.status !== 200) return { id: "git", title: "Git", groups: [], notes: [{ words: said(read, "This computer's settings couldn't be read."), tone: "warning" }] };
  return { id: "git", title: "Git", groups: GIT_ROWS.map(each => {
    const value = read.value[GIT_SETTINGS[each.key][0]];
    const lines = each.key !== "branchPrefix";
    return { id: each.groupId, ...(each.group ? { title: each.group, note: each.line } : {}), rows: [row({
      id: each.id, title: each.title, ...(lines ? { lines: { min: 5, max: 12 } } : { line: each.line }), placeholder: each.placeholder,
      kind: "text", value: typeof value === "string" ? each.key === "branchPrefix" ? cleanBranchPrefix(value) : value.trim() : each.key === "branchPrefix" ? DEFAULT_BRANCH_PREFIX : "",
      can: typeof value === "string" || value === undefined ? true : { why: "This value is set in the settings file as something other than text. Change it there." },
    })] };
  }) };
}

async function gitChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  const each = GIT_ROWS.find(each => each.id === at.id);
  if (!each) return { status: 400, message: "That isn't something to change." };
  const path = [...GIT_SETTINGS[each.key]];
  const read = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (read.status !== 200) return refusal(read);
  const was = read.value[path[0]!] ?? null;
  value = each.key === "branchPrefix" ? cleanBranchPrefix(String(value)) : String(value).trim();
  const written = await asked(context, settingsRoute, "PATCH", "kuvo/config", { changes: [{ path, value, was }] });
  if (written.status === 409 && written.value.message === CHANGED_SINCE) return STALE;
  return refusal(written);
}

// MARK: Worktrees

const workAnswer = (context: RouteContext) => workAsksRoute(context, context.route.match(WORK_ASKS_ROUTE)!);

async function worktreesPage(context: RouteContext): Promise<SettingsPage> {
  const config = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (config.status !== 200) return unreadPage("worktrees", "Worktrees", config);
  const work = await asked(context, workAnswer, "GET", "kuvo/work/settings");
  if (work.status !== 200) return unreadPage("worktrees", "Worktrees", work);
  for (const file of [WORK_SETTINGS_FILE, WORKTREES_FILE, PROJECTS_FILE]) {
    const checked = readSettings(file);
    if (checked.problem) return unreadPage("worktrees", "Worktrees", { status: 409, value: { message: checked.problem } });
  }
  const chosen = config.value.worktree?.directory;
  const where = await asked(context, worktreesRoute, "GET", `kuvo/worktrees/where${typeof chosen === "string" ? `?directory=${encodeURIComponent(chosen)}` : ""}`);
  if (where.status !== 200) return unreadPage("worktrees", "Worktrees", where);
  const listed = await asked(context, worktreesRoute, "GET", "kuvo/worktrees");
  if (listed.status !== 200) return unreadPage("worktrees", "Worktrees", listed);
  let engineRuns = false;
  try { engineRuns = Boolean(context.readEngine()?.port); } catch {}
  const projects = where.value.projects ?? [];
  const commonPlace = projects.length > 0 && projects.every((each: any) => each.folder === projects[0].folder);
  // Where new worktrees go is the folder field itself (the default its placeholder): a project is
  // listed only where it goes elsewhere (its own settings), or where its place needs you.
  // (The engine keeps each project's in a folder of its own in the one chosen, or the default.)
  const expected = typeof chosen === "string" && chosen ? chosen.replace(/^~(?=\/)/, homeFolder()) : String(where.value.default);
  const elsewhere = (folder: unknown) => typeof folder === "string" && folder !== expected && !folder.startsWith(expected.replace(/\/+$/, "") + "/");
  const needsYou = (each: any) => (each.exists === false && each.canMake === false) || (each.inside && !each.ignored);
  const listedPlaces = (commonPlace ? [projects.find(needsYou) ?? projects[0]] : projects).filter((each: any) => needsYou(each) || elsewhere(each.folder));
  const places: SettingRow[] = listedPlaces.slice(0, 5).map((each: any) => row({
    id: `place-${each.project}`, title: commonPlace ? "New worktrees go in" : each.name,
    kind: "status", value: each.folder ?? "Known once the engine runs",
    ...(each.exists === false && each.canMake === false ? { line: "Kuvo can’t create this folder. Choose another one.", tone: "warning" as const } : each.inside && !each.ignored ? { line: `It’s inside ${commonPlace ? each.name + "’s" : "this project’s"} repository, so git would list each worktree as new files there. Choose a folder outside it.`, tone: "warning" as const } : {}),
  }));
  if (listedPlaces.length > 5) places.push(row({ id: "more-worktree-projects", title: `And ${listedPlaces.length - 5} more ${listedPlaces.length - 5 === 1 ? "project" : "projects"}`, kind: "status" }));
  const shown = (folder: string) => (folder === homeFolder() || folder.startsWith(homeFolder() + "/") ? "~" + folder.slice(homeFolder().length) : folder);
  return { id: "worktrees", title: "Worktrees", groups: [
    { id: "folders", rows: [
      row({ id: "worktrees-folder", title: "Worktree folder", line: "Each session’s worktree is a folder in it. Leave it empty for the default.", kind: "folder", value: typeof chosen === "string" ? chosen : "", placeholder: shown(String(where.value.default)), folder: { choose: "Choose a Folder…", clear: "Default Folder", prompt: "Choose", message: "Choose where new worktrees go. Each one is a folder in it." }, can: (config.value.worktree === undefined || isObject(config.value.worktree)) && (chosen === undefined || typeof chosen === "string") ? true : { why: "This folder is set in the settings file as something other than text. Change it there." } }),
      ...places,
      row({ id: "projects-folder", title: "New projects folder", line: "Where projects your bots make go. A bot asks you first, and Kuvo never deletes them.", kind: "folder", value: work.value.projectsFolder ?? "", placeholder: "None", folder: { choose: "Choose a Folder…", clear: "None", prompt: "Choose", message: "Choose where new projects go. Each one is a folder in it." }, can: true }),
    ] },
    { id: "session-worktrees", title: "Sessions’ Worktrees", note: "Removing one deletes its folder and anything in it that isn’t committed. Its branch stays if it isn’t merged yet.", rows: [row({ id: "worktrees", title: "Sessions’ Worktrees", kind: "list", empty: "None yet. Choose Worktree in a new session’s composer to give it one.", items: listed.value.worktrees.map((tree: any) => item({
      id: `tree-${tree.path}`, title: tree.branch, line: `${tree.projectName} · ${tree.sessions} ${tree.sessions === 1 ? "session" : "sessions"}`,
      kind: "actions", value: { record: readWorktrees().find(record => record.root === tree.path), uncommitted: tree.uncommitted, ahead: tree.ahead },
      actions: [{ id: "remove", title: "Remove…", style: "destructive", can: tree.working ? { why: `An agent is still working in the worktree of ${tree.branch}, so it stays. Stop it first, then remove the worktree.` } : engineRuns ? true : { why: `The engine isn't running, so the worktree of ${tree.branch} stays. Try again once it's started.` }, confirm: { title: `Remove the worktree for ${tree.branch}?`, body: worktreeRemovalWords(tree), button: "Remove" } }],
    })) })] },
  ] };
}

export function worktreeRemovalWords(tree: { uncommitted?: boolean; ahead?: number; branch: string }): string {
  const note = tree.uncommitted === true ? "Its folder is deleted, with changes in it that aren’t committed." : tree.uncommitted === false ? "Its folder is deleted. Git sees nothing in it that isn’t committed." : "Its folder and anything in it that isn’t committed are deleted.";
  return note + (tree.ahead && tree.ahead > 0 ? ` ${tree.branch} keeps its ${tree.ahead} ${tree.ahead === 1 ? "commit" : "commits"} not merged yet.` : "") + " Its sessions stay, without a folder to run in.";
}

export async function worktreesChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  const typed = String(value).trim();
  if (at.id === "projects-folder") return staleOrRefusal(await asked(context, workAnswer, "PATCH", "kuvo/work/settings", { projectsFolder: typed || null, was: { projectsFolder: at.value || null } }));
  if (at.id !== "worktrees-folder") return { status: 400, message: "That isn't something to change." };
  const config = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (config.status !== 200) return refusal(config);
  if ((config.value.worktree?.directory ?? "") !== at.value) return STALE;
  const folder = typed ? typed.replace(/^~(?=$|\/)/, homeFolder()) : null;
  if (folder && !path.isAbsolute(folder)) return { status: 400, message: "Give a full path to the folder." };
  return staleOrRefusal(await asked(context, settingsRoute, "PATCH", "kuvo/config", { changes: [{ path: ["worktree", "directory"], value: folder ? path.resolve(folder) : null, was: config.value.worktree?.directory ?? null }] }));
}

async function worktreesAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  if (action.id !== "remove" || !at.id.startsWith("tree-")) return noActions();
  return staleOrRefusal(await asked(context, worktreesRoute, "POST", "kuvo/worktrees/remove", { path: at.id.slice(5), was: at.value }));
}

// MARK: Storage

/** What Kuvo keeps on this computer, measured (`storage-sizes.ts`): the whole as a bar of its parts,
 * Refresh, Kuvo's folder (and worktree folders outside it) with what each takes; what agents made,
 * with how much older versions keep; and the pages that clear the rest (Worktrees, Archived
 * Sessions). What agents made comes from the coding agents' side, which may be down: the rest stays. */
async function storagePage(context: RouteContext): Promise<SettingsPage> {
  const here = context.request.fromApp === true && IS_MAC;
  const where = computerHere(context);
  void measureSizes();
  const sizes = readSizes();
  const measuring = isMeasuring();
  const problem = measureProblem();
  const parts = sizes?.parts.map(part => ({
    id: part.id, title: part.title, value: part.bytes, words: fileSize(part.bytes),
    ...(part.folders?.length ? { parts: part.folders.map(folder => ({ title: folder.title, value: folder.bytes, words: fileSize(folder.bytes) })) } : {}),
  }));
  const finder = (path: string): SettingAction[] => (here ? [{ id: "show", title: "Show in Finder", opens: `file://${encodeURI(path)}` }] : []);
  const folderRow = (id: string, title: string, path: string, bytes: number | undefined) => row({
    id, title, line: path.startsWith(homeFolder() + "/") ? "~" + path.slice(homeFolder().length) : path,
    kind: here ? "actions" : "status", ...(bytes === undefined ? {} : { value: bytes, unit: fileSizeUnit(bytes) }), ...(here ? { actions: finder(path) } : {}),
  });
  const groups: SettingGroup[] = [
    { id: "computer", title: computerTitle(context), note: `What Kuvo keeps on ${where}.`, rows: [row({
      id: "used", title: sizes ? `${fileSize(sizes.total)} used by Kuvo` : "Measuring what Kuvo keeps…",
      ...(problem && !measuring ? { line: `It couldn’t be measured just now: ${problem}`, tone: "warning" as const } : {}),
      kind: "meter", ...(parts ? { parts } : {}), ...(measuring ? { busy: "Measuring…" } : {}),
      actions: [{ id: "refresh", title: "Refresh" }],
    })] },
    { id: "folders", title: "Storage by Folder", rows: [
      folderRow("home-folder", "Kuvo’s folder", K, sizes?.home),
      // (Worktrees kept outside it, by the folder they're in: theirs alone, not the folder's.)
      ...(sizes?.outside ?? []).map((each, ix) => folderRow(`worktree-folder-${ix}`, `${each.trees} ${each.trees === 1 ? "worktree" : "worktrees"} in`, each.path, each.bytes)),
    ] },
  ];
  const notes: { words: string; tone: "quiet" | "warning" }[] = [];
  const read = await asked(context, artifactsRoute, "GET", "kuvo/artifacts/storage");
  if (read.status === 200) {
    const kept = read.value;
    const unit = kept.budget < 1_000_000_000
      ? { name: "MB", factor: 1_000_000, decimals: 2, rounding: "nearest" as const }
      : { name: "GB", factor: 1_000_000_000, decimals: 2, rounding: "nearest" as const };
    groups.push({ id: "agents-made", title: "What Agents Made", rows: [
      row({ id: "copies", title: "Copies of what agents made", line: "Each version of the files agents write or change, so you can see what changed and put one back. Never what looks like a secret, and only the changes for code git keeps.", kind: "status", value: kept.copies, unit: fileSizeUnit(kept.copies) }),
      row({ id: "older-budget", title: "Older versions keep up to", line: "Past it, the oldest go first. The newest version of each file, memory, skills and what’s kept always stay.", kind: "number", value: kept.budget, unit, min: 0, can: true,
        suggestions: [0.25, 0.5, 1, 2, 5, 10].map(gb => ({ value: gb * 1_000_000_000, title: `${gb} GB` })),
        confirmBelow: { value: kept.older, confirm: budgetConfirmation(kept.older) },
      }),
      row({ id: "older-copies", title: "Older versions", line: kept.older === 0 ? "None kept." : `${fileSize(kept.older)} now.`, kind: "actions", value: kept.older,
        actions: [{ id: "clear", title: "Clear…", style: "destructive", can: kept.older > 0 ? true : { why: "None kept." }, confirm: { title: "Clear older versions?", body: "Their copies go; each stays listed, without one. The newest version of each file, memory, skills and what’s kept stay.", button: "Clear" } }],
      }),
    ] });
    if (kept.low) notes.push({ words: `The disk is nearly full, so only the newest version of each file keeps a copy until ${fileSize(kept.floor)} is free again.`, tone: "warning" });
  } else {
    notes.push({ words: `What agents made couldn’t be read: ${said(read, "it didn’t answer.")}`, tone: "warning" });
  }
  groups.push({ id: "manage", rows: [
    row({ id: "manage-worktrees", title: "Worktrees", line: "Sessions’ own copies of a project, and where they go.", kind: "open", section: "worktrees" }),
    row({ id: "manage-archived", title: "Archived Sessions", line: "Bring sessions back, or delete them for good.", kind: "open", section: "archived" }),
  ] });
  return { id: "storage", title: "Storage", groups, ...(notes.length ? { notes } : {}) };
}

async function storageChange(context: RouteContext, at: SettingItem, value: unknown, confirmed?: unknown): Promise<Refused | null> {
  if (at.id !== "older-budget") return { status: 400, message: "That isn't something to change." };
  return staleOrRefusal(await asked(context, artifactsRoute, "POST", "kuvo/artifacts/storage", { budget: Math.floor(Number(value)), was: at.value, wasOlder: at.confirmBelow?.value, confirmed }));
}

async function storageAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  if (at.id === "used" && action.id === "refresh") {
    // (Begun, not waited for: the page says it's measuring, and shows the sizes once they're in.)
    void measureSizes(true);
    return null;
  }
  if (at.id !== "older-copies" || action.id !== "clear") return noActions();
  return staleOrRefusal(await asked(context, artifactsRoute, "POST", "kuvo/artifacts/storage/clear", { was: at.value }));
}

// MARK: Archived Sessions

const sessionsAnswer = (context: RouteContext) => listRoute(context, context.route.match(LIST_ROUTE)!);
const sessionAnswer = (context: RouteContext) => sessionActionRoute(context, context.route.match(SESSION_ACTION_ROUTE)!);

async function archivedHere(context: RouteContext): Promise<SettingsPage> {
  const read = await asked(context, sessionsAnswer, "GET", "kuvo/sessions/list");
  if (read.status !== 200) return unreadPage("archived", "Archived Sessions", read);
  if (read.value.pending?.length) return unreadPage("archived", "Archived Sessions", { status: 503, value: { message: "Some sessions can't be read right now. Try again in a moment." } });
  const groups = new Map<string, SettingItem[]>();
  let held: string | undefined;
  const states: { id: string; was: { updated: number; archived: boolean; confirm: unknown } }[] = [];
  for (const group of read.value.archivedGroups) for (const id of group.sessions) {
    const facts = read.value.sessions[id]!;
    const menu = await asked(context, sessionAnswer, "GET", `kuvo/sessions/${encodeURIComponent(id)}/actions`);
    if (menu.status !== 200) return unreadPage("archived", "Archived Sessions", menu);
    held ??= menu.value.held;
    const was = { updated: facts.updated, archived: true, confirm: menu.value.actions.find((each: any) => each.id === "delete")?.confirm };
    states.push({ id, was });
    const actions: SettingAction[] = menu.value.actions.map((each: any) => ({
      id: each.id, title: each.label, ...(each.destructive ? { style: "destructive" } : {}),
      ...(each.confirm ? { confirm: { title: each.confirm.title, body: each.confirm.message, button: each.confirm.label } } : {}),
      ...(menu.value.held ? { can: { why: menu.value.held } } : {}),
    }));
    const project = `${group.kind}:${group.directory}`;
    const rows = groups.get(project) ?? [];
    rows.push(item({ id: `session-${id}`, title: facts.title || "New Session", line: new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short", timeZone: zoneHere(readZoneSetting()).zone }).format(new Date(facts.updated)), kind: "actions", value: was, actions }));
    groups.set(project, rows);
  }
  const all = states.length ? await asked(context, deletesRoute, "POST", "kuvo/sessions/deletes", { sessions: states.map(each => each.id) }) : null;
  if (all && all.status !== 200) return unreadPage("archived", "Archived Sessions", all);
  const projectTitle = (project: string) => read.value.archivedGroups.find((group: { kind: string; directory: string; title: string }) => `${group.kind}:${group.directory}` === project)!.title;
  return { id: "archived", title: "Archived Sessions", groups: [
    { id: "archive-controls", rows: [row({ id: "delete-all", title: "Archived Sessions", kind: "actions", value: states, actions: [{ id: "delete-all", title: "Delete All…", style: "destructive", can: held ? { why: held } : states.length ? true : { why: "None yet. Archive a session from its menu to put it away." }, ...(all ? { confirm: { title: all.value.title, body: all.value.message, button: all.value.label } } : {}) }] })] },
    ...[...groups].map(([project, items]) => ({ id: `project-${project}`, title: projectTitle(project), rows: [row({ id: `archived-${project}`, title: projectTitle(project), kind: "list", items, search: { placeholder: "Search archived sessions", empty: "No archived session matches “{query}”." } })] })),
  ], ...(states.length ? {} : { notes: [{ words: "None yet. Archive a session from its menu to put it away.", tone: "quiet" as const }] }) };
}

export const ARCHIVE_FEATURE = "line.merged-archives";
export const ARCHIVE_PEER_ACTION = /^kuvo\/peer-archive\/(ses[A-Za-z0-9_-]{1,95})\/(unarchive|delete|delete-all)$/;
export const archiveCopies = peerSnapshots<{ groups: SettingsPage["groups"] }>(
  "archiveCopy", "archives-v1.json", ARCHIVE_FEATURE, { groups: list(SETTING_GROUP) },
);

const archiveReaders = { readEngine: readEngineFile, bridge: bridgeTarget };
export const shareArchived = (host: Pick<LinkHost, "send">, readers = archiveReaders) => archiveCopies.push(host, async () => {
  const json: RouteContext["json"] = (status, value) =>
    [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
  const page = await archivedHere({
    ...readers, readBridge: readBridgeFile, request: { method: "GET", path: "kuvo/settings/archived", headers: {} },
    route: "kuvo/settings/archived", phone: undefined, signal: undefined, info: { name: "" },
    phonesChanged() {}, hold: null, json, text: json,
  });
  return page.groups.length ? { groups: page.groups } : null;
});

type ArchiveFrom = { peer: string; session: string; revision: string };
const archiveSource = (value: unknown): ArchiveFrom | undefined => {
  if (!isObject(value) || typeof value.peer !== "string" || typeof value.session !== "string" || typeof value.revision !== "string") return;
  return { peer: value.peer, session: value.session, revision: value.revision };
};

async function archivedPage(context: RouteContext): Promise<SettingsPage> {
  const link = readLink();
  const reach = readReach();
  const moved = movedOf(link);
  const [here, peers] = await Promise.all([
    archivedHere(context),
    Promise.all(peersOf(link).map(async peer => ({ peer, copy: await archiveCopies.read(peer.key) }))),
  ]);
  const local = listComputer(link);
  const groups: SettingGroup[] = here.groups.map(group => ({
    ...group, rows: group.rows.map(source => ({
      ...source, computer: local,
      ...(source.items ? { items: source.items.map(entry => ({ ...entry, computer: local })) } : {}),
    })),
  }));
  for (const { peer, copy } of peers) {
    if (!copy) continue;
    // (Away by the one rule, `standingOf`.)
    const { away, lastSeen } = copyStanding(peer.key, copy.seen, reach);
    const computer = { id: peer.key, name: peer.name };
    const stamp = { computer, away, ...(away ? { lastSeen } : {}) };
    const peerRow = (source: SettingItem, id: string, session: string) =>
      item({ ...source, id, ...stamp, value: { peer: peer.key, session, revision: source.revision }, actions: away ? [] : source.actions });
    for (const group of copy.groups) {
      const id = `${peer.key}:${group.id}`;
      if (group.id === "archive-controls") {
        const source = group.rows.find(row => row.id === "delete-all");
        const states = source?.value as { id: string }[] | undefined;
        if (away || !source || !Array.isArray(states) || !states.length || states.some(each => moved.has(each.id) && moved.get(each.id) !== peer.key)) continue;
        groups.push({ id, title: peer.name, rows: [peerRow(source, `delete-all-${peer.key}`, states[0]!.id)] });
        continue;
      }
      const rows = group.rows.flatMap(source => {
        const items = (source.items ?? []).flatMap(entry => {
          if (!entry.id.startsWith("session-")) return [];
          const session = entry.id.slice(8);
          if (moved.has(session) && moved.get(session) !== peer.key) return [];
          return [peerRow(entry, `session-${peer.key}:${session}`, session)];
        });
        return items.length ? [row({ ...source, ...stamp, id: `${peer.key}:${source.id}`, items })] : [];
      });
      if (rows.length) groups.push({ ...group, id, rows });
    }
  }
  const pending = pendingComputers(ARCHIVE_FEATURE, key => peers.some(({ peer, copy }) => peer.key === key && copy !== null), link);
  return { ...here, groups, pendingComputers: pending, ...(groups.length > here.groups.length ? { notes: here.notes?.filter(note => note.tone !== "quiet") } : {}) };
}

/** The owner checks the exact menu and confirmation revision it published before changing anything. */
export async function* archivedPeerAction(context: RouteContext, session: string, action: string): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  const body = parsed(request.body);
  if (!request.fromOwnerPeer || !isObject(body) || typeof body.was !== "string") return yield* json(403, {});
  const page = await archivedHere(context);
  const id = action === "delete-all" ? "delete-all" : `session-${session}`;
  const at = rowsOf(page).find(row => row.id === id);
  const chosen = at && actionsOf(at).find(each => each.id === action);
  if (!at || !chosen || at.revision !== body.was) return yield* json(409, { message: CHANGED_SINCE });
  const why = whyNot(chosen.can);
  if (why) return yield* json(403, { message: why });
  if (action === "delete-all" && (!Array.isArray(at.value) || at.value[0]?.id !== session)) return yield* json(409, { message: CHANGED_SINCE });
  const veto = queuedRefusal(context.admitQueued?.() ?? null, json);
  if (veto) return yield* veto;
  const refused = await archivedAct(context, at, chosen);
  return yield* (refused ? json(refused.status, { message: refused.message }) : json(200, {}));
}

async function archivedAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  const source = archiveSource(at.value);
  if (source) {
    const method = "POST";
    const path = `kuvo/peer-archive/${source.session}/${action.id}`;
    const body = { was: source.revision };
    const write = ownerWrite(method, path, body);
    const peer = peersOf(readLink()).find(peer => peer.key === source.peer);
    if (!write || !peer || readReach()[peer.key] !== undefined) return { status: 409, message: CHANGED_SINCE };
    return staleOrRefusal(await sendOwnerWrite(write, body, {
      key: peer.key, name: peer.name, scope: "session computer", local: false, reachable: true,
    }, context));
  }
  if (at.id === "delete-all" && action.id === "delete-all") {
    for (const each of at.value as { id: string; was: unknown }[]) {
      const done = staleOrRefusal(await asked(context, sessionAnswer, "DELETE", `kuvo/sessions/${encodeURIComponent(each.id)}`, { was: each.was }));
      if (done) return done;
    }
    return null;
  }
  if (!at.id.startsWith("session-")) return noActions();
  const id = encodeURIComponent(at.id.slice(8));
  if (action.id === "delete") return staleOrRefusal(await asked(context, sessionAnswer, "DELETE", `kuvo/sessions/${id}`, { was: at.value }));
  if (action.id === "unarchive") return staleOrRefusal(await asked(context, sessionAnswer, "POST", `kuvo/sessions/${id}/archive`, { archived: false, was: at.value }));
  return noActions();
}

// MARK: Clock

/** A zone's name as a menu shows it: "Europe/Istanbul" as "Europe › Istanbul", "America/New_York" as
 * "America › New York". */
const zoneTitle = (zone: string): string => zone.replaceAll("_", " ").split("/").join(" › ");

/** Every zone this runtime knows, by name, UTC among them. */
function zoneNames(): string[] {
  let names: string[] = [];
  try { names = Intl.supportedValuesOf("timeZone"); } catch {}
  return [...new Set([...names, "UTC"])].sort();
}

/** The zone's offset from UTC now, as "GMT+3" or "GMT−4:30". */
function offsetOf(zone: string, at = new Date()): string {
  try {
    return new Intl.DateTimeFormat("en-US", { timeZone: zone, timeZoneName: "shortOffset" }).formatToParts(at).find(part => part.type === "timeZoneName")?.value.replace("-", "−") ?? "";
  } catch {
    return "";
  }
}

/** The value of the time zone row while this box has no zone of yours yet: its own clock's. */
const OWN_CLOCK = "own-clock";

async function clockPage(context: RouteContext): Promise<SettingsPage> {
  const name = context.info.name || "this computer";
  const setting = readZoneSetting();
  const here = zoneHere(setting);
  const offset = offsetOf(here.zone);
  const choices = zoneNames().map(zone => ({ id: zone, title: zoneTitle(zone) }));
  // (One a peer's runtime knows and this one doesn't is still shown as it is.)
  if (here.yours && !choices.some(choice => choice.id === here.zone)) choices.unshift({ id: here.zone, title: zoneTitle(here.zone) });
  if (!here.yours) choices.unshift({ id: OWN_CLOCK, title: `${name}'s clock (${zoneTitle(here.zone)})` });
  // (A computer without the zone's rules, a Linux machine without tzdata, wakes on UTC: said so.)
  const unruled = here.yours && !zoneFilesFor(setting).zoneinfo && !hasZoneRules(here.zone);
  const everywhere = "on all your computers";
  const line = unruled
    ? `${name} has no rules for this time zone, so its wake-ups go by UTC. Install its time zone data (tzdata) to fix it.`
    : setting?.carried
      ? `Kept from what your Mac last gave ${name}: its bots' wake-ups go by it. Pick it, or another, to set it ${everywhere}.`
      : here.yours
        ? `Bots' wake-ups and times of day go by it ${everywhere}. It's ${offset} now.`
        : `None set yet: ${name} goes by its own clock. Pick yours, and your other computers take it too.`;
  return {
    id: "clock", title: "Clock",
    groups: [{
      id: "clock", title: "Clock", rows: [row({
        id: "time-zone", title: "Time zone", line, ...(unruled ? { tone: "warning" as const } : {}), kind: "choice", value: here.yours ? here.zone : OWN_CLOCK, choices, can: true,
      })],
    }],
  };
}

async function clockChange(context: RouteContext, at: SettingItem, value: unknown): Promise<SettingChange | null> {
  if (at.id !== "time-zone") return { status: 400, message: "That isn't something to change." };
  // (Its own clock while none is set is what applies already: nothing to write. The zone it shows
  // picked again is nothing to write either, but where it's only carried from before: then it's yours.)
  if (value === OWN_CLOCK || (value === at.value && !readZoneSetting()?.carried)) return null;
  try {
    const setup = readLink()?.setup;
    if (usesAcceptedSaves(setup)) {
      if (setup?.role !== "primary") return { status: 409, message: "Change your setup's time zone on its main computer." };
      const seed = await setupSeedRefusal("setupTimeZone");
      if (seed) return seed;
      const veto = context.admitQueued?.();
      if (veto) return { status: veto.status, notAdmitted: veto.notAdmitted,
        message: isObject(veto.value) && typeof veto.value.message === "string" ? veto.value.message : "The change could not be admitted." };
      const choice = zoneChoice(String(value));
      if (!choice) return { status: 400, message: "This computer doesn't know that time zone." };
      const save = submitShared("setupTimeZone", choice);
      if (!save) return { status: 409, message: "The main computer could not save the time zone." };
      return { save: sharedSaveReply(save) };
    }
    return setZone(String(value)) ? null : { status: 400, message: "This computer doesn't know that time zone." };
  } catch {
    return { status: 503, message: "Your time zone couldn't be changed here just now: try again in a moment." };
  }
}

// MARK: This computer

/** What this computer's page is made from. A test's own, where it has no Mac to ask. */
export const computerFacts = {
  updateMain: isUpdateMain,
  /** A Mac's own box in a home of its own (the one its helper keeps), a Mac's in its app's
   * folder (the retired app's), or a computer you added. */
  kind: (): "ownMac" | "appMac" | "machine" => (!IS_MAC ? "machine" : APP_FOLDER ? "appMac" : "ownMac"),
  /** Whether this box restarts what it runs itself. */
  restarts: (): boolean => !IS_MAC || !APP_FOLDER || boxAnswers("kuvo/engine/restart"),
  /** Whether its engine runs now (its record names where it answers). */
  running: (context: RouteContext): boolean => { try { return Boolean(context.readEngine()?.port); } catch { return false; } },
  atLogin: helperAtLogin,
  /** What macOS allows its helper, as it sees it now (`freshBoxPermissions`). */
  permissions: (): BoxPermissions | Promise<BoxPermissions> => freshBoxPermissions(),
  update: updateStatus,
  working: () => agentsWorking(),
  restart: () => engineControl("restart"),
  devices: (): boolean | null => macLink.config?.phones ?? null,
  letDevices: letDevicesConnect,
};

/** The page's name: "This Mac" to the app beside its box; this computer's name to any other. */
const computerTitle = (context: RouteContext): string =>
  context.request.fromApp === true ? (process.platform === "darwin" ? "This Mac" : "This Computer") : context.info.name || (IS_MAC ? "Mac" : "Computer");
/** This computer, in a sentence: "this Mac" to the app beside its box; its name to any other. */
const computerHere = (context: RouteContext): string =>
  context.request.fromApp === true ? (process.platform === "darwin" ? "this Mac" : "this computer") : context.info.name || "this computer";

const LOGIN_ITEMS = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";
/** What's missing without each permission (what it's for is the permission's own `line`). */
const NOT_ALLOWED: Record<string, string> = {
  screenRecording: "Not allowed. Agents can’t look at the screen.",
  accessibility: "Not allowed. Agents can’t click or type in other apps.",
  fullDiskAccess: "Not allowed. Agents may be blocked from opening protected files.",
};

async function computerPage(context: RouteContext): Promise<SettingsPage> {
  const here = context.request.fromApp === true;
  const where = computerHere(context), name = context.info.name || where;
  const kind = computerFacts.kind();
  const ownMac = kind === "ownMac" && process.platform === "darwin";
  const atLogin = ownMac ? computerFacts.atLogin() : null;
  const working = await computerFacts.working().catch(() => null);
  /** What opens only on this computer: given to the app beside its box, said to any other. */
  const opensHere = (id: string, title: string, address: string): SettingAction => (here ? { id, title, opens: address } : { id, title, can: { why: `Open it on ${name} itself.` } });
  const stops = working === true ? `Agents are at work on ${where} now: what they’re doing stops.` : working === false ? `Nothing is at work on ${where} now.` : `What agents are doing on ${where} now stops.`;

  // How it stands, and Restart.
  const restarts = computerFacts.restarts();
  const running = computerFacts.running(context);
  const rows: SettingRow[] = [row({
    id: "state", title: running ? "Running" : "Not running", kind: "actions", value: running,
    ...(running ? {} : { tone: "warning" as const }),
    line: !running ? "Its engine isn’t running, so no agent works here. Restart starts it again." : ownMac ? (atLogin === false ? `${HELPER_NAME} keeps it running.` : `${HELPER_NAME} keeps it running, and starts it at login.`) : kind === "appMac" ? `Kuvo Code on ${where} runs it.` : "It runs whether or not an app is open.",
    actions: [{
      id: "restart", title: "Restart…",
      ...(restarts ? {} : { can: { why: `Kuvo Code on ${name} restarts it: restart it there.` } }),
      confirm: { title: `Restart Kuvo on ${where}?`, body: `${stops} Kuvo is back in a moment.`, button: "Restart" },
    }],
  })];

  // Its version, and its update.
  const update = computerFacts.update();
  const version = update.version ? `Kuvo Code ${update.version}` : "Kuvo Code";
  const failed = update.last?.result === "failed" ? update.last.message : undefined;
  if (update.updating) rows.push(row({ id: "version", title: version, line: update.updating === "checking" ? "Looking for a newer version." : "Getting the release ready.", kind: "status", busy: update.updating === "checking" ? "Checking…" : "Updating…" }));
  else if (!update.setUp || update.blocked) rows.push(row({ id: "version", title: version, line: update.blocked ?? `Updates aren’t set up for ${where} yet.`, kind: "status" }));
  else rows.push(row({ id: "version", title: version, line: failed ?? "This computer updates with your setup.",
    kind: computerFacts.updateMain() ? "actions" : "status", value: update.version, actions: computerFacts.updateMain() ? [{ id: "update", title: "Update Now" }] : [] }));

  // "Let my devices connect": a Mac's switch. A computer you added always lets them in.
  const devices = computerFacts.devices();
  const reach = `Your phone and other computers reach ${where}, end to end encrypted.`;
  if (kind === "machine") rows.push(row({ id: "devices", title: "Let my devices connect", line: `${reach} A computer you added always lets them in.`, kind: "status", value: "On" }));
  else if (kind === "appMac") rows.push(row({ id: "devices", title: "Let my devices connect", line: reach, kind: "switch", value: devices === true, can: { why: `Change it in Kuvo Code on ${name}.` } }));
  else if (devices === null) rows.push(row({ id: "devices", title: "Let my devices connect", line: reach, kind: "switch", value: false, can: { why: "This copy of Kuvo has no relay for your devices to connect through." } }));
  // (Switched on the Mac itself: turned off from a device, that device would cut itself off with
  // no way to turn it on again.)
  else if (!here) rows.push(row({ id: "devices", title: "Let my devices connect", line: reach, kind: "switch", value: devices, can: { why: `Change it in Kuvo Code on ${name} itself.` } }));
  else rows.push(row({
    id: "devices", title: "Let my devices connect", line: reach, kind: "switch", value: devices, can: true,
    ...(devices ? { confirm: { title: "Stop letting your devices connect?", body: `Your phone, and Kuvo Code on your other computers, can’t reach ${where} until you turn this on again.`, button: "Turn Off" } } : {}),
  }));
  const groups: SettingGroup[] = [{ id: "computer", rows }];
  // The coding agents' own updaters: off, Kuvo turns them off and says when there's a new version.
  groups.push({ id: "agents", title: "Coding Agents", rows: [row({
    id: "agents-update", title: "Update coding agents automatically", kind: "switch", value: agentsUpdateThemselves(K), can: true,
    line: "Off: each agent keeps its version until you update it in Accounts. On: agents update themselves as they run.",
  })] });

  // What macOS allows the program a Mac's own agents run under: only there.
  const permissions = await computerFacts.permissions();
  if (permissions.applies) {
    const helper = permissions.name ?? HELPER_NAME;
    const allows: SettingRow[] = [];
    if (atLogin !== null) {
      allows.push(atLogin
        ? row({ id: "login-items", title: "Login Items", line: `Kuvo starts when you log in to ${where}.`, kind: "status", value: "On" })
        : row({ id: "login-items", title: "Login Items", line: `Off, so Kuvo on ${where} doesn’t start at login.`, tone: "warning", kind: "actions", value: "Off", actions: [opensHere("open-login-items", "Open Login Items", LOGIN_ITEMS)] }));
    }
    for (const each of permissions.list) {
      const words = { allowed: each.line, not: NOT_ALLOWED[each.id] ?? each.line };
      if (each.allowed === false) allows.push(row({ id: `allows-${each.id}`, title: each.name, line: words.not, tone: "warning", kind: "actions", value: "Not allowed", actions: [opensHere(`open-${each.id}`, "Open System Settings", each.settings)] }));
      else allows.push(row({ id: `allows-${each.id}`, title: each.name, line: each.allowed ? words.allowed : `${each.unknownLine} ${words.allowed}`.trim(), kind: "status", value: each.allowed ? "Allowed" : "Not known yet" }));
    }
    groups.push({ id: "macos", title: `What macOS Allows ${helper}`, note: `${helper} is the program your agents run under. Only you allow these, in System Settings.`, rows: allows });
  }
  return { id: "computer", title: computerTitle(context), groups };
}

const updateAnswer = (context: RouteContext) => updateRoute(context);

async function computerChange(_context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  if (at.id === "agents-update") {
    if (typeof value !== "boolean") return { status: 400, message: "Turn it on or off." };
    if (value !== agentsUpdateThemselves(K)) writePrivate(agentUpdatesFile(K), JSON.stringify(value));
    return null;
  }
  if (at.id !== "devices") return { status: 400, message: "That isn't something to change." };
  try {
    return (await computerFacts.letDevices(value === true)) ? null : { status: 409, message: "This copy of Kuvo has no relay for your devices to connect through." };
  } catch {
    return { status: 503, message: "It couldn’t be changed just now: try again in a moment." };
  }
}

async function computerAct(context: RouteContext, at: SettingItem, action: SettingAction): Promise<Refused | null> {
  if (at.id === "state" && action.id === "restart") {
    const { status, message } = await computerFacts.restart();
    return refusal({ status, value: message ? { message } : {} });
  }
  if (at.id === "version" && action.id === "update") return refusal(await asked(context, updateAnswer, "POST", "kuvo/update", { now: true }));
  return noActions();
}


// MARK: Sections



const PAGES: Record<string, Section> = {
  clock: { read: clockPage, change: clockChange, act: noActions, look: () => `${mtime(ZONE_FILE)} ${ownClock()}` },
  conversations: {
    read: async () => ({
      id: "conversations", title: "Conversations",
      groups: [{
        id: "context", title: "Context", rows: [row({
          id: "compaction", title: "Summarize long conversations at", kind: "choice",
          line: "Smaller model windows summarize sooner. Never disables automatic summaries on large Claude Code models. Other agents may still summarize at their own limits.",
          value: compactionChoice(K), can: true,
          choices: compactionChoices.map(id => ({ id, title: id === "never" ? "Never (large windows)" : `${Number(id) / 1000}k tokens` })),
        })],
      }],
    }),
    change: async (_context, _at, value) => {
      if (!isCompactionChoice(value)) return { status: 400, message: "Choose one of the listed thresholds." };
      if (value !== compactionChoice(K)) writePrivate(compactionFile(K), JSON.stringify(value));
      return null;
    },
    act: noActions,
    look: () => compactionChoice(K),
  },
  permissions: {
    read: permissionsPage, change: permissionsChange, act: permissionsAct,
    look: () => {
      let fence = "";
      try { const { on, reason, action } = fenceState(); fence = `${on} ${reason ?? ""} ${action ?? ""}`; } catch {}
      return `${mtime(APPROVAL_FILE)} ${mtime(CONFIG_FILE)} ${mtime(ALLOWED_FILE)} ${fence}`;
    },
  },
  browser: { read: browserPage, change: browserChange, act: browserAct, look: () => browserSettingsFiles().map(mtime).join(" ") },
  git: { read: gitPage, change: gitChange, act: noActions, look: () => `${mtime(CONFIG_FILE)}` },
  worktrees: { read: worktreesPage, change: worktreesChange, act: worktreesAct, look: () => String(Math.floor(Date.now() / 1000)) },
  storage: { read: storagePage, change: storageChange, act: storageAct, look: () => `${Math.floor(Date.now() / 1000)} ${mtime(SIZES_FILE)} ${isMeasuring()}` },
  archived: { read: archivedPage, change: noActions, act: archivedAct, look: () => String(Math.floor(Date.now() / 1000)) },
  agents: agentsSection,
  vault: vaultSection,
  mcp: mcpSection,
  "custom-auto": customAutoSection,
  skills: skillsSection,
  computer: { read: computerPage, change: computerChange, act: computerAct, look: () => `${mtime(agentUpdatesFile(K))} ${mtime(MAC_LINK_FILE)} ${mtime(path.join(RUN, "permissions.json"))} ${mtime(path.join(RUN, "update.json"))} ${mtime(path.join(RUN, "update.lock"))} ${mtime(path.join(RUN, "engine.json"))} ${macLink.config?.phones ?? ""}` },
};

// Told as a change is made here, so a page followed shows it at once.
const followers = new Set<() => void>();
const changed = () => { for (const told of [...followers]) told(); };

/** Whether a value fits a row of its kind: a switch's true or false, one of a choice's choices. */
function fits(at: SettingItem, value: unknown): boolean {
  if (at.kind === "number") return typeof value === "number" && Number.isFinite(value) && (at.min === undefined || value >= at.min) && (at.max === undefined || value <= at.max);
  if (at.kind === "folder") return typeof value === "string";
  if (at.kind === "text") return typeof value === "string";
  if (at.kind === "switch") return typeof value === "boolean";
  if (at.kind === "choice") return typeof value === "string" && (at.choices ?? []).some(choice => choice.id === value);
  return false;
}

// Each section's changes here, one after another (a change waits for the one before it to end).
const turns = new Map<string, Promise<unknown>>();
function inTurn<T>(key: string, work: () => Promise<T>): Promise<T> {
  const mine = (turns.get(key) ?? Promise.resolve()).then(work, work);
  const settled = mine.catch(() => {});
  turns.set(key, settled);
  void settled.then(() => { if (turns.get(key) === settled) turns.delete(key); });
  return mine;
}

const whyNot = (can: SettingCan | undefined): string | null => (can === undefined || can === true ? null : can.why);

const idOf = (part: string | undefined): string | null => {
  try { return decodeURIComponent(part ?? ""); } catch { return null; }
};

/** `POST kuvo/settings/<section>/<row>/<action>/check {was?, values}`: a form's values checked as
 * they're typed, changing nothing, in whichever process is asked (it only reads). */
async function checked(context: RouteContext, section: Section, rowPart: string, actionPart: string): Promise<[number, unknown]> {
  if (context.request.method.toUpperCase() !== "POST") return [405, { message: "Check it with POST." }];
  const given = parsed(context.request.body);
  if (!fitsShape(SETTINGS_CHECK, given)) return [400, { message: "Send its form's values as an object (values)." }];
  const body = given as { was?: string; values: Record<string, unknown> };
  const page = await section.read(context, true);
  const at = rowsOf(page).find(each => each.id === idOf(rowPart));
  if (!at) return [404, { message: "That isn't on this page now." }];
  const action = actionsOf(at).find(each => each.id === idOf(actionPart));
  if (!action?.form) return [404, { message: "That has no form to check." }];
  if (body.was !== undefined && expectedRevision(at, action) !== body.was) return [409, { message: CHANGED_SINCE, page }];
  return [200, section.check ? await section.check(context, at, action, body.values) : { fields: [], ready: true }];
}

export async function* settingsPageRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, json, signal } = context;
  const method = request.method.toUpperCase();
  const [, sectionPart, events, rowPart, actionPart, checkPart] = match;
  if (sectionPart === undefined) {
    if (method !== "GET") return yield* json(405, { message: "Read them with GET." });
    return yield* json(200, { sections: SECTIONS.map(each => (each === COMPUTER ? { ...each, title: computerTitle(context) } : each)) });
  }
  const id = idOf(sectionPart);
  const section = id && Object.hasOwn(PAGES, id) ? PAGES[id] : undefined;
  if (!section) return yield* json(404, { message: "This computer has no settings by that name." });

  if (events) {
    if (method !== "GET") return yield* json(405, { message: "Follow it with GET." });
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    const listen = (told: () => void) => {
      followers.add(told);
      if (id === "archived") archiveCopies.followers.add(told);
      return () => { followers.delete(told); archiveCopies.followers.delete(told); };
    };
    return yield* followedLive(() => section.read(context), listen, signal, { look: section.look });
  }

  if (method === "GET" && rowPart === undefined) {
    const query = new URLSearchParams(request.path.split("?")[1]);
    if (query.get("refresh-models") === "1" && (id === "agents" || id === "custom-auto")) {
      if (yield* heldRead(request, signal)) return;
      let engine = null;
      try { engine = context.readEngine(); } catch {}
      await listsHere(engine, id === "custom-auto" ? context.bridge() : null, undefined, false, true);
    }
    return yield* json(200, await section.read(context));
  }
  if (checkPart !== undefined) return yield* json(...(await checked(context, section, rowPart!, actionPart!)));
  if (rowPart !== undefined ? method !== "POST" : method !== "PATCH") {
    return yield* json(405, { message: rowPart !== undefined ? "Do it with POST." : "Read it with GET, or change a row with PATCH." });
  }
  // Made by the box's long-running process, whichever is asked (`box route` over SSH passes it on),
  // so every change to a section is checked in turn against the page as it is then. With none
  // running (a machine with no phones, peers or vault, reached over SSH alone), here.
  if (!holdsVault()) {
    const answer = await askHolder(request, signal);
    if (answer) {
      // (A holder from before this, as the box updates, has no such route: said in words.)
      const body = answer.status === 404 && !/"message"/.test(Buffer.from(answer.body, "base64").toString())
        ? Buffer.from(JSON.stringify({ message: "Kuvo on this computer is updating: try again in a moment." }))
        : Buffer.from(answer.body, "base64");
      return yield* [{ head: { status: answer.status, headers: { "content-type": answer.type || "application/json" } } }, { body }];
    }
  }
  const body = parsed(request.body);
  const given = isObject(body) ? body : {};
  const rowId = rowPart !== undefined ? idOf(rowPart) : given.row;
  if (typeof rowId !== "string" || !rowId) return yield* json(400, { message: "Name the row to change (row)." });
  if (typeof given.was !== "string") return yield* json(400, { message: "Send the row's revision as it was read (was)." });
  const changeShape = SETTINGS_PAGE_ROUTES.find(route => route.route === "PATCH kuvo/settings/:section")!.body!;
  if (rowPart === undefined && !fitsShape(changeShape, given)) return yield* json(400, { message: "Send a row, its value and revision, and confirmed only as true or false." });
  if (rowPart !== undefined && !fitsShape(SETTINGS_ACT, given)) return yield* json(400, { message: "Send the row's revision (was), and its form's values as an object (values)." });
  // One change to a section at a time, each checked against the page as it is then: the row's own
  // revision, whether it can be changed from here, the value; then made.
  const configChanges: string[] = [];
  context = { ...context, configChanges };
  const { status, value, notAdmitted } = await inTurn(id!, async (): Promise<{ status: number; value: unknown; notAdmitted?: true }> => {
    const page = await section.read(context);
    const at = rowsOf(page).find(each => each.id === rowId);
    if (!at) return { status: 404, value: { message: "That isn't on this page now." } };
    const action = rowPart !== undefined ? actionsOf(at).find(each => each.id === idOf(actionPart)) : undefined;
    if (expectedRevision(at, action) !== given.was) return { status: 409, value: { message: CHANGED_SINCE, page } };
    let done: SettingChange | null;
    if (rowPart !== undefined) {
      if (!action) return { status: 404, value: { message: "That isn't something it does." } };
      if (action.opens !== undefined) return { status: 400, value: { message: "That opens a page: open it in the app." } };
      const why = whyNot(action.can);
      if (why) return { status: 403, value: { message: why } };
      const values = isObject(given.values) ? given.values : undefined;
      if (action.form && !values) return { status: 400, value: { message: "Fill in its form (values)." } };
      done = await section.act(context, at, action, values);
    } else {
      if (at.can === undefined) return { status: 400, value: { message: "That isn't something to change." } };
      const why = whyNot(at.can);
      if (why) return { status: 403, value: { message: why } };
      if (!("value" in given) || !fits(at, given.value)) return { status: 400, value: { message: at.kind === "choice" ? "Pick one of its choices." : (at.kind === "text" || at.kind === "folder") ? "Send text." : at.kind === "number" ? "Send a number within its limits." : "Send true or false." } };
      if (at.confirmBelow && typeof given.value === "number" && given.value < at.confirmBelow.value && given.confirmed !== true) {
        return { status: 400, value: { message: at.confirmBelow.confirm.body } };
      }
      done = await section.change(context, at, given.value, given.confirmed);
    }
    if (done && "save" in done) {
      const reply = done.save;
      if (reply.status !== 200) return reply;
      changed();
      const value = { ...await section.read(context), ...reply.value };
      return { status: 200, value } satisfies SaveReply;
    }
    if (done?.stale) return { status: 409, value: { message: done.message, page: await section.read(context) } };
    if (done && done.status !== 200) return { status: done.status, notAdmitted: done.notAdmitted, value: { message: done.message, ...(done.fields ? { fields: done.fields } : {}) } };
    changed();
    return { status: 200, value: { ...await section.read(context), ...(configChanges.length ? { configChanges } : {}) } };
  });
  return yield* (notAdmitted ? queuedRefusal({ status, value, notAdmitted }, json)! : json(status, value));
}
