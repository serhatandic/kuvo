# Current-beta stored families

Each row represents one family from the stored-format inventory. Paths are relative to
the scratch root. An asterisk matches one path component. Unless a row specifies a
runtime check, the test compares the representative files byte for byte after opening
the home through the selected resident. External formats use synthetic input where
the writer is the desktop, a provider, Chromium or the retired Mac app.

| Family | Producer | Representative path | Reopening expectation |
| --- | --- | --- | --- |
| Link state | `startSetup`, `writeLink` | `machine/run/link.json` | Setup and identity answer |
| Signed documents | `checkpointedTransition`, `writeLink` | `machine/run/setup-checkpoints/*.json` | Preserved checkpoint envelopes |
| Phone trust and sync | `refreshPhoneTrust` with a standby projection | `machine/run/phone-trust.json` | Preserved process-scoped cache |
| Setup discovery and welcome | `writeCard`, `setWelcomed` | `machine/run/welcomed.json` | Preserved welcome receipt |
| Mac link and remembered machines | `saveMacConfig` | `machine/run/mac-link.json` | Preserved app settings |
| Peer directory and reach | `writeAgents` | `machine/run/agents.json` | Directory regenerated |
| Add, reinstall and seed | `takeSeed` with a loopback OpenCode stand-in | `machine/run/seeded.json` | Preserved seed receipt |
| Work Queue ledger | `movePast` | `machine/run/work-queue/numbers.json` | Preserved number floor |
| Work Queue categories/status migration | `changeQueues` | `machine/run/work-queue/model.json` | Preserved status model |
| Computer enrollment history | `rememberEnrollment`, `projectMembership`, signed checkpoint | `machine/run/link.json` | Enrollment history preserved |
| Computer removal delivery | `membershipOf`, `projectMembership`, signed checkpoint | `machine/run/link.json` | Removal receipt preserved |
| Encrypted backup archives | `seal` | `machine/fixture-deposit/*/point.kuvo` | Preserved ciphertext |
| Relay backup upload reservations | `relayBackupAct` | `account/.kuvo-backups/relay/*/*/.upload.*` | Preserved journal and partial |
| Offline edit proposals and recovery | `keepRecord` | `machine/run/owned-data/edit-deletions/*.json` | Preserved deletion fence |
| Work Queue items and history | `changeQueues` | `machine/run/work-queue/queues/usr_you.json` | Preserved item and detail answer |
| Work Queue labels, delivery and receipts | `keepLabel` | `machine/run/work-queue/labels.json` | Preserved label |
| Managed attachment files | `keptFiles` | `machine/attachments/ses_golden/*/golden.txt` | Preserved attachment bytes |
| Engine database | OpenCode 2.2.6 SQL fixture loaded through SQLite | `machine/data/opencode/opencode.db` | Preserved database |
| Conversation metadata and queue | `Store`, `changeQueue`, `keepWaiting` | `machine/run/waiting/msg_goldenqueued.json` | Preserved queued message and readable transcript |
| Coding agent native storage | `Accounts.add` and synthetic Codex auth input | `machine/agents/codex/accounts/golden/auth.json` | Preserved provider bytes |
| Letters, receipts and calls | `leaveLetter` | `machine/run/outbox/sending/ltr_golden.json` | Expired letter removed |
| Pending work and work status | `noteStopped` | `machine/run/work/stopped.json` | Preserved stop marker |
| Bot metadata and edits | `makeBot` with a loopback OpenCode stand-in | `machine/bots/bot_*/bot.json` | Preserved bot descriptor and list answer |
| Bot text and ownership markers | `makeBot` | `machine/bots/bot_*/MEMORY.md` | Preserved bot memory |
| Wake-ups and watches | `seedRoutine` | `machine/wakes/ses_golden/wakes/*` | Preserved schedule |
| Group chats and bridge delivery | `GroupStore.saveGroup`, `saveMessages` | `machine/groups/grp_golden/*.json` | Preserved group and transcript |
| Projects and worktrees | `changeProjects`, Git, `updateWorktrees` | `machine/box/worktrees.json` | Preserved worktree and project list |
| Git preferences and backups | `moveGitSettings`, Git | `machine/box/git.json` | Preserved migration settings |
| Artifacts | `ArtifactStore.shown` | `machine/agents/artifacts/copies/*/*` | Preserved file copy |
| Search database | Synthetic Grok search index built with SQLite FTS5 | `machine/agents/grok/accounts/golden/sessions/session_search.sqlite` | Preserved external index |
| Moves and handoffs | `noteMovedAway`, `noteEnded` | `machine/run/moves/decisions/mov_golden.json` | Preserved switch receipt |
| Computer settings and local tools | `setProjectsFolder` | `machine/box/work.json` | Preserved project root preference |
| Engine/plugin/MCP configuration | `adoptBox`, `changeSettings` | `machine/box/opencode.json` | Preserved settings and disabled MCP entry |
| Local fences and audit | `addRule` | `machine/box/sandbox-allowed.json` | Preserved folder rule |
| Personalities | `changePersonalities` | `machine/bots/personalities.json` | Preserved library and list answer |
| Time zone | `setZone` | `machine/box/time-zone.json` | Preserved zone setting |
| Vault key wrapper | `makeVault` | `machine/box/vault/key.json` | Preserved key wrapper |
| Vault integrity records | `addItem`, `setGrant` | `machine/box/vault/grants.json` | Preserved authenticated grant |
| Vault values, activity and pause | `addItem`, `setPaused` | `machine/box/vault/values/*` | Preserved sealed value |
| SSH trust and agent sockets | `keysIn`, `trustHostKeys` | `machine/ssh/known_hosts` | Preserved host key |
| Sign-ins and integration credentials | `Accounts.add`, `adoptBox` | `machine/agents/accounts.json` | Preserved managed account index |
| Plan limits and usage readings | `limitsRoutes` with a loopback Cursor stand-in | `machine/agents/usage/limits.json` | Preserved plan reading |
| Cursor fallback cache | `CursorHistory.read` with a loopback Cursor stand-in | `machine/agents/usage/cursor-history/*.json` | Preserved daily cache |
| Usage settings | `UsageSettings.take` | `machine/agents/usage/settings.json` | Preserved scoped usage preferences |
| Insights SQLite | `InsightsStore.write` | `machine/agents/usage/insights.sqlite` | Schema and saved row |
| Browser profile | Synthetic Chromium Preferences input | `machine/browser/profile/Default/Preferences` | Preserved external profile |
| Browser history and settings | `mergeHistory`, `changeBrowserSettings` | `machine/browser/*.json` | Preserved history and preferences |
| Desktop settings | Synthetic desktop settings input | `machine/desktop/settings.json` | Preserved external preferences |
| Runtime rendezvous | `writeServeRecord` with a stale PID | `machine/run/serve.json` | Replaced by live resident |
| Runtime markers and locks | `writePrivate` | `machine/run/serve.needed` | Preserved demand marker |
| Repair and update state | `noteUpdate` | `machine/run/update.json` | Preserved update round |
| Installed release | `tarOf`, `verifyBoxes`, `keepRelease` with fixture signer | `machine/updates/release/*` | Preserved verified release cache |
| Rehome journal and staged trees | `noteRecord`, `writeJournal` | `machine/updates/rehome/*.json` | Preserved completed move |
| Logs and disposable caches | `writePrivate` | `machine/run/fixture.log` | Preserved previous log |
| Legacy Mac source | Synthetic retired app input used by the importer | `account/Library/Application Support/run.kuvo.code.trial713/phone-link.json` | Preserved legacy source bytes |
