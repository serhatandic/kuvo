# Rust Settings coverage (6.1b)

“Covered” means the behavior is asserted by a process scenario. The shared conformance area is `protocol/conformance/settings.test.mjs`. Partial coverage and missing cases are listed explicitly.

## Scope and owners

- Auto mode, its reviewer and the sandbox are postponed past 0.8 (user 11-25). Settings omits their rows; compatibility routes return the unavailable state. Existing approval records are retained; their effective and displayed mode is Ask First. Custom Auto model lists are separate.
- Browser settings and Never Offered action: 6.4. Worktrees place/list/removal: 6.3. Pages report unavailable state rather than inventing data.
- Config change receipts (`configChanges`) are recorded under the config writer’s existing lock in `box/config-apply/receipts.json`; Git conformance checks the actual receipt and its setup scope. Consuming receipts for Also Apply change sets remains open. Git migration and the Agents, Vault and Custom Auto pages now have their own process scenarios. Peer archived-session snapshots and owner actions remain in 6.1b. Custom Auto turn-level fallback and model-picker advertisement → 4.6; list storage and Settings forms are present but not runnable until then. MCP server OAuth/sign-in moves to row 4.7e (Work Queue #991).
- Computer login items, permissions, device switch and exact running/working status need the remaining system-owner wiring. Storage measures the current layout; native Windows allocation parity, compatibility roots and refresh races still need proof.

## TypeScript cases

### `settings-agents.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 83: the page lists the built-in agents, then yours, each with how it's used and what it offers | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 105: with its engine down, the agents are still listed from the settings, and the page says what it can't know | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 115: an agent is turned off and on through the engine's settings, and one turned off stops being where new sessions start | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 132: New Agent's form is checked as it's typed, refused with the same words, and makes an agent the engine reads | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 186: Edit… saves what's changed, keeps what it doesn't show, writes nothing for what's built in, and is stale once the agent changed | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 213: permissions kept in an order the form can't show are left exactly as they are, and said so | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 231: what the form can't show is kept: a model written as an object, and one of yours that's only turned off | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 242: a name the settings can't keep an agent by is refused | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 253: Reset to Built-In and Delete take the agent's settings out, and a deleted one stops being where sessions start | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |
| 265: Claude Code's agents on this computer are offered, and one imported comes over as a subagent | Covered — settings: Agents independent records; Markdown fixture imports into the same engine config. |
| 280: a settings file whose agents aren't an object is left alone, and the page says so | Partial — settings: Agents independent records, add/check/edit/stale/delete; other details unproven. |

### `settings-change-shape.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 6: PATCH Settings declares and checks its change body | Partial — generated shape checker used by route; malformed requests not fully exercised. |

### `settings-custom-auto.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 43: Custom Auto lists, ordered models, exact Swift words and ordinary forms | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 53: name check and refused submit have the same words; wrong types never write | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 59: add and rename; unrelated list changes do not stale a form; changed form returns 409 | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 65: ordered model edits and list reordering use the existing route | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 73: can.why returns 403 with storage untouched | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 78: unavailable source has no invented defaults | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 82: external changes and every mutation route update the events page | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 95: model forms have bounded fields and unsupported efforts never write | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 108: first and last entries omit impossible moves | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 117: a large catalog keeps two fields per form and only the edited model's efforts | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 130: signed-out accounts say why Custom Auto passes their models by | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 136: limit resets use the computer's configured zone and include a date only outside today | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |
| 156: editing a model and its different supported effort uses a single editable field | Partial — settings: Custom Auto list storage, check/add/edit/order/stale/clear; other details unproven. |

### `settings-mcp.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 103: the page lists your servers with how each is doing, in the box's words | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 134: with its engine down, the servers are still listed, and the page says why it can't say how they are | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 141: a server is turned off and on through the engine's settings, and a stale switch is refused with the page now | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 160: Add Server's form is checked as it's typed, in the box's words, and adds a server | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 199: a server's setup pasted into its name fills the form in | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 211: Edit… opens with the server's settings, saves what's changed, keeps what it doesn't show, and is stale once the server changed | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 227: Remove… takes the server out of the settings, and the rest stay | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 233: a server's sign-in is led by the box: started, followed, done, and its server connected again | → 4.7e — MCP server OAuth flow moves into the box. |
| 276: a sign-in turned down says so in the box's words, and can be tried again; one that ended long ago is gone | → 4.7e — MCP server OAuth flow moves into the box. |
| 299: a popular server is added from its row, and shows as added | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 309: a timeout is set as a number, and one written by hand as something else says why it can't be changed here | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |
| 321: the page followed is sent again as the settings change elsewhere | Partial — settings: remote MCP form check/save, independent timeout/switch; remaining cases unproven. |

### `settings-pages.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 60: the box serves and keeps the automatic summary threshold for this computer | Covered — settings: sections and conversation threshold. |
| 74: the coding agents don't update themselves until you turn it on, as the box keeps it for its bridge | Covered — settings: coding-agent automatic updates. |
| 92: the box lists its sections, each with what a sidebar and search show | Covered — settings: sections and conversation threshold. |
| 118: the browser's page is its settings, and a change is kept as kuvo/browser/settings keeps it | Gap — browser state/action owner 6.4. |
| 142: a change from a row read before another changed it is refused with the page now, and another row's still goes | Gap — browser state/action owner 6.4. |
| 159: a site never offered is offered again from its row | Gap — browser state/action owner 6.4. |
| 187: Full Access can't be turned on from Settings, and says why; it can be turned off, as kuvo/approval turns it off | Partial — settings: permission rule and Full Access refusal; other branches untested. |
| 209: letting your other computers start work here is kept as kuvo/approval keeps it | Gap — scenario not yet ported. |
| 222: what agents may do is the engine's permissions, each choice a rule for everything, the others left as they are | Partial — settings: permission rule and Full Access refusal; other branches untested. |
| 268: picking what an action does with no rule writes none, so Plan mode's own rule (no edits) is never widened | Gap — scenario not yet ported. |
| 296: a row's revision is its value's: the same value set through the old route, or the computer renamed, leaves it | Gap — scenario not yet ported. |
| 311: two changes to one row from the same reading, at once: one is made, the other told it's stale | Gap — scenario not yet ported. |
| 320: a page followed is said again when its settings change through the old routes, or by hand in the file | Gap — scenario not yet ported. |
| 334: Also Allowed lists the folders given, and one removed from its row is gone from kuvo/sandbox/allowed | Superseded by user 11-25: sandbox postponed. Compatibility endpoints in settings: saved Auto. |
| 372: over SSH, Also Allowed is read and removed through the process that holds it | Superseded by user 11-25: sandbox postponed. Compatibility endpoints in settings: saved Auto. |
| 386: over SSH, two changes to one row from the same reading go through one process: one is made, the other told it's stale | Gap — browser state/action owner 6.4. |
| 411: the sandbox's row is what kuvo/sandbox says, and its button does what that names | Superseded by user 11-25: sandbox postponed. Compatibility endpoints in settings: saved Auto. |
| 439: Restart Engine starts the box's own tool, and answers done; where the app restarts a Mac's engine, it says so | Superseded by user 11-25: sandbox postponed. Compatibility endpoints in settings: saved Auto. |
| 467: the sandbox's button is the box's: a route it runs, asked first where it stops work, and this Mac's own Software Update only from the app on it | Superseded by user 11-25: sandbox postponed. Compatibility endpoints in settings: saved Auto. |
| 479: a page followed is said again as it changes | Gap — browser state/action owner 6.4. |
| 494: the sandbox's row warns while commands can still reach your login's services | Superseded by user 11-25: sandbox postponed. Compatibility endpoints in settings: saved Auto. |
| 504: Git rows wrap config, with independent revisions and stale writes refused | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 513: Git writes with independent revisions and refuses stale changes | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 522: Git keeps an explicitly empty prefix | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 526: Git events follow both the page and existing config route | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 536: Git warns without rows when its settings cannot be read | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 544: Git refuses a non-text setting without touching the file | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 553: Git cleans the prefix and trims instructions as they are used | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 562: Git requires text | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 567: Git: the prefix in a card of its own, each set of instructions full width under its own heading | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 584: Git events follow the old one-time settings migration route | Partial — settings: Git independent revisions, clean text and external events; migration and malformed-file cases open. |
| 598: Worktrees: one folder field showing the default, the projects folder beside it, and the sessions' worktrees | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 621: Worktrees lists a project's place only where it needs you: a folder inside its repository | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 641: Worktrees writes through both routes, refuses stale revisions, and unrelated folders still write | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 653: work settings optional was is atomic and old calls remain valid | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 662: Worktrees explains a field it cannot replace and warns on unreadable files | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 678: Worktrees events follow config and work settings routes | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 698: Worktrees events follow old record and removal routes, with optional stale protection | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 723: Worktree folder changes refuse a config edit after the page read | Gap — Worktrees owner 6.3 route scenarios; wrapper not yet proven. |
| 771: This Mac's page says how it stands in its rows' titles: running, its version, and your devices let in, with what macOS allows its helper | Gap — computer state/permission and update scenarios. |
| 797: This Mac shows its permissions and the main computer can check for updates | Gap — computer state/permission and update scenarios. |
| 823: Let my devices connect is switched on This Mac's page, asked first as it's turned off, and Restart restarts it | Gap — computer state/permission and update scenarios. |
| 843: a computer you added has the same page without a Mac's rows: no helper, nothing of macOS, and devices always let in | Gap — computer state/permission and update scenarios. |
| 855: Update Now says it’s looking first, and an engine that's down is said where Running would be | Gap — computer state/permission and update scenarios. |

### `settings-skills.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 20: Skills lists editable skills and the Swift editor words | Partial — settings: skill check/create/edit/stale form; exact metadata, warnings and unknown front matter unproven. |
| 28: check and refused submit share name warnings and do not write | Partial — settings: skill check/create/edit/stale form; exact metadata, warnings and unknown front matter unproven. |
| 37: edit preserves unknown header keys and rejects stale forms | Partial — settings: skill check/create/edit/stale form; exact metadata, warnings and unknown front matter unproven. |
| 44: wrong switch type is 400 and unrelated config change does not stale the skill | Gap — wrapper implemented; case not yet asserted. |
| 51: held permissions refuse changes without touching the file | Gap — wrapper implemented; case not yet asserted. |
| 59: another skill's change does not stale this editor, rewording keeps switch revision | Gap — wrapper implemented; case not yet asserted. |
| 67: legacy skill writes reject a changed SKILL.md via was | Gap — wrapper implemented; case not yet asserted. |
| 73: events follow authored files, permissions and other places | Gap — wrapper implemented; case not yet asserted. |
| 86: GitHub install links share check and submit parsing | Gap — wrapper implemented; case not yet asserted. |
| 98: quoted descriptions round trip and malformed Other Places stays untouched | Gap — wrapper implemented; case not yet asserted. |
| 110: discovered skills keep metadata and both lists offer the existing search control | Gap — wrapper implemented; case not yet asserted. |
| 127: unreadable or invalid skill folders do not hide readable skills | Gap — wrapper implemented; case not yet asserted. |

### `settings-storage.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 41: Storage describes decimal units and suggestions while preserving words and confirmations | Gap — Storage wrapper/measurement implemented; case not yet asserted. |
| 63: Storage floors budget writes, refuses stale and wrong types; disk changes do not conflict | Gap — Storage wrapper/measurement implemented; case not yet asserted. |
| 76: Storage route optional was is atomic; calls without it still work | Partial — library conformance: artifacts storage and budget (guard and clear). |
| 86: Storage refuses an unavailable Clear with its reason and touches no settings | Gap — Storage wrapper/measurement implemented; case not yet asserted. |
| 99: Storage events follow the page, old budget route and clear route | Gap — Storage wrapper/measurement implemented; case not yet asserted. |
| 119: Storage does not invent what agents made when the bridge is unavailable, and shows the rest | Gap — Storage wrapper/measurement implemented; case not yet asserted. |
| 128: Storage requires explicit confirmation below the threshold before deleting copies | Gap — Storage wrapper/measurement implemented; case not yet asserted. |
| 148: Storage atomically refuses budget changes when the warning's older count changed | Gap — Storage wrapper/measurement implemented; case not yet asserted. |
| 157: The artifact storage route requires confirmation before any shrinking write deletes copies | Partial — library conformance: artifacts storage and budget (guard and clear). |
| 172: The store itself refuses an unconfirmed shrink before writing settings or deleting copies | Partial — library conformance: artifacts storage and budget (guard and clear). |
| 186: New storage budgets preserve the existing binary default and use the same base as the box's unit and suggestions | Partial — kuvo_services artifacts budget unit test; untouched binary-default write not asserted. |
| 191: The binary default displays as 1.07 GB and saves untouched with exactly its original bytes | Partial — kuvo_services artifacts budget unit test; untouched binary-default write not asserted. |
| 207: Storage measures what Kuvo keeps: the whole as parts, Kuvo's folder, Refresh, and the pages that clear the rest | Partial — settings: Storage measures categories and links; Refresh still unproven. |
| 258: Storage counts a worktree kept outside Kuvo's folder by itself, never the rest of the folder it's in | Covered — settings: Storage counts only the recorded outside worktree. |
| 286: Storage tries a measure that failed again only when it's old, or Refresh is pressed | Gap — Storage wrapper/measurement implemented; case not yet asserted. |

### `settings-vault.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 77: it's one of this computer's sections, and before there's a vault it says where its first item is added | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 92: the items are listed by name with what each is and how it's used, and never a value | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 111: Add Item is checked as it's typed without the values themselves, refused in the same words, and adds an item | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 160: Edit… opens empty for each value, shows one only through the vault's values route, and saves only what changed | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 199: Who Can Use It… takes access away from any client, and lets it be used more only from one that makes sure it's you | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 241: a key's other field is taken away from Edit…, and a new file is added only with its bytes | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 257: Delete… takes the item away with its values and grants | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 267: what agents wait on you for is answered from the page: once from any client, always and from now on only from one that makes sure it's you | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 313: Pause stops every agent's use; a paused bot is let go on | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 329: the activity says what happened in a sentence, yours plainly, and never a value | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 337: a locked vault says so and shows nothing else; a request that isn't a client's reads nothing | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 349: the page followed is sent again as the vault changes, without a value | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 365: no section's form can carry what's secret, nor read a value but through the vault's own route | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 405: vault pause accepts the original signed envelope and refuses to replace a future one | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 422: vault activity upgrades mixed lines and keeps a future entry through an append and prune | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |
| 442: the vault socket accepts a legacy hello and refuses a future request before dispatch | Partial — settings: Vault forms hide values, add and edit, direct value comparison, unauthorized read; other details unproven. Underlying service: rust-vault-coverage.md. |

### `approval-settings-round.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 60: a change to Settings › Permissions made while the box was down reaches a conversation in Auto as it starts | Superseded for Auto mode by user 11-25; new test asserts saved Auto is displayed as Ask First. Settings changes still need round scenario. |

### `git-settings.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 75: a box updated before your Mac keeps the branch prefix its worktrees started with, once: a phone's starts as yours did | Partial — settings: Git migration legacy prefix and one-time Mac preferences; other cases unproven. |
| 94: a box whose settings weren't its own as it started takes its old branch prefix when your Mac moves its Settings › Git there | Partial — settings: Git migration legacy prefix and one-time Mac preferences; other cases unproven. |
| 109: your Mac's own branch prefix, moved as the box's settings become its own, comes before the one the box kept | Partial — settings: Git migration legacy prefix and one-time Mac preferences; other cases unproven. |
| 119: a Mac whose instructions the box had its own of already doesn't bring them back after you clear the box's | Partial — settings: Git migration legacy prefix and one-time Mac preferences; other cases unproven. |
| 133: a Mac with nothing to give doesn't turn a later Mac's Settings › Git away | Partial — settings: Git migration legacy prefix and one-time Mac preferences; other cases unproven. |

### `group-settings.test.ts`

| Case (source line) | Coverage / gap |
|---|---|
| 75: a computer's box gives its group chats what they go by, with no Mac: personalities, who sets what, Ask First, what's held back | Gap — group approval/personality integration scenario. |
| 90: what they go by follows this computer: Auto picked, a rule taken out | Gap — group approval/personality integration scenario. |
| 99: a Mac says only your clock; an older Mac's Ask First and personalities don't go over the box's own | Gap — group approval/personality integration scenario. |
| 120: while this computer's Settings › Permissions can't be read, its group chats keep what they hold back | Gap — group approval/personality integration scenario. |

## Differential and platform results

Mac: 8/8 Rust settings scenarios; 7/7 applicable TypeScript scenarios (the new Auto postponement differs deliberately from the reference). Differential on a fixed signed setup: five Conversations and Git requests including Git receipt match; the two settings files and `box/config-apply/receipts.json` match (random receipt IDs and expiry times normalized, with receipt identity and scope checked in the scenario). Unrelated startup inventories differ. Full-home differential is not a match.

Linux static musl (lokum, capped container), WSL Ubuntu-24.04, and native Windows: 8/8 each. The first Linux run used a binary built before the new-session default adjustment and caught the saved-mode assertion; rebuilt and passed. Mac Clock conformance 1/1, Rust engine-sessions 81/81, workspace check with tests and TS check clean. No settings feature is advertised as fully ported yet.

Second slice (Agents, Vault, Custom Auto, Git migration): Mac settings 12/12 Rust and 11/11 applicable TS cases; Linux static musl on lokum 12/12. WSL Ubuntu-24.04 and native Windows each 12/12 (scratch and Node archive removed). The four new scenarios run on actual boxes; the reference bridge is started only for Custom Auto. They uncovered missing JSON content type in the TS Settings-to-bridge request; fixed with that scenario. Signed outer Vault requests have a Rust-only mutation/replay assertion. The differential used the same fixed seed with a custom agent, a legacy Git prefix, and two Custom Auto lists: seven answers and the three resulting settings files match with an app-origin Agents read. Unrelated startup files still differ; this is not full-home parity. Rust engine-sessions 81/81, workspace check with tests and TS check clean.

134 source-case entries. The Claude import has a process scenario; it deliberately rejects hardlinked files (including the Nix-store exception). Richer Markdown frontmatter and import validation still need matching scenarios. Custom Auto unavailable-bridge behavior is now different because its storage is resident; per-model availability notices and effort/catalog parity need proof.

Size after the second slice: 207,162 non-test characters (Settings modules, disk measurement, receipt recorder, Custom Auto storage and Git migration), compared with 212,700 in the TS settings-page modules, excluding indentation. The Rust count includes storage and migration code whose TS equivalents are separate; the 0.97× ratio is a conservative partial comparison, not a completed 1:1 size comparison.
