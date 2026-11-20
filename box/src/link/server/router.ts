import { mutationNonce } from "../ownership/mutation";
import { ordinaryEdit, editRouteAllowed, offlineEditOutbox, offlineEditReceipt, readOfflineEdit, saveOfflineEdit } from "../ownership/edits";
import { isObject, parsed } from "../../shared/json";
import { ownerWrite, runOwnerWrite, sendOwnerWrite } from "../ownership/write";
import { mayActivate, ownerOf, DATA_ID } from "../ownership/resolve";
import { ownerMessageHere, messageReceipt, queuedDraftOutcome, queueOwnerMessage, sendAddressedMessage } from "../ownership/messages";
import { readOwned } from "../ownership/read";
import { offlineEditsReady, ownershipReady } from "../ownership/store";
import { LOCAL_SETUP_SAVE } from "../../shared/setup-local";
import { LABEL_RESTORE_ROUTE } from "../../shared/routes/work-queue";
import { GITHUB_WORK_QUEUE } from "../../shared/github";
// The box's routes
// What a phone, or this Mac's app, asks of this box: each route to the area that answers it, the
// first that takes a request answering it; what none takes goes on to the engine (`engineRoute`).
// Which route answers is by its path alone: a query string never changes it (as on a Mac).

import type { RouteFeature } from "../../shared/routes/feature";
import { processRuns as runs } from "../../shared/platform";
import { accountsRoute, credentialsRoute } from "../areas/accounts";
import { agentToolsRoute, startRoute } from "../areas/agent-tools";
import { SESSION_FOLDERS_ROUTE, SESSION_MOVE_ROUTE, sessionFoldersRoute, sessionMoveRoute } from "../areas/session-move";
import { DELETES_ROUTE, deletesRoute, SESSION_ACTION_ROUTE, sessionActionRoute, sessionMovedElsewhere } from "../areas/session-actions";
import { foundRoute } from "../areas/found";
import { searchRoute } from "../areas/search";
import { memberComputersRoute, saveLocalSetup, setupRoute } from "../areas/setup";
import { changesRoute } from "../areas/changes";
import { askAnswerRoute, asksRoute } from "../areas/asks";
import { appNoticeRoute, attentionRoute } from "../areas/attention";
import { arrivedRoute, botEventsRoute, botRoute, botSeenRoute, botsRoute, botTemplatesRoute, newBotRoute, retellRoute } from "../areas/bots";
import { CARRY_ROUTE, carryRoute } from "../areas/carry";
import { claudeCodeRoute } from "../areas/claude-code";
import { engineControlRoute, signInReturnRoute } from "../areas/engine";
import { AGENTS_START, agentsStartRoute } from "../areas/agents-down";
import { fileAt } from "../areas/file";
import { folderTreeRoute, foldersRoute } from "../areas/folders";
import { FOLDERS_SUGGESTED_ROUTE, foldersSuggestedRoute } from "../areas/folders-suggested";
import { ARTIFACT_ROUTE, artifactsRoute } from "../areas/artifacts";
import { GROUP_ROUTE, groupsRoute } from "../areas/groups";
import { infoRoute } from "../areas/info";
import { integrationsRoute } from "../areas/integrations";
import { namedRoute } from "../areas/named";
import { noticesRoute } from "../areas/notices";
import { personalitiesDocumentRoute, personalityEventsRoute, personalityRoute } from "../areas/personalities";
import { devicesPairRoute, pairingRoute, phonesRemoveRoute, phonesRoute } from "../areas/phones";
import { DESKTOP_UNINSTALL_ROUTE, desktopUninstallRoute } from "../uninstall";
import { QUEUE_ROUTE, queueRoute } from "../areas/queue";
import { SESSION_ASKS_ROUTE, sessionAsksRoute } from "../areas/session-asks";
import { GROUP_MENTIONS_ROUTE, MENTIONS_ROUTE, groupMentionsRoute, mentionsRoute } from "../areas/mentions";
import { GLANCE_ROUTE, glanceRoute } from "../areas/glance";
import { LIST_ROUTE, listRoute } from "../lists/sessions";
import { WAITING_ROUTE, waitingRoute } from "../areas/waiting";
import { COMPUTERS_ROUTE, computerNameSaveRoute, computersRoute } from "../areas/computers";
import { COMPUTERS_ADD_ROUTE, LOCAL_COMPUTER_ADD, computersAddRoute, localComputerAdd } from "../areas/computers-add";
import { SESSION_MODELS_ROUTE, sessionModelsRoute } from "../areas/session-models";
import { MODELS_OFFERED_ROUTE, modelsOfferedRoute } from "../areas/models-offered";
import { MODELS_STARRED_ROUTE, modelsStarredRoute } from "../areas/models-starred";
import { READ_ALL_ROUTE, readAllRoute } from "../areas/read-all";
import { workQueueClient, WORK_QUEUE_ROUTE, workQueueRoute } from "../areas/work-queue";
import { pluginSearchRoute } from "../areas/plugins";
import { programsRoute } from "../areas/programs";
import { backupsRoute } from "../areas/backups";
import { approvalRoute, SESSION_APPROVAL_ROUTE, SESSION_DECISIONS_ROUTE } from "../areas/approval";
import { autopilotRoute, SESSION_AUTOPILOT_ROUTE } from "../areas/autopilot";
import { DRAFT_ROUTE, draftClaimedHere, draftRoute, TYPED_ROUTE, typedRoute } from "../areas/typed";
import { readSubmissionID, SUBMISSION_FEATURE, SUBMISSION_ROUTE, SUBMIT_ROUTE, submissionsRoute, submissionOwner } from "../ownership/submissions";
import { followedSetup } from "../setup/record";
import { PARTS_ROUTE, partsRoute, SEND_ROUTE, sendRoute } from "../areas/messages";
import { sandboxRoute } from "../areas/sandbox";
import { providerModelsRoute } from "../areas/providers";
import { SIGN_IN_ACCOUNTS_ROUTE, SIGN_IN_METHODS_ROUTE, SIGN_IN_START_ROUTE, SIGN_IN_STATUS_ROUTE, legacySignInRoute, signInRoute, workQueueSignInRoute } from "../areas/sign-in";
import { answerProjects } from "../areas/projects";
import { settingsRoute } from "../areas/settings";
import { SETTINGS_PAGE_ROUTE, settingsPageRoute } from "../areas/settings-pages";
import { skillsRoute } from "../areas/skills";
import { moveReceiptRoute, moveEventsRoute, movePlacesRoute, movesRoute } from "../areas/moves";
import { sessionWakesRoute, wakeEventsRoute, wakesRoute } from "../areas/wakes";
import { worktreeEventsRoute, worktreesRoute } from "../areas/worktrees";
import { APP_ICON_ROUTE, appIconRoute, newProjectRoute, GOALS_ROUTE, goalsRoute, WORK_ASKS_ROUTE, WORK_ROUTE, workAsksRoute, workRoute } from "../areas/work";
import { phoneHere } from "../compat/leave-running";
import { peerPlaces, readBridgeFile, readEngineFile } from "../engine/client";
import { isMachinesRoute } from "../mac/machines";
import { ownMacRoute } from "../mac/own-mac";
import { readLink } from "../state";
import { ownMachinesRoute } from "../mac/own-machines";
import { linkRoute } from "../mac/routes";
import { boxUpdateRoute, updateRoute } from "../updates/update";
import { permissionsRoute } from "../permissions";
import { AT_WORK_ROUTE, atWorkRoute, stopRoute } from "../at-work";
import { rehomeRoute } from "../rehome/route";
import { appOpenRoute, appRoute } from "../mac/app";
import { proofRoute } from "../mac/app-keys";
import { serveRoute } from "../serve-kept";
import { browserRoute } from "../browser/install";
import { browserViewRoute, BROWSER_DATA_ROUTE, BROWSER_NEVER_ROUTE, browserDataRoute, browserNeverRoute } from "../browser/routes";
import { IS_MAC, OWN_HOME_MAC } from "../home";
import { vaultRoute } from "../areas/vault";
import { AGENTS, agentsRoute, engineRoute } from "./forward";
import type { BridgeRecord, EngineRecord } from "../../shared/records";
import type { NoticeHold } from "../notices/hold";
import { versionedRoutes } from "./format";
import type { Answer, AnswerPiece, Asker, LinkHandler, LinkRequest, RouteContext } from "./context";
import { clientUpdateRequired, computerUpdateRequired } from "../updates/client";
import { retryFleetRoute } from "../updates/progress";
import { dismissFleetRoute } from "../areas/computers";
import { ACCOUNT_CATALOG_ROUTE, accountCatalogRoute } from "../areas/account-catalog";
import { SIGN_IN_ACTION_ROUTE, signInActionRoute } from "../areas/sign-in-action";
import { pairedSubmitter } from "../ownership/authority";

