# #713 piece 4: what a 0.6 / 0.7.2 / 0.8.0-beta.1 Mac keeps, and where each thing goes

Read-only inventory (2026-11-15) for the box-side move (`box/src/link/rehome`, `rehome --run`).
Nothing was run or built; everything is from reading tags `v0.6.6`, `v0.7.2`, `v0.8.0-beta.1`
(`KuvoCode/`, `Packages/`) and this branch (`swift/`, `box/`).

- `S` = `~/Library/Application Support/run.kuvo.code`, `K` = `~/.kuvo-code`.
- Preferences = `~/Library/Preferences/run.kuvo.code.plist` (UserDefaults domain `run.kuvo.code`).
  The box already reads that file with `/usr/bin/plutil -convert xml1 -o - <file>` in
  `box/src/link/browser/from-app.ts:44-68` (a precedent for the upgrades below; a `Data` value is
  `<data>` base64 holding compact JSON).
- "suggest" classes: `box upgrade` (the move must carry it), `already moved` (cloned by `BESIDE`
  and read by the box as is), `client's own` (the retired app's UI state; the desktop app keeps its
  own, so the old value is simply not carried), `dropped` (not carried, with what is lost).
- Parts 5 to 7 are the two sub-reports this inventory was built from, kept whole: part 6 has the
  port-ready algorithm of every Swift upgrade, part 5 the engine facts and the old keeper's steps.
  Where part 6 and the tables disagree, the tables are the later check (one known case:
  `passwords.never` / `passwords.offersToSave` DO reach the box, through `browser/from-app.ts`).

