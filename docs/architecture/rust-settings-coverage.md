# Rust Settings coverage (6.1)

“Covered” means the behavior is asserted by a process scenario that passes against both boxes. The shared conformance area is `protocol/conformance/settings.test.mjs`. Every TypeScript case is sorted: **Covered** (normal use, ported and proven), **B — later (rare)** with its reason, or **C** (superseded, or owned by the row named).

Sort (6.1c): of the 128 entries open after 6.1b, 86 are now covered (A), 6 are rare (B) and 36 are superseded or another row's (C). Two covered rows also name a rare part (Storage's nearly-full-disk note; a computer renamed while a page is read).

## Scope and owners

- Auto mode, its reviewer and the sandbox are postponed past 0.8 (user 11-25). Settings omits their rows; compatibility routes return the unavailable state. Existing approval records are retained; their effective and displayed mode is Ask First. Custom Auto model lists are separate.
- Browser settings and Never Offered action: 6.4. Worktrees place/list/removal: 6.3. Pages report unavailable state rather than inventing data.
- Config change receipts (`configChanges`) are recorded under the config writer’s existing lock in `box/config-apply/receipts.json`; Git conformance checks the actual receipt and its setup scope. Consuming receipts for Also Apply change sets remains open in the separate cross-computer config-apply routes; the Settings receipt itself is covered. Git migration and the Agents, Vault and Custom Auto pages have process scenarios. Archived Sessions uses the accepted setup peer copy and owner-write transport. Paired-device forwarding through a third computer needs the scoped-forward authority transport (→ 2.2/5.2). Custom Auto turn-level fallback and model-picker advertisement → 4.6; list storage and Settings forms are present but not runnable until then. MCP server OAuth/sign-in and the only consumer of the persisted settings-flows records move to row 4.7e (Work Queue #991); the page protocol can already describe/check flows.
- Computer Helper/login item permission data → 7.2 (installed Helper wiring); Mac device-switch state and live grant/relay change → 7.3 (released Mac link configuration). Group chats/settings → 5.3. Storage measures the current layout and Refresh is covered; native Windows allocation parity and compatibility roots wait for the next native Windows run (B — later: no Windows path changed in 6.1c).

## TypeScript cases

### `settings-agents.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 83: the page lists the built-in agents, then yours, each with how it's used and what it offers | Covered — settings: Agents lists the built-in agents, then yours…. |
| 105: with its engine down, the agents are still listed from the settings, and the page says what it can't know | Covered — settings: …keeps listing them with the engine down (no cached models offered). |
| 115: an agent is turned off and on through the engine's settings, and one turned off stops being where new sessions start | Covered — settings: Agents turns one off and on…. |
| 132: New Agent's form is checked as it's typed, refused with the same words, and makes an agent the engine reads | Covered — settings: Agents New Agent checks as it's typed…. |
| 186: Edit… saves what's changed, keeps what it doesn't show, writes nothing for what's built in, and is stale once the agent changed | Covered — settings: Agents Edit saves what changed…. |
| 213: permissions kept in an order the form can't show are left exactly as they are, and said so | Covered — settings: Agents leaves permissions in an order its form can't show…. |
| 231: what the form can't show is kept: a model written as an object, and one of yours that's only turned off | Covered — settings: Agents leaves … a model written as an object, and one only turned off. |
| 242: a name the settings can't keep an agent by is refused | Covered — settings: Agents New Agent … (names the settings can't keep). |
| 253: Reset to Built-In and Delete take the agent's settings out, and a deleted one stops being where sessions start | Covered — settings: Agents Reset and Delete…. |
| 265: Claude Code's agents on this computer are offered, and one imported comes over as a subagent | Covered — settings: Agents offers Claude Code's agents…. |
| 280: a settings file whose agents aren't an object is left alone, and the page says so | Covered — settings: Agents Reset and Delete…; agents that aren't an object are left alone. |

### `settings-change-shape.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 6: PATCH Settings declares and checks its change body | Covered — settings: a Settings change body is checked before anything is read or written. |