/** One of the box's routes: `when(route, request)` says whether it takes the request, and gives
 * what its `answer(context, match)` is given: the route's match for one with a part of its own
 * (`Match` a RegExpMatchArray), else `true`. */
export interface Route<Match = true> {
  feature: RouteFeature;
  when: (route: string, request: LinkRequest, phone?: Asker) => Match | false | null;
  answer: (context: RouteContext, match: Match) => Answer;
}

// `when(route, request)`: whether it takes the request (a route's match, for one with a part of its
// own); `answer(context, match)`: its answer, as it comes.
// Their order counts where they overlap: a list and its events come before the route for one of
// its own ("events" isn't an id), as personalities', asks' and wake-ups' do.
const WORK_QUEUE_OAUTH = new RegExp(`^api/integration/${GITHUB_WORK_QUEUE}/connect/oauth(?:/att_[a-zA-Z0-9]+)?$`);
async function* backupRoutes(context: RouteContext) {
  yield* (await import("../areas/computer-backups")).computerBackupsRoute(context);
}

const ROUTES: (Route | Route<RegExpMatchArray>)[] = [
  { feature: "info", when: route => route === "kuvo/info", answer: infoRoute },
  { feature: "vault", when: route => route === "kuvo/vault" || route.startsWith("kuvo/vault/"), answer: vaultRoute },
  { feature: "sandbox", when: route => route === "kuvo/sandbox" || route.startsWith("kuvo/sandbox/"), answer: sandboxRoute },
  { feature: "file", when: route => route === "kuvo/file", answer: ({ request, json }: RouteContext) => fileAt(request.path, json) },
  // The same file as a viewer shows it: its text, or that it isn't text (`file.text`).
  { feature: "file.text", when: route => route === "kuvo/file/text", answer: ({ request, json }: RouteContext) => (request.method.toUpperCase() === "GET" ? fileAt(request.path, json, "text") : json(405, {})) },
  { feature: "file.info", when: route => route === "kuvo/file/info", answer: ({ request, json }: RouteContext) => (request.method.toUpperCase() === "GET" ? fileAt(request.path, json, "info") : json(405, {})) },
  { feature: "update.fleet", when: route => route === "kuvo/update/retry", answer: retryFleetRoute },
  { feature: "update.fleet", when: route => route === "kuvo/update/dismiss", answer: dismissFleetRoute },
  { feature: "update", when: route => route === "kuvo/update", answer: updateRoute },
  // A release from a folder here, offered by Kuvo Code (PROTOCOL.md, "Updates").
  { feature: "box.backups.status", when: route => route === "kuvo/box/backups/status", answer: backupRoutes },
  { feature: "box.backups", when: route => route === "kuvo/box/backups" || route.startsWith("kuvo/box/backups/"),
    answer: backupRoutes },
  { feature: "box.update", when: route => route === "kuvo/box/update", answer: boxUpdateRoute },
  // Moving this Mac's box into a home of its own (PROTOCOL.md, "Moving this Mac's box").
  { feature: "box.rehome", when: route => route === "kuvo/box/rehome", answer: rehomeRoute },
  // What macOS allows the program this Mac's box runs under (PROTOCOL.md, "What macOS allows").
  { feature: "box.permissions", when: route => route === "kuvo/box/permissions", answer: permissionsRoute },
  // This Mac's app, registered with a Mac's box in a home of its own, and opened for its browser
  // (PROTOCOL.md, "This Mac's app"); the box's server kept for clients ("Kept for clients").
  { feature: "box.app", when: route => route === "kuvo/box/app", answer: appRoute },
  { feature: "box.app", when: route => route === "kuvo/box/app/open", answer: appOpenRoute },
  { feature: "box.serve", when: route => route === "kuvo/box/serve", answer: serveRoute },
  { feature: "box.uninstall", when: route => route === DESKTOP_UNINSTALL_ROUTE, answer: desktopUninstallRoute },
  // What's at work here, for what's always on screen (Kuvo Helper's menu bar item), and stopping
  // Kuvo on this Mac until it next starts (PROTOCOL.md, "In the menu bar").
  { feature: "box.atWork", when: route => route.match(AT_WORK_ROUTE), answer: atWorkRoute },
  { feature: "box.atWork", when: route => route === "kuvo/box/stop", answer: stopRoute },
  // What its app asks of the keys a Mac's box in a home of its own holds (PROTOCOL.md, "Its keys").
  { feature: "box.appKeys", when: route => route === "kuvo/box/proof", answer: proofRoute },
  // (A machine's: this Mac's agents browse in its app's panel.)
  { feature: "browser", when: route => !IS_MAC && route === "kuvo/browser", answer: browserRoute },
  // Its browser's live view, a machine's (PROTOCOL.md, "The browser's live view"): a Mac's box says
  // in words why it has none.
  { feature: "browser.view", when: route => route === "kuvo/browser/view", answer: browserViewRoute },
  // Its browser's history, sites with data, and settings: every box's own (PROTOCOL.md, "The browser's data").
  { feature: "browser.data", when: route => route.match(BROWSER_DATA_ROUTE), answer: browserDataRoute },
  { feature: "browser.data.never", when: route => route.match(BROWSER_NEVER_ROUTE), answer: browserNeverRoute },
  { feature: "integrations", when: (route, request) => /^kuvo\/integrations(\/|\?|$)/.test(request.path), answer: integrationsRoute },
  { feature: "folders", when: route => route === "kuvo/folders", answer: foldersRoute },
  { feature: "folders.tree", when: route => route === "kuvo/folders/tree", answer: folderTreeRoute },
  { feature: "folders.suggested", when: route => route === FOLDERS_SUGGESTED_ROUTE, answer: foldersSuggestedRoute },
  { feature: "plugins.search", when: route => route === "kuvo/plugins/search", answer: pluginSearchRoute },
  { feature: "programs", when: route => route === "kuvo/programs", answer: programsRoute },
  { feature: "providers.models", when: route => route === "kuvo/providers/models", answer: providerModelsRoute },
  { feature: "skills", when: route => /^kuvo\/(skills|skill-library)(\/|$)/.test(route), answer: skillsRoute },
  { feature: "skills.trash", when: route => /^kuvo\/skill-trash(\/|$)/.test(route), answer: skillsRoute },
  { feature: "config", when: (route, request) => /^kuvo\/(config|mcp\/catalog|keys)(\/|\?|$)/.test(request.path), answer: settingsRoute },
  // The box's settings as it describes them, for any client to draw (PROTOCOL.md, "A box's settings").
  { feature: "settings", when: route => route.match(SETTINGS_PAGE_ROUTE), answer: settingsPageRoute },
  { feature: "setup.takeover", when: route => route === "kuvo/setup/standby", answer: setupRoute },
  { feature: "setup.handover", when: route => route === "kuvo/setup/handover", answer: setupRoute },
  { feature: "setup.merge", when: route => route === "kuvo/setup/merge", answer: setupRoute },
  { feature: "setup", when: route => route === "kuvo/setup" || route === "kuvo/setup/events" || route === "kuvo/setup/judge" || route === "kuvo/setup/attached" || route === "kuvo/setup/welcomed" || route === "kuvo/setup/ready", answer: setupRoute },
  // (A Mac's box with an app to ask answers these with it, before this: `macFront`.)
  { feature: "computers", when: route => !IS_MAC && isMachinesRoute(route), answer: memberComputersRoute },
  // A Mac's box in a home of its own answers its machines and While Agents Work itself, for a phone
  // as its app did (PROTOCOL.md, "Machines", In a home of its own).
  { feature: "machines", when: route => isMachinesRoute(route) && (OWN_HOME_MAC || IS_MAC && readLink()?.setup?.checkpointed === true), answer: ownMachinesRoute },
  { feature: "mac", when: route => OWN_HOME_MAC && route === "kuvo/mac", answer: ownMacRoute },
  { feature: "phones", when: route => route === "kuvo/phones", answer: phonesRoute },
  { feature: "phones", when: route => route === "kuvo/phones/remove", answer: phonesRemoveRoute },
  { feature: "phones.pair", when: route => route === "kuvo/phones/pair", answer: pairingRoute },
  { feature: "devices.pair", when: route => route === "kuvo/devices/pair", answer: devicesPairRoute },
  { feature: "box.ownHome", when: route => route === "kuvo/link" || route.startsWith("kuvo/link/"), answer: linkRoute },
  { feature: "attention", when: route => route === "kuvo/attention/notice" || route === "kuvo/attention/answered", answer: appNoticeRoute },
  { feature: "attention", when: route => route === "kuvo/attention", answer: attentionRoute },
  { feature: "notices", when: route => route === "kuvo/notices" || route === "kuvo/notices/events", answer: noticesRoute },
  { feature: "named", when: route => route === "kuvo/named", answer: namedRoute },
  { feature: "agent-tools", when: route => route === "kuvo/agent-tools", answer: agentToolsRoute },
  { feature: "sessions.start.empty", when: route => route === "kuvo/sessions", answer: startRoute },
  { feature: "sessions.move", when: route => SESSION_MOVE_ROUTE.test(route), answer: sessionMoveRoute },
  { feature: "projects.folders", when: route => SESSION_FOLDERS_ROUTE.test(route), answer: sessionFoldersRoute },
  // Pin, Rename, Archive and Delete, and what a session's menu offers (PROTOCOL.md, "A session's actions").
  { feature: "sessions.actions", when: route => route.match(SESSION_ACTION_ROUTE), answer: sessionActionRoute },
  { feature: "sessions.actions", when: route => route === DELETES_ROUTE, answer: deletesRoute },
  { feature: "approval", when: route => route === "kuvo/approval" || SESSION_APPROVAL_ROUTE.test(route) || SESSION_DECISIONS_ROUTE.test(route), answer: approvalRoute },
  { feature: "autopilot", when: route => SESSION_AUTOPILOT_ROUTE.test(route), answer: autopilotRoute },
  // What a typed message means, and what "/" offers (PROTOCOL.md, "What a typed message means").
  { feature: "typed.draft", when: route => route.match(DRAFT_ROUTE), answer: draftRoute },
  { feature: SUBMISSION_FEATURE, when: route => route.match(SUBMIT_ROUTE), answer: draftRoute },
  { feature: SUBMISSION_FEATURE, when: route => route.match(SUBMISSION_ROUTE), answer: submissionsRoute },
  { feature: "typed", when: route => route.match(TYPED_ROUTE), answer: typedRoute },
  // A message as fields, the box writing what the agent reads; the parts of messages from before
  // (PROTOCOL.md, "A message's parts").
  { feature: "messages.send", when: route => route.match(SEND_ROUTE), answer: sendRoute },
  { feature: "messages.send", when: route => route === PARTS_ROUTE, answer: partsRoute },
  { feature: "queue.items", when: route => route.match(QUEUE_ROUTE), answer: queueRoute },
  // What a conversation waits on you for, worded, and answering it (PROTOCOL.md, "A conversation's asks").
  { feature: "sessions.asks.box", when: route => route.match(SESSION_ASKS_ROUTE), answer: sessionAsksRoute },
  // What "@" and "&" suggest in a conversation (PROTOCOL.md, "Mentions").
  { feature: "sessions.mentions", when: route => route.match(MENTIONS_ROUTE), answer: mentionsRoute },
  { feature: "groups.mentions", when: route => route.match(GROUP_MENTIONS_ROUTE), answer: groupMentionsRoute },
  // A session at a glance: its row's and card's (PROTOCOL.md, "A session at a glance").
  { feature: "sessions.glance", when: route => route.match(GLANCE_ROUTE), answer: glanceRoute },
  { feature: "sessions.list", when: route => route.match(LIST_ROUTE), answer: listRoute },
  { feature: "waiting", when: route => route.match(WAITING_ROUTE), answer: waitingRoute },
  // Adding one over SSH, before a computer's own routes, which it starts like (PROTOCOL.md, "Computers", Adding one).
  { feature: "computers.add.credentials", when: route => route === "kuvo/computers/add/credentials" ? route.match(COMPUTERS_ADD_ROUTE) : null, answer: computersAddRoute },
  { feature: "computers.add.from-any-client", when: route => route === "kuvo/computers/add/code" ? route.match(COMPUTERS_ADD_ROUTE) : null, answer: computersAddRoute },
  { feature: "setup.merge", when: route => route === "kuvo/computers/add/merge-code" ? route.match(COMPUTERS_ADD_ROUTE) : null, answer: computersAddRoute },
  { feature: "computers.add", when: route => route.match(COMPUTERS_ADD_ROUTE), answer: computersAddRoute },
  { feature: "computers.name-save", when: route => route === "kuvo/computers/name-save", answer: computerNameSaveRoute },
  { feature: "computers", when: route => route.match(COMPUTERS_ROUTE), answer: computersRoute },
  // What a session's model button shows, and whether its model can run (PROTOCOL.md, "A session's model").
  { feature: "sessions.models", when: route => route === SESSION_MODELS_ROUTE, answer: sessionModelsRoute },
  // The models a session can use here, and the one a new session starts on (PROTOCOL.md, "The models offered").
  { feature: "models.offered", when: route => route === MODELS_OFFERED_ROUTE, answer: modelsOfferedRoute },
  // Your starred models, shared by every client's picker (PROTOCOL.md, "The models offered").
  { feature: "models.starred", when: route => route === MODELS_STARRED_ROUTE, answer: modelsStarredRoute },
  // Mark All as Read (PROTOCOL.md, "What's been read").
  { feature: "sessions.read", when: route => route === READ_ALL_ROUTE, answer: readAllRoute },
  // The Work Queue (PROTOCOL.md, "The Work Queue"), before a bot's work, whose routes it starts like.
  { feature: "work-queue", when: route => route.match(WORK_QUEUE_ROUTE), answer: workQueueRoute },
  { feature: "search", when: route => route === "kuvo/search", answer: searchRoute },
  { feature: "git.backups", when: route => route === "kuvo/git/backups" || route === "kuvo/git/backups/restore", answer: backupsRoute },
  { feature: "git", when: route => route.startsWith("kuvo/git/"), answer: changesRoute },
  { feature: "asks", when: route => route === "kuvo/asks" || route === "kuvo/asks/events", answer: asksRoute },
  { feature: "asks", when: route => route.match(/^kuvo\/asks\/([^/]+)$/), answer: askAnswerRoute },
  { feature: "engine.restart", when: route => route.match(/^kuvo\/engine\/(start|restart)$/), answer: engineControlRoute },
  // Try Again for the coding agents (PROTOCOL.md, "The coding agents' state").
  { feature: "agents.start", when: route => route === AGENTS_START, answer: agentsStartRoute },
  { feature: "sign-in.methods", when: (route, request) => (request.method === "GET" && route === `api/integration/${GITHUB_WORK_QUEUE}`) ||
    WORK_QUEUE_OAUTH.test(route), answer: workQueueSignInRoute },
  { feature: "sign-in.methods", when: (route, request, phone) => {
    if (!phone?.key || phone.kind === "desktop" || request.method !== "POST") return false;
    return /^(kuvo\/agents\/)?api\/integration\/([^/]+)\/connect\/oauth$/.exec(route);
  }, answer: legacySignInRoute },
  { feature: "sign-in.return", when: route => route === "kuvo/sign-in/return", answer: signInReturnRoute },
  { feature: "accounts.catalog", when: route => route === ACCOUNT_CATALOG_ROUTE, answer: accountCatalogRoute },
  { feature: "sign-in.action", when: route => route === SIGN_IN_ACTION_ROUTE, answer: signInActionRoute },
  { feature: "sign-in.methods", when: route => route === SIGN_IN_METHODS_ROUTE || route === SIGN_IN_STATUS_ROUTE || route === SIGN_IN_ACCOUNTS_ROUTE || route === SIGN_IN_START_ROUTE, answer: signInRoute },
  { feature: "claude-code", when: route => route === "kuvo/claude-code", answer: claudeCodeRoute },
  { feature: "agents", when: (route, request) => request.path.startsWith(AGENTS), answer: agentsRoute },
  { feature: "accounts.ended", when: route => route === "kuvo/accounts/ended" || route === "kuvo/accounts/refreshable" || /^kuvo\/accounts\/[^/]+\/(forget|refresh)$/.test(route), answer: accountsRoute },
  { feature: "accounts.credentials", when: route => route === "kuvo/accounts/credentials", answer: credentialsRoute },
  { feature: "accounts.found", when: route => route === "kuvo/accounts/found" || route === "kuvo/accounts/found/use", answer: foundRoute },
  { feature: "personalities.events", when: route => route === "kuvo/personalities/events", answer: personalityEventsRoute },
  { feature: "personalities", when: route => route === "kuvo/personalities/document" || route === "kuvo/personalities/take", answer: personalitiesDocumentRoute },
  { feature: "personalities", when: route => route.match(/^kuvo\/personalities(?:\/([^/]+))?$/), answer: personalityRoute },
  { feature: "bots.make", when: route => route === "kuvo/bots/templates", answer: botTemplatesRoute },
  // (A new bot; reading them is `botsRoute`, further on.)
  { feature: "bots.make", when: (route, request) => request.method.toUpperCase() === "POST" && route === "kuvo/bots", answer: newBotRoute },
  // The box's projects, as the sidebar and Edit Project change them (PROTOCOL.md, "Projects").
  { feature: "work.projects", when: route => route === "kuvo/projects/new", answer: newProjectRoute },
  { feature: "projects", when: route => route === "kuvo/projects" || route === "kuvo/projects/events", answer: ({ request, readEngine, readBridge, signal }: RouteContext) => answerProjects(request, readEngine, readBridge, signal) },
  { feature: "worktrees.events", when: route => route === "kuvo/worktrees/events", answer: worktreeEventsRoute },
  { feature: "worktrees", when: route => /^kuvo\/worktrees(\/(remove|restore|record|where))?$/.test(route), answer: worktreesRoute },
  // A bot's work (PROTOCOL.md, "Work"), its stop-all before a bot's own route.
  { feature: "work", when: route => route.match(WORK_ROUTE), answer: workRoute },
  { feature: "work.projects", when: route => route.match(WORK_ASKS_ROUTE), answer: workAsksRoute },
  { feature: "work.goals", when: route => route.match(GOALS_ROUTE), answer: goalsRoute },
  { feature: "work.goals.apps", when: route => route === APP_ICON_ROUTE, answer: appIconRoute },
  // Bots' own keys, which no box keeps now (PROTOCOL.md, "Bots have no keys of their own"): said in
  // words, for a client from before that still asks.
  { feature: "bots", when: route => route === "kuvo/bots/keys/refresh" || /^kuvo\/bots\/[^/]+\/keys(\/[^/]+)?$/.test(route), answer: ({ json }: RouteContext) => json(404, { message: "Bots use this computer's integrations now." }) },
  { feature: "sessions.agents", when: route => route.match(CARRY_ROUTE), answer: carryRoute },
  { feature: "bots.edit", when: (route, request) => ["PATCH", "DELETE"].includes(request.method.toUpperCase()) && route.match(/^kuvo\/bots\/([^/]+)$/), answer: botRoute },
  { feature: "groups", when: route => GROUP_ROUTE.test(route), answer: groupsRoute },
  { feature: "artifacts", when: route => ARTIFACT_ROUTE.test(route), answer: artifactsRoute },
  { feature: "bots", when: route => route.match(/^kuvo\/bots\/([^/]+)\/arrived$/), answer: arrivedRoute },
  { feature: "bots.state", when: route => route.match(/^kuvo\/bots\/([^/]+)\/seen$/), answer: botSeenRoute },
  { feature: "bots.elsewhere", when: route => route === "kuvo/bots/retell", answer: retellRoute },
  { feature: "bots.events", when: route => route === "kuvo/bots/events", answer: botEventsRoute },
  { feature: "bots", when: route => route === "kuvo/bots", answer: botsRoute },
  { feature: "moves", when: route => route === "kuvo/moves", answer: movesRoute },
  { feature: "moves", when: route => route === "kuvo/moves/events", answer: moveEventsRoute },
  { feature: "moves", when: route => route === "kuvo/moves/places", answer: movePlacesRoute },
  { feature: "moves", when: route => route === "kuvo/moves/receipt", answer: moveReceiptRoute },
  { feature: "wakes", when: route => route === "kuvo/wakes", answer: wakesRoute },
  { feature: "wakes.events", when: route => route === "kuvo/wakes/events", answer: wakeEventsRoute },
  { feature: "wakes.edit", when: route => route.match(/^kuvo\/wakes\/([^/]+)(?:\/(cancel|now|pause|watch|events))?$/), answer: sessionWakesRoute },
];