Method and its limits: keys were swept with `git grep` for `UserDefaults`, `@AppStorage`,
`defaults.`, `forKey:` and key-name constants, then diffed between versions. Keys built from
pieces (a tuple, an interpolated string) can escape that sweep: `engine.botMemoryRuleAdded` on this
branch was found only by reading. AppKit's and Sparkle's own keys in the same domain
(`NSWindow Frame …`, `NSSplitView …`, `SU…`) were not enumerated (unverified names; all client's own).

---

## 1. Counts

Table A, v0.7.2 (a family such as `analytics.run.<id>` counts once): 127 keys.

| suggest | keys |
| --- | --- |
| box upgrade | 36 (18 to port, 4 the box already takes itself with the browser's data, 14 usage panel settings) |
| already moved | 0 (no preference is moved by `BESIDE`) |
| client's own | 69 |
| dropped | 22 |

Table B, v0.7.2, entries directly in `S` (the 7 files of `S/Usage` counted singly): 35.

| suggest | entries |
| --- | --- |
| already moved | 13 (`opencode/{data,config,state,cache,workspace}`, `engine`, `integrations`, `bots/`, `wakes/`, `Chats`, `agents`, `phone-link.json`, `Attachments`) |
| box upgrade | 7 (`bots.json`, `groups.json`, `groups/` in its old shape, `personalities.json`, `agent-messages.json`, `Browser/` (built), `opencode-files/` (proposed)) |
| dropped | 15 (`machine-groups.json`, `machine-groups/`, `engine-linux`, `browser-linux`, `bin/kuvo-wake`, `Backups/`, `e2e-engine.json`, `*.unreadable-*.json`, `Usage/` x7) |

beta.1 adds 5 files (3 box upgrade, 2 dropped); 0.6.6 lacks `Backups`, `machine-groups*`.

---

## 2. Table A: preferences

Versions: `6` = 0.6.6, `7` = 0.7.2, `b` = 0.8.0-beta.1, `m` = this branch. Unless said, a key is
written by 6, 7, b and m with the same shape. File:line is v0.7.2 `KuvoCode/…` unless prefixed.

### 2.1 User data and settings that change behaviour

| key | shape | what | on this branch | suggest |
| --- | --- | --- | --- | --- |
| `projectFolders` | string array, sidebar order; bots' folders filtered on read (`Stores/Workspace.swift:148`) | the projects you opened | `swift/KuvoCode/Stores/Workspace.swift:1838-1853`, once per box (`projectsOnBox.<box>`): `PUT kuvo/projects?directory=` | **box upgrade** → `K/box/projects.json` `{"version":1,"projects":[{"directory","name"?,"folders":[],"run"?,"removed"?:true}]}` (`box/src/link/areas/projects.ts:26,258`). Only entries not starting `ssh://` (a machine's are that machine's). Paths under `S` need the move's path map. |
| `projectSettings` | `Data` JSON `{"<directory>": {"name"?, "folders": [..], "run"?, "terminal"?}}` (`:199-209`; `terminal` not in 6) | names, extra folders, Run command | same hand-over; a settings-only project goes as `removed: true` | **box upgrade** → same file. `terminal` has no place in the box (stripped, `projects.ts:50`): say it. |
| `worktrees` | `Data` JSON `{"<folder sessions run in>": {"project","root","branch","base"?}}` (`:213-222`, `:813-818`) | sessions' worktrees | `swift/…/Workspace.swift:1486-1512`: `POST kuvo/worktrees/record {"project","root","folder","branch","base"?}` | **box upgrade** → `K/box/worktrees.json` `worktrees: [{"project","root","folder","branch","base"?}]` (`areas/worktree-records.ts:13,108`); only roots that exist; path map applied (0.7.2 worktrees are very likely under `S/opencode/data/opencode/worktree/…`, unverified). |
| `removedWorktrees` | dictionary `{folder: project}` (`:161`) | where archived sessions of a removed worktree belong | `swift/…/Workspace.swift:474-490`: `POST kuvo/worktrees/record {"removed":true,"folder","project"}` | **box upgrade** → `K/box/worktrees.json` `handedOver: {folder: project}` |
| `machines` | `Data` JSON `[{"destination","otherAddresses"?:[..],"identity"?,"reachedAt"?}]` (`Stores/Machines.swift:8-17,555,886`) | your machines (SSH hosts) | `swift/…/Machines.swift:1789-1840`: written to `box/machines.json` (`OwnBoxHome.file`) | **box upgrade** → `K/machines.json`, the same array, merged by `destination` (`box/src/link/mac/machines.ts:20`). Named by no doc. |
| `git.branchPrefix`, `git.commitInstructions`, `git.pullRequestInstructions` | strings (`Stores/GitBranches.swift:219-222`); prefix unset = `"kuvo/"` | Settings › Git | `swift/…/GitBranches.swift:183`: `POST kuvo/git/settings/move` | **box upgrade** → keys `branch_prefix`, `commit_instructions`, `pull_request_instructions` in `K/config/opencode/opencode.json`, only where absent, then `K/box/git.json` `{"moved":true}`; call `moveGitSettings(given, file, lock, gitFile)` (`areas/git-words.ts:120`) |
| `keepsAwake` | Bool, unset = on (`Stores/Awake.swift:8,11`) | Keep This Mac Awake | `swift/…/Keeper/OwnBoxMove.swift:129` → `--keeps-awake off` | **box upgrade** → `K/box/work.json` `{"keepsAwake":false}`; `rehome/move.ts:516` does it from the flag, so `--run` must read the pref itself |
| `toolApproval` | String `"approveAll"` \| `"askFirst"`, unset = `approveAll` (`Views/Session/ComposerMenus.swift:6-12`) | default approval of new sessions, bots, seats | referenced nowhere (gone) | **box upgrade** for `askFirst` → `K/box/approval.json` `{"last":"askFirst"}` (`work/approval.ts:35`); `approveAll` has no exact counterpart (box default `auto`): the user's call, say it |
| `engine.defaultsAdded`, `engine.searchMovedToExa` | Bool each (`Stores/EngineConfig.swift:77-78`) | "web search added once", "random replaced by Exa once" | `swift/…/EngineConfig.swift:120-124` `defaultsAdded()` → marks `websearch`, `search-exa` (m also has `engine.botMemoryRuleAdded` → `bot-memory`) | **box upgrade** → `K/box/defaults-added`, one mark a line (`areas/config.ts:179-201`). Without it a `websearch` setting the user took out comes back as Exa. |
| `groupMemoryWaiting` | dictionary `{botID: [chatID…]}` (`Stores/BotGroups.swift:1526`) | chats a bot is still to forget | `GroupMigration.run` | **box upgrade** → `K/groups/pending-memory.json` (merged) |
| `movesUnderWay` | `Data` JSON `[{"kind":"bot"\|"chat"\|"group","id","sessions":[..],"from"?,"to"?,"original"?,"copy"?,"wasPaused","stage":"copying"\|"switched"}]` (`Stores/Moves.swift:80-106`; not in 6) | moves between computers under way | read by `GroupMigration` for `"held"`, then removed | **box upgrade** (input only: a group chat named there is written `"status":"held"`); a move left at `switched` is finished by nothing: say it |
| `passwords.agentSignIn`, `passwords.agentSignInAccounts` | string array of hosts; dictionary `{host: account}` (`Stores/Passwords.swift:28-35`) | sites agents may sign in to alone | `swift/…/Passwords.swift:137-171` with the Keychain's logins | **box upgrade** → vault grants `{everyone:true}` `how:"own"`; needs the running box (signed files, Keychain values) and an `owed: passwords` step that does not exist yet (part 6 g) |
| `passwords.never`, `passwords.offersToSave` | string array; Bool default true (`:19-25`) | the browser's password offers | **the box takes them itself**: `box/src/link/browser/from-app.ts:64-68,96-104` | **box upgrade (built)** → `K/browser/settings.json`; needs `readRecord(K).from` = `S` and the plist still there |
| `browser.searchEngine`, `panel.tidiesAgentTabs` | String; Bool default true (`Stores/PanelSettings.swift:17,34`) | the browser's search engine; closing agents' tabs | same take-in (`from-app.ts:60-63`) | **box upgrade (built)** → `K/browser/settings.json` |
| `phoneLink` | Bool, default off, set only by `defaults write` (`Stores/PhoneLink.swift:21`) | "Let my devices connect" | m: `PUT kuvo/link/config {"phones","machines","relay","name","app"}` | **box upgrade** (or decision) → `K/run/mac-link.json` `{"phones","relay","name","app"}` (`mac/link.ts:46-51`). With no file a box in its own home starts with `phones: true` (`ownMacConfig`, `mac/link.ts:91-101`): a change for every 0.7 user, where it was off. PROTOCOL.md "For released Macs": the phone link was never on in a release, so no paired phones to carry. |
| `phoneLink.relay` | String URL (`:102-104`) | a relay of your own | same | **box upgrade** → `relay` in the same file (else the default relay) |
| `botEditsWaiting` | dictionary `{botID: ["instructions"\|"name"\|"model"…]}` (`Stores/Bots.swift:104`) | edits to a machine's bot made while it was away | `takeOldWaitingEdits` → `botEditOutbox` → `PATCH kuvo/bots/<id>?on=` | **dropped** unless handed to `holdEdit` once `serve` runs: `run/bots-held/<bot>.json` is sealed with the box's link secret, so it cannot be written as a file. Loses: a name/look/personality/model change not yet sent to a machine. |
| `copilotClaudeModels` | string array (`Stores/ProviderCompatibility.swift:13`) | learned model ids | removed (`swift/…/App/AppModel.swift:688`) | **dropped**: each box learns it |
| `updates.betas` (7, b, m) | Bool, unset = whether this build is a beta (`Stores/Updates.swift:27-29`) | beta updates | client's | **client's own**; the box's update channel is its own (unverified whether the desktop app asks again) |
| `analytics.enabled` | Bool, registered default true (`Stores/Analytics.swift:29,86`) | the opt-out | client's | **client's own**, but a privacy choice: if the desktop app sends anything, an opt-out here should be honoured (decision) |

### 2.2 Bookkeeping and caches (dropped)

| key | shape | what is lost |
| --- | --- | --- |
| `remoteSessions` | `Data` JSON `{sessionID: {"machine","title"?}}` (`Workspace.swift:72-75,1434`) | nothing: a cache of sessions on machines that are away |
| `machines.waiting` | `{machineID: [script…]}` (`Machines.swift:574-579`) | clean-ups still owed to machines that weren't reachable (a removed machine's files, a deleted bot's folder there) |
| `machines.waitingSince` | `{machineID: Date}` | with the above |
| `machines.unreadable-<unix seconds>` (7, b, m) | `Data`, a `machines` value that wouldn't decode, set aside (`:897-907`) | nothing the app could read; say if one exists |
| `machineClientID` | String UUID (`:237`) | this Mac's name to engines it started on machines; boxes go by link keys now (unverified that nothing still needs it) |
| `machineCopies` | `{machineID: {integration: "<mac>\|<machine>"}}` (`App/AppModel+SignIns.swift:187`) | which sign-in copy each machine has; re-learned |
| `machineSignOuts` | `{machineID: [account…]}`, `keys:` prefix for keys only (`:246-251`) | sign-outs a machine hasn't taken yet: an account signed out on the Mac may stay signed in on a machine that was away |
| `botInstructions` | `{sessionID: sha256 hex}` (`Bots.swift:846`) | nothing: a send-once cache; unused on m |
| `calibration` (7, b, m), `calibration.backedUp` | Int (`Stores/Calibration.swift:31,34`) | nothing; **read it**: 7/b = 1 means each Mac bot has `bot.json`; unset (6) means none has |
| `usage.agentProviders` | string array | nothing: cache of the bridge's accounts |

### 2.3 The retired app's own UI state (client's own; nothing in the box)

What the user notices: unsent drafts, starred models and the last model picked are not there in
the desktop app unless it imports them (it reads none of these today: no `CFPreferences` /
`Library/Preferences` read in `gpui/`).

| key | shape | what |
| --- | --- | --- |
| `composerDrafts` | `Data` JSON `{"<session or project key>": {"text","files":[URL],"quote"?,"quotes":[{"id","text","response"}],"comments":[ReviewComment]}}` (`Stores/Drafts.swift:10-61`) | typed and not sent |
| `composerDraftsSending` | `Data` JSON `{id: {"key","draft",…,"at"}}` | a draft on its way as the app quit |
| `commandMessages` | array of `[messageID, command]`, newest 500 (`Stores/CommandMessages.swift:10-18`; m: `swift/KuvoClient/…/CommandMessages.swift:11`) | shows "/review main" instead of the long prompt |
| `favoriteModels` | String (`Views/Session/ModelPicker.swift:40`) | starred models |
| `lastModelRef`, `lastModel` | String JSON `{"id","providerID","variant"?}`; older `provider/model/variant` (`Views/Session/SessionView.swift:463-495`) | the model new sessions start with |
| `newSessionWorktree` | Bool | new session in a worktree, last choice |
| `changes.pushAfterCommit` | Bool | Commit and push, last choice |
| `notifications.enabled`, `notifications.sound`, `notifications.badge` | Bool, default true (`Stores/Notifier.swift:14-27`) | Mac notifications |
| `appearance`, `appearance.accent`, `appearance.window`, `appearance.textSize`, `appearance.font` | String / Double (`App/AppInfo.swift:74`, `Design/Look.swift:7-10`) | look |
| `transcript.steps`, `showsTabBar`, `pairStyle`, `settings.tab`, `settings.tabs`, `settings.previewTab`, `sessionsWidth`, `changesWidth`, `splitWidth`, `collapsedProjects`, `collapsedSections`, `openFiles.editor` | String / Bool / Double | window and layout |
| `panel.startsWith`, `panel.revealsForAgent`, `panel.newTab.showsTools`, `panel.newTab.showsSuggestions`, `panel.newTab.suggestionCount` | String / Bool / Int (`Stores/PanelSettings.swift:6-14`) | the side panel |
| `moveToApplications.declined` | Bool | the "move to Applications" ask |
| `analytics.noticeShown`, `.installID`, `.installDate`, `.activeDay`, `.version`, `.usageDay`, `.internal`, `.onboarded`, `.onboardingSteps`, `.accountConnected`, `analytics.run.<process>`, old `analytics.running`, `.launchedAt`, `.aliveAt` | Bool / String / Date / dictionary (`Stores/Analytics.swift:29-46`) | analytics' own |
| system keys read, never written: `AppleFontSmoothing` (registered default 0), `AppleActionOnDoubleClick` | | |

### 2.4 The usage panel (UsageKit; same keys in 6, 7, b)

`swift/KuvoCode/Stores/UsageSettingsOnBox.swift:162-175` brings these to the box once
(`POST api/kuvo/usage/settings/import`, body from `HostSettings.toImport()`,
`swift/UsageKit/…/Embedding/HostSettings.swift:82-96,205-235`). The box keeps them in the coding
agents' bridge's folder, `K/agents/usage/settings.json` =
`{"codex"?, "kuvo":1, "units": {"<part>": {"value", "stamp": {"v","by","imported"?:true}}}, "imported"?:true, "setup"?}`
(`box/src/agents/usage/settings.ts:284-320,438-452,475-481`; `take(given)` is the import: once,
only parts no box has set, each checked). Parts: `layout`, `totalSpend`, `notifications`,
`codexFallback` (`providers` is deliberately not brought).

| key | shape | suggest |
| --- | --- | --- |
| `openusage.layout.v1` (`Data` JSON, placed widgets), `.providerOrder`, `.metricOrderByProvider`, `.seededDefaults` (`Data` JSON), `.menuBarPins`, `.expandedMetrics`, `.expandOnEnable` (string arrays) | → part `layout` = `{"placed":[descriptorID],"providerOrder":[],"metricOrder":{},"pinned":[],"expanded":[],"expandOnEnable":[],"seeded":[]}`; left out when there is no arrangement | **box upgrade** → `K/agents/usage/settings.json` through `UsageSettings.take` |
| `showTotalSpend` (Bool, default true), `openusage.totalSpend.period`, `openusage.totalSpend.metric` (String) | → `totalSpend` = `{"show","period"?,"metric"?}`, only if one of the three is set | **box upgrade** |
| `openusage.notifications.underTenPercent`, `.healthyToClose`, `.closeToRunningOut` (Bool, default false) | → `notifications`, only if one is set | **box upgrade** |
| `openusage.codex.fallbackModel` (String) | → `codexFallback` | **box upgrade** |
| `openusage.settings.schemaVersion` (Int) | the panel's own migrations (`App/SettingsMigrator.swift`): metric ids in the layout keys are renamed by its steps, so a layout from an old schema may name ids the box's check drops (unverified which 0.6.6 / 0.7.2 schema numbers differ from the branch's) | dropped; read it before trusting `layout` |
| `openusage.enabledProviders.v1`, `openusage.knownProviders.v1`, `openusage.disabledProviders.v1` | string arrays: which cards are on | dropped by design (`HostSettings.swift:78-84`) |
| `openusage.providerAccounts.v1` | `Data` JSON `[{"id","family","identityKey","label"?,"sources":[{"kind","anchor"?,"holdsDefaultSource"}],"removedTombstone"}]` | dropped: re-derived at launch |
| `openusage.providerSnapshots.v9`, `openusage.shellEnvSnapshot.v2`, `kuvo.usage.unavailableProviders.v1`, `kuvo.usage.connectedProviders.v1`, `copilot.billingOrg` | caches | dropped |
| `openusage.layout.v1.expandedProviders`, `.menuBarStyle`, `openusage.onboarding.welcomePending`, `.customizeHintPending`, `openusage.panel.height.dashboard` / `.customize` / `.settings`, `density`, `timeFormat`, `logLevel`, `hideUsageWhileScreenSharing`, `increaseTransparency`, `reduceAnimations`, `meterStyle`, `resetDisplayMode`, `alwaysShowPacing`, `dashboardStyle`, `barMetric` (and `appearance`, shared with the app) | how the panel looks on this Mac | client's own |

### 2.5 Differences in other versions

- **0.6.6**: no `calibration`, `calibration.backedUp`, `machines.unreadable-*`, `movesUnderWay`,
  `updates.betas`; `projectSettings` has no `terminal`. Everything else as 0.7.2.
- **0.8.0-beta.1** adds only client's own keys: `settings.selectedTab`, `sidebarBox`,
  `sidebarSwipeReverses`. Nothing 0.7.2 has is gone.
- **This branch only** (in no release, so the move meets them only on a Mac that ran a later
  build): hand-over bookkeeping `projectsOnBox.<box>`, `projectsUnsent`,
  `removedWorktreesHandedOver.<box>`, `personalities.takenFromOld`, `passwords.movedToVault`,
  `machines.unsaved` (a machines list not yet written to the box: read it before `machines`),
  `botEditOutbox` (`Data` JSON `{botID: {"box","fields","delete"}}`), `machineUnsentViews`,
  `usage.settingsUnsent`, `usage.hostSettings.said`, `engine.botMemoryRuleAdded`; caches
  `usage.boxServices`, `usage.boxHistoryServices`; client's own `backgroundWork.noteSeen`,
  `.noteOpened`, `.relaunchInBackground`, `made.seen`, `notifications.alwaysSeen`,
  `onboarding.step`, `openedSubSessions`, `openedReviewParts`, `reviewLayout`, `reviewerModel`,
  `windowTabs`, `voiceStyle`, `voice.tip.shown`, `voice.whisper.on` / `.paused` / `.cellular`.
  The keys 0.7.2 has that the branch no longer references: `toolApproval`, `botInstructions`
  (and `engine.defaultsAdded` / `engine.searchMovedToExa` only as the marks above).

### 2.6 What is NOT in preferences in 0.7.2 (and so needs nothing)

- Pinned and archived sessions: the engine session's metadata, `"kuvo.pinned": true`,
  `"kuvo.archived": true` (`Stores/Workspace.swift:345-362`), plus the engine's `time.archived`.
  The box reads the same (`box/src/link/lists/rule.ts:45,48`).
- Unread marks: the engine session's `time.viewed` against `time.idle` (`Workspace.swift:996,1990`).
- Session titles, per-session model, messages: the engine's database.
- Permissions, MCP servers and their on/off, the Auto model list, providers, commands, skills:
  the engine's settings `S/opencode/config/opencode/opencode.json` (+ `skills/`), moved whole.
- Accounts (provider sign-ins): the engine's own data folder, moved whole.
- No sandbox setting, no voice setting, no time-zone setting exists in 0.7.2 (a bot's folder has
  `timezone` and `zoneinfo` files, written by the app at each start, `Stores/Bots.swift:521-530`).

---

## 3. Table B: files, folders, Keychain

### 3.1 In `S`

| path | shape | versions | what | moved by `BESIDE`? | on this branch | suggest |
| --- | --- | --- | --- | --- | --- | --- |
| `opencode/data` | `opencode/opencode.db` (+ `-wal`, `-shm`), `opencode/snapshot/`, `opencode/log/`, worktrees | all | the engine's data (XDG_DATA_HOME) | yes → `K/data` | – | **already moved**; database is engine 2.0.32's, brought up to date in staging (commit `55a3fc502`) |
| `opencode/config` | `opencode/opencode.json`, `opencode/skills/`, `opencode/plugins/kuvo-code.js` | all | the engine's settings | yes → `K/config` | – | **already moved**; see surprises 7 and 8 for what the settings can name outside it |
| `opencode/state`, `opencode/cache`, `opencode/workspace` | the engine's | all | XDG_STATE_HOME, XDG_CACHE_HOME (+ npm, bun), the engine's cwd | yes | – | **already moved** |
| `engine/` | `<version>/opencode` (`2.0.32`) | all | the engine binary | yes → `K/engine` | – | **already moved** (the box brings 2.2.6) |
| `integrations/` | `<id>/…` and `<id>.json`: what integrations' servers keep (a sign-in) (`App/AppModel.swift:252`, `Views/Settings/IntegrationsSettings.swift:263,327`) | all | KUVO_INTEGRATIONS | yes → `K/integrations` | box: `INTEGRATION_DATA` (`home.ts:144`) | **already moved** |
| `bots/<bot_id>/` | `MEMORY.md`, `workspace/`, `wakes/`, `session`, `directory`, `via`, `timezone`, `zoneinfo`, `groups/<chat>.md`, `paused`; `bot.json` only from 0.7.2 (Calibration step 1) | all | each bot's folder | yes → `K/bots` | `describeOld` writes missing `bot.json` | **already moved**, but a bot without `bot.json` (every 0.6.6 bot; a 0.7.2 bot if step 1 never ran) is invisible to the box (`bots/here.ts:44-56`): the upgrade writes it from `bots.json` |
| `bots.json` | array of `{"id","name","job","instructions","jobInMemory"?,"look":{"color","shape","face"},"machine"?,"project"?,"folder","sessionID","created","model"?:{"id","providerID","variant"?},"personality"?,"agents"?,"updated"? (not in 6)}`, ISO dates, pretty (`Stores/Bots.swift:10-42,111`) | all | the list of bots (this Mac's and machines') | **no** | read by `keepJobs` at `path.dirname(BOTS)/bots.json` (`box/src/link/bots/jobs.ts:72-80`) | **box upgrade**: clone to `K/bots.json` (so a job kept only in `job`/`instructions` is taken once) and write each Mac bot's `bot.json`, `session`, `directory`, `via` where missing |
| `wakes/<sessionID>/` | `session`, `directory`, `via`, `paused`, one file per wake-up named `<due seconds>-<letters>[-<repeat>[@…]]` holding its note, watches (`Packages/OpenCodeKit/…/Engine/Wakes.swift`) | all | sessions' wake-ups (KUVO_WAKES) | yes → `K/wakes` | same `kuvo-wake` family; only `--summary` and stricter number parsing were added since | **already moved** (`directory` rewritten by `rehome/own.ts:40`) |
| `groups.json` | array of `BotGroup` (part 6 a) | all | this Mac's group chats | **no** (it is beside the folder) | `GroupMigration.run` | **box upgrade** → `K/groups/<id>/group.json` `{"kuvo":1,"status","group"}` |
| `groups/<grp_id>.json` | array of `GroupMessage` | all | each chat's messages | the folder moves, in the OLD shape | `GroupMigration.run` | **box upgrade** → `K/groups/<id>/messages.json`. **Loud**: the box loads only `groups/<id>/group.json` folders (`box/src/agents/groups/store.ts:217-236`); unconverted, every chat is gone from view. |
| `personalities.json` | array of `{"id":"custom.<uuid>","name","description","symbol"}`; b adds `"updated"` | all | your personalities | no | `Personalities.moveToBox` | **box upgrade** → `K/bots/personalities.json` `{"kuvo":1,"personalities":[…,"updated":0],"removed":[]}` |
| `personalities-removed.json` | `[{"id","removed"}]` | b | removals | no | same | **box upgrade** (b) |
| `agent-messages.json` | `[{"from"?,"name","to","machine","text"}]` (`Stores/AgentMail.swift:30-38`) | all | messages waiting for an agent on a machine that was away | no | `AgentMail.handOver` → `kuvo/link/letters/take` | **box upgrade** → `K/run/outbox/sending/<id>.json`; from 0.7.2 no machine has `peers: true` in `phone-link.json`, so by the Swift rule none would go: a decision (send by the bare `key`, or say they were not carried) |
| `agent-letters.json`, `agent-letters-seen.json` | part 6 e | b | letters and seen marks | no | same | **box upgrade** (b) → `run/outbox/sending/`, `run/letters-seen.json` |
| `agents-elsewhere.json`, `machine-bots.json` | b: announce cache; `{botID: Data}` of what each machine's bot looked like (`v0.8.0-beta.1:KuvoCode/Stores/Bots.swift:1085-1088`) | b | caches | no | deleted / unused | **dropped**: nothing lost |
| `phone-link.json` | `{"phones":[{"key","name","added","address"?}],"machines":{machineID:{"name","address","key","anotherMac"?}}}` (`Stores/PhoneLink.swift:29-37,82-95`); m adds `peers`, `sign`, `removed` | all | machines' bridges' keys; paired phones | yes → `K/phone-link.json` | box reads only `.machines` there (`mac/machines.ts:23,51`); phones are the box's `run/link.json` | **already moved**. `phones` in it is read by nothing in the box: fine while no release ever had the phone link on (PROTOCOL.md "For released Macs"). |
| `Chats/` | folders of chats (sessions of no project) | all | where chats run | yes → `K/chats` | `CHATS` (`home.ts:178`) | **already moved** |
| `Attachments/` | a folder per session: pasted images, converted copies (`Stores/Attachments.swift:29`) | all | files sent with messages | yes, merged into `K/attachments` | `attachmentRoots()` | **already moved** |
| `agents/` | the coding agents' bridge's folder (`Stores/OutsideAgents.swift:142`): sessions, agents' homes | all | KUVO_AGENTS_HOME | yes → `K/agents` | – | **already moved** (the box adds `usage/` in it) |
| `Browser/` | `history.json` `[{"host","url","title","visits","lastVisit"}]` (dates in seconds since 2001), `Icons/<host>.png` (`Stores/BrowserHistory.swift:8-26`) | all | the panel browser's history | no | – | **box upgrade (built)**: `takeInAppBrowserData` (`browser/from-app.ts:73-106`) reads it from `S` once → `K/browser/history.json`, `K/browser/icons/`. Needs `S/Browser` still there when the box first starts, and `readRecord(K).from`. |
| `opencode-files/` | `<place>-<name>` copies of files an imported opencode's settings named with `{file:…}` (`Stores/OpencodeImport.swift:178-226`) | all | secrets and prompts the engine's settings read | **no** | – | **box upgrade (proposed)**: the moved `opencode.json` still says `{file:S/opencode-files/…}`; it works only while `S` exists. Move it (e.g. `K/opencode-files`) and add it to the path map, or say it. |
| `machine-groups.json`, `machine-groups/<id>.json` | `[{"discussing","group","held","machine","paused","speaking"?}]`; messages | 7, b | cache of machines' group chats | no | `BoxGroups.takeOldCache` → the window's cache | **dropped**: the truth is on each machine; lost only for a machine that is gone for good |
| `Backups/` | `<date> <version>/…` copies + `settings.plist` (`Stores/Calibration.swift:96-143`) | 7, b | the app's own backups, newest 3 | no | – | **dropped** (stays in `S`) |
| `engine-linux/`, `browser-linux/` | downloads for machines | all | caches | no | – | **dropped**: fetched again |
| `bin/kuvo-wake` | script written at each launch | all | the wake-up command | no | the box's release brings its own | **dropped** |
| `e2e-engine.json` | tests' | all | – | no | – | **dropped** |
| `<name>.unreadable-<when>[-n].json` | a file that wouldn't decode, set aside (`Stores/JSONFile.swift:6-22`) | all | what a damaged list held | no | – | **dropped**; say when one exists (it is the user's data in a shape nothing reads) |
| `Usage/pricing/`, `Usage/log-scan-cache/`, `Usage/cursor-usage-history/`, `Usage/antigravity/auth.json` | UsageKit's caches (`Packages/UsageKit/…/Support/HostApp.swift:16-23`) | all | re-read | no | – | **dropped**: the box keeps its own in `K/agents/usage` (how far back Cursor's history can be read again: unverified) |
| `Usage/openrouter.json`, `Usage/zai.json`, `Usage/proxy.json` | user-written: an API key (`apiKey`/`api_key`/`key`), `{"proxy":{"enabled","url"}}` | all | optional files a user may have made by hand | no | still UsageKit's | **dropped**: rare, hand-made; say if present |

### 3.2 Outside `S`

| place | what | suggest |
| --- | --- | --- |
| `~/Library/Caches/run.kuvo.code/Conversations/` | a copy of each session as last seen (`Stores/SessionCopies.swift:14-18`) | client's own cache |
| `~/Library/Caches/run.kuvo.code/…` (Sparkle, URL cache), `~/Library/HTTPStorages/run.kuvo.code` | the app's | client's own |
| `~/Library/WebKit/run.kuvo.code` (`WKWebsiteDataStore.default()`, `Views/Settings/BrowserSettings.swift:128`) | the panel browser's cookies and site data: where you were signed in | **dropped**: every site sign-in in the browser is lost (the box's browser is a Chromium with its own profile); 0.7.2 also copied these cookies to machines (`Stores/SiteSignIns.swift:21`) |
| `~/Library/Logs/Kuvo Code/` (`Usage.log`, the app's) | logs | dropped |
| `$TMPDIR/run.kuvo.code/integration-keys/KUVO_KEY_*` | key files the engine's settings name with `{file:…}` (`Stores/IntegrationKeys.swift:43-49`), rewritten each start, gone at quit | not moved; see surprise 8 |
| `$TMPDIR/run.kuvo.code/{agents,engine,…}`, `$TMPDIR/kuvo-*` | scratch | dropped |
| Keychain, generic password, service `run.kuvo.code.phone-link`, accounts `key`, `secret` (label "Kuvo Code phone link", `Stores/PhoneLink.swift:1368-1398`) | this Mac's link key and relay secret; made the first time the link is on | carried by the keys hand-over (`handOverKeys`, `kuvo-keys copy` → `run.kuvo.code.box.phone-link`) |
| Keychain, generic password, service `run.kuvo.code.integrations`, account `KUVO_KEY_<ID>_<NAME>` (label "Kuvo Code integration key", `Stores/IntegrationKeys.swift:11-30`) | integrations' keys | carried by the keys hand-over |
| same service, account `KUVO_KEY_<ID>_<NAME>__<BOT>` | bots' own keys | **dropped** by the box itself (`dropBotKeys`, `box/src/link/keys.ts:129-143`; #726); the bot's servers naming one are taken out and noted in `<bot>/keys-gone.json` |
| Keychain, internet passwords, creator `'KUVO'` (0x4B55564F), server = host, account = username, description "Kuvo Code browser password" (`Stores/Passwords.swift:76-145`) | every saved sign-in | **box upgrade** → the box's vault (`addItem` + `setGrant`); only `kuvo-keys passwords` can read them, and no `owed: passwords` step exists |
| Keychain, generic password, service `run.kuvo.code.usage-api-key`, account = provider id (label "Kuvo Code API key (<provider>)", `Packages/UsageKit/…/Providers/APIKeySecrets.swift:14-19`) | API keys pasted into the usage panel | **dropped, and named by no doc**: the keys hand-over copies only `phone-link`, `integrations`, `vault`; nothing in `box/` or `mac/` reads this service. The user pastes them again, if the desktop app's usage view takes keys at all (unverified). |
| Keychain, service `run.kuvo.code.vault` | – | not in any release (no vault before this branch) |
| Login item | b only: `SMAppService.mainApp` (`v0.8.0-beta.1:KuvoCode/Stores/OpenAtLogin.swift:14-25`); none in 6, 7 | the old app would still open at login until unregistered; only the app itself can unregister `mainApp` (unverified whether anything else can) |

---

## 4. Every `box upgrade`, source → target

Paths under `S` in any value go through the move's path map (`rehome/paths.ts`).

1. **Projects**: prefs `projectFolders` (order) + `projectSettings` → `K/box/projects.json`
   `{"version":1,"projects":[{"directory","name"?,"folders":[],"run"?,"removed"?:true}]}`;
   `name` dropped when empty or the folder's basename, `folders` absolute and deduplicated,
   settings-only entries `removed: true`, `ssh://` entries skipped, `terminal` dropped and said.
2. **Worktrees**: pref `worktrees` → `K/box/worktrees.json` `worktrees[]` (roots that exist);
   pref `removedWorktrees` → `handedOver{}` in the same file.
3. **Machines**: pref `machines` (+ `machines.unsaved`) → `K/machines.json`, merged by `destination`.
4. **Git**: three `git.*` prefs → `moveGitSettings` → `K/config/opencode/opencode.json` keys +
   `K/box/git.json {"moved":true}`.
5. **Keep awake**: pref `keepsAwake == false` → `K/box/work.json {"keepsAwake":false}`.
6. **Approval**: pref `toolApproval == "askFirst"` → `K/box/approval.json {"last":"askFirst"}`;
   `approveAll` is the user's call.
7. **Defaults added**: prefs `engine.defaultsAdded`, `engine.searchMovedToExa` (+ branch
   `engine.botMemoryRuleAdded`) → `K/box/defaults-added` lines `websearch`, `search-exa`, `bot-memory`.
8. **Link**: prefs `phoneLink`, `phoneLink.relay` → `K/run/mac-link.json`
   `{"phones","relay","name","app"}` (decision: the box's own default is `phones: true`).
9. **Group chats**: `groups.json`, `groups/<id>.json`, prefs `groupMemoryWaiting`,
   `movesUnderWay`, `bots.json` → `K/groups/<id>/messages.json`, `K/groups/<id>/group.json`
   `{"kuvo":1,"status":"active"|"held","group"}`, `K/groups/pending-memory.json`,
   `groups/imported.json`; seats on machines' bots come out (orphan seats: no box reader).
10. **Bots**: `bots.json` → `K/bots.json` (for `keepJobs`) and, per Mac bot, `bot.json`
    `{"kuvo":1,"id","name","job","look","model"?,"personality"?,"project"?,"created","updated"}`,
    `session`, `directory`, `via` where missing.
11. **Personalities**: `personalities.json` (+ b `personalities-removed.json`) →
    `K/bots/personalities.json` `{"kuvo":1,"personalities":[{"id","name","description","symbol","updated"}],"removed":[{"id","removed"}]}` through `mergePersonalities`.
12. **Letters**: `agent-messages.json` (+ b `agent-letters.json`, `agent-letters-seen.json`) with
    `phone-link.json`'s `machines[*].key` → `K/run/outbox/sending/<id>.json`
    `{"from","to","text","wait":false,"at","peer","answers"?}` and `K/run/letters-seen.json`.
13. **Saved sign-ins**: Keychain internet passwords (creator `'KUVO'`) + prefs
    `passwords.agentSignIn`, `passwords.agentSignInAccounts` → the vault, once `serve` runs:
    `addItem({kind:"login", name, sites:[host], username, signInOnly:true, values:{password}})`,
    then `setGrant(id, {everyone:true}, "own")` where the host and its account match. A password
    under 4 characters is refused by the vault (`vault/store.ts:361`).
14. **Usage panel settings**: the `openusage.layout.v1*`, total spend, notification and Codex
    fallback prefs → `K/agents/usage/settings.json` through `UsageSettings.take`.
15. **Browser (built)**: `S/Browser/` + four prefs → `K/browser/{history.json,icons,settings.json}`
    by `takeInAppBrowserData`; the move only has to leave `S/Browser` and the plist readable and
    keep `from` in its record.
16. **`opencode-files/` (proposed)**: move and map, or say.

---

## 5. Surprises: what would be lost with nobody having listed it

1. **Projects** (`projectFolders`, `projectSettings`) are in neither §8a of the #713 note nor
   PROTOCOL.md "For released Macs". Unported, the sidebar's projects, their names, extra folders
   and Run commands are gone (sessions survive in the database).
2. **The machines list** is a preference in every release. Unported, a 0.7 user has no machines.
3. **`toolApproval`** is carried by nothing on this branch: an "Ask first" user becomes `auto`.
4. **`engine.defaultsAdded`**: without the marks file the box adds Exa web search again to a user
   who took it out, and treats a `{"provider":"random"}` setting as its own to replace.
5. **Usage API keys** (Keychain service `run.kuvo.code.usage-api-key`) and hand-made
   `S/Usage/{openrouter,zai,proxy}.json` are copied by nothing.
6. **Usage panel settings** reach the box only through the retired app's import; the cards'
   order, pins, total spend and notification switches reset otherwise.
7. **`S/opencode-files/`** is named by absolute path inside the moved `opencode.json`.
8. **Integration key files**: 0.7.2's `opencode.json` names keys as
   `{file:$TMPDIR/run.kuvo.code/integration-keys/KUVO_KEY_…}`. The box has a pattern for older
   namings (`olderKeysNamedHere`, `box/src/link/areas/config.ts:166`), but whether a box in a home
   of its own rewrites these to its own `box/keys` files as it starts was not traced: **verify**,
   or every integration with a key fails after the move.
9. **Group chats in the old shape** move with the `groups` folder and are invisible until converted.
10. **Bots without `bot.json`** (all of 0.6.6's) are invisible; and with `bots.json` left behind, a
    job kept only in the list's `job` / `instructions` is not taken.
11. **Orphan seats**: `GroupMigration` takes machines' bots' seats out of a chat and leaves their
    sessions for the app to delete as each machine connects; nothing in `box/` reads
    `orphan-seats.json`. Ported as is, those sessions stay on the machines.
12. **The panel browser's site sign-ins** (WebKit cookies) do not carry; saved passwords do, but
    only with an `owed: passwords` step that is not written.
13. **Drafts, starred models, last model** are the old client's and are read by nothing.
14. **`phoneLink`**: off for every released user; a box in its own home turns "Let my devices
    connect" on by default.
15. **beta.1's login item** (`SMAppService.mainApp`) keeps opening the retired app.
16. **A move left at `switched`** (`movesUnderWay`) leaves its original folder where it was.
17. **`machineSignOuts` / `machines.waiting`**: sign-outs and clean-ups still owed to machines that
    were away are forgotten.
18. From part 5: `moveRefusal` wants `S/box/run/served.json` and `runningIn` reads only `S/box/run`,
    so a running 0.7 app and its engine are invisible to the box's checks (`heldOpen` in the new
    `run-steps.ts` looks at who has the database open, which may already cover it).

---

## Answers 3 and 4, compactly

**3. Bots in 0.7.2.** The list is `S/bots.json` (shape in Table B). Each of this Mac's bots also
has a folder `S/bots/<bot_id>/`: `MEMORY.md` (its memory, with a `## Job` section when
`jobInMemory`), `workspace/` (where it works unless it has a `project`), `wakes/` (its wake-ups,
same files as a session's), `session` (its session id), `directory` (where it works, a path),
`via` (`bridge` when it runs with the coding agents), `timezone` and `zoneinfo` (written at each
start), `groups/<chat>.md` (what it remembers of each group chat), `paused` (`paused` or
`moving`), and from 0.7.2 `bot.json` (Calibration step 1; `{"kuvo":1,"id","name","job","look",
"model"?,"personality"?,"project"?,"created","updated"}`). A session's wake-ups are in
`S/wakes/<sessionID>/`. The box reads the same folder (`botHere`, `box/src/link/bots/here.ts:44-56`:
needs `bot.json` with `kuvo === 1`, the matching `id`, a `name`, and a non-empty `session`), and the
same wake-up files. It needed two Swift-run upgrades: `describeOld` (`swift/KuvoCode/Stores/Bots.swift:703-725`,
Calibration steps 1 and 3) for `bot.json` and `session`, and `GroupMigration.describeBots` for
`directory` and `via`; and it reads `bots.json` beside the bots folder itself, once per bot, for a
job from before (`jobs.ts:46-80`). `job.json` (`{"line","details","updated"}`) is the box's and is
in no release.

**4. Integrations, vault, link, machines in 0.7.2.**
- Integrations' (MCP servers') configuration: the engine's settings,
  `S/opencode/config/opencode/opencode.json` (`mcp` servers; keys written as `{file:…}`). What
  their servers keep: `S/integrations/<id>/…`, `S/integrations/<id>.json`. Their keys: Keychain
  generic passwords, service `run.kuvo.code.integrations`, account `KUVO_KEY_<ID>_<NAME>`
  (a bot's own: `…__<BOT>`), written to `$TMPDIR/run.kuvo.code/integration-keys/` at each start.
- Vault: none in any release, beta.1 included. Saved sign-ins are Keychain internet passwords
  with creator `'KUVO'`.
- Link: `S/phone-link.json` (`phones`, `machines`), prefs `phoneLink` and `phoneLink.relay`,
  Keychain service `run.kuvo.code.phone-link`, accounts `key` and `secret`.
- Machines and SSH hosts: the preference `machines`
  (`[{"destination","otherAddresses"?,"identity"?,"reachedAt"?}]`; `destination` is what `ssh`
  is given, `identity` a key file's path). There is no `machines.json` in a release. Each
  machine's bridge key is in `phone-link.json`'s `machines`. SSH's own files (`~/.ssh`) are the
  user's and are not the app's.

---

## Part 5. Sub-report: the engine in each release (answer 1) and the old keeper's move (answer 5)

### #713 rehome inventory: Q1 (engine facts per release) and Q5 (the old keeper's move)

Worktree: `/Users/serhatandic/.kuvo-code/data/opencode/worktree/c5e2b5/shiny-squid`.
S = `~/Library/Application Support/run.kuvo.code` (`AppInfo.supportDirectory` =
`URL.applicationSupportDirectory.appending(path: Bundle.main.bundleIdentifier ?? "run.kuvo.code")`,
`v0.6.6:KuvoCode/App/AppInfo.swift:44-47`; only `isBeta` was added to that file by v0.8.0-beta.1).

Tag paths are `<tag>:<path>:<line>`. Where a file is byte-identical in all three tags, only v0.6.6 lines are given.
`git diff --stat` over `Packages/OpenCodeKit/Sources/OpenCodeKit/Engine/` shows only `RemoteEngine.swift`
(0.6.6→0.7.2) and `BoxFiles.swift`, `RemoteEngine.swift`, `Wakes.swift` (0.7.2→0.8.0-beta.1) changed, so
`ServerProcess.swift`, `EngineInstaller.swift`, `CommandEnvironment.swift`, `ShellEnvironment.swift`, `Launcher.swift`
are identical in v0.6.6, v0.7.2 and v0.8.0-beta.1.

---

#### Q1

### (a) Engine version, download, install place — identical in all three tags

- Version constant: `public static let version = "2.0.32"` — `Packages/OpenCodeKit/Sources/OpenCodeKit/OpenCodeKit.swift:5`
  (same line and value in v0.6.6, v0.7.2, v0.8.0-beta.1).
- `EngineRelease.pinned` (`Engine/EngineInstaller.swift:20-25`):
  - tarball `https://registry.npmjs.org/@opencode/cli-darwin-arm64/-/cli-darwin-arm64-2.0.32.tgz`
  - integrity `sha512-wASwXBsWaOAYcZrh4ZLl4oaVxPCUAGVb7zZvXBBF8TjWE4uWgY0D2tj6gn80JfcO3b/s9woM6dP/gKqYKK2EdQ==`
    (npm `dist.integrity`, SHA-512 base64, checked in `verifyIntegrity`, lines 181-191)
  - `teamIdentifier: "5NZ4Q7NXJ4"`; signature requirement
    `anchor apple generic and certificate leaf[subject.OU] = "5NZ4Q7NXJ4"` with `kSecCSCheckAllArchitectures` (lines 194-204).
  - Only a darwin-arm64 pin exists in the tags (no darwin-x64). Linux pins (for machines over SSH): x64
    `…/cli-linux-x64-2.0.32.tgz`, `sha512-Q50ofBQ3…JitDA==`; arm64 `…/cli-linux-arm64-2.0.32.tgz`, `sha512-YR5O03ir…NR5w==` (lines 29-48).
- Install place: `EngineInstaller(root: AppInfo.supportDirectory.appending(path: "engine"))`
  (`v0.6.6:KuvoCode/App/AppModel.swift:173`, `v0.7.2:…:185`, `v0.8.0-beta.1:…:183`); executable is
  `root/<version>/opencode` (`EngineInstaller.swift:106-108`) → **`S/engine/2.0.32/opencode`**.
  - Unpacked into `S/engine/.staging-<UUID>/` (`package/bin/opencode` inside the tarball), moved into place, staging removed (lines 124-142).
  - `removeOtherVersions()` deletes every other entry of `S/engine` after an install (lines 220-225), so only one version folder is normally there.
  - `installed()` returns it only if executable and the signature still verifies (lines 111-115).
  - Override: env `KUVO_OPENCODE_BINARY` runs another binary (`AppModel.swift:194` v0.6.6 / `:209` v0.7.2 / `:212` v0.8.0-beta.1).
- Other engine-related folders under S in the tags: `S/engine-linux` (Linux engine cache for machines,
  `AppModel+Machines.swift:253` v0.6.6), `S/browser-linux`.

### (b) How the app starts the engine — `ServerProcess.swift` identical in all three tags

`EngineFolders(root: AppInfo.supportDirectory.appending(path: "opencode"))`
(`AppModel.swift:150-152` v0.6.6, `:160-162` v0.7.2, `:158-160` v0.8.0-beta.1) → root = `S/opencode`.

`EngineFolders` (`Packages/OpenCodeKit/Sources/OpenCodeKit/Engine/ServerProcess.swift:8-63`):

| Variable | Value | Line | What the engine adds |
|---|---|---|---|
| `XDG_DATA_HOME` | `S/opencode/data` | 29 | `opencode/` → `S/opencode/data/opencode/` (`dataDirectory`, line 21) |
| `XDG_CONFIG_HOME` | `S/opencode/config` | 30 | `opencode/` → `S/opencode/config/opencode/opencode.json` (`configFile`, line 25) |
| `XDG_STATE_HOME` | `S/opencode/state` | 31 | `opencode/` (engine's own; unverified in Swift, no Swift code reads it) |
| `XDG_CACHE_HOME` | `S/opencode/cache` | 32 | `opencode/` (unverified in Swift); also `kuvo-npx/` (Launcher script) |
| `npm_config_cache` | `S/opencode/cache/npm` | 35 | |
| `BUN_INSTALL` | `S/opencode/cache/bun` | 36 | |
| `TMPDIR` | `<FileManager.temporaryDirectory>/<bundle id>/engine` + `"/"` | 39, 46-48 | NOT under S (the user temp folder, `…/T/run.kuvo.code/engine/`) |

- `create()` (lines 50-62) makes every path above plus `workspace`; removes `S/opencode/cache/tmp` (an older test build's);
  deletes entries of the temporary folder older than 3 days.
- cwd: `folders.workspace` = **`S/opencode/workspace`** (line 18; `directory: folders.workspace`, line 127). Kept empty on purpose.
- argv: **`["serve", "--stdio", "--port", "0"]`** (line 126). The engine picks a free port, prints `{"url": …}` as its first stdout
  line (handshake, lines 220-238), and exits when stdin closes (comment lines 80-84).
- Environment build order (lines 119-125): `base` (below) → `KUVO_OWN_<name>` for each of
  `XDG_DATA_HOME, XDG_CONFIG_HOME, XDG_STATE_HOME, XDG_CACHE_HOME, npm_config_cache, BUN_INSTALL` = the user's own value or `""`
  (`CommandEnvironment.swift:15,18,29-31`) → the `EngineFolders` variables above →
  `OPENCODE_PASSWORD` = random 32 bytes, base64url without padding (lines 117, 122, 240-247) →
  `OPENCODE_CLIENT = "desktop"` (line 124) → `OPENCODE_SERVER_PASSWORD` removed (line 125).
- Password/port: never written to a file under S by the released apps (the endpoint is kept in memory:
  `ServerEndpoint(url:, password:)`, line 205). Only DEBUG `E2E.note` writes `S/e2e-engine.json` (`KuvoCode/App/E2E.swift:11`).
  No pid file and no lock file for the engine exist in any of the three tags
  (`git grep '\.pid"|\.lock"|flock\('` in `KuvoCode/*.swift`: no hits in all three).
- Stop: `stop()` closes stdin; `process.terminate()` (SIGTERM) after 5 s if still running (lines 209-216).
- `base` environment = `ShellEnvironment.load()` (the user's login shell env via `$SHELL -ilc "/usr/bin/env -0"`, then `-lc`,
  else the app's own; `ShellEnvironment.swift:13-25`) with, set in `AppModel.start()`
  (v0.6.6 `AppModel.swift:218-262`; v0.7.2 `:234-277`; v0.8.0-beta.1 `:237-280`):
  - `PATH` = `S/bin` + `":"` + shell PATH (`Bots.command.deletingLastPathComponent()`; `Bots.command` = `WakeUps.command` = `S/bin/kuvo-wake`,
    `WakeUps.swift:41`; `Launcher.install` puts `kuvo-npx` (+ `runtime/node`) in `S/bin`)
  - `KUVO_BOTS` = `S/bots` (`Bots.swift:81` v0.6.6 / `:113` later)
  - `KUVO_WAKES` = `S/wakes` (`WakeUps.swift:45`)
  - `KUVO_INTEGRATIONS` = `S/integrations`
  - `OPENCODE_CONFIG_CONTENT` = merged JSON (ProviderCompatibility config, browser tools' config), only when non-empty
  - **HOME is not set by the app**: it is whatever the login shell has (the user's real home). No `OPENCODE_CONFIG`,
    `OPENCODE_DISABLE_*` or other `OPENCODE_*` is set for this Mac's engine in the three tags (the only `OPENCODE_*` names in
    `ServerProcess.swift` are `OPENCODE_PASSWORD`, `OPENCODE_CLIENT`, `OPENCODE_SERVER_PASSWORD`).
- `CommandEnvironment.install` writes the plugin **`S/opencode/config/opencode/plugins/kuvo-code.js`** each start
  (`ServerProcess.swift:116`, `CommandEnvironment.swift:26,63-69`).
- Differences between the tags in this area: none in `ServerProcess.swift`/`EngineFolders`; `AppModel.start()` sets the same
  variables in all three (only line numbers move).

### (c) Engine files the Swift app names directly

- Database: `StorageReport.engineData` = `support.appending(path: "opencode/data/opencode")`
  (`KuvoCode/Views/Settings/StorageSettings.swift:230`, same line in all three tags); files
  **`["opencode.db", "opencode.db-wal", "opencode.db-shm"]`** (line 246) →
  **`S/opencode/data/opencode/opencode.db`** (+ `-wal`, `-shm`).
  - The engine names its database by release channel (`opencode.db` stable, `opencode-<channel>.db` others) per
    `Packages/UsageKit/Sources/UsageKit/Providers/OpenCode/OpenCodePaths.swift:23-26,54` (globs `opencode*.db`); the app's own code
    hardcodes `opencode.db`. Whether a 2.0.32 engine ever wrote another name there: unverified.
  - Read by UsageKit: `usage.start(engineData: engineFolders.dataDirectory)` → `HostAccounts.setOpenCodeDataDirectories([engineData.path])`
    (`AppModel.swift:157` v0.6.6; `UsageAccounts.swift:25-26`).
- Snapshots: `S/opencode/data/opencode/snapshot` (`StorageSettings.swift:248`).
- Engine logs: `S/opencode/data/opencode/log` (`StorageSettings.swift:249, 288`; `deleteLogs` removes all but the newest and truncates that one).
- Settings: `S/opencode/config/opencode/opencode.json` (`EngineConfig(file: engineFolders.configFile)`,
  `AppModel.swift:47` v0.6.6 / `:57` later); skills in `S/opencode/config/opencode/skills` (`AppModel.swift:55` / `:65`);
  the whole `S/opencode/config` is counted with sessions (`StorageSettings.swift:247`).
- `auth.json`: the app does **not** read or write `S/opencode/data/opencode/auth.json`. `OpencodeImport` reads the *user's own*
  opencode: `configFolder` = `$XDG_CONFIG_HOME/opencode` else `~/.config/opencode`; `dataFolder` = `$XDG_DATA_HOME/opencode` else
  `~/.local/share/opencode`; files `["opencode.json", "opencode.jsonc", "config.json"]` and `dataFolder/auth.json`
  (`v0.7.2:KuvoCode/Stores/OpencodeImport.swift:53-66, 74, 297`). Its import engine runs with a throwaway root
  `<tmp>/<bundle id>/opencode-import-<UUID>` (lines 105-107) and env `["PATH": "/usr/bin:/bin", "HOME": home.path]` (line 122).
  Copied files land in `S/opencode-files` (line 180).
- Worktrees: no Swift constant names `S/opencode/data/opencode/worktree`; the engine puts them there (the box's
  `home.ts:145-147` comment says the engine's data is "where new worktrees go"). Path under S in the tags: unverified from Swift.
- `state`, `cache`: only created (`EngineFolders.create`); nothing in Swift reads inside them.
- Per-bot settings: `<bot folder>/.opencode/opencode.json` (`KuvoCode/Stores/BotKeys.swift:7,80,125`).
- v0.7.2 and v0.8.0-beta.1 only (machines, not this Mac): `$K/data/opencode/opencode.db` read with `bun:sqlite`
  (`v0.7.2:KuvoCode/Stores/Machines.swift:685-686`, `v0.8.0-beta.1:…:827-828`).

Other things directly under S in the tags (from `supportDirectory.appending(path:)` greps), for the move's plan:

| Under S | v0.6.6 | v0.7.2 | v0.8.0-beta.1 |
|---|---|---|---|
| `opencode/{data,config,state,cache,workspace}`, `engine/`, `engine-linux/`, `browser-linux/`, `integrations/`, `bin/kuvo-wake`, `bots.json`, `bots/`, `wakes/`, `groups.json`, `groups/`, `Chats/`, `Attachments/`, `Browser/`, `agents/`, `agent-messages.json`, `opencode-files/`, `phone-link.json`, `e2e-engine.json` (DEBUG) | yes | yes | yes |
| `personalities.json` | yes (`Personalities.swift:37`) | yes | no hit in grep (moved; unverified where) |
| `Backups/` (`Calibration.swift:96`; holds copies of `bots.json, groups.json, groups, personalities.json, agent-messages.json, phone-link.json`) | no | yes | yes |
| `machine-groups.json`, `machine-groups/` | no | yes | yes |
| `agent-letters.json`, `agents-elsewhere.json`, `agent-letters-seen.json` (`AgentMail.swift:698-700`), `machine-bots.json` (`Bots.swift:1089`) | no | no | yes |

None of the three tags has `S/box`, a keeper, a launch agent or a lock: `git grep 'LaunchAgents|SMAppService|--box'` hits only
`SMAppService.mainApp` (Open at Login) in `v0.8.0-beta.1:KuvoCode/Stores/OpenAtLogin.swift:14-25`.

Keychain in the tags (generic passwords): service `<bundle id>.phone-link` (`PhoneLink.swift:884` v0.6.6, `:1369` v0.7.2, `:2079` v0.8.0-beta.1),
`<bundle id>.integrations` (`IntegrationKeys.swift:11` / `:11` / `:12`); internet passwords with a creator code (`Passwords.swift:88`).
No `.vault` service and no access group in the tags (no grep hits).

### (d) This branch's box

- Engine pin: `box/src/shared/engine-pin.json` `"version": "2.2.6"` (darwin aarch64 and x86_64, linux x86_64 and aarch64, each with
  `url` + `integrity`); `ENGINE_VERSION = pin.version` (`box/src/link/add/files.ts:12,39`); the Swift package agrees
  (`swift/OpenCodeKit/Sources/OpenCodeKit/OpenCodeKit.swift:5` = `"2.2.6"`). Keeper scripts carry it as `@ENGINE@`
  (`box/src/keeper/kuvo-start:18`, `kuvo-launch:8`; `box/src/release/files.ts:31`).
- Binary: `$K/engine/<V>/opencode` with `K="$HOME/.kuvo-code"` (`kuvo-start:8,19`; `add/files.ts:117`; `install/command.ts:241,261`).
  If the pinned one is missing, `kuvo-start:19` takes the newest executable `engine/*/opencode`.
- Folders (`box/src/keeper/kuvo-launch:12-13`, `kuvo-start:28-29`):
  `XDG_DATA_HOME="$K/data" XDG_CONFIG_HOME="$K/config" XDG_STATE_HOME="$K/state" XDG_CACHE_HOME="$K/cache"
  npm_config_cache="$K/cache/npm" BUN_INSTALL="$K/cache/bun" KUVO_INTEGRATIONS="$K/integrations" TMPDIR="$K/tmp"`;
  `mkdir -p "$K/data" "$K/config" "$K/state" "$K/cache" "$K/workspace" "$K/integrations" "$K/tmp"`.
  `KUVO_OWN_XDG_*` kept first (`kuvo-launch:11`).
- Start (`kuvo-launch:164-167`): `export KUVO_BOTS="$K/bots" KUVO_WAKES="$K/wakes"`, `PATH="$K/bin:$PATH"`, `cd "$K/workspace"`,
  `env -u KUVO_AGENTS_PASSWORD -u BUN_BE_BUN "$S" "$K/engine/$V/opencode" serve --port "$port" --hostname 127.0.0.1 >"$R/engine.log" 2>&1 </dev/null &`
  — a fixed port, **no `--stdio`**, log in `run/engine.log`, readiness by `grep "server listening"`. Password comes in as
  `OPENCODE_PASSWORD` through `launch.env` (`kuvo-start:289`). `OPENCODE_CONFIG` = `$K/keys/settings.json` or `$K/box/opencode.json`
  on a machine only (`kuvo-launch:144-147`, not for `own_mac`); `OPENCODE_CONFIG_CONTENT` from `kuvo-link.js engine-config` (line 152).
- `box/src/link/home.ts`: `K = KUVO_HOME || ~/.kuvo-code` (14); `RUN = K/run`, `ENGINE_FILE = run/engine.json`, `PID_FILE = run/link.pid` (62-67);
  `ENGINE_DATA = KUVO_ENGINE_DATA || K/data/opencode` (147); `ENGINE_CONFIG = KUVO_ENGINE_CONFIG || (OWN_HOME_MAC ? K/config : undefined)` (204);
  `BOTS = K/bots` (174); `CHATS = K/chats` in a home of its own (178); `SESSION_WAKES = K/wakes`, `WAKE_COMMAND = K/bin/kuvo-wake` (150-151);
  `INTEGRATION_DATA = K/integrations` (144); `KIND_FILE = K/box/kind` (20).
- Database for the move: `path.join(data, "opencode", "opencode.db")` (`rehome/engine-check.ts:33`), any `opencode*.db`
  (`rehome/database.ts:55`), `-wal` counted (`rehome/move.ts:240-241`).
- The rewrite's check engine (`rehome/engine-check.ts:45-68`): `spawn(engine, ["serve", "--stdio", "--port", "0"], { cwd: scratch/workspace, env: { PATH: "/usr/bin:/bin:/usr/sbin:/sbin", HOME: scratch/home, TMPDIR, XDG_DATA_HOME: scratch/data, XDG_CONFIG_HOME, XDG_STATE_HOME, XDG_CACHE_HOME, OPENCODE_PASSWORD, OPENCODE_CLIENT: "desktop", OPENCODE_DISABLE_AUTOUPDATE/MODELS_FETCH/FREE_MODELS/PROJECT_CONFIG/CLAUDE_CODE/FILEWATCHER: "1", proxies → 127.0.0.1:9 } })`, auth `Basic opencode:<password>`.

Old S → box home, as `rehome/move.ts:34-40` (`BESIDE`, `APP_ATTACHMENTS`) already maps it:

| S (tags) | `~/.kuvo-code` |
|---|---|
| `opencode/data` (→ `…/opencode/opencode.db`, `snapshot`, `log`) | `data` |
| `opencode/config` (→ `opencode/opencode.json`, `skills`, `plugins/kuvo-code.js`) | `config` |
| `opencode/state` | `state` |
| `opencode/cache` | `cache` |
| `opencode/workspace` | `workspace` |
| `engine` (`2.0.32/opencode`) | `engine` |
| `integrations`, `bots`, `wakes`, `groups`, `agents`, `phone-link.json` | same names |
| `Chats` | `chats` |
| `Attachments` | `attachments` (merged) |
| `box/*` (not in the tags) | `<name>` (never `bin`; `run` without process records) |

Not in `BESIDE` (so not moved by the current plan): `bots.json`, `groups.json`, `personalities.json`, `agent-messages.json`,
`agent-letters*.json`, `agents-elsewhere.json`, `machine-bots.json`, `machine-groups(.json)`, `Backups`, `Browser`, `opencode-files`,
`engine-linux`, `browser-linux`, `bin` (`kuvo-wake`, `kuvo-npx`, `runtime`). On this branch these reach the box through the app's own
upgrades before a move (the move waits for `box/run/served.json`, see below); for a 0.7-era S with no `box/` that step never ran.

Differences that matter for the port (released apps vs this branch's box):

1. **Engine 2.0.32 vs 2.2.6.** `KNOWN_SCHEMAS = ["59 20261030054400_job"]` (`rehome/database.ts:19`) is "as its pinned version made them";
   `PATH_REWRITER.refuses` → `schemaRefusal` (`rehome/rewrite.ts:153-158`) refuses a database of another schema. A database last opened by
   2.0.32 very likely carries a different mark (unverified: no 2.0.32 database was inspected). Also `stagedEngine` (`rewrite.ts:87-101`) checks with
   the version `V=` in the release's `kuvo-start` *if that version is among the staged `engine/` folders*, else the newest staged — for a
   0.7 S that is `2.0.32`, unless 2.2.6 is installed into `engine/` first.
2. Engine start: stdin-bound `--stdio --port 0` with no records on disk (tags) vs detached, fixed port, `run/engine.json`/`engine.log` (box).
3. `TMPDIR`: user temp folder (tags) vs `$K/tmp` (box).
4. Tags have no `S/box`, so `moveRefusal`'s `served.json` condition (`move.ts:214-218`: `served.bridge === BRIDGE_VERSION` and `served.app === appVersion`)
   and `planMove`'s `listOf(box)` assume a layout the released apps never had.
5. Tags only pin darwin-arm64; the box also pins darwin x86_64.

---

#### Q5 — the old keeper's move on this branch

Files: `swift/KuvoCode/Keeper/OwnBoxMove.swift` (OBM), `swift/KuvoCode/Keeper/Keeper.swift`, `swift/KuvoCode/Keeper/MacBoxRunner.swift`,
`swift/KuvoCode/Keeper/KeeperRecord.swift`, `swift/KuvoCode/Stores/BackgroundWork.swift`,
`mac/helper/Sources/KuvoHelperKit/{KeychainMove,BoxHelper,BoxHelperBundle,HostKeychain,KeysTool}.swift` (KeychainMove is in the helper kit, not under `swift/KuvoCode`).

All of it is behind `AppModel.movesOwnBox`, which is `false` and "nothing turns it on yet" (`swift/KuvoCode/App/AppModel.swift:844-847`).

### How it runs the box's CLI (OBM 146-174)

```
<engine> run --config=/dev/null --no-env-file <kuvo-link.js> rehome <root> <arguments…>
```
- executable: the app's engine (`AppModel.engineOverride`, else `S/engine/<v>/opencode` if installed, else `<home>/<root>/engine/<OpenCodeEngine.version>/opencode` if it is the pinned build; OBM 118-126)
- argv literally `["run", "--config=/dev/null", "--no-env-file", script.path] + arguments` (OBM 157); `script` = `BoxProcess.script` (the app's bundled `kuvo-link.js`)
- env (OBM 151-155): `HOME=<home>`, `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, `BUN_BE_BUN=1`, `BUN_RUNTIME_TRANSPILER_CACHE_PATH=0`,
  `KUVO_HOME=<appBox = S/box>` (or `<home>/<root>` when `ofHome: true`), `KUVO_ENGINE_DATA=<S/opencode/data/opencode>`,
  `KUVO_APP_VERSION=<app version>`, `TMPDIR=<temp dir>`, plus test stand-ins
- cwd `<home>`; default limit 120 s; the answer is the **last stdout line parsed as JSON**; `Said{status, answer, errors}`; `status` nil when it timed out or was killed.
- `<root>` = `Here.root` (`.kuvo-code` for the real app).

### Steps, in order

**0. Keeper start (`Keeper.main`, Keeper.swift 86-116)**
- `OwnBoxMove.takeLock()` (OBM 464-475): `flock(LOCK_EX|LOCK_NB)` on `~/Library/Caches/<bundle id>/keeper-move.lock` (0600, folder 0700). Not taken → `exit(0)` ("Another keeper runs already").
- `settledBeforeStart(move)` (Keeper.swift 121-127): loops `move.settle()`; `.wait` → sleep 600 s and ask again; `.retire` → `exit(0)`; `.startOld` → go on.
- then `Here.gate(...)`, then `run()`: `flock` on `S/box/run/keeper.lock`, writes `S/box/run/keeper.json`
  (`{pid, version, build, url, password, started}`, KeeperRecord.swift 9-24, 42-47) and `S/box/run/box.pid` (own pid, Keeper.swift 194).

**1. `settle()` (OBM 227-251)** — before anything is written in S:
- `rehome <root> --status` (120 s). Not answered / status ≠ 0 / empty → `.wait` if `<home>/<root>/updates/rehome/journal.json` exists with `step != "done"`
  (or unreadable), or `updates/rehome/state.json` has `state == "done" && way == "move"`, or `<home>/<root>/box/kind` reads `mac`; else `.startOld` (OBM 208-220, 231).
- `state` `"moving"` or `"clearing"` → `rehome <root> --resume` (limit 8 h) → `after(said)` (OBM 255-259).
- `state == "done"` and `way == "move"` → `finish(owed: answer["owed"])`.
- `state == nil` → `rehome <root> --new --check` (30 s; twice more after 2 s pauses if `made` is missing); `made` nil → `.wait`;
  `made == true` → `finish(owed: [], helperOnlyIfMissing: true)`.
- otherwise `wentBack()`.

**2. Asking whether to move (`Keeper.due`, Keeper.swift 356-373; `OBM.due`/`isDue` 179-204)** — in the keeper's watch loop, only when not handing over
to a newer app and not busy; asked at the first look and every 60th look (ten minutes), or at once when the mtime of
`<home>/<root>/updates/rehome/state.json` changed:
- `rehome <root> --ready` → due iff `answer.ready == true` and `answer.how` is `"move"` or `"resume"` (`why` logged otherwise).
- `release()` = `makeRelease(root:)` (OBM 481-494): `<tmp>/kuvo-box-release-<UUID>/` filled by `BoxRelease.write(to:)` (with `RemoteEngineInstaller.$scopedRoot = root`)
  plus the app's `Contents/Helpers/Kuvo Helper.app` copied to `<release>/bin/Kuvo Helper.app`. Failure → `gaveUp(...)` =
  `rehome <root> --failed "The box's files for its home weren't made: <error>"`.
- `rehome <root> --ready --release <release>` again; not due → release folder removed.

**3. Stopping the old box (`Keeper.moveBox`, Keeper.swift 379-405; `MacBoxRunner.stop(keepingClaim: true)`, MacBoxRunner.swift 290-305)**
- `awake.agentsWork(false)`, then `box.stop(keepingClaim: true)`:
  - collects pids of the engine `ServerProcess`, the agents bridge and the box's `serve` process (in memory, **not from pid files**);
  - `stopEngine()` (closes the engine's stdin; `BoxProcess.engineChanged(nil, …)` clears `S/box/run/engine.json`), `agents.stop()`, `box.stop()`
    (writes nil to `S/box/run/serve.json`, closes its stdin), `host.stop()`, `IntegrationKeys.removeFiles()`;
  - `waitForExit(pids, within: 4)` (lines 309-323): polls `kill(pid, 0)` every 50 ms for 4 s, then `SIGTERM`, 1 s more, then `SIGKILL`;
  - with `keepingClaim` the `flock` on `S/box/run/box.lock` (lines 86-97) is **kept**, so the app starts no box meanwhile.
- No signal is sent to anything found by pattern or pid file; everything stopped is a child the keeper started.

**4. `move(release)` (OBM 412-445)** — release folder removed on return (`defer`):
1. `identity.copiesLeft()` (journal state `copying` or `copied`) → helper `keys uncopy`; failure →
   `rehome <root> --failed "An earlier try's copies of this Mac's keys couldn't be taken away: <why>"` → `.startOld`.
2. `identity.copyKeys()` = `KeychainMove.copy(trusting: <app>/Contents/Helpers/Kuvo Helper.app)` **in the keeper's own process**.
   On `Failure.leftovers` → helper `keys clear`, then copy once more. Any other failure →
   `rehome <root> --failed "This Mac's keys weren't copied for its box: <error>"` → `wentBack()`.
3. helper `keys check`; failure → `rehome <root> --failed "Kuvo Code Box couldn't read its copies of this Mac's keys: <why>"` → `wentBack()`.
4. `rehome <root> --release <release> [--keeps-awake off]` (limit 8 h; `--keeps-awake off` when `!Awake.isOn`). No answer → `wentBack()`.
5. `after(said)` → `acted` (OBM 279-301) by `answer.result`:
   - `"done"` → `finish(owed: status.owed)`
   - `"failed" | "refused" | "nothing"` → if `status.state == "done" && status.way == "move"` → `finish(owed:)`; else `wentBack()`
   - `"back"` → `wentBack()`
   - `"stuck" | "waits"` → `holdsOnBecause = answer.message`; `.wait`
   - anything else → if cut off (`status == nil`) or empty answer → `settle()` once more (never in a loop); else `.wait`

The helper runs (`Identity.live`, OBM 507-515) are literally
`<app>/Contents/Helpers/Kuvo Helper.app/Contents/MacOS/Kuvo Helper keys <check|uncopy|clear> --home <home>/<root>` (limit 60 s, no env given);
success = exit 0, else the stderr (or stdout) text is the reason. Note it is the **helper inside the app bundle**, not the one in `~/.kuvo-code/bin`.
`keys uncopy|clear|back` first take `flock` on `<home>/run/helper.lock` (BoxHelper.swift 117-122); `keys check` doesn't.

**5. Moved: `finish(owed:)` (OBM 325-348)**
1. `helperRuns()` (OBM 369-376): with `KUVO_HOME=<home>/<root>`:
   `kuvo-link.js helper start --app <bundle id>` (+ `--if-missing` for a new Mac's box). Status ≠ 0 → `.wait`.
   (The keeper on this branch does **not** call `helper install --load` itself; `helper start` does it inside the box when the agent's file isn't there.)
2. if `owed` contains `"keychain"`: `KeychainMove.deleteOriginals()` in the keeper's process; throw → `.wait`; then
   `rehome <root> --settled keychain` (must exit 0 and `status.owed` no longer contain it, OBM 351-357) else `.wait`.
3. if `owed` contains `"agent"`: `rehome <root> --settled agent` **before** unregistering (unregistering ends this process) else `.wait`.
4. `identity.unregister()` (OBM 517-526): `SMAppService.agent(plistName: BackgroundWork.plistName).unregister()`; an error counts as done when
   `service.status` is `.notRegistered` or `.notFound`; otherwise `.wait`.
5. `.retire` → `Keeper.finish(.stopped)` → exit code 0 (Keeper.swift 23-26, 220-236), which launchd doesn't restart (`KeepAlive.SuccessfulExit = false`).

**6. Not moved: `wentBack()` (OBM 314-320)** — if copies are left, helper `keys uncopy` (failure only logged); `.startOld` →
Keeper.swift 401-404: `await box.start()` then `await box.agents.startAgain()`.

**While `.wait`** (Keeper.swift 390-395): sleep 600 s, `settle()` again, nothing running meanwhile; SIGTERM during a move → `finished`, nothing restarted.

**New Mac (`makeNew`, OBM 384-407), from the app as it opens:** `rehome <root> --new --check` → `made` true → `helper start --app <id> --if-missing`;
else if `ready` → `rehome <root> --new --release <release> [--keeps-awake off]` (result `done` or `nothing`) → `helper start --app <id>`.
Sets `BackgroundWork.boxStartsOnItsOwn`, which stops `register()` (BackgroundWork.swift 91-94).

There is no `--run` sub-command anywhere (not in OBM, not in `command.ts`).

### `KeychainMove` exactly (`mac/helper/Sources/KuvoHelperKit/KeychainMove.swift`)

- Items: `kSecClassGenericPassword`, file-based login keychain (`kSecUseDataProtectionKeychain = false`, HostKeychain.swift 67, 84);
  **no access group** anywhere; access is by ACL (`SecAccessCreate` with `SecTrustedApplicationCreateFromPath`, HostKeychain.swift 144-157),
  `kSecAttrAccessible = kSecAttrAccessibleAfterFirstUnlock`.
- Services: `static let services = ["phone-link", "integrations", "vault"]` (line 23), each prefixed:
  originals `<app bundle id>.<service>` (`run.kuvo.code.phone-link`, `run.kuvo.code.integrations`, `run.kuvo.code.vault`);
  copies `<helper id>.<service>` where helper id = app id + `.box` (`BoxHelperBundle.idSuffix`, BoxHelperBundle.swift 16, 23) →
  `run.kuvo.code.box.phone-link`, `run.kuvo.code.box.integrations`, `run.kuvo.code.box.vault`.
- Accounts: whatever accounts each original service has (`keychain.items(service)` lists all, by account and label, without reading secrets);
  no fixed account names in this file (the doc comment's example is `phone-link/key`).
- Journal: `<home>/updates/rehome/keychain.json` (line 108), written 0600 via `PrivateFile.write` (temp file, fsync, rename; folder 0700):
  `{"v":1,"state":"copying|copied|settling|settled|returning","app":"<app id>","helper":"<helper id>","items":[{"service":"phone-link","account":"…","label":"…"?}]}`
  (Swift `JSONEncoder`, lines 26-63; `label` optional).
- `copy(trusting:)` (145-177): journal `settled`/`settling` → returns the count, does nothing; `copying`/`copied` → throws
  ("An earlier try's copies … are still there"); `returning` → throws; keychain locked → `refused`. For each service: lists originals; if
  **any** item already exists under the helper's service → `Failure.leftovers("<service>/<account>")`. Writes journal `copying` with all entries
  first; for each: read `<app>.<service>`/account (missing → skipped; refused → throw), `SecItemAdd` under `<helper>.<service>`, same account,
  same data, same label, **trusting only the helper bundle** (`errSecDuplicateItem` → leftovers). Then journal `copied`. Same value, same format: a byte copy.
- `uncopy()` (185-202, helper's): only for `copying`/`copied` and `!moveIsDone` (`updates/rehome/state.json` `state == "done" && way == "move"`);
  deletes each journaled copy, then removes the journal. Originals never touched.
- `clearLeftovers()` (207-228, helper's `keys clear`): only with **no** journal and no `<home>/box/keychain.json`; deletes each helper-named item
  the app has an original of; returns the kept ones (`{"kept": […]}`).
- `unreadable()` (231-236, helper's `keys check`): journal must be `copied|settling|settled`; prints `{"unreadable":[…]}`, exit 0 iff empty.
- `deleteOriginals()` (242-261, keeper's): `settled` → no-op; requires `copied` or `settling`; for each entry whose original still exists the copy must
  exist (else `copyMissing`); journal → `settling`; deletes each `<app>.<service>`/account (`errSecItemNotFound` ok); journal → `settled`.
- `back(trusting:)` (267-298, helper's `keys back --app <Kuvo Code.app>`): every helper item made again under the app's names trusting the app, then the helper's deleted, journal removed.
- Internet passwords (0.7's saved sign-ins, creator `'KUVO'` = `0x4B55564F`) are **not** part of `KeychainMove`; only `KeysTool` (`kuvo-keys passwords`)
  writes them to `<home>/updates/rehome/passwords.json` as `{"v":1,"logins":[{"host","account","password","modified"?}]}` (KeysTool.swift 160-190), and that file's
  own comment says "Not to ship as it is". The Swift keeper never calls `kuvo-keys`.

### Launch agents

- Old keeper: label **`<bundle id>.box`** = `run.kuvo.code.box`; plist name **`run.kuvo.code.box.plist`** inside the app at
  `Contents/Library/LaunchAgents/` (BackgroundWork.swift 40-44; build phase `swift/KuvoCode.xcodeproj/project.pbxproj:283-287`):
  `BundleProgram Contents/MacOS/<exe>`, `ProgramArguments [<exe>, "--box"]`, `AssociatedBundleIdentifiers [<bundle id>]`, `RunAtLoad true`,
  `KeepAlive {SuccessfulExit: false}`, `ProcessType Standard`, `LimitLoadToSessionType Aqua`. Registered with `SMAppService.agent(plistName:).register()`
  (BackgroundWork.swift 116); kickstart `/bin/launchctl kickstart [-k] gui/<uid>/run.kuvo.code.box` (158).
  Unregistered only with `SMAppService…unregister()` (no `launchctl bootout`, no plist in `~/Library/LaunchAgents`).
  Note: the old keeper's label `run.kuvo.code.box` is the same string as the helper's bundle ID.
- New helper: label **`run.kuvo.code.machine`** (`BoxHelperBundle.launchAgent`, BoxHelperBundle.swift 19; `box/src/link/uninstall.ts:27`), file
  `~/Library/LaunchAgents/run.kuvo.code.machine.plist`, `ProgramArguments [<K>/bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper, "keep", "--home", <K>]`,
  `RunAtLoad`, `KeepAlive {SuccessfulExit: false}`, `AbandonProcessGroup`, `AssociatedBundleIdentifiers [<app>]` (`box/src/link/helper.ts:29-43`).
- Released tags have neither (see Q1).

### Marker files the keeper reads or writes

| Path | Who | Shape |
|---|---|---|
| `~/Library/Caches/<bundle id>/keeper-move.lock` | keeper, flock | empty |
| `S/box/run/keeper.lock`, `S/box/run/box.lock` | keeper, flock | empty |
| `S/box/run/keeper.json` | keeper | `{pid, version, build, url, password, started}` |
| `S/box/run/box.pid` | keeper | pid as text |
| `<home>/updates/rehome/state.json` | box writes; keeper reads `state`, `way`, and its mtime | `RehomeRecord` |
| `<home>/updates/rehome/journal.json` | box writes; keeper reads `step` | `Journal` (`v:1, way, began, step, from, to, placements, clearing, kept, …`) |
| `<home>/updates/rehome/keychain.json` | keeper (`copy`, `deleteOriginals`) and helper | see above |
| `<home>/box/kind` | box writes `mac\n`; keeper reads | one word |
| `<tmp>/kuvo-box-release-<UUID>/` | keeper makes, removes | release files + `bin/Kuvo Helper.app` |

### What it leaves in S

The keeper itself deletes nothing in S. The box's `clear` step **renames** (never deletes) `S/box`, `S/opencode` (whole), `S/engine`, `S/integrations`,
`S/bots`, `S/wakes`, `S/groups`, `S/Chats`, `S/agents`, `S/phone-link.json`, `S/Attachments` into the kept copy (`journal.kept`, kept for
`KEPT_FOR` = 30 days; `move.ts:163-164, 548-556`; `state.ts:32`). Everything else stays in S: `bots.json`, `groups.json`, `Backups`, `Browser`,
`opencode-files`, `engine-linux`, `browser-linux`, `bin/`, the other `.json` lists, and the keeper's `keeper-move.lock` in Caches. The app bundle's
launch agent plist stays in the bundle (only unregistered). Where `journal.kept` points was not read: unverified.

### The box side (`box/src/link/rehome/`)

`command.ts` — `kuvo-link.js rehome <root> …`, one JSON line, exit `{done:0, back:0, nothing:0, refused:1, failed:1, stuck:3, waits:4}`, 2 for bad usage (lines 20-25):

| Sub-command | Does | Line |
|---|---|---|
| `--status` | `rehomeStatus(home)`: `{state, way, step, tries, nextTry, waitingSince, overdue, failed, movedAt, keptUntil, owed, asked, from, to}` | 44-47 |
| `--settled keychain\|agent` | removes it from `state.json`'s `owed`; `{settled, status}` | 57-65 |
| `--failed <why>` | `keeperFailed`: counts a try; `{noted, status}` | 66-74 |
| `--new --check` | `newReadiness`: `{ready, made, why?}` | 75-82 |
| `--ready [--release <folder>]` | `rehomeReady`: `{ready, how, why, status}`; always exit 0 | 83-86 |
| `--resume` | `rehomeResume` | 92 |
| `--back` | `rehomeBack` | 93 |
| `--new --release <folder> [--keeps-awake on\|off]` | `rehomeNew` | 94 |
| (none) `[--release <folder>] [--keeps-awake on\|off]` | `rehome`: the move | 95 |

Guards: `delete process.env.KUVO_BOX` (30); only an app folder named `run.kuvo.code` may act on the real `~/.kuvo-code` (50-56); not darwin → `refused`
with `NOTHING_TO_MOVE` unless `--resume` (88-91). There is no `--run`.

`state.ts` — `<home>/updates/rehome/`: `state.json` (`RehomeRecord`, + `kept`, `merged`, `counted`, `movedBack`, `leftovers`), `journal.json`,
`journal.acts` (one JSON act per line: `rename|made|wrote|keychain`), `lock/pid` (mkdir lock; holder must be a live `rehome` or `kuvo-link.js install` process),
`staging/`. Steps `["plan","install","clone","rewrite","check","place","fix","clear","done"]`; before `clear` a failure goes back, from it on only forward.
`MOST_TRIES = 3`, `AFTER_A_FAILURE = 1 h`, `OVERDUE_AFTER = 3 d`, `KEPT_FOR = 30 d`.

`move.ts` — `moveRefusal` (209-246): darwin only; not already done; `S/box/run/served.json` has `bridge == BRIDGE_VERSION` and `app == appVersion`;
nothing running by `S/box/run/{engine.json→opencode, bridge.json→kuvo-agents, serve.json→kuvo-link, link.pid→kuvo-link}` (`runningIn`, 94-101, by pid **and** process name);
app folder named `run.kuvo.code`; release has `bin/kuvo-start`, `bin/kuvo-link.js`, `bin/Kuvo Helper.app/Contents/MacOS/Kuvo Helper`; no box already in the home;
same APFS volume; free space ≥ (`opencode.db` + `-wal`) × 1.1 + 200 MB; rewriter's schema check. `carryOn` (448-569): `install` (what's in the home set aside in
`trash/earlier-box-<stamp>`), `clone`, `rewrite`, `check` (engine started on the copy), `place` (then `box/work.json` if awake off, `box/kind` = `mac\n` last),
`fix` (outside: git worktree links, Claude history, Grok sessions, Claude Code Keychain sign-ins), `clear` (refuses if something started or changed meanwhile;
renames into kept), `rewriter.cleared`, `finish` (552-599: `state: "done"`, `way`, `movedAt`, `keptUntil`, `kept`, `merged`, **`owed: ["keychain", "agent"]`**, `from`, `to`).
`failed` (601-617) undoes by the acts and notes the try.

`outside.ts` — worktree repair (`git worktree repair`), `~/.claude/projects/<slug>` and `~/.grok/sessions/<encoded>` renames, and Claude Code's own
Keychain items `Claude Code-credentials-<sha256(folder)[:8]>` copied with `/usr/bin/security -i` (`add-generic-password -U -a … -s … -X <hex>`), originals deleted
in `cleared`. This is the **coding agents'** Keychain, not Kuvo Code's own items.

`own.ts` — rewrites the box's own JSON/JSONL/path files in the staged copy; renames what is kept by a path's hash.

Related box command (`box/src/link/helper.ts`, `main.ts:1243`): `helper install [--load] [--app <bundle ID>]` (writes the plist; `--load`:
`launchctl bootout gui/<uid>/run.kuvo.code.machine`, wait, `launchctl bootstrap gui/<uid> <plist>` up to 4 tries), `helper start [--if-missing] [--app <bundle ID>]`
(install+load if missing, `bootstrap` if not loaded, `launchctl kickstart gui/<uid>/run.kuvo.code.machine`; if disabled in Login Items:
`open -g -j -n <Kuvo Helper.app> --args keep --home <K>`), `helper status`. Refuses unless darwin, a home of its own and `box/kind == mac`.

### What the Swift keeper did that no box code does

1. **Stopping the old app's processes.** The box only *checks* (`runningIn`) and refuses; the keeper stopped its own children
   (stdin close → SIGTERM after 4 s → SIGKILL after 1 s more) and held `S/box/run/box.lock`. For a 0.7 app there are no `S/box/run` records at all, so
   `runningIn` sees nothing even while a 0.7 engine runs on `S/opencode/data/opencode/opencode.db`.
2. **Copying Kuvo Code's own Keychain items** (`KeychainMove.copy`, in-process in the app's signed program) and the copy → check → uncopy/clear
   orchestration. The box has no call to `kuvo-keys copy` or to `Kuvo Helper keys …` in `rehome/` (grep of `move.ts` for `kuvo-keys`, `helper`: none besides
   the release check); `KeysTool` exists in the helper kit but nothing in `box/src/link/rehome` runs it.
3. **Deleting the originals** (`KeychainMove.deleteOriginals`) — the box only records `owed: ["keychain"]` and takes `--settled keychain`.
4. **Unregistering the old launch agent** (`SMAppService.agent(plistName: "run.kuvo.code.box.plist").unregister()`) — the box only records
   `owed: ["agent"]`. SMAppService can only be called by the app's own program; nothing in TypeScript can do the same (a `launchctl bootout gui/<uid>/run.kuvo.code.box`
   would stop it but not unregister it; not present in box code either).
5. **Making the release folder** (`BoxRelease.write` + the helper bundle) and passing it as `--release`.
6. **The schedule and retries**: asking `--ready` every ten minutes / on `state.json` change, the 600 s wait-and-settle loop, `--failed` reports
   for keeper-side failures, the one-keeper lock in Caches.
7. **Starting the old box again** when it didn't move (`box.start()`, `agents.startAgain()`), and **calling `helper start --app <id>`** after a move
   (the command exists in the box; the call was the keeper's).
8. Passing `--keeps-awake off` from the app's own setting (`Awake.isOn`).
9. Saved sign-ins (internet passwords) hand-over: only `kuvo-keys passwords` exists, called by nothing.

---

## Part 6. Sub-report: every Swift upgrade, port-ready (answer 2)

### Q2: the retired Swift app's one-time upgrades, for porting into `box/src/link/rehome`

Read-only research on branch `kuvo/713-piece-4-the-move-from-0-7-rehome-run`. Tags read: `v0.6.6`, `v0.7.2`, `v0.8.0-beta.1`.
S = `~/Library/Application Support/run.kuvo.code`. K = the box's home (`S/box` under the Swift app; `~/.kuvo-code` in a home of its own). Paths are `swift/KuvoCode/...` on the branch unless a tag is named.

Conventions that hold for every Swift-written JSON file below (`v0.7.2:KuvoCode/Stores/JSONFile.swift`):
- `JSONFile.write(value, to:, isoDates:, readable:)`: `JSONEncoder`; `isoDates` → dates as ISO 8601 **without fractional seconds** (`"2026-11-15T10:00:00Z"`); `readable` → `.prettyPrinted, .sortedKeys`. Optionals that are nil are **left out** (synthesized `encodeIfPresent`), a `Set<String>` is a JSON array.
- A file that won't decode is renamed `<name>.unreadable-<yyyyMMddTHHmmssZ-ish>.json` (and `-2`, `-3`…) by `JSONFile.read`, so such files may sit in S.
- Preferences written as `Data` (`UserDefaults.set(try? JSONEncoder().encode(x))`) are plist `<data>` holding compact JSON; dates there would be Swift's default (seconds since 2001) but none of the ones below hold dates.

Where the box's folders are (`box/src/link/home.ts`): `RUN = K/run`, `BOX = K/box`, `BOTS = $KUVO_BOTS || K/bots` (Swift app: `S/bots`, `Keeper/MacBoxRunner.swift:392`), `CONFIG_FILE` on a Mac = `$KUVO_ENGINE_CONFIG/opencode/opencode.json` (Swift app: `S/opencode/config/opencode/opencode.json`; own home: `K/config/opencode/opencode.json`, `home.ts:204`, `areas/config.ts:13`). The agents bridge's group chats: `$KUVO_GROUPS` (Swift app: `S/groups`, `MacBoxRunner.swift:407`), on a machine `K/groups` (`box/src/agents/main.ts:206-210`). **Unverified**: whether a Mac's box in a home of its own counts as `onMachine` in `agents/main.ts` (i.e. reads `K/groups` with no `KUVO_GROUPS`); I did not read how `onMachine`/`machineHome` are set.

---

#### a. Group chats — `Stores/GroupMigration.swift` `GroupMigration.run` (`:61`), run at `App/AppModel.swift:87-90` (before `BoxGroups()` and before `Moves()` at `:96`)

### Input (identical in v0.6.6, v0.7.2, v0.8.0-beta.1: the `BotGroup`/`GroupMessage`/`GroupTask` structs have the same fields in all three)
- `S/groups.json` — `JSONFile.write(list, isoDates: true, readable: true)` (`v0.7.2:KuvoCode/Stores/BotGroups.swift:315,1691`): array of `BotGroup` (`:9-48`):
  `{"id": "grp_<10 of a-z0-9>", "name", "purpose"?, "members": [botID…], "seats": {botID: sessionID}, "heard": {botID: messageID}, "agentSeats"?: [sessionID…], "created": ISO, "viewed"?: ISO, "retracted"?: {botID: text}, "meeting"?: {"next": ISO, "every"?: string, "topic": string, "at"?: int, "weekday"?: int}}`.
- `S/groups/<id>.json` — array of `GroupMessage` (`:126-148`), same writer (`:1700`):
  `{"id", "author"?: botID (absent = you), "text", "time": ISO, "kind": "message"|"wrapUp"|"note"|"meeting", "steps"?: [Message.ToolCall…], "tasks"?: [{"id","bot","task","state": "proposed"|"waiting"|"working"|"done"|"failed", "prompt"?, "started"?: ISO, "finished"?: ISO, "after"?: [botID…]}], "report"?: string}`.
- Preference `groupMemoryWaiting`: plist dictionary `[String: [String]]` = bot ID → chat IDs it is still to forget (`v0.7.2 BotGroups.swift:1526`).
- Preference `movesUnderWay`: `Data` JSON array of records; only `kind == "group"` → `id` is read (`GroupMigration.swift:254-257`). Shape in (k).
- `S/bots.json` (shape in (c)): to know which bot lives on a machine (`macBots()` `:292`), and for `describeBots`.
- `S/groups/imported.json` (written by an earlier run): `{"list"?: "<sha256 hex of groups.json bytes>", "ids": [chatID…]}` (`Imported`, `:51`).
- `machine-groups.json`/`machine-groups/`: only via `BoxGroups.takeOldCache()` called first at `:65` — see (j). Not otherwise involved.

### Algorithm (`:61-158`)
1. `BoxGroups.takeOldCache()` (j).
2. `pendingMemory = defaults.dictionary("groupMemoryWaiting") as? [String:[String]]`; `list = bytes of S/groups.json`; `hash = sha256 hex(list)`; `imported = S/groups/imported.json` or empty. `hasList = list != nil && hash != imported.list`. If `!hasList && pendingMemory == nil` → nothing.
3. `mkdir S/groups` (0700).
4. If `hasList`: `held` = group IDs in `movesUnderWay`; `bots` = `bots.json`; parse `groups.json` as `[[String: Any]]` (failure → `failed += "groups.json"`, no chats handled). `orphans` = `S/groups/orphan-seats.json` or `{}`. For each chat with `id` matching `\Agrp_[a-z0-9]+\z`:
   - if `S/groups/<id>/group.json` exists → `skipped`; else if `imported.ids` has it → `deleted` (stays deleted); else:
   - `messages` = `S/groups/<id>.json` filtered (`:214-230`): keep one only if `id` is String, `text` is String, `time` parses as ISO 8601 (with or without fractional seconds); `kind` missing → `"message"`, else must be one of the four; `author` `null` → removed, non-string → message dropped; `tasks`, when present, must be an array where every entry has String `id`,`bot`,`task` and `state` among the five, else the message is dropped. Everything else on the message (`steps`, `report`, tasks' `prompt/started/finished/after`) passes through untouched. Unreadable file → `[]`.
   - `group` (`:233-251`): requires `members` to be `[String]` else `failed`. `{"id", "name": name non-empty else "Group Chat", "members", "seats": {String:String} or {}, "heard": same or {}, "created": raw if ISO date, else first kept message's `time`, else now}`; plus `purpose` if non-empty string, `agentSeats` if `[String]`, `viewed` if ISO date, `retracted` if non-empty `{String:String}`, `meeting` only if `next` is a date and `topic` a string → `{"next","topic","every"? (String),"at"? (Int),"weekday"? (Int)}`.
   - Orphan seats (`:108-118`): for each `(bot, seat)` in `seats` where `bots.json` says that bot has a non-nil `machine`: remove `seats[bot]`, remove `heard[bot]`, remove `seat` from `agentSeats`, append `seat` to `orphans[machine]`.
   - `status = held.contains(id) ? "held" : "active"`.
   - Write (each to `.<name>.new`, 0600, then rename; folder 0700): first `S/groups/<id>/messages.json` = the filtered array (pretty, slashes unescaped), then **last** `S/groups/<id>/group.json` = `{"kuvo": 1, "status": status, "group": group}` (pretty, sorted keys). On error → `failed`.
   - Also seeds the window's cache `S/group-chats.json` + `S/group-chats/<id>.json` (`seed`, `:282`) — client drawing only.
   - After the loop: `orphan-seats.json` written as `{machine: [sessionID…]}` (sorted keys; file removed when empty).
5. If `pendingMemory` present: merge into `S/groups/pending-memory.json` (`{botID: [chatID…]}`, per bot the sorted union), written with sorted keys; on success remove the pref `groupMemoryWaiting`.
6. `describeBots()` (`:302-321`): for each `bots.json` bot with `machine == nil` whose `folder` exists: write `<folder>/bot.json` if absent (`Bot.Descriptor`, (c)); write `<folder>/session` = `sessionID` if it differs; `<folder>/directory` = `bot.directory` (= `project ?? <folder>/workspace`) if absent; `<folder>/via` = `bridge` if `agents == true` and absent.
7. Only if `hasList` and `failed` is empty: `keepOldFiles` (`:326`) copies every `S/groups/<grp_id>.json` to `S/Backups/groups-<app version>-<first 8 of hash>/groups/<name>` and `S/groups.json` to `…/groups.json` (originals **stay**); then `imported.list = hash`, `imported.ids = sorted(union(imported.ids, moved, skipped))`, written to `S/groups/imported.json` by `JSONEncoder` (`{"ids":[…],"list":"…"}`).
- Orphan seats are deleted later, per machine as it connects, by the app: `dropOrphanSeats(on:api:)` `:177` → engine `DELETE` session; 404 counts as done. **This has no box-side counterpart** (`grep orphan-seats box/src` finds nothing): `orphan-seats.json` is read only by Swift.

### Route
None: files only.

### Box reader (`box/src/agents/groups/store.ts`)
- `GroupStore.load()` (`:217`): every folder of the root named `^grp_[a-z0-9]+$`; `read(id)` (`:230`): `group.json` must have numeric `kuvo <= 1` and `group` passing `isGroup` (`:78`: `id` equal to the folder, `name` string, `members` string[], `created` date, `seats`/`heard` string maps, optional `seatBoxes` map, `purpose`, `agentSeats` string[], `retracted` map, `viewed` date, `meeting` per `isMeeting`); `status === "held"` else active; optional `talking`, `paused`, `turning`. `messages.json` must pass `isMessages` (`:99`) **whole** — one bad message and the chat isn't loaded (which is why Swift filters first).
- Written by the bridge as `{kuvo: 1, status, talking?, paused?, turning?, group}` (`saveGroup` `:238`), JSON 2-space.
- `pending-memory.json`: `pendingMemory()` `:397` keeps entries whose key matches `^bot_[A-Za-z0-9]+$` with a string[] value; consumed by `runner.catchUpMemory()` (`groups/runner.ts:220`, started from `agents/main.ts:439`, each minute).
- A `"held"` chat no move holds is woken by the box as it starts (`moves/move.ts:1642-1671`, `wakeHeldGroups`).
- Other files in that root (bridge's own): `settings.json`, `peer-memory.json`, `peer-work.json`, `peer-turns.json`, `peer-outbox.json`, `<id>/awake`.

**port:** read `groups.json`, `groups/<id>.json`, `bots.json`, prefs `groupMemoryWaiting` and `movesUnderWay`, `groups/imported.json`; write `<groups root>/<id>/messages.json` then `<id>/group.json` `{"kuvo":1,"status","group"}`, merge `<groups root>/pending-memory.json`, keep `imported.json` and the `Backups/groups-<version>-<hash8>` copy, write each Mac bot's `bot.json`/`session`/`directory`/`via` where missing; orphan seats (sessions on machines) need a new box-side home or are said as dropped, since only Swift read `orphan-seats.json`.

---

#### b. Personalities — `Stores/Personalities.swift` `moveToBox` (`:211`, called `Keeper/MacBoxRunner.swift:438` before the box starts), `takeFromOld` (`:225`)

### Input
- `S/personalities.json`: v0.6.6 = v0.7.2 (no diff): `JSONFile.write(custom, readable: true)` → array of `{"description","id": "custom.<lowercase uuid>","name","symbol": "person.crop.circle"}` (`v0.7.2:KuvoCode/Stores/Personalities.swift:4-9,36,70`). **No `updated` field.** Presets (`preset.*`) are never in the file.
- v0.8.0-beta.1 adds `"updated"?: Double` (ms count) on each, and `S/personalities-removed.json`: array of `{"id","removed": Double}` (beta.1 `Personalities.swift:13,49-52,139-140`). Not present in 0.6.6/0.7.2.
- Preference `personalities.takenFromOld`: Double, the old files' latest modification time (`timeIntervalSince1970`, seconds) when last taken.

### Algorithm
- `moveToBox` (`:211-221`): only if at least one old file exists **and** `S/bots/personalities.json` does not: `oldLibrary` = personalities with `updated = kept.updated ?? 0`, removed = the removed file or `[]`; write `S/bots/personalities.json` = `{"kuvo":1,"personalities":[…],"removed":[…]}` (sorted keys, slashes unescaped, atomic); set `personalities.takenFromOld` = old files' max mtime. Old files stay.
- `takeFromOld` (`:225-230`, once the box is up): if old mtime > pref → `POST kuvo/personalities/take {"document": {"personalities":[…],"removed":[…]}}` → `{"changed":[ids]}`; then set the pref. This is the merge path when the box file already exists.

### Box (`box/src/link/personalities/library.ts`)
- File: `PERSONALITIES_FILE = BOTS/personalities.json` (`:67`), lock `RUN/.personalities.lock`. Written as `JSON.stringify({ kuvo: 1, ...library }, null, 2) + "\n"` (`:128`) = `{"kuvo":1,"personalities":[{"id","name","description","symbol","updated"}],"removed":[{"id","removed"}]}`.
- `keptPersonality` (`:83`): `id` must match `^custom\.[A-Za-z0-9._-]{1,80}$`; `name` = `plainName(name).slice(0,80)`, `description` trimmed, ≤2000; both non-empty or the entry is dropped; `symbol` string ≤80 else `"person.crop.circle"`; `updated = Number(updated) || 0`; count ≤ 2^52. Merge rule (`newerTake` `:93`): a removal beats a change; else higher count; tie → the one whose `name\ndescription` sorts last. Removals: newest 500.
- Route: `takePersonalitiesDocument` (`personalities/take.ts:33`) → `meetPersonalities`, then `noteDocsChanged()` and `retellBots`.

**port:** read `S/personalities.json` (+ `personalities-removed.json` from beta.1); write/merge `BOTS/personalities.json` as `{"kuvo":1,"personalities":[{id,name,description,symbol,updated:0 if none}],"removed":[…]}` using `mergePersonalities`/`changePersonalities` (so an existing box library is met, not overwritten); the pref `personalities.takenFromOld` is the app's bookkeeping only.

---

#### c. Bots' old edits — `Stores/Bots.swift` `takeOldWaitingEdits` (`:731`, Calibration step 2), `handOverOldEdits` (`:764`, each time this Mac's bots are followed, `:391`)

### Inputs
- Preference `botEditsWaiting` (v0.6.6, v0.7.2, v0.8.0-beta.1: `Bots.swift:104`): plist dictionary `[String: [String]]` = bot ID → which things changed while its machine was away, values among `"instructions"`, `"name"`, `"model"` (comment at v0.7.2 `:100-102`). It does **not** hold the new values; those are in `bots.json`.
- Preference `botEditOutbox`: **in no released tag** (`git grep` finds it in none of v0.6.6/v0.7.2/v0.8.0-beta.1); only builds after beta.1 wrote it. `Data` JSON `{botID: {"box": "mac"|machineID, "fields": {…as PATCH kuvo/bots/<id> takes…}, "delete": bool}}` (`OldEdit`, `:747-751`).
- Preference `botInstructions` (all three tags, `private static let sentKey`, v0.7.2 `:846`): plist dictionary `[String: String]` = session ID → sha256 hex of the instructions text last sent to that bot's session; a cache so a launch re-sends only changed ones. **Not referenced on this branch** (`grep botInstructions swift/KuvoCode` → nothing). No box equivalent needed.
- `S/bots.json` (`JSONFile.write(list, isoDates: true, readable: true)`): array of `Bot`. v0.7.2 (`Bots.swift:10-42`): `{"id","name","job","instructions","jobInMemory"?: bool,"look": {"color","shape","face"},"machine"?: machineID,"project"?: folder (app's way, `ssh://<machine><path>` for a machine's),"folder": absolute path where it lives,"sessionID","created": ISO,"model"?: {"id","providerID","variant"?},"personality"?: id,"agents"?: bool,"updated"?: ISO}`. v0.6.6: the same without `updated`. beta.1: same as 0.7.2. Branch adds `limits`, `allowed` (a cache of what boxes said, `:125`, `:803`).
- In v0.7.2 each bot's folder already has `session`, `directory`, `via` (`writeSession`, v0.7.2 `:513-538`) and `bot.json` (`Bot.Descriptor`, written by `describe`, and Calibration step 1 `describeAll(on: nil)`). v0.6.6 has **no** `bot.json` (`git grep -c bot.json v0.6.6 … Bots.swift` → none) and no Calibration.

### Algorithm
- `takeOldWaitingEdits` (`:731-742`): for each id in `botEditsWaiting` whose bot is in the list **with a machine** and not already in the outbox: `fields = {"name": bot.name, "look": bot.look, "personality": bot.personality or null}` plus `"model": bot.model` when set; `outbox[id] = {box: bot.machine, fields}`; save `botEditOutbox`. (`botEditsWaiting` itself isn't removed here. Note the job/instructions change, `"instructions"`, is **not** carried: only name, look, personality, model.)
- `handOverOldEdits` (`:764-786`): for each outbox entry sorted by id: `DELETE` or `PATCH` `kuvo/bots/<id>?on=<machine>` (+ `&removed` when the machine isn't one of the app's, `isRemoved` `:215`), body = the `fields` object. Not answered, or status ≥500, 409, 404 → kept for next time; any other status (2xx, or a refusal logged) → entry removed and the pref rewritten.
- `describeOld` (`:703-725`, Calibration steps 1 and 3): for each bot with `machine == nil` whose folder exists: rewrite `<folder>/bot.json` when it doesn't decode, has another `id`, or `kuvo != 1`; write `<folder>/session` when it differs from `sessionID`.
- `bot.json` as Swift writes it (`descriptorData` `:789`, pretty, sorted, ISO dates): `{"created","id","job","kuvo":1,"look":{"color","shape","face"},"model"?,"name","personality"?,"project"? (path as the computer it lives on writes it),"updated" (= `updated ?? created`)}`.

### Box
- Route `PATCH|DELETE kuvo/bots/<id>?on=…[&removed]` → `holdEdit` (`box/src/link/areas/bots.ts:54`, `bots/held.ts:234`). Held file: `RUN/bots-held/<botID>.json` = `{"id": "bte_<base64url>", "bot", "to": peer key ("" when unknown), "on"?: name, "change"?: {name?,look?,personality?,job?,model?,effort?}, "times"?: {field: ms}, "delete"?: true, "since": ms, "at": ms, "known"?: {name, look?}, "seal": base64url}` (`:86-98`). **The seal is an HMAC keyed from this box's link secret** (`sealWith` `:108`, `secretOf` `:106`: on a Mac `macLink.overlay?.secret`, in memory); `readHeld` drops any file whose seal doesn't check. `holdEdit` also needs the keeper peer (`keeperOf` `:151`: from peers' agents lists, or `on` matching a peer's key or a machine peer's `id`), else answers 404 (which the Swift loop keeps for next time).
- What the box reads of a Mac's bots: only their folders — `botHere` (`bots/here.ts:44`): `BOTS/<bot_id>/bot.json` with `kuvo === 1`, matching `id`, string `name`, and a non-empty `BOTS/<id>/session`; `via` = `bridge` → runs with the agents.
- `bots.json` is read by the box in exactly one place: `jobs.ts` `macListed()` (`:72`, file `path.dirname(BOTS)/bots.json`) inside `jobOf` (`:46`): when a bot has no `job.json` and no `## Job` in `MEMORY.md`, and `IS_MAC`, and the listed bot's `jobInMemory !== true` → job = `{line: plainName(listed.job), details: detailsBeyond(line, listed.instructions)}`, written to `BOTS/<id>/job.json` as `{"line","details","updated"}` (and `job` mirrored into `bot.json`). `keepJobs()` (`:82`) runs this for every bot as the box starts (`main.ts:359`, `:879`).

**port:** read prefs `botEditsWaiting` (+ `botEditOutbox` if present) and `bots.json`; make sure each Mac bot's folder has `bot.json` (kuvo 1) and `session` (0.6.6 has no `bot.json`); keep `bots.json` beside the bots folder (or run `keepJobs` against it) so jobs from `job`/`instructions` are taken; the held edits can't be written as files without the box's link secret (the seal) and peers, so they are either handed to `holdEdit` once `serve` runs or said as dropped; `botInstructions` is dropped.

---

#### d. Settings › Git — `Stores/GitBranches.swift` `GitPreferences.move` (`:183`), called `App/AppModel.swift:723` (this Mac's box) and `AppModel+Machines.swift:616` (each machine)

### Input (same keys in all three tags, `GitBranches.swift:219-222` in 0.7.2)
- `git.branchPrefix`: String; unset means the default `"kuvo/"`; `""` means no prefix. 0.7.2 cleaned it on read (trim, drop whitespace and `~ ^ : ? * [ \`, control characters, collapse `..`, strip leading `-`/`/`/`.`).
- `git.commitInstructions`, `git.pullRequestInstructions`: String, free text; unset or blank = none.

### Algorithm
`kept` = the three that are set (as strings) → `POST kuvo/git/settings/move` with body `{"branchPrefix"?: string, "commitInstructions"?: string, "pullRequestInstructions"?: string}`; nothing set → no request. Answer `{"moved": bool}` ignored; prefs are kept (for a computer added later).

### Box — `moveGitSettings` (`box/src/link/areas/git-words.ts:120`, route at `areas/changes.ts:1204`)
- 400 unless an object with each offered value a string; 409 if `!ownsSettings()` (never on a Mac: `OWN_MARK` is null there, `config.ts:14,39`); under `CONFIG_LOCK` (`RUN/.config.lock`):
- `real` = offered values: `branchPrefix` as given (an empty one counts), instructions trimmed and only if non-empty.
- `movedBefore = (BOX/git.json).moved === true`. If not moved before: for each `real` entry whose settings key is `undefined` in the settings → write it. If not moved before and `real` is non-empty → `BOX/git.json` gets `{"moved": true}` merged in (`keepInGitFile` `:40`, compact JSON). So: the first Mac that offers anything wins once; values the box already has are never overwritten; after `moved`, nothing changes.
- Then `takeOldBranchPrefix` (`:97`): a `branchPrefix` an older box kept in `BOX/git.json` is taken into the settings where they have none, and removed from `git.json`.
- Where the values land: top-level keys of the engine's settings file `CONFIG_FILE`: `"branch_prefix"`, `"commit_instructions"`, `"pull_request_instructions"` (`GIT_SETTINGS` `:15-19`), written by `writeSettings` (`config.ts:108`: whole file, keys sorted, 2-space, trailing newline, 0600, via temp + rename, through a symlink to its target). `readSettings` refuses a file that isn't plain JSON (`problem`), and then nothing is written.
- Defaults on read: `branchPrefixHere` = `cleanBranchPrefix(set)` when a string, else `"kuvo/"`; instructions `""`.

**port:** read the three prefs; write into the Mac's engine settings (`…/opencode/opencode.json`) the keys `branch_prefix` / `commit_instructions` / `pull_request_instructions` only where absent and only while `box/git.json` has no `"moved": true`, then set `{"moved": true}` there (call `moveGitSettings(given, file, lock, gitFile)` directly: it takes the paths as arguments).

---

#### e. Letters — `Stores/AgentMail.swift` `handOver(to:)` (`:221`), set up via `Stores/PhoneLink.swift:311`

### Input
- `S/agent-messages.json` (v0.6.6, v0.7.2, beta.1; `JSONFile.write(waiting)`, compact): array of `Waiting` `{"from"?: sessionID, "name": sender's name, "to": sessionID, "machine": machineID, "text"}` (v0.7.2 `AgentMail.swift:30-38`). Messages kept for an agent whose machine was away.
- `S/agent-letters.json` (**beta.1 only**): array of `Mailing` `{"id","from"?,"to","toName","machine","text","wait": bool,"at": ms,"answers"?}` (beta.1 `:686-696`). The branch decodes `{id, from?, to, machine, text, at, answers?}` and ignores the rest (`:200-208`).
- `S/agent-letters-seen.json` (**beta.1 only**): `{letterID: ms}`.
- `S/agents-elsewhere.json` (**beta.1 only**): just deleted (`:223`).
- `phone-link.json` `machines`: `{machineID: {"name","address","key","anotherMac"?,"peers"?,"sign"?}}` — needed to turn a machine into its box's key (`PhoneLink.key(of:)` `:800`: `machineHosts[machine].key` **only if** `peers == true && anotherMac != true`). In v0.7.2 `MachineHost` is `{name, address, key, anotherMac?}` — **no `peers`** — so for a 0.7.2 file every `key(of:)` is nil until this version has connected to the machine once.

### Algorithm (`:221-252`)
If none of the three files exists → nothing. `letters` = each `Mailing` whose machine has a key → `{id, from, to, text, peer: key, at, answers}`, plus each `Waiting` whose machine has a key → `{id: "wtg_" + hex(first 16 bytes of sha256(from ?? "" + "\0" + to + "\0" + text)), from, to, text, peer: key, at: now ms}`. `POST kuvo/link/letters/take {"letters": […], "seen": {id: ms}}`; only on 200: delete `agent-letters.json` and `agent-letters-seen.json`; `agent-messages.json` is rewritten with the `Waiting` entries whose machine had no key (or deleted when none are left).

### Box — `box/src/link/mac/routes.ts:231-258`
- Up to 500 letters; each needs `id` matching `LETTER_ID` `/^[A-Za-z0-9_-]{6,80}$/`, string `to`, `text`, `peer`; written to `RUN/outbox/sending/<id>.json` (`SENDING`, `letters/letter.ts:19-20`) as compact JSON `{"from": string|null, "to", "text": cutText(text) (≤32 000 UTF-16 units), "wait": false, "at": number (given or now), "peer", "answers"?}`.
- `seen`: merged under `PEER_LOCK` into `RUN/letters-seen.json` (`LETTERS_SEEN`) = `{id: ms}`, only ids matching `LETTER_ID` with `now - at < 2 days` (`LETTER_MEMORY`), keeping the larger time.
- Then `macSendOutbox()`. A letter older than a day (`LETTER_LIFE`) isn't delivered by the receiving box.

**port:** read `agent-messages.json` (+ beta.1's `agent-letters.json`, `agent-letters-seen.json`) and `phone-link.json`'s `machines` for each machine's `key`; write `run/outbox/sending/<id>.json` `{from,to,text,wait:false,at,peer,answers?}` and merge `run/letters-seen.json`; from 0.7.2 nothing says a machine "talks to peers", so `agent-messages.json` entries either go by the bare `key` (a decision) or stay/are said as not carried; delete `agents-elsewhere.json`.

---

#### f. Worktrees, projects, session notes — `Stores/Workspace.swift`

### Preferences (v0.6.6, v0.7.2, beta.1 all write the first five; shapes equal unless said)
- `worktrees`: `Data` JSON `{"<folder its sessions run in>": {"project": primary folder, "root": worktree's top folder, "branch", "base"?}}` (`Worktree`, v0.7.2 `:213-222`, saved `:818`). A machine's are written the app's way: `ssh://<machineID><absolute path>` (`RemoteFolder`, v0.7.2 `Stores/Machines.swift:983-1005`) in the key, `project` and `root`.
- `removedWorktrees`: plist dictionary `{folder: project}` (v0.7.2 `:161`), same naming.
- `projectFolders`: plist string array, the sidebar's order (bots' folders filtered out on read: under `S/bots/` or containing `/.kuvo-code/bots/`, branch `:649`).
- `projectSettings`: `Data` JSON `{"<directory>": {"name"?, "folders": [..], "run"?, "terminal"?}}`. `terminal` exists in v0.7.2 (`:199-209`) and not in v0.6.6 (its struct is `name, folders, run`); the branch's struct has no `terminal` and the box strips it (`areas/projects.ts:50`).
- `remoteSessions`: `Data` JSON `{sessionID: {"machine", "title"?}}` (v0.7.2 `:72-75`): which machine a session is on while that machine is away. A client's cache; the branch still reads/writes it (`:2787-2792`) and never sends it anywhere.
- Branch-only bookkeeping (in no tag): `removedWorktreesHandedOver.<box>` Bool (`:475`), `projectsOnBox.<box>` Bool (`:1826`, `:1846`), `projectsUnsent` string array of folders (`:1830`). `<box>` is `"mac"` for this Mac (`BoxClient.mac`, `Stores/BoxClient.swift:15`) or a machine's id.
- Pinned / archived in v0.7.2: **not in preferences** — flags in the engine session's metadata, literal keys `"kuvo.archived": true` and `"kuvo.pinned": true` (v0.7.2 `:355,362`, `setFlag` → `api.setMetadata`), plus the engine's own `time.archived`. They live in the engine's database and travel with it. The box reads the same: `isArchived` = `time.archived || metadata["kuvo.archived"] === true || metadata.kuvo?.archived === true`, `isPinned` = `metadata["kuvo.pinned"] === true` (`box/src/link/lists/rule.ts:45,48`).

### How the branch hands each over
- Kept worktrees (`:1486-1512`, inside the read of a box's worktrees): for each `worktrees` entry whose `root` is on that box and whose root the box doesn't list: `POST kuvo/worktrees/record {"project", "root", "folder": <the dictionary key>, "branch", "base"?}` with paths as the box names them (`ssh://machine` prefix stripped, `onBox` `:1817`). 404 → dropped here too; other <500 → skipped; error → retried next time.
- Removed worktrees (`handOverRemovedWorktrees` `:474-490`): once per box (pref `removedWorktreesHandedOver.<box>`): for each `removedWorktrees` entry on that box, sorted by folder: `POST kuvo/worktrees/record {"removed": true, "folder", "project"}`.
- Projects (`loadProjects` `:1838-1853`): the first time a box answers `GET kuvo/projects` (pref `projectsOnBox.<box>` unset): every folder in `projectFolders` on that box plus every key of `projectSettings` on that box goes into `projectsUnsent`; then `send` (`:1907`): for each, sorted: `PUT kuvo/projects?directory=<path>` with body (sorted keys) `{"folders": [paths on the box], "name": name or "", "removed": !projectFolders.contains(folder), "run": run or ""}` (`boxBody` `:1947`). 2xx or 400 → off the unsent list.

### Box
- `K/box/worktrees.json` (`areas/worktree-records.ts:13,108`), 2-space JSON: `{"worktrees": [{"project","root","folder","branch","base"?, "made"?, "prefix"?}], "removed": {folder: project} (latest 200), "restorable": [record + "head", "snapshot"?], "handedOver"?: {folder: project}}`.
  - `kuvo/worktrees/record` (`areas/worktrees.ts:293-311`): `removed: true` → needs absolute, different `folder` and `project`; added to **`handedOver`** unless the folder is a kept worktree's or already in `removed`/`handedOver`. Otherwise needs absolute `project`, `root`, `folder` (defaults to `root`), non-empty `branch`; **404 when `root` doesn't exist on disk**; appended to `worktrees` unless one with that `root` is there.
  - `worktreeProjects()` (`:61`) is the rule sessions are filed by: `handedOver`, `removed`, `restorable`, `worktrees` (root and folder), plus git's own linked worktrees.
- `K/box/projects.json` (`areas/projects.ts:26,258`), 2-space: `{"version": 1, "projects": [{"directory", "name"?, "folders": [..], "run"?, "removed"?: true, "madeBy"?, "addedByBox"?}]}`; lock `projects.json.lock`. `PUT` → `keptProject` (`:285`): `removed` only when sent `true`; `name` trimmed, dropped when empty or equal to the folder's basename; `folders` deduplicated, absolute, without the directory itself; `run` trimmed, dropped when empty. A new one is put **first** (`[project, ...projects]`, `:421`), so sending in sorted order leaves the box's list in reverse of that; the Swift app kept its own order in `projectFolders`. **Unverified**: which order the desktop app draws (box's list order or its own).

**port:** read prefs `worktrees`, `removedWorktrees`, `projectFolders`, `projectSettings` (this Mac's entries only: those not starting `ssh://`; a machine's belong to that machine's box); write `box/worktrees.json` (`worktrees` for roots that still exist, `handedOver` for removed ones) and `box/projects.json` (`{"version":1,"projects":[…]}`, in `projectFolders` order, settings-only ones as `removed: true`, `terminal` dropped and said); `remoteSessions`, `projectsOnBox.*`, `projectsUnsent`, `removedWorktreesHandedOver.*` are client bookkeeping, dropped; pinned/archived need nothing (engine metadata).

---

#### g. Passwords — `Stores/Passwords.swift` (`Passwords.ready` `:137`, `Legacy` `:252`)

### Input
- **No released tag has a vault** (`git grep` for `movedToVault`/`VaultValues`/`.vault"` in v0.6.6, v0.7.2, v0.8.0-beta.1 → nothing; `Passwords.swift` is byte-identical across the three). All saved sign-ins of 0.6.6–beta.1 are Keychain **internet passwords** (v0.7.2 `Passwords.swift:76-145`): `kSecClass = kSecClassInternetPassword`, `kSecAttrCreator = 0x4B55564F` (`'KUVO'`; a non-`run.kuvo.code` bundle uses `'KV'` + two hashed letters), `kSecAttrServer = host`, `kSecAttrAccount = username` (may be `""`), `kSecAttrProtocol = kSecAttrProtocolHTTPS`, `kSecAttrLabel = host`, `kSecAttrDescription = "Kuvo Code browser password"`, data = the password, UTF-8. No service, no access group.
- `passwords.agentSignIn`: string array (sorted) of hosts agents may sign in to on their own.
- `passwords.agentSignInAccounts`: plist dictionary `{host: account}` — the account allowed for each such host. (v0.7.2 `keepAgentAccounts` `:59` fills a missing one with the host's newest login at launch.)
- `passwords.never`: string array (sorted) of hosts you chose Never for. `passwords.offersToSave`: Bool, default `true`. Both are still read by the Swift browser on the branch (`:23-27`, `:347-352`) and go nowhere: they belong to the side panel's browser (a client's).
- `passwords.movedToVault`: Bool, branch only: the move below is done.

### Algorithm (`ready`, `:137-171`)
Refresh this Mac's vault; if `passwords.movedToVault` → done. `found` = every internet password with the creator code (nil if the Keychain can't say → try again later). For each: read its password (unreadable → left, try again); if the vault has a sign-in whose `sites` contains the host with the same username → make it `signInOnly` if it isn't; else add an item: `VaultDraft(kind: .login, name: freeName(host, account), sites: [host], username: account or nil, signInOnly: true, values: {"password": …})`, where `freeName` (`:120`) = `host`, else `"host (account)"`, else `"host 2"`, `"host 3"`… (case-insensitive against every item's name). If `agentSignIn` contains the host **and** `agentSignInAccounts[host] == account` → grant `{everyone: true}` `how: "own"`. Then delete the Keychain item. When none is left: set `passwords.movedToVault`, remove `passwords.agentSignIn` and `passwords.agentSignInAccounts`.

### Box (`box/src/link/vault/store.ts`, routes `areas/vault.ts`)
- Routes: `POST kuvo/vault/items` (`:282`) body `{kind, name, sites, username?, signInOnly?, values}` → `{item, key?}`; `POST kuvo/vault/grants` (`:243`) body `{"item": id, "who": {"everyone": true}, "how": "own"}`.
- Files in `K/box/vault/`: `items.json`, `grants.json`, `paused.json`, each **signed**: `{"v": 2, "gen": n, "data": …, "sig": base64url}` with `sig = HMAC-SHA256(fileKey, "<basename>\0<gen>\0" + JSON.stringify(data))` (`:68`, `keepSigned` `:153`), 1-space JSON + newline. `data` of `items.json` = `[{"id": "v_<10 a-z0-9>", "kind": "login", "name", "sites": [host], "username"?, "fields": ["password"], "signInOnly"?: true, "created": ms, "changed": ms, "used"?}]`; `grants.json` = `[{"item", "who": {"everyone": true}, "how": "own", "at": ms}]`; `paused.json` = `{}` is written with the first item (`:414`). `addOne` checks: name unique (409), a login needs a site, each value ≥ 4 characters (`SHORTEST_VALUE`, `:361`) and ≤ 1 MB — **a saved password shorter than 4 characters is refused**.
- Values on a Mac (`onHost()`, `KEYS_ON_HOST`): not in files — asked of the host (`vault/set {id, values}`), which keeps them in the Keychain: generic password, service `run.kuvo.code.vault`, account = the item's id, data = JSON `{field: value}`, label `"Kuvo Code vault item"`, `kSecAttrAccessibleAfterFirstUnlock` (`Stores/VaultValues.swift:11-41`).
- Where the vault's key lives on a Mac: the same service, account `files`: JSON `{"key": base64url of 32 bytes, "generations": {"items": n, "grants": n, "paused": n}, "adopt"?: bool}` (`VaultValues.swift:44-88`). The app hands the key to the box on `serve`'s stdin (`home.ts:225-239`, `takeVaultSecrets`; env `KUVO_VAULT_FILE_KEY` is a test's). The box checks each file's `gen` against the host's (`primeGenerations` `:89`): a file whose generation the host doesn't know counts as tampered.
- `kuvo/vault/key` (`areas/vault.ts:191`): **409 on a Mac** (“This Mac's vault is its Keychain: it has no key to give.”). It exists only for a machine's vault: `{"key"}` = the unlock key (base64url, 32 bytes) that wraps the store key in `box/vault/key.json` `{"v":1,"wrapped","check"}`; values there are AES-256-GCM files `box/vault/values/<id>`.
- Already built for the move: `kuvo-keys passwords --home <K>` (`mac/helper/Sources/KuvoHelperKit/KeysTool.swift:161-190`) writes `K/updates/rehome/passwords.json` (0600, **plain text**) = `{"logins": [{"account","host","modified"?: ms,"password"}], "v": 1}` and leaves the Keychain items; the notes (§8a.2) say this isn't to ship as it is. `grep owed box/src/link/rehome` shows `owed: ["keychain", "agent"]` only (`move.ts:587`): **no `owed: passwords` step exists yet**, and nothing in `box/src/link` reads `passwords.json`.

**port:** read the internet passwords (creator `'KUVO'`; only `kuvo-keys` can) and prefs `passwords.agentSignIn`, `passwords.agentSignInAccounts`; write through the vault once `serve` runs with its key (items can't be written as files: they are signed with a Keychain-held key and their values live in the Keychain): per login `addItem({kind:"login", name: free name, sites:[host], username, signInOnly:true, values:{password}})`, then `setGrant(id, {everyone:true}, "own")` where the host is in `agentSignIn` and its account matches; the two agent prefs have to be carried into the hand-over (today's `passwords.json` has no field for them); `passwords.never` and `passwords.offersToSave` are the browser's and are dropped or left to the client.

---

#### h. Keep This Mac Awake — `Stores/Awake.swift`

- Preference `keepsAwake` (v0.6.6, v0.7.2, beta.1: `Awake.swift:8`): Bool; unset = **on** (`UserDefaults.standard.object(forKey:) as? Bool ?? true`, branch `:14-16`).
- Branch: `Keeper/OwnBoxMove.swift:129` `keepsAwakeOff: { !Awake.isOn }` → the box's move is run with `--keeps-awake off` (`:402`, `:441`).
- Box: `K/box/work.json` (`WORK_SETTINGS_FILE`, `work/settings.ts:12`), 2-space JSON object merged key by key (`:61-68`), here `{"keepsAwake": false}` (beside `projectsFolder`). `keepsAwakeSetting` (`:74-77`): on unless `keepsAwake === false`. Already in the move: `rehome/move.ts:516-521` calls `setKeepsAwake(false, box/work.json)` when `journal.keepsAwakeOff`.

**port:** read pref `keepsAwake`; when it is `false`, write `box/work.json` `{"keepsAwake": false}` (already done through `--keeps-awake off`; reading the pref itself would replace the flag).

---

#### i. Bots' own keys — `Stores/BotKeys.swift` `handOver` (`:149`)

- v0.7.2 Keychain items (`v0.7.2:KuvoCode/Stores/IntegrationKeys.swift:11-30,101-135`): generic password, service `run.kuvo.code.integrations` (bundle id + `.integrations`), account = the variable: shared `KUVO_KEY_<ID>_<NAME>`, a bot's own `KUVO_KEY_<ID>_<NAME>__<BOT>` where each part is uppercased with every non-letter/digit as `_` (so `bot_abc123` → `__BOT_ABC123`), label `"Kuvo Code integration key"`, `kSecAttrAccessibleAfterFirstUnlock`. Every bot's keys were there, this Mac's and machines'.
- Branch `handOver` (`:149-178`): for each machine's bot with no project: its variables (from the Keychain's names, or from the box `GET kuvo/box/kept-keys?bot=<id>` once the box has moved); for each integration all of whose keys are there: `PUT kuvo/bots/<id>/keys/<service> {"keys": {name: value}}` to the machine, then remove them (Keychain, or `DELETE kuvo/box/kept-keys?bot=<id>` `{"names": […]}`).
- Box now: **bots have no keys of their own** (#726, `box/src/link/bots/own-keys.ts:1-13`). `dropBotKeys` (`box/src/link/keys.ts:129-143`) as the box starts: asks its host for `keys/names`, deletes every name matching the bot-key pattern (`…__BOT_…`) through `keys/delete`, removes such files from `box/bot-keys`, `box/keys`, `keys`, and the `box/bot-keys` folder. Each bot's folder settings (`workspace/.opencode/opencode.json`) lose servers naming such a key, and the integration is noted in `<bot folder>/keys-gone.json` `{"integrations": [ids]}`.

**port:** read nothing; write nothing — the box's own start (`dropBotKeys`, `own-keys.ts`) removes what's left; the Keychain copy made by `kuvo-keys copy` may skip accounts containing `__BOT_`, said as dropped.

---

#### j. Machines' group chat cache — `Stores/BoxGroups.swift` `takeOldCache` (`:120-134`)

- Input: `S/machine-groups.json` (v0.7.2 and beta.1; not in v0.6.6, which has no `MachineGroups.swift`): `JSONFile.write(chats, isoDates: true, readable: true)` → array of `Chat` `{"discussing": bool, "group": BotGroup (as in a), "held": bool, "machine": machineID, "paused": bool, "speaking"?: botID}` (v0.7.2 `Stores/MachineGroups.swift:14-22,479`). `S/machine-groups/<groupID>.json`: array of `GroupMessage` (`:484`). beta.1's differs from 0.7.2 only in where the denied rules come from, not in what's kept.
- Algorithm: if the old file exists: append each old chat not already in `S/group-chats.json` (same `group.id` and `machine`), write `group-chats.json`, move each `machine-groups/<id>.json` to `S/group-chats/<id>.json` unless one is there, then delete `machine-groups/` and `machine-groups.json`.
- `group-chats.json` / `group-chats/` are the Swift window's cache of what each computer said (`BoxGroups.file`, `:96-97`): a client's, read by nothing in `box/`. The truth is each machine's own `~/.kuvo-code/groups`.

**port:** read nothing for the box; nothing to write. `machine-groups.json` and `machine-groups/` are a client cache: carried as files only if the last copy of a gone machine's chats is to be kept (a decision), otherwise said as dropped.

---

#### k. `movesUnderWay` — `Stores/Moves.swift`

- Shape (v0.7.2 = beta.1, `Moves.swift:80-106`; v0.6.6 has no `Moves.swift`): `Data` JSON array of `{"kind": "bot"|"chat"|"group", "id": botID | session | groupID, "sessions": [sessionID…], "from"?: machineID (absent = this Mac), "to"?: machineID, "original"?: folder, "copy"?: folder, "wasPaused": bool, "stage": "copying"|"switched"}`; removed from the prefs when empty.
- Branch: `Moves.init` (`:171-174`) just removes the key. `GroupMigration` reads it first (AppModel constructs `boxGroups` at `:87`, before `moves` at `:96`) to mark a moving chat `"held"`.
- Box: moves are the boxes' own now (`box/src/link/moves/*`, records in `RUN/moves`, `left.json`). As it starts, `resumeMoves` (`moves/move.ts:1642-1671`) lifts any bot's or chat's folder whose `paused` file says `moving` that no move of its own holds (`unhold(folder, false)`), and wakes held group chats nothing holds (`wakeHeldGroups`). A 0.7 move at stage `switched` (the copy is what counts, the original still to go) is **not** finished by anything: the original stays where it was.

**port:** read `movesUnderWay` only for (a)'s `"held"` status; write nothing — the box lifts the holds itself; an unfinished 0.7 move's leftover original is said, not cleaned up.

---

#### l. `copilotClaudeModels`

- v0.6.6/v0.7.2/beta.1 `Stores/ProviderCompatibility.swift:13-33`: string array of GitHub Copilot's Claude model ids (`claude-…`) learned from an engine's list; unset → `["claude-opus-5.5","claude-sonnet-5.5","claude-haiku-5.5","claude-fable-5.1"]`. It made the engine settings `providers.github-copilot.models.<id>.compatibility.supportsThinkingBlockBinding = false`.
- Branch: the file is gone; the only reference is `App/AppModel.swift:688` `UserDefaults.standard.removeObject(forKey: "copilotClaudeModels")`. PROTOCOL.md `:10278-10279` says each box learns it itself ("Settings per box"). I did not read the box code that learns it (**unverified** which file; `box/src/link/compat/` holds only `adopt-settings.ts`, `launch-lock.ts`, `leave-running.ts`).

**port:** read nothing, write nothing; dropped, said.

---

#### m. `toolApproval`

- v0.6.6/v0.7.2/beta.1 `Views/Session/ComposerMenus.swift:6-12`: String, `"approveAll"` or `"askFirst"`; unset = `approveAll`. The default approval for a new session (`@AppStorage` in `SessionView.swift:137,350`), and for a new bot (`Bots.swift:246`), a group's seats (`BotGroups.swift:1444`, `MachineGroups.swift:436`) and a session an agent starts with no parent (`AgentMail.swift:419`). `askFirst` = session rules asking for `edit, write, patch, shell, webfetch, external_directory, kuvo-browser_browser_save_pdf, kuvo-browser_browser_upload, kuvo-browser_watch_for_change`; `approveAll` = none.
- Branch: **the key is referenced nowhere** (`grep toolApproval swift/KuvoCode box/src protocol .claude/notes` → nothing). Approval lives on the box: `K/box/approval.json` (`work/approval.ts:35,54-58`), a merged object `{"last"?: "askFirst"|"acceptEdits"|"auto"|"fullAccess", "fullAccess"?: bool, "otherComputers"?: bool, "since"?: ms}`; `defaultApproval()` (`:82`) = `last` when valid (and Full Access confirmed if it is that), else `"auto"`. Routes `kuvo/approval` and `kuvo/sessions/<id>/approval` (`areas/approval.ts`). `since` (`approvalsSince` `:94`) is set the first time it's asked: a conversation from before it with no kept approval "ran as its rules say".
- There is no exact counterpart of `approveAll` among the four (it ran everything without asking, with no sandbox): mapping it to `auto` or `fullAccess` changes what the user already uses, so it is theirs to decide. `askFirst` → `"askFirst"` is exact.

**port:** read pref `toolApproval`; write `box/approval.json` `{"last": "askFirst"}` when it was `askFirst`; for `approveAll`/unset write nothing (the box's default `auto`) unless the user decides otherwise — today nothing carries it, to be said.

---

#### n. Everything else found

### Calibration (`Stores/Calibration.swift`; prefs `calibration` Int = last step done, `calibration.backedUp` Int)
| Step | v0.6.6 | v0.7.2 / beta.1 | Branch |
| --- | --- | --- | --- |
| 1 `"bots' descriptions"` | no Calibration at all | `app.bots.describeAll(on: nil)`: writes each of this Mac's bots' `bot.json` | `app.bots.describeOld()` (c) |
| 2 `"bots' waiting edits"` | – | – | `app.bots.takeOldWaitingEdits()` (c) |
| 3 `"bots' folders"` | – | – | `app.bots.describeOld()` again (rewrites a half-written `bot.json`, the `session` file) |

Before the due steps run, `Backups.make()` copies to `S/Backups/<yyyy-MM-dd HHmmss UTC> <version>/`: `bots.json`, `groups.json`, `groups`, `bots/personalities.json`, `personalities.json`, `personalities-removed.json`, `agent-messages.json`, `phone-link.json`, `box/machines.json` and `settings.plist` (the whole preferences domain, binary plist); newest 3 kept (v0.7.2's list: `bots.json, groups.json, groups, personalities.json, agent-messages.json, phone-link.json`). So a Mac coming from 0.7.2 has `calibration = 1`; from 0.6.6, unset.

### An upgrade the docs' lists don't name: the machines list
- Preference `machines` (all tags; v0.7.2 `Stores/Machines.swift:555`): `Data` JSON array of `Machine` `{"destination", "otherAddresses"?: [..], "identity"?, "reachedAt"?}` (v0.7.2 `:8-17`; same four fields on the branch).
- Branch `Machines.load` (`:1826-1840`): list = `machines.unsaved` pref if any, else the file; plus the `machines` pref's entries not already in it by `destination`; `store` (`:1789-1808`) writes `OwnBoxHome.file("machines.json", inSupport: "box/machines.json")` (compact `JSONEncoder` array, 0600), and only once it reads back equal removes both prefs. An unreadable pref is set aside under another key.
- Box reads it: `LIST_FILE = K/machines.json` (`box/src/link/mac/machines.ts:21`), with `phone-link.json` at `APP_FOLDER ?? K` (`:24`).
- **port:** read pref `machines` (and `machines.unsaved`); write `K/machines.json` = the array, merged by `destination` with one already there.

### Also carried by nothing listed, found while reading
- `phone-link.json` (`S/phone-link.json`; v0.7.2 `{"phones": […], "machines": {id: {name,address,key,anotherMac?}}}`): the branch reads it from `OwnBoxHome.file("phone-link.json", inSupport: "phone-link.json")` (`Stores/PhoneLink.swift:174`) and the box from `K/phone-link.json` in a home of its own. Its `machines` map is what (e) needs. I did not trace how `phones` reach the box's link state (**unverified**).
- `S/group-chats.json`, `S/group-chats/`: branch-only window cache (a, j).
- `S/groups/orphan-seats.json`: branch-only, Swift-only reader (a).

### §8a table ("What only the retired Mac app sends") and PROTOCOL.md `:10251-10289`, checked row by row
| Row | Verified | Note |
| --- | --- | --- |
| Group chats | yes | Line refs drifted: `GroupMigration.run` is at `App/AppModel.swift:87-90` (table says `:84`); bridge refs `store.ts:391`, `runner.ts:215`, `agents/main.ts:439` are right. Not named in either doc: `orphan-seats.json`/`dropOrphanSeats`, and `describeBots`. |
| Personalities | yes | `moveToBox` `:211`, `MacBoxRunner.swift:438` (table: `:436`). |
| Bots' edits | yes | `botEditOutbox` is in no tag (PROTOCOL says "builds after 0.8.0-beta.1", right). From `botEditsWaiting` only name/look/personality/model are carried, not a job change. The held file needs the box's secret. |
| Bots' keys | yes | Now simply dropped by the box (`dropBotKeys`). |
| Settings › Git | yes | Call sites `AppModel.swift:723`, `AppModel+Machines.swift:616`; `git-words.ts:120` (table: `:114`), `changes.ts:1204`. |
| Letters | yes | `AgentMail.swift:221` (table: `:190`), `mac/routes.ts:231` (table: `:221`). `agent-letters*.json` are beta.1 only; from 0.7.2 `agent-messages.json` entries find no peer key (`peers` isn't in 0.7.2's `phone-link.json`). |
| Worktree records | yes | `Workspace.swift:473`, `:1498`; `worktrees.ts:293` (table: `:289`). The docs don't name the **projects** hand-over (`projectFolders`/`projectSettings` → `PUT kuvo/projects`), which the same app does once per box (`:1838-1853`). |
| `passwords.agentSignIn*` | yes | The larger part is the passwords themselves (pre-vault internet passwords), which the table leaves to `owed: passwords`; no such step is in `box/src/link/rehome` yet. |
| Keep This Mac Awake | yes | `OwnBoxMove.swift:129` (table: `:70`); already in `rehome/move.ts:516`. `GeneralSettings.swift:60` is the row's label, the pref is in `Awake.swift`. |
| "Not taken in": `machine-groups*`, `copilotClaudeModels`, `movesUnderWay` | yes | As (j), (l), (k). |

Named by neither doc: the `machines` preference → `machines.json`; projects (above); `toolApproval` (m); `botInstructions` (c, a dropped cache); `groupMemoryWaiting` is named only through `pending-memory.json`; `calibration`, `calibration.backedUp`, `personalities.takenFromOld`, `passwords.movedToVault`, `passwords.never`, `passwords.offersToSave`, `remoteSessions`.

### Not checked
- I read no test, ran nothing, and did not open `swift/MAC-APP-REFERENCE.md`.
- Other preference keys the Swift app reads (`phoneLink.relay`, `machineUnsentViews`, `openedSubSessions`, `openedReviewParts`, last model, drafts…) were not inventoried: this pass followed the upgrades asked about plus what the greps for "COMPAT", "older version", "takenFromOld", "handedOver", "moved", "legacy" turned up in the stores named.