### `settings-custom-auto.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 43: Custom Auto lists, ordered models, exact Swift words and ordinary forms | Covered — settings: Custom Auto's words, list moves…. |
| 53: name check and refused submit have the same words; wrong types never write | Covered — settings: Custom Auto keeps list revisions, form checks…. |
| 59: add and rename; unrelated list changes do not stale a form; changed form returns 409 | Covered — settings: Custom Auto keeps list revisions…. |
| 65: ordered model edits and list reordering use the existing route | Covered — settings: Custom Auto keeps list revisions… (model and list order). |
| 73: can.why returns 403 with storage untouched | Covered — settings: Custom Auto's words… its last list kept. |
| 78: unavailable source has no invented defaults | C — differs deliberately: Rust keeps Custom Auto lists in the resident, so there is no separate source to be unavailable. |
| 82: external changes and every mutation route update the events page | Covered — settings: Custom Auto's words… and the page followed. |
| 95: model forms have bounded fields and unsupported efforts never write | Covered — settings: Custom Auto keeps list revisions… (unsupported effort refused, same fields). |
| 108: first and last entries omit impossible moves | Covered — settings: Custom Auto's words, list moves only where they go. |
| 117: a large catalog keeps two fields per form and only the edited model's efforts | B — later (rare): a catalog of a thousand models; the form is two fields by construction. |
| 130: signed-out accounts say why Custom Auto passes their models by | Covered — settings: Custom Auto keeps list revisions… (signed-out notice). |
| 136: limit resets use the computer's configured zone and include a date only outside today | C → 4.6: limit resets come from Custom Auto's turn-level fallback. |
| 156: editing a model and its different supported effort uses a single editable field | Covered — settings: Custom Auto's words… edits to a model's effort. |

### `settings-mcp.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 103: the page lists your servers with how each is doing, in the box's words | Covered — settings: MCP lists your servers with how each is doing, and Try Again connects a failed one. |
| 134: with its engine down, the servers are still listed, and the page says why it can't say how they are | Covered — settings: MCP lists your servers… (engine down). |
| 141: a server is turned off and on through the engine's settings, and a stale switch is refused with the page now | Covered — settings: MCP switches a server off and on…. |
| 160: Add Server's form is checked as it's typed, in the box's words, and adds a server | Covered — settings: MCP Add Server checks as it's typed…. |
| 199: a server's setup pasted into its name fills the form in | Covered — settings: MCP Add Server… fills in a pasted setup. |
| 211: Edit… opens with the server's settings, saves what's changed, keeps what it doesn't show, and is stale once the server changed | Covered — settings: MCP Edit keeps what its form doesn't show…. |
| 227: Remove… takes the server out of the settings, and the rest stay | Covered — settings: MCP Edit… Remove keeps the rest. |
| 233: a server's sign-in is led by the box: started, followed, done, and its server connected again | C → 4.7e: MCP server sign-in moves into the box (Work Queue #991). |
| 276: a sign-in turned down says so in the box's words, and can be tried again; one that ended long ago is gone | C → 4.7e: MCP server sign-in moves into the box (Work Queue #991). |
| 299: a popular server is added from its row, and shows as added | Covered — settings: MCP adds a popular server from its row…. |
| 309: a timeout is set as a number, and one written by hand as something else says why it can't be changed here | Covered — settings: MCP adds a popular server… and keeps a timeout as a number. |
| 321: the page followed is sent again as the settings change elsewhere | Covered — settings: MCP Edit… and the page follows the file. |