/** The first dispatcher entry that accepts this route and request. */
export function matchBoxRoute(route: string, request: LinkRequest, phone?: Asker) {
  for (const entry of ROUTES) {
    const match = entry.when(route, request, phone);
    if (match) return { entry, match };
  }
  return null;
}

// The handler LinkHost runs for each request. `hold`: the running bridge's notices held while
// you're at a computer (none in `box route`, a command of its own). `info`: every box passes its
// name; only a test's handler leaves it out (and its `kuvo/info` has none), so its `{}` is cast.
export function engineHandler(readEngine: () => EngineRecord = readEngineFile, info: RouteContext["info"] = {} as RouteContext["info"], readBridge: () => BridgeRecord = readBridgeFile, phonesChanged: RouteContext["phonesChanged"] = () => {}, hold: NoticeHold | null = null): LinkHandler {
  // The agents' bridge, while it runs: its record, while the process it names is still it.
  const bridge = (): BridgeRecord | null => {
    try {
      const found = readBridge();
      return found?.port && found?.password && runs(found.pid, "kuvo-agents") ? found : null;
    } catch {
      return null;
    }
  };
  return versionedRoutes(async function* (request, phone, signal) {
    const text = (status: number, words: string): AnswerPiece[] => [{ head: { status, headers: { "content-type": "text/plain" } } }, { body: Buffer.from(words) }];
    const json = (status: number, value: unknown): AnswerPiece[] => [{ head: { status, headers: { "content-type": "application/json" } } }, { body: Buffer.from(JSON.stringify(value)) }];
    // A phone using this machine, whatever it asks; not the app beside a Mac's own box.
    const route = request.path.split("?")[0];
    const holderLabelRestore = request.fromHolder && route === LABEL_RESTORE_ROUTE;
    if (!request.fromApp && !holderLabelRestore) phoneHere();
    // (What only the link says of a session: never a request's own word.)
    if (phone?.kind === "desktop") request = { ...request, desktop: true };
    if (!request.internal && !holderLabelRestore && (request.fromApp || request.fromMac) && (request.served || route !== "kuvo/info" || request.headers["x-kuvo-client-version"])
      && !["kuvo/box/app", "kuvo/box/update", "kuvo/box/proof"].includes(route)) {
      const refusal = clientUpdateRequired({ app: request.headers["x-kuvo-client-version"], client: "desktop" });
      if (refusal) return yield* json(426, refusal);
    }
    const method = request.method.toUpperCase();
    if (!request.internal && ((!request.fromHolder && (phone?.key || request.fromApp || request.fromMac))
      || (holderLabelRestore && method === "POST"))) {
      const behind = computerUpdateRequired(route, method);
      if (behind) return yield* json(503, behind);
    }
    const context: RouteContext = { request, phone, signal, route, readEngine, readBridge, bridge, info, phonesChanged, hold, json, text };
    const desktop = request.desktop || request.fromMac || request.fromApp;
    if (route === "kuvo/data/edits" || route.startsWith("kuvo/data/edits/")) {
      if (method !== "POST" && !editRouteAllowed(context)) return yield* json(403, { message: "Open this from a paired Kuvo Code client." });
      if (!offlineEditsReady()) return yield* json(503, { message: "Wait for Kuvo Code to finish updating." });
      if (route === "kuvo/data/edits" && method === "POST") return yield* await saveOfflineEdit(context, peerPlaces());
      if (route === "kuvo/data/edits" && method === "GET") return yield* await readOfflineEdit(context, peerPlaces());
      if (route === "kuvo/data/edits/outbox" && method === "GET") return yield* offlineEditOutbox(context);
      if (route !== "kuvo/data/edits" && method === "GET") return yield* offlineEditReceipt(context, route.slice("kuvo/data/edits/".length));
      return yield* json(405, {});
    }
    const paired = phone?.key && pairedSubmitter(context, readLink());
    if (route === "kuvo/data/owner" || route === "kuvo/data/messages" || route.startsWith("kuvo/data/messages/")) {
      if (!(desktop || paired)) return yield* json(403, { message: "Open this from Kuvo Code." });
      if (route === "kuvo/data/messages" && request.method.toUpperCase() === "POST")
        return yield* sendAddressedMessage(context);
      if (route === "kuvo/data/messages" || request.method.toUpperCase() !== "GET") return yield* json(405, {});
      return yield* (route === "kuvo/data/owner" ? await readOwned(context) : messageReceipt(context, route.slice("kuvo/data/messages/".length)));
    }
    if (ownershipReady() && !["GET", "HEAD"].includes(request.method.toUpperCase())) {
      const body = parsed(request.body);
      const proposal = isObject(body) && ordinaryEdit(context, body);
      if (proposal) {
        if (!offlineEditsReady()) return yield* json(503, { message: "Wait for Kuvo Code to finish updating." });
        return yield* await saveOfflineEdit(context, peerPlaces(), proposal);
      }
      if (desktop || paired) {
        const write = ownerWrite(request.method.toUpperCase(), route, body);
        const nonce = write ? mutationNonce(request, body) : undefined;
        if (nonce === null) return yield* json(400, { message: "Send a valid operation id." });
        const owner = write && ownerOf(write.ref, { name: info.name });
        if (write && owner && !owner.local) {
          const result = await sendOwnerWrite(write, body, owner, context, nonce);
          return yield* json(result.status, result.value);
        }
        if (write && nonce && workQueueClient(context)) {
          if (!mayActivate(write.ref)) return yield* json(409, { message: "This record is held while its owner changes." });
          const result = await runOwnerWrite(write, body, context, nonce);
          return yield* json(result.status, result.value);
        }
      }
      const session = /^(?:(?:kuvo\/agents\/)?api\/session|kuvo\/sessions|kuvo\/wakes)\/(ses_[A-Za-z0-9_-]+)(?:\/|$)/.exec(route)?.[1];
      const group = /^kuvo\/groups\/(grp_[a-z0-9]+)(?:\/|$)/.exec(route)?.[1];
      if ((session || group) && DATA_ID.test((session ?? group)!)) {
        const ref = { kind: session ? "conversations" as const : "groupTranscript" as const, id: session ?? group };
        const scoped = session && route.match(SUBMIT_ROUTE);
        const draft = session && method === "POST" && (scoped || route.match(DRAFT_ROUTE));
        if (draft && (desktop || paired)) {
          const known = queuedDraftOutcome(context, session!, body);
          if (known) return yield* known;
          if (draftClaimedHere(session!, body, Boolean(scoped))) return yield* draftRoute(context, draft);
          if (scoped) {
            const pin = isObject(body) && readSubmissionID(body.submission);
            if (!pin || !isObject(body) || body.id !== pin.id) return yield* json(400, { message: "Use the original owner and stable submission ID." });
            const link = readLink();
            if (followedSetup(link)?.id !== pin.setup || submissionOwner(link, session!) !== pin.origin)
              return yield* json(409, { ambiguous: true, message: "Look up that submission on its original computer." });
          }
        }
        const owner = ownerOf(ref, { name: info.name });
        if (owner && !owner.local) {
          if ((desktop || paired) && method === "POST" && session && (SEND_ROUTE.test(route) || draft))
            return yield* queueOwnerMessage(context, session, owner);
          if ((desktop || paired) && method === "POST" && group && route === `kuvo/groups/${group}`)
            return yield* queueOwnerMessage(context, group, owner);
          if (session && owner.key && SESSION_ACTION_ROUTE.test(route))
            return yield* json(409, sessionMovedElsewhere(owner.key, owner.name === owner.key ? "" : owner.name));
          return yield* json(409, { computer: owner.key, message: `Open this record on ${owner.name} to change it.` });
        }
        if (!mayActivate(ref) && owner?.local)
          return yield* json(409, { message: "This record is held while its owner changes." });
        if ((desktop || paired) && method === "POST" && (session && SEND_ROUTE.test(route) || group && route === `kuvo/groups/${group}`)) {
          const result = await ownerMessageHere(context, (session ?? group)!);
          if (result) return yield* result;
        }
      }
    }
    if (route === LOCAL_COMPUTER_ADD) return yield* localComputerAdd(context);
    if (route === LOCAL_SETUP_SAVE) return yield* saveLocalSetup(context);
    const matched = matchBoxRoute(route, request, phone);
    // Each answer receives its own predicate's match, a pairing TypeScript cannot follow.
    if (matched) return yield* matched.entry.answer(context, matched.match as never);
    return yield* engineRoute(context);
  });
}