### `settings-pages.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 60: the box serves and keeps the automatic summary threshold for this computer | Covered — settings: sections and conversation threshold. |
| 74: the coding agents don't update themselves until you turn it on, as the box keeps it for its bridge | Covered — settings: coding-agent automatic updates. |
| 92: the box lists its sections, each with what a sidebar and search show | Covered — settings: sections and conversation threshold. |
| 118: the browser's page is its settings, and a change is kept as kuvo/browser/settings keeps it | C → 6.4: browser settings. |
| 142: a change from a row read before another changed it is refused with the page now, and another row's still goes | C → 6.4: browser settings. |
| 159: a site never offered is offered again from its row | C → 6.4: Never Offered. |
| 187: Full Access can't be turned on from Settings, and says why; it can be turned off, as kuvo/approval turns it off | Covered — settings: Permissions Full Access can't be turned on here…. |
| 209: letting your other computers start work here is kept as kuvo/approval keeps it | Covered — settings: Permissions Full Access… other computers keep the old route's file. Its Default Mode “Auto” is superseded (Auto postponed, user 11-25). |
| 222: what agents may do is the engine's permissions, each choice a rule for everything, the others left as they are | Covered — settings: Permissions writes each choice as a rule for everything…. |
| 268: picking what an action does with no rule writes none, so Plan mode's own rule (no edits) is never widened | Covered — settings: Permissions picking what applies with no rule writes none…. |
| 296: a row's revision is its value's: the same value set through the old route, or the computer renamed, leaves it | Covered — settings: Permissions Full Access… (same value through the old route keeps the revision). Browser part → 6.4; a computer renamed while a page is read is B — later (rare): the line is outside the revision by construction. |
| 311: two changes to one row from the same reading, at once: one is made, the other told it's stale | Covered — settings: Permissions changes serialize per row…. |
| 320: a page followed is said again when its settings change through the old routes, or by hand in the file | Covered — settings: Permissions changes serialize… / picking what applies… the page follows the file. |
| 334: Also Allowed lists the folders given, and one removed from its row is gone from kuvo/sandbox/allowed | C — sandbox postponed (user 11-25). |
| 372: over SSH, Also Allowed is read and removed through the process that holds it | C — sandbox postponed (user 11-25). |
| 386: over SSH, two changes to one row from the same reading go through one process: one is made, the other told it's stale | C → 6.4: browser settings over SSH. |
| 411: the sandbox's row is what kuvo/sandbox says, and its button does what that names | C — sandbox postponed (user 11-25). |
| 439: Restart Engine starts the box's own tool, and answers done; where the app restarts a Mac's engine, it says so | C — sandbox postponed (user 11-25). |
| 467: the sandbox's button is the box's: a route it runs, asked first where it stops work, and this Mac's own Software Update only from the app on it | C — sandbox postponed (user 11-25). |
| 479: a page followed is said again as it changes | C → 6.4: browser page followed. |
| 494: the sandbox's row warns while commands can still reach your login's services | C — sandbox postponed (user 11-25). |
| 504: Git rows wrap config, with independent revisions and stale writes refused | Covered — settings: Git: the prefix in a card of its own…. |
| 513: Git writes with independent revisions and refuses stale changes | Covered — settings: Git: the prefix… stale writes refused. |
| 522: Git keeps an explicitly empty prefix | Covered — settings: Git: the prefix… (explicitly empty prefix). |
| 526: Git events follow both the page and existing config route | Covered — settings: Git follows the page, kuvo/config…. |
| 536: Git warns without rows when its settings cannot be read | Covered — settings: Git follows… and leaves settings it can't read. |
| 544: Git refuses a non-text setting without touching the file | Covered — settings: Git follows… or change. |
| 553: Git cleans the prefix and trims instructions as they are used | Covered — settings: Git: the prefix… cleaned as they're used. |
| 562: Git requires text | Covered — settings: Git: the prefix… (requires text). |
| 567: Git: the prefix in a card of its own, each set of instructions full width under its own heading | Covered — settings: Git: the prefix in a card of its own, instructions under their own headings. |
| 584: Git events follow the old one-time settings migration route | Covered — settings: Git events follow the one-time move of a Mac's settings. |
| 598: Worktrees: one folder field showing the default, the projects folder beside it, and the sessions' worktrees | C → 6.3: Worktrees. |
| 621: Worktrees lists a project's place only where it needs you: a folder inside its repository | C → 6.3: Worktrees. |
| 641: Worktrees writes through both routes, refuses stale revisions, and unrelated folders still write | C → 6.3: Worktrees. |
| 653: work settings optional was is atomic and old calls remain valid | C → 6.3: Worktrees. |
| 662: Worktrees explains a field it cannot replace and warns on unreadable files | C → 6.3: Worktrees. |
| 678: Worktrees events follow config and work settings routes | C → 6.3: Worktrees. |
| 698: Worktrees events follow old record and removal routes, with optional stale protection | C → 6.3: Worktrees. |
| 723: Worktree folder changes refuse a config edit after the page read | C → 6.3: Worktrees. |
| 771: This Mac's page says how it stands in its rows' titles: running, its version, and your devices let in, with what macOS allows its helper | C → 7.2: This Mac's Helper and macOS permission rows (installed Helper wiring). |
| 797: This Mac shows its permissions and the main computer can check for updates | C → 7.2/7.3: Mac permissions (7.2); a main Mac's Update Now and away-device words (7.3). |
| 823: Let my devices connect is switched on This Mac's page, asked first as it's turned off, and Restart restarts it | C → 7.3: Let my devices connect on a Mac (released Mac link configuration). |
| 843: a computer you added has the same page without a Mac's rows: no helper, nothing of macOS, and devices always let in | Covered — settings: This Computer uses the added computer's update and device state. |
| 855: Update Now says it’s looking first, and an engine that's down is said where Running would be | C → 7.3: a Mac's update progress and engine-down title. An added computer reading quickly with its engine quiet is covered (This Computer and the engine's pages read quickly…). |

### `settings-skills.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 20: Skills lists editable skills and the Swift editor words | Covered — settings: Skills lists editable skills…. |
| 28: check and refused submit share name warnings and do not write | Covered — settings: Skills lists editable skills, refuses a name taken in the check's words. |
| 37: edit preserves unknown header keys and rejects stale forms | Covered — settings: Skills edits keep unknown header lines…. |
| 44: wrong switch type is 400 and unrelated config change does not stale the skill | Covered — settings: Skills lists editable skills… and keeps unrelated settings. |
| 51: held permissions refuse changes without touching the file | Covered — settings: Skills lists editable skills… (held permissions). |
| 59: another skill's change does not stale this editor, rewording keeps switch revision | Covered — settings: Skills edits… go stale only for their own file. |
| 67: legacy skill writes reject a changed SKILL.md via was | Covered — settings: Skills edits… the old route refuses a changed one. |
| 73: events follow authored files, permissions and other places | Covered — settings: Skills follows its files and settings…. |
| 86: GitHub install links share check and submit parsing | Covered — settings: Skills follows… checks install links as it submits. |
| 98: quoted descriptions round trip and malformed Other Places stays untouched | Covered — settings: Skills edits… (quoted description) / Skills follows… malformed places. |
| 110: discovered skills keep metadata and both lists offer the existing search control | Covered — settings: Skills lists the engine's own with search, deleted ones with theirs. |
| 127: unreadable or invalid skill folders do not hide readable skills | Covered — settings: Skills lists… and skips folders it can't read. |

### `settings-storage.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 41: Storage describes decimal units and suggestions while preserving words and confirmations | Covered — settings: Storage describes what agents made in decimal units…. The nearly-full-disk note is B — later (rare): it needs a nearly full disk. |
| 63: Storage floors budget writes, refuses stale and wrong types; disk changes do not conflict | Covered — settings: Storage describes… floors a budget, and refuses stale, wrong… changes. |
| 76: Storage route optional was is atomic; calls without it still work | Covered — library conformance (artifacts storage and budget) and settings: Storage asks before keeping less…. |
| 86: Storage refuses an unavailable Clear with its reason and touches no settings | Covered — settings: Storage describes… refuses … unavailable changes. |
| 99: Storage events follow the page, old budget route and clear route | Covered — settings: Storage asks… and follows the page, the old budget route and Clear. |
| 119: Storage does not invent what agents made when the bridge is unavailable, and shows the rest | C — differs deliberately: Rust keeps the artifact store in the resident, so there is no separate process to be unavailable. |
| 128: Storage requires explicit confirmation below the threshold before deleting copies | Covered — settings: Storage asks before keeping less than older versions take. |
| 148: Storage atomically refuses budget changes when the warning's older count changed | Covered — settings: Storage asks… (a stale warning's older count). |
| 157: The artifact storage route requires confirmation before any shrinking write deletes copies | Covered — library conformance: artifacts storage and budget (guard and clear). |
| 172: The store itself refuses an unconfirmed shrink before writing settings or deleting copies | Covered — kuvo_services artifacts store unit test (refusal before write). |
| 186: New storage budgets preserve the existing binary default and use the same base as the box's unit and suggestions | Covered — settings: Storage describes… (binary default kept untouched). |
| 191: The binary default displays as 1.07 GB and saves untouched with exactly its original bytes | Covered — settings: Storage describes… (1.07 GB default and MB unit). |
| 207: Storage measures what Kuvo keeps: the whole as parts, Kuvo's folder, Refresh, and the pages that clear the rest | Covered — settings: Storage measures files by category… / Storage Refresh measures again. |
| 258: Storage counts a worktree kept outside Kuvo's folder by itself, never the rest of the folder it's in | Covered — settings: Storage counts only the recorded outside worktree. |
| 286: Storage tries a measure that failed again only when it's old, or Refresh is pressed | B — later (rare): a measure that fails needs unreadable records; Refresh itself is covered. |

### `settings-vault.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 77: it's one of this computer's sections, and before there's a vault it says where its first item is added | Covered — settings: Vault before there's one says where its first item is added…. |
| 92: the items are listed by name with what each is and how it's used, and never a value | Covered — settings: Vault before there's one… then lists items by what they are. |
| 111: Add Item is checked as it's typed without the values themselves, refused in the same words, and adds an item | Covered — settings: Vault Add Item checks without the values themselves…. |
| 160: Edit… opens empty for each value, shows one only through the vault's values route, and saves only what changed | Covered — settings: Vault Edit opens empty for each value…. |
| 199: Who Can Use It… takes access away from any client, and lets it be used more only from one that makes sure it's you | Covered — settings: Vault Who Can Use It…. |
| 241: a key's other field is taken away from Edit…, and a new file is added only with its bytes | Covered — settings: Vault Edit… takes a key's other field away / Vault Add Item… (a file only with its bytes). |
| 257: Delete… takes the item away with its values and grants | Covered — settings: Vault Edit… (Delete…). |
| 267: what agents wait on you for is answered from the page: once from any client, always and from now on only from one that makes sure it's you | Covered — settings: Vault answers what agents wait on you for from the page. Rows for a bot's and a seat's use asks are the vault's (6.2). |
| 313: Pause stops every agent's use; a paused bot is let go on | Covered — settings: Vault Pause stops every agent's use and lets a paused bot go on. |
| 329: the activity says what happened in a sentence, yours plainly, and never a value | Covered — settings: Vault before there's one… (activity said plainly). |
| 337: a locked vault says so and shows nothing else; a request that isn't a client's reads nothing | Covered — settings: Vault locked after a restart says so…. |
| 349: the page followed is sent again as the vault changes, without a value | Covered — settings: Vault Pause… the page follows without a value. |
| 365: no section's form can carry what's secret, nor read a value but through the vault's own route | Covered — settings: Vault locked… no form carries a value. |
| 405: vault pause accepts the original signed envelope and refuses to replace a future one | C → 6.2: vault pause envelope format (rust-vault-coverage.md). |
| 422: vault activity upgrades mixed lines and keeps a future entry through an append and prune | C → 6.2: vault activity format (rust-vault-coverage.md). |
| 442: the vault socket accepts a legacy hello and refuses a future request before dispatch | C → 6.2: vault socket hello (rust-vault-coverage.md). |

### `approval-settings-round.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 60: a change to Settings › Permissions made while the box was down reaches a conversation in Auto as it starts | C — Auto postponed (user 11-25); saved Auto reads as Ask First (settings: a saved Auto session reads as Ask First). |

### `git-settings.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 75: a box updated before your Mac keeps the branch prefix its worktrees started with, once: a phone's starts as yours did | Covered — settings: Git migration takes a legacy prefix and a Mac's preferences once. |
| 94: a box whose settings weren't its own as it started takes its old branch prefix when your Mac moves its Settings › Git there | B — later (rare): once per computer, as it updates from an older Mac app. |
| 109: your Mac's own branch prefix, moved as the box's settings become its own, comes before the one the box kept | B — later (rare): once per computer, as it updates from an older Mac app. |
| 119: a Mac whose instructions the box had its own of already doesn't bring them back after you clear the box's | B — later (rare): once per computer, as it updates from an older Mac app. |
| 133: a Mac with nothing to give doesn't turn a later Mac's Settings › Git away | B — later (rare): once per computer, as it updates from an older Mac app. |

### `group-settings.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 75: a computer's box gives its group chats what they go by, with no Mac: personalities, who sets what, Ask First, what's held back | C → 5.3: group chats. |
| 90: what they go by follows this computer: Auto picked, a rule taken out | C → 5.3: group chats. |
| 99: a Mac says only your clock; an older Mac's Ask First and personalities don't go over the box's own | C → 5.3: group chats. |
| 120: while this computer's Settings › Permissions can't be read, its group chats keep what they hold back | C → 5.3: group chats. |

## Peer archives and lightness

- `setup-lists.test.ts:991`: Covered by settings: Archived Sessions publishes owner menus, preserves source confirmations, forwards an unarchive to its owner, and follows the updated peer copy. This also passed with a Rust owner and TS reader, and the reverse. Peer snapshots use `run/peers/<key>/archives-v1.json` and the existing bounded multipart transport; owner writes use the existing receipt ledger. Away, moved-owner and paired-device paths remain to be tested; scoped device forwarding → 2.2/5.2.
- Organic file changes: conversation, Git and approval event streams compare file identity/revision before rebuilding. Agent Settings uses the config/import directory stamp and the reference's ten-second catalog refresh; no repeated account discovery on every page read. Dynamic pages keep their reference cadence.
- Agents and Custom Auto share the existing model catalog cache. Agents, MCP and Skills use passive running-only engine reads; they neither start OpenCode nor renew its idle lifetime. Reading pages never starts OpenCode; the cold-settings and repeat-read scenarios assert this. The durable agent catalog belongs to 4.7b.
- Custom Auto reads its list store and cached model catalog in parallel, once per page. The storage and picker answers use the same typed store/alias fold.
- Peer archive sharing checks the archived-row revision when local sessions change (unrelated active turns do not rebuild owner menus), plus a ten-second fallback, does no archive read on a lone computer, and publishes only changed copies (or the existing peer keep-alive). No new processes.
- This Computer bounds its read of what's at work to one second: with the engine not answering, Rust's `kuvo/box/at-work` waited on the session list past ten seconds and held the page (TypeScript answers in under half a second). The route itself still waits that long (`kuvo_engine/src/waiting.rs:222`, the Helper menu's reader); its owner should bound it.
- Remaining lightness costs: active Vault and Archives Settings streams still rebuild a page each second (`settings/vault.rs:43` vault file/ask/activity reads and `settings/archived.rs:25` one current action-menu check per archived session); reducing that safely needs event/revision hooks from the underlying services. Dynamic MCP (`settings/mcp.rs:33`) and Skills (`settings/skills.rs:18`) streams also re-read current facts at the TS cadence. These are active-follow costs, not startup work.

## Differential and platform results

Mac: 8/8 Rust settings scenarios; 7/7 applicable TypeScript scenarios (the new Auto postponement differs deliberately from the reference). Differential on a fixed signed setup: five Conversations and Git requests including Git receipt match; the two settings files and `box/config-apply/receipts.json` match (random receipt IDs and expiry times normalized, with receipt identity and scope checked in the scenario). Unrelated startup inventories differ. Full-home differential is not a match.

Linux static musl (lokum, capped container), WSL Ubuntu-24.04, and native Windows: 8/8 each. The first Linux run used a binary built before the new-session default adjustment and caught the saved-mode assertion; rebuilt and passed. Mac Clock conformance 1/1, Rust engine-sessions 81/81, workspace check with tests and TS check clean (first slice did not advertise the incomplete Settings feature).

Second slice (Agents, Vault, Custom Auto, Git migration): Mac settings 12/12 Rust and 11/11 applicable TS cases; Linux static musl on lokum 12/12. WSL Ubuntu-24.04 and native Windows each 12/12 (scratch and Node archive removed). The four new scenarios run on actual boxes; the reference bridge is started only for Custom Auto. They uncovered missing JSON content type in the TS Settings-to-bridge request; fixed with that scenario. Signed outer Vault requests have a Rust-only mutation/replay assertion. The differential used the same fixed seed with a custom agent, a legacy Git prefix, and two Custom Auto lists: seven answers and the three resulting settings files match with an app-origin Agents read. Unrelated startup files still differ; this is not full-home parity. Rust engine-sessions 81/81, workspace check with tests and TS check clean.

Third slice (peer Archives, live catalog facts, lighter reads): macOS Rust 17/17, TypeScript 15/15 applicable (the user’s Auto-mode postponement and no-cold-start checks intentionally skip the reference). The archive owner-action scenario passed in both mixed directions, too. The eight-request differential (including an archived session; date separator corrected from the first run) matches all answers and the three relevant setting files. Unrelated startup inventories still differ. Final workspace check with tests, Bun check and Rust engine-sessions 81/81 passed. Static musl on lokum (one 1 CPU/1 GB container), WSL Ubuntu-24.04 and native Windows each 17/17. Both cross builds ran in one lane; scratch homes, Node archive and cross targets removed.

Fourth slice (6.1c: MCP Servers, Agents, Skills, Storage, Vault, Permissions, Git, Custom Auto, change bodies): macOS settings 52/52 Rust and 50/50 applicable TypeScript (two Rust-only cases skip the reference). Linux static musl on lokum (one 1 CPU/1 GB container) 52/52. Rust engine-sessions shows only main's seven known step-5 failures; agent-tools 4/4 and agents 12/12. A nine-request MCP differential (read, check, add, switch, timeout, edit, a stale Try Again, config, read) matches every answer and the resulting `box/opencode.json`; unrelated startup inventories still differ. Native Windows was not run: no Windows path is involved.

Rust fixes found by the new cases: MCP Try Again (outside the row's revision, as the reference), a missing program's words, a popular server gone meanwhile reads as stale, a name taken meanwhile is refused as taken; Agents offers no cached models while the engine isn't answering; This Computer no longer waits on an unanswering engine. Deliberate differences: the vault's “Only Kuvo on your devices…” note names the computer by its setup name where TypeScript uses its host name; Custom Auto lists and the artifact store are resident, so their “source unavailable” states don't exist.

The Settings feature, vault forms, Claude import and Custom Auto list routes are advertised, as are the merged archive copies. `settings.flows` remains with 4.7e; Custom Auto models are not offered as runnable until 4.6.

134 source-case entries. The Claude import has a process scenario; it deliberately rejects hardlinked files (including the Nix-store exception). Richer Markdown frontmatter and import validation still need matching scenarios. Custom Auto unavailable-bridge behavior is now different because its storage is resident. The signed-out notice and unsupported-effort refusal are asserted; alias/limit expiry and the large-catalog case remain unproven.

Size after the second slice: 207,162 non-test characters (Settings modules, disk measurement, receipt recorder, Custom Auto storage and Git migration), compared with 212,700 in the TS settings-page modules, excluding indentation. The Rust count includes storage and migration code whose TS equivalents are separate; the 0.97× ratio is a conservative partial comparison, not a completed 1:1 size comparison.

Final size: 214,469 non-test characters of Settings/storage/migration source plus 22,556 added non-test characters in supporting Rust modules across the three slices (counting all additions, even replaced lines), a conservative 237,025 total. TS Settings modules alone: 212,700; including their old list storage, Git migration and Claude import: 243,810. This is 1.11× the TS pages alone, 0.97× the expanded reference, below the 1.2× target. Counts exclude leading indentation.

6.1c size: the Settings modules (`kuvo_services/src/settings/*.rs`) are 201,154 non-test characters against 216,421 in the TypeScript `link/areas/settings*.ts`, counted the same way (leading indentation and line breaks left out): 0.93×. 6.1c added 2,392 (MCP 17,497 → 19,569 against TypeScript's 29,054, which also holds the sign-in flows of 4.7e).
