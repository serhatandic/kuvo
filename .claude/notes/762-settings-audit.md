# #762 Settings audit: Codex, Claude Code desktop, Zed, OpenBot vs Kuvo (2026-11-16)

Research only, no code changes. The question: does Kuvo give users every setting they need before they move to the desktop app? The answer is the gap list in §2, each gap with a recommendation to pick from.

## Sources

- **Ours:** `gpui/kuvo/src/settings/rows.rs` (`YOURS`, `YOURS_LATER`, `OWN`, `LATER`), `box/src/link/areas/settings-pages.ts` and `settings-mcp.ts` (the sections the box describes), `box/src/shared/routes/work.ts`, kuvo-web `code/settings.md`.
- **0.7 (the retired Swift app):** a full inventory of `swift/KuvoCode/Views/Settings/*`, `SettingsView.swift`, the app menus, the sidebar menus and onboarding (§4).
- **Codex (ChatGPT desktop app, Codex):**
  - learn.chatgpt.com/docs/reference/settings (formerly developers.openai.com/codex/app/settings)
  - learn.chatgpt.com/codex/notifications
  - learn.chatgpt.com/codex/environments/git-worktrees and /local-environment
  - learn.chatgpt.com/docs/whats-new
  - developers.openai.com/codex/config-basic and config-sample
  - our screenshots `refs-1116/codex-{git,worktrees,storage}-settings.png`
- **Claude Code desktop (the Code tab of Claude's desktop app):**
  - code.claude.com/docs/en/desktop
  - claude.com/docs/cowork/claude-desktop-changelog
  - support.claude.com/en/articles/12626668 (Quick Entry)
  - camp-claude.github.io/learn/settings-tour, a third-party source; only "menu bar icon" rests on it alone
- **Zed:** `~/.kuvo-code/tmp/zed-ref`: `crates/settings_ui/src/page_data.rs` (every page) and `assets/settings/default.json`.
- **OpenBot** (nightly-labs/openbot, openbot.run):
  - github.com/nightly-labs/openbot README and PRIVACY.md
  - everydev.ai/tools/openbot/llms.txt
  - our screenshots `refs-1116/openbot-settings-{yours,server}.png` and `openbot-marketplace-*.png`
  - Several of its sections show only as names: their contents are marked unconfirmed.

"Unconfirmed" means no source showed it; nothing below is guessed.

## 1. The table: their settings, and ours

Ours means:
- **Yes**: the desktop app has it.
- **Yes, not in the desktop app yet**: 0.7 had it, or the box can do it, but the desktop app shows no control.
- **No**: we don't have it.

"Box page" means a section the box describes (`kuvo/settings/<id>`). "Yours" means the app's own settings on each device.

### Codex (ChatGPT desktop app)

| Section | Setting | Ours | Our place |
|---|---|---|---|
| General | Send with Enter, or require ⌘Enter | No | Yours › General (gap G7) |
| General | Prevent sleep while running | Yes, not in the desktop app yet (box has `keepsAwake`; 0.7 General) | Box page This Mac (G1) |
| General | Follow-up while running: steer or queue | No | — (G15, skip as a setting) |
| Profile | Name, picture, stats, invites | No | — (skip: no accounts of ours) |
| Keyboard shortcuts | Search, rebind, reset | Yes, not in the desktop app yet (0.7: read-only list) | Yours › Keyboard Shortcuts (G9) |
| Notifications | Turn done: never / in background / always | Yes, not in the desktop app yet (0.7: on/off) | Yours › Notifications (G3) |
| Notifications | Needs permission, asks a question (each a switch) | Partly: 0.7 had one switch for all | Yours › Notifications (G3) |
| Notifications | Ask macOS for permission | Yes, not in the desktop app yet (0.7: Open… to System Settings) | Yours › Notifications |
| Appearance | Light / Dark / System | Partly: the desktop app has Light/Dark; 0.7 had System too | Yours › Appearance (G5) |
| Appearance | Accent, background, foreground colors; contrast | Partly: 0.7 had accent | Yours › Appearance (G5) |
| Appearance | UI font and size, code font and size | Yes, not in the desktop app yet (0.7: text size, typeface) | Yours › Appearance (G4) |
| Appearance | Translucent sidebar | Yes, not in the desktop app yet (0.7: Glass/Solid) | Yours › Appearance (G5) |
| Appearance | Pointer cursors; import/share themes | No | — (skip) |
| Pets | Overlay pet | No | — (skip) |
| Browser | Agent may browse / download / upload: ask / allow / block, per-site exceptions | Partly: Permissions has "Open web pages" and "Search the web"; no per-site rules | Box page Permissions (G14) |
| Computer Use | Review access | Yes: This Mac › What macOS allows | Box page This Mac |
| Personalization | Personal AGENTS.md for this machine | No (the engine reads global instructions; no setting shows them) | Box page Agents (G11) |
| Personalization | Memories | Yes, bots' memory (in the bot, not Settings) | Bot editor |
| Suggested prompts | Follow-up suggestions | No | — (skip) |
| Archived chats | List, Unarchive | Yes (box page; also Delete and Delete All) | Box page Archived Sessions |
| Git | Branch prefix | Yes | Box page Git |
| Git | Commit / PR instructions | Yes | Box page Git |
| Git | PR merge method (Merge/Squash) | No (Kuvo doesn't merge PRs) | — (skip) |
| Git | Always force push | No | — (skip, G16) |
| Git | Create draft pull requests | No (the box's Create Pull Request has no draft option) | Box page Git (G12) |
| Git | Review delivery: inline / separate chat | No (0.7's "Review findings: list / one at a time" is a different thing) | — (skip) |
| Worktrees | Worktree root | Yes | Box page Worktrees |
| Worktrees | Always fetch upstream first | No | — (skip, G13) |
| Worktrees | Delete old worktrees automatically; how many to keep; ask before deleting | No (Remove one at a time) | Box page Worktrees (G13) |
| Worktrees › Environments | Setup scripts per project; action buttons | Partly: 0.7's project editor had a worktree setup command | Project editor (§4) |
| Storage | Total by kind, data folder, Browse | Yes, not in the desktop app yet (0.7 Storage) | Box page Storage (G6) |
| Storage | Clean up old chats | Yes, not in the desktop app yet (0.7: Clean Up Old Sessions) | Box page Storage (G6) |
| config.toml only | approval policy, sandbox, MCP, model and effort | Yes: Permissions, MCP Servers, Models | Box pages |
| Unconfirmed | Telemetry opt-out, export, launch at login, menu bar, language, update channel, default model | — | — |

### Claude Code desktop

| Section | Setting | Ours | Our place |
|---|---|---|---|
| Code | Prompt suggestions | No | — (skip) |
| Code | Allow bypass permissions mode | Yes: Full Access (turned on in a conversation, off in Permissions) | Box page Permissions |
| Code | Browser tools on/off | Partly: Permissions › Open web pages | Box page Permissions |
| Code | Worktree location | Yes | Box page Worktrees |
| Code | Branch prefix | Yes | Box page Git |
| Code | Archive sessions when their PR merges or closes | No | — (G13: a later feature, not a switch) |
| Code | Connect new sessions to Remote Control | Yes: Let my devices connect | Box page This Mac |
| Code | Preview, keep preview sessions | No | — (skip) |
| General | Run on startup | Yes: Login Items row (status, and Open Login Items) | Box page This Mac |
| General | Quick access shortcut (double-tap ⌥) | No | — (skip for now: a feature, not a setting) |
| General | Voice shortcut | Partly: 0.7 showed ⇧⌘Space, not changeable | Yours › Voice |
| General | Menu bar icon (third-party source only) | Being built (#761) | Yours › General (G2) |
| General | Keep computer awake | Yes, not in the desktop app yet (box `keepsAwake`) | Box page This Mac (G1) |
| This computer › System | Computer use, denied apps | Partly: This Mac › What macOS allows | Box page This Mac |
| This computer › System | Storage: disk use, delete inactive sessions | Yes, not in the desktop app yet (0.7 Storage) | Box page Storage (G6) |
| Import & export | Export sessions | Partly: 0.7 had Export as Markdown on each session (not in Settings) | Conversation menu (G8) |
| Account-level | Privacy (data controls), usage, billing | Partly: Usage; Privacy in 0.7 | Yours › Privacy (G10) |
| Outside Settings | Permission mode for new sessions (remembered) | Yes (remembered, shown in Permissions) | Box page Permissions |
| Outside Settings | Start on the default model and effort | Partly: Models › Model for new sessions | Models |
| Outside Settings | Environments (env vars), MCP connectors, plugins | Partly: MCP; plugins in 0.7 Engine | Box pages MCP, Engine |
| Outside Settings | `.worktreeinclude`, `cleanupPeriodDays` | No | — (skip) |
| Outside Settings | Updates: automatic, channel admin-only | Yes: This Mac (Kuvo on the computer); app's own updates in 0.7 | Box page This Mac; Yours › Updates |
| Unconfirmed | Notification options, send key, theme or font size for Code, telemetry toggle names, language, shell | — | — |

### Zed (only what's relevant to an agent harness; its editor settings are left out)

| Page | Setting | Ours | Our place |
|---|---|---|---|
| General | Telemetry: diagnostics, metrics (two switches) | Yes, not in the desktop app yet (0.7: one switch) | Yours › Privacy (G10) |
| General | Auto update, preview channel | Yes, not in the desktop app yet (0.7: automatic checks, betas) | Yours › Updates |
| General | Restore on startup; on last window closed | No | — (skip: the app restores and keeps running) |
| Appearance | Theme mode, light/dark theme | Partly (no System) | Yours › Appearance (G5) |
| Appearance | UI font and size; agent panel font and size | Yes, not in the desktop app yet (0.7) | Yours › Appearance (G4) |
| Appearance | Reduce motion | No (macOS's own Reduce Motion should be followed) | — (skip as a setting) |
| Keymap | Base keymap, edit keybindings | No | — (G9: list first) |
| AI › Agent | Default model (commit message and thread summary models exist only in its settings files, not on a page) | Yes: Models (new sessions, titles, subagents) | Models |
| AI › Agent | Notify when agent waiting: primary screen / all / never | Yes, not in the desktop app yet | Yours › Notifications (G3) |
| AI › Agent | Play sound when done: never / when hidden / always | Yes, not in the desktop app yet (0.7: switch) | Yours › Notifications (G3) |
| AI › Agent | Prevent idle sleep | Yes, not in the desktop app yet | Box page This Mac (G1) |
| AI › Agent | Use ⌘Enter to send | No | Yours › General (G7) |
| AI › Agent | Thinking display; expand edit/terminal cards | Yes, not in the desktop app yet (0.7: Agent's steps) | Yours › Appearance |
| AI › Agent | Auto compact and threshold | Yes, not in the desktop app yet (0.7 Engine: on/off) | Box page Engine |
| AI › Agent | Auto-generate titles; show turn stats; feedback thumbs; max idle threads | No | — (skip) |
| AI | LLM providers, external agents, MCP servers, skills, sandbox, tool permissions | Yes: Accounts, MCP; Permissions; Skills in 0.7 | Accounts; box pages |
| Network | Proxy, server URL | No | — (skip) |
| Terminal | Shell, env, font | Partly: 0.7 Engine › Shell | Box page Engine |
| Git | Auto fetch, blame, etc. | No | — (skip: editor features) |

### OpenBot

Many of its sections show only as names in our screenshots; their contents are unconfirmed.

| Side | Section | Setting | Ours | Our place |
|---|---|---|---|---|
| Your settings | General | "Link analytics to my account", "Help make OpenBot better" | Yes, not in the desktop app yet (0.7: Share anonymous usage) | Yours › Privacy (G10) |
| Your settings | Profile | Account sessions | No | — (skip: no accounts of ours) |
| Your settings | Notifications | unconfirmed | Yes, not in the desktop app yet | Yours › Notifications |
| Your settings | Voice | unconfirmed | Yes, not in the desktop app yet | Yours › Voice |
| Your settings | Dynamic Island, Agent dock | unconfirmed | No | — (skip) |
| Your settings | Computer Use | unconfirmed | Yes: This Mac | Box page This Mac |
| Your settings | Billing | Buy a license | No | — (skip) |
| Your settings | Mobile Connect | Pair the iPhone by QR code; pushes when the host is idle | Yes, not in the desktop app yet (0.7 Phone; box `kuvo/phones`) | Box page Phone (G1b) |
| Your settings | Updates | Check, download, restart | Yes, not in the desktop app yet | Yours › Updates |
| Server settings | Personality, Agents | unconfirmed | Partly: bot editor; Agents in 0.7 | Bot editor; box page Agents |
| Server settings | Vault | unconfirmed | Yes, not in the desktop app yet (0.7 Vault) | Box page Vault |
| Server settings | Members, Audit log | unconfirmed | No | — (skip: one person) |
| Server settings | Permissions | unconfirmed | Yes | Box page Permissions |
| Server settings | Remote desktop | Screen Recording, Accessibility checks | Yes: This Mac | Box page This Mac |
| Server settings | MCP | unconfirmed | Yes | Box page MCP Servers |
| Server settings | Storage | Delete files, clear caches and logs | Partly: What Agents Made; 0.7 had logs and downloads | Box page Storage (G6) |
| Server settings | Sites, Browser profiles | — | No | — (skip) |
| Server settings | Providers | — | Yes: Accounts | Accounts |
| Server settings | Import | Agents from a Grok Bot .zip | Partly: 0.7 "Bring Over" from opencode, and import from Claude Code | Box page Engine (§4) |
| Server settings | Transfer | unconfirmed | Partly: bots and sessions move between computers | — |
| Server settings | Routines | — | Yes: bots' wake-ups (in bots, not Settings) | Bot editor |
| Unconfirmed | — | Send key, font size, language, launch at login, export | — | — |

## 2. Gaps

Each gap gives what it does, who needs it, who has it, which side it's on, what it needs, a size and a recommendation.

Recommendations:
- **Add before the user's move**: the user relies on it daily, or 0.7 had it and losing it would be noticed in week one.
- **Add before 0.8**: everyone else's move.
- **Skip**.

Box side always means a row the box describes (`kuvo/settings/<section>`, kinds as in PROTOCOL.md: `switch`, `choice`, `text`, `number`, `folder`, `list`, `actions`, `status`), made from a box route, so every client and the phone get it.

### G1. Keep this Mac awake while agents work
- **What:** stops the Mac sleeping on its own while an agent or bot works, window open or not. The screen can still sleep.
- **Who needs it:** anyone who leaves work running and walks away. On by default; laptop users on battery turn it off.
- **Who has it:** Codex ("Prevent sleep while running"), Claude ("Keep computer awake"), Zed ("Prevent idle sleep"), 0.7.
- **Side:** the box. It's a fact about that computer.
- **What it needs:** the box already keeps the setting (`kuvo/work/settings`, `keepsAwake`, `awakeNow`; `work/awake.ts`). Add one `switch` row to This Mac, written through `PATCH kuvo/work/settings`, with a line that says when it's keeping the Mac awake now (`awakeNow`). Show the row only where the box keeps its Mac awake itself: a Mac's own Kuvo (`keepsMacAwakeItself()`, `work/awake.ts`), shown from the box's own answer (`awakeNow` present), not from the operating system. A Mac added as a machine, a box beside 0.7, and Linux refuse `keepsAwake` today (`areas/work.ts`); there the page says nothing about it.
- **Size:** S.
- **Recommendation:** **Add before the user's move.** Every product has it, it's on by default, and today the desktop app can't turn it off.

### G1b. Phone: pair and remove phones
- **What:** pair an iPhone with a QR code, list paired phones, remove one.
- **Who needs it:** everyone with the phone app.
- **Who has it:** OpenBot (Mobile Connect), 0.7 (behind a hidden switch).
- **Side:** the box. The routes are there: `GET kuvo/phones`, `POST kuvo/phones/pair`, `DELETE kuvo/phones/pair`, `POST kuvo/phones/remove`. Describe it as the `phone` section: a `list` of phones with Remove…, and an Add action whose flow shows the QR code. The flow kind exists (MCP sign-in uses it), but it has no QR code yet. Size: M.
- **Recommendation:** **Add before the user's move** if pairing a new phone today needs 0.7. Otherwise **Add before 0.8**. Check with the lead which is true.

### G2. Kuvo in the menu bar, and "keeps running when the app is closed"
- **What:** a menu bar item (#761 is building it).
- **Keeping running is already a fact, not a setting.** On a Mac's own box, Kuvo Helper runs Kuvo whether the app is open or not, and This Mac says so ("Kuvo Helper keeps it running, and starts it at login"). Login Items is shown there too.
- **Who has it:** Claude has a menu bar icon (third-party source) and "Run on startup". Codex: unconfirmed. Zed: "On last window closed".
- **Side:** Yours (whether this device shows the item).
- **Size:** S, on top of #761.
- **Recommendation:**
  - **No switch for "keep running".** The app makes it obvious: This Mac's Running row, and the menu bar item itself.
  - **Add before 0.8:** a "Show Kuvo in the menu bar" switch in Yours › General, on by default, for people who keep a clean menu bar. Claude has the same.
  - 0.7's **Stop Background Work…** (⌥⌘Q) has no equivalent yet. See G17.

### G3. Notifications: when, and on which device
- **What:** whether this device shows a banner when a session finishes, when an agent needs you (permission or a question), and when a bot finishes or a usage notice comes. Also a sound, and the Dock count.
- **Who needs it:** everyone. Users really differ here: some want only "needs me".
- **Who has it:**
  - Codex: finished, as Never / In background / Always; separate switches for needs permission and asks a question.
  - Zed: waiting, as primary screen / all / never; sound, as never / when hidden / always.
  - 0.7: on/off, sound, Dock count.
  - OpenBot: its section; contents unconfirmed.
- **Side:** Yours, on each device. The box decides what's news (`kuvo/notices`, with the box's words); each device decides whether it shows it. The phone keeps its own switches in the phone app (to tell the phone session).
- **Across several computers:** no "send to which device" routing setting. Every box's notices reach every client that's connected, and each device filters for itself. That needs nothing on the box beyond what `kuvo/notices` already says.
- **Needs:** Yours › Notifications.
  1. **When a session finishes:** Only When Kuvo Isn't in Front (default) / Always / Never.
  2. **When an agent needs you:** switch, on.
  3. **Play a sound:** switch.
  4. **Count on the Dock icon:** switch.
  5. A button to macOS's notification settings when notifications aren't allowed.
- **Usage notices** stay on the box's Usage page, as now ("Sent by <computer>, to every app that connects to it").
- **Size:** M (the desktop app also has to show the box's notices, if it doesn't yet).
- **Recommendation:** **Add before the user's move.** It's 0.7's page, plus Codex's two-way split. Match Codex's wording for the three choices.

### G4. Text size (and typeface) of the conversation and the sidebar
- **What:** bigger or smaller text in conversations and the sidebar; the conversation's typeface. Code stays monospaced.
- **Who needs it:** people with large or small screens, or poor eyesight. Users really differ.
- **Who has it:** Codex (UI and code font size), Zed (UI, buffer and agent font size), 0.7 (Text size, Typeface).
- **Side:** Yours.
- **Size:** M. The desktop app draws at fixed sizes (`text_size(px(..))`), so this needs a scale in `kuvo_ui`'s theme, not one row.
- **Recommendation:** **Add before the user's move:** text size. **Typeface: Skip.** Only Codex and Zed (code editors) offer fonts, and 0.7's Rounded/Serif look like noise (§3).
- Also follow macOS's own larger-text and Reduce Motion where it's cheap, rather than adding switches for them.

### G5. Appearance: System mode, accent color, glass or solid window
- **What:** follow macOS's light/dark mode; an accent color; a frosted or solid window.
- **Who has it:** Codex (theme with System, accent, translucent sidebar), Zed (theme mode with System), 0.7 (all three).
- **Side:** Yours.
- **Size:** S for System mode; S–M for accent; M for glass.
- **Recommendation:**
  - **System mode: Add before the user's move.** Everyone has it, and today's Light/Dark menu forgets macOS's schedule.
  - **Accent: Add before 0.8.** Brand-adjacent: show the options to the user first.
  - **Glass/Solid: Add before 0.8,** only if the desktop app draws glass at all. Otherwise Skip.

### G6. Storage: what Kuvo keeps on a computer, and cleaning up old sessions
- **What:** disk use by kind (sessions, worktrees, logs, downloads, What Agents Made), the data folder, **Clean Up Old Sessions…** (unused for more than 30/90/180/365 days, keep pinned), and deleting logs.
- **Who needs it:** everyone, after a few months.
- **Who has it:** Codex (Storage), Claude (System › Storage, `/storage`), OpenBot (Storage), 0.7.
- **Side:** the box. Sessions, worktrees and logs are each computer's. Today the box page has only What Agents Made.
- **Needs:**
  - The page's `status` rows by kind, from a new box route (sizes it works out).
  - Clean Up Old Sessions as an `actions` row with a form (age, keep pinned) and a confirm in the box's words, done through `kuvo/sessions/deletes`.
  - Delete Logs as an `actions` row.
- **Size:** M.
- **Recommendation:** **Add before 0.8.** Before the move, only if the user's disk needs it. Nothing is lost meanwhile, because Archived Sessions already deletes.

### G7. Send with Enter, or with ⌘Enter
- **What:** whether Enter sends, or adds a line while ⌘Enter sends.
- **Who needs it:** people who write long prompts; people coming from Slack want Enter. Users really differ, and it's one switch.
- **Who has it:** Codex, Zed (`use_modifier_to_send`). Claude: unconfirmed. 0.7: no.
- **Side:** Yours. Each device's keyboard habit; the phone has its own.
- **Size:** S. The composer's field `submits_on_enter`.
- **Recommendation:** **Add before 0.8,** in Yours › General as "Send with: Return / ⌘Return". Codex's wording and choices. Before the move only if the user asks.

### G8. Export a session; backing up everything
- **What:** save one conversation as Markdown; or keep a copy of sessions, bots and their memory.
- **Who has it:**
  - Claude: session export, everything by default.
  - OpenBot: Import, and Transfer (unconfirmed what it does). PRIVACY.md mentions export without describing it.
  - 0.7: Export as Markdown… on a session.
- **Side:** the box. It has the session and turns it into Markdown; the client saves the file.
- **Needs:** a route that answers a session as Markdown, and **Export…** in the conversation menu. That isn't Settings.
- **Size:** S–M.
- **Recommendation:**
  - **Export a session: Add before 0.8** (0.7 had it).
  - **Back up everything: Skip** in Settings for now. Everything is in `~/.kuvo-code` on each computer, which Time Machine and other backups already cover. Say that in the docs (privacy/uninstall pages). Bots and sessions already move between computers.

### G9. Keyboard shortcuts page
- **What:** a searchable list of every shortcut. Codex also rebinds them.
- **Who has it:** Codex (rebind), Zed (keymap), Claude (⌘/ lists only), 0.7 (read-only list).
- **Side:** Yours.
- **Size:** S for the list; L for rebinding.
- **Recommendation:** **List: Add before the user's move** (0.7 had it, cheap). **Rebinding: Skip** until people ask.

### G10. Privacy: anonymous usage
- **What:** turn off anonymous usage (#222, OpenPanel).
- **Who needs it:** anyone who won't share. Required before any event is sent.
- **Who has it:** OpenBot (two switches), Zed (diagnostics, metrics), 0.7 (Share anonymous usage). Codex and Claude: unconfirmed in the app.
- **Side:**
  - Yours, for what the app sends.
  - If #222 also has boxes send events (they run without an app), each box gets its own switch on its This Computer page, set there. The app's switch covers only what the app sends, never writing every box's (that would make the Mac a hub). #222 decides; list it in its brief.
- **Size:** S.
- **Recommendation:** **Add with #222, before its first event,** so before 0.8. One switch, "Share anonymous usage", on Yours › Privacy, with a link to the privacy page. Not two switches like Zed.

### G11. Instructions for every session on a computer
- **What:** your own standing instructions ("always answer in Turkish", "use pnpm"), added to every session on that computer. That's Codex's "personal AGENTS.md for this machine" and the engine's global instructions.
- **Who has it:** Codex (Personalization). Claude uses `~/.claude/CLAUDE.md`, a file, not a setting. 0.7: no.
- **Side:** the box. It's the engine's settings on that computer.
- **Needs:** a `text` row (many lines) on the box's Agents section, written to the engine's global instructions file.
- **Size:** S–M.
- **Recommendation:** **Add before 0.8.** Users really differ, and it's one field.

### G12. Create pull requests as drafts
- **What:** Create Pull Request makes a draft.
- **Who has it:** Codex (on by default). Others: no.
- **Side:** the box. A `switch` on Git, next to the PR instructions.
- **Size:** S.
- **Recommendation:** **Add before 0.8,** only if Create Pull Request is used from the desktop app by then. Teams really differ.

### G13. Cleaning up worktrees
- **What:**
  - Codex deletes old worktrees on its own (keep 15, a recovery snapshot first, asked first), and can fetch upstream first.
  - Claude archives a session when its PR merges.
- **Side:** the box.
- **Recommendation:**
  - **Skip the switches and the count.** A count is a limit on what's kept, and the app should make it obvious instead.
  - **Add before 0.8:** a **Remove Unused…** action on Worktrees. It removes the worktrees whose sessions are archived and have nothing uncommitted, saying first which ones. Done with `kuvo/worktrees/remove` for each one. Size S–M.
  - Archiving on PR merge: a later feature, not Settings.

### G14. Per-site rules for the agent's browser
- **What:** let the agent browse, download or upload on some sites without asking; block others.
- **Who has it:** Codex (Browser: ask / allow / block, with per-site exceptions).
- **Side:** the box (Permissions).
- **Size:** M.
- **Recommendation:** **Skip for now.** Permissions' note already says rules for particular files and commands aren't on the page yet (0.7's Rules editor). Port that one editor (§4) and let it take sites too, rather than a second place.

### G15. A message sent while the agent works: steer or queue
- **Who has it:** Codex (General).
- **Recommendation:** **Skip as a setting.** Offer both where you send (Send, or "Send after this turn") so no one has to pick a default. To check against what our composer does now. Not built here.

### G16. Settings the others have that we skip
| Setting (who) | Why we skip it |
|---|---|
| Force push (Codex) | Risky; the app should push safely |
| Merge method (Codex) | Kuvo doesn't merge PRs |
| Prompt suggestions (Claude) | — |
| Pets, profile, billing (Codex, OpenBot) | — |
| Proxy (Zed) | — |
| Restore on startup (Zed) | — |
| Members and audit log (OpenBot) | One person per setup |
| Language | No one confirmed one |
| Quick entry shortcut (Claude) | A feature to plan, not a setting |

### G17. Stop Kuvo on this Mac, and Remove Kuvo from this Mac
- **What:**
  - 0.7 had Stop… (Background work) and ⌥⌘Q. The box has an uninstall command (`kuvo-link.js uninstall`, `box/src/link/uninstall.ts`; "Settings, once it's there").
  - The desktop app offers Uninstall only for a machine (computers page), not for this Mac.
- **Who has it:** nobody among the four confirmed one, but people moving off Kuvo need it. Our footprint rule says Kuvo is gone when its folders are deleted, and `uninstall.md` explains how.
- **Side:** the box. On This Mac:
  - **Stop Kuvo on this Mac…**: confirm, then it stays off until you open the app or log in again. On a machine: "Stop".
  - **Remove Kuvo from this Mac…**: confirm with the box's words of what stops and where its folder is. Runs the uninstall.
  - Each is an `actions` row with a confirm.
- **Size:** S–M for Stop; M for Remove.
- **Recommendation:** **Stop: Add before the user's move,** since 0.7 users had it and the menu bar item (#761) may cover it, so check there first. **Remove: Add before 0.8.**

### G18. Defaults for new bots
- **What:** a model or effort that new bots start on.
- **Today:**
  - A new bot starts by the same rule as a new session (`box/src/link/bots/make.ts`, `work/model-rule.ts`), and New Bot shows and lets you change the model.
  - Models › Model for new sessions covers the case where there's no last pick.
- **Who has it:** none of the four have bot defaults (Claude has "start with the default model and effort" for sessions).
- **Recommendation:** **Skip.** One rule for sessions and bots is simpler, and the bot editor makes the model obvious. Never add caps or limits on bots.

## 3. Settings we have that look like noise

None of the four products has any of these:
- **MCP Servers › Timeouts in milliseconds** (box page, three number fields). Codex keeps these in config.toml only; Zed has none in its UI.
  - Suggest: drop the group from the page and leave it to the settings file. If kept, show it in seconds.
- **0.7 Engine › Limits** (tool output lines and bytes, image max width and height), **Keep recent sessions warm**, **Subagents can start subagents**, **Files the Engine Doesn't Watch**.
  - Suggest: don't port these to the Engine page. They stay in its settings file, behind "Open Settings File".
  - Port only Summarize long conversations, Format files, Shell, Web Search and Plugins.
- **0.7 Agents › agent editor's "Most steps per message"** (`steps`): a cap on what an agent does.
  - Our rule says nothing caps bots. Don't port it as a field; leave it in the settings file.
- **0.7 Appearance › Typeface (Rounded, Serif)** and **Review findings: list / one at a time**. Nobody else has either.
  - Suggest dropping both. Keep Agent's steps, which matches Zed's thinking display.
- **0.7 Usage panel's Density, Increase transparency, Reduce animations, Log detail**. macOS has its own transparency and motion switches.
  - #761 should follow those rather than port these.
- **0.7 Panel › New Tab Page: number of sites (4/8/12/16)**. Suggest one fixed count.
- Not noise, though unique:
  - **Clock** (bots' wake-ups across computers).
  - **Let your other computers start work here**.
  - **Usage notices**.
  - Each is a fact of our multi-computer design.

## 4. 0.7 settings the desktop app hasn't got yet, and where each goes

Section names are rows.rs ids. "Box" means a section the box should describe, so the phone gets it too.

**Yours › General** (`general`)
- Show the tab bar
- Sessions side by side (halves / compact / joined)
- Swipe between computers (direction)
- Open code and text in (editor)
- Show in the menu bar (new, G2)
- Send with Return / ⌘Return (new, G7)
- Background work, now a fact on This Mac (G2, G17)
- Keep this Mac awake moves to the box's This Mac (G1).

**Yours › Appearance** (`appearance`, has Theme)
- System mode (G5)
- Accent (G5)
- Window Glass/Solid (G5)
- Text size (G4)
- Agent's steps (each / summary / minimal)
- Reset to Defaults
- Typeface and Review findings: drop (§3)

**Yours › Notifications** (`notifications`)
- Sessions on/off
- Play a sound
- Dock count
- Open System Settings
- New choices from G3

**Yours › Voice** (`voice`)
- Voice style (composer / voice field)
- More accurate voice input (Whisper): download, pause, resume, remove

**Yours › Keyboard Shortcuts** (`shortcuts`)
- The searchable list

**Yours › Side Panel** (`panel`)
- Start each panel with (Changes / New Tab)
- Show the panel when the agent browses
- New Tab Page: Tools, Suggested sites (drop the count, §3)
- Browsing history › Clear
- Website Data (search, Remove, Remove All). The box already has "Close agents' tabs" on its Browser page.
- Whether history and website data are the device's or the box's depends on where the desktop app's panel browser keeps them (#693). Put them wherever that is.

**Yours › Updates** (`updates`)
- Check for updates automatically
- Get beta updates
- Check Now (the app's own updates; This Mac covers Kuvo on the computer)

**Yours › Privacy** (`privacy`)
- Share anonymous usage (G10)
- Links to the privacy page, Storage, Browser and Vault

**Box › Accounts** (`OWN`, to move onto the box, #741). Desktop already has sign in, install, disconnect, sign out and remove. Missing:
- Rename… an account
- Use This Account (plans)
- Refresh Models
- Update All and an agent's update
- Copy Command
- "Agents Use Resets Without Asking"
- Your Own providers: Add Provider, the API, models, headers, keys, Remove

**Box › Models** (`OWN`)
- Has new sessions, titles and subagents.
- Missing: Mode for new sessions (`default_agent`).

**Box › Usage** (`OWN`)
- Has limits and the 3 notices.
- 0.7's panel and menu bar choices go to #761 (Yours), minus the noise in §3.
- Missing: Hide while sharing your screen, Time format, Always show pacing, Reset usage settings.

**Box › Custom Auto** (`custom-auto`)
- Lists: name, order, models
- New List, Delete List
- Try All Again

**Box › Agents** (`agents`)
- The agents list
- The editor: name, use as, description, model, on/off, instructions, what it may do, rules. Not "most steps", per §3.
- Reset to Built-In, Delete Agent
- Import from Claude Code
- Instructions for every session (new, G11)

**Box › Permissions** (described)
- Missing:
  - Rules (action, pattern, effect)
  - Always Allowed (saved approvals, remove)
  - Blocks (`policies`)
  - Also Allowed › Add (it has Remove only)
- The page's own note already says so.

**Box › Commands** (`commands`)
- List; editor: name, description, agent, model, run as subagent, template
- Delete
- Import from Claude Code

**Box › Skills** (`skills`)
- Use switch, Edit, Remove, New Skill
- Recently Deleted › Restore
- Get Skills (repository, add or remove library skills)

**Box › Integrations** (`integrations`)
- Add App / Add Key / Connect / Turn On / Try Again
- Sign In Again, Change Key/App, Reconnect, Remove

**Box › Vault** (`vault`)
- Unlock
- Waiting for You (Allow Once / Always / Don't / Give)
- Items, the item sheet (all fields, who can use it, Let a Bot Use It)
- Pause Agents' Use
- Activity
- Offer to save sign-ins. Now on the box's Browser page, with Never Offered: done.

**Box › MCP Servers** (described)
- Done, but:
  - 0.7's catalog is there as Popular.
  - Timeouts are noise (§3).

**Box › Engine** (`engine`)
- Status (Restart / Start). This Mac has Restart.
- Bring Over… from opencode: an `actions` row. It copies opencode's setup into this computer's engine, so it belongs to the computer, not to Yours.
- Data folder › Show in Finder
- Summarize long conversations automatically
- Format files after changes
- Shell
- Web Search (service, API key)
- Plugins
- Keep snapshots of files: stays in the settings file, not on the page. Turning it off breaks Undo From Here, so it shouldn't be one click away.
- Settings file › Open / Show in Finder
- The rest stays in the file (§3).

**Box › Git, Worktrees** (described)
- Done.
- Missing: Worktrees' Show in Finder.
- New: G12, G13.

**Box › Storage** (described: What Agents Made only)
- Missing:
  - Disk use by kind
  - Data folder
  - Clean Up Old Sessions (with its form)
  - Delete Logs
  - Delete Downloads for machines (G6)

**Box › Archived Sessions** (described)
- Done.

**Box › Phone** (`phone`)
- Pair a Phone (QR code)
- Phones list with Remove (G1b)

**Box › This Mac** (described)
- Restart, version, update and devices are done.
- Login Items, Screen Recording and Accessibility are done.
- Missing:
  - Keep this Mac awake (G1)
  - Stop / Remove Kuvo (G17)

**Not in Settings** (to keep on the port's list):
- **The computers page** (desktop app: Add, Set Up Again, Uninstall). Missing: Make This Mac Its Home, Stop Using an address, Same Computer As.
- **Project editor:** name, location, folders, Run command, worktree setup command (Codex's Environments; Claude's `.worktreeinclude`).
- **Session menu:** Export as Markdown (G8), Copy as Markdown.
- **Changes:** Push afterwards.
- **Composer:** the reviewer model, starred models.
- **Bot editor and group chat editor:** all fields.

## 5. In short

**Add before the user's move:**

| Gap | Name | Side | Size |
|---|---|---|---|
| G1 | Keep this Mac awake | Box, This Mac | S |
| G1b | Phone pairing (if new phones still need 0.7) | Box | M |
| G3 | Notifications | Yours | M |
| G4 | Text size | Yours | M |
| G5 | System appearance | Yours | S |
| G9 | Shortcuts list | Yours | S |
| G17 | Stop Kuvo on this Mac (unless #761 covers it) | Box | S–M |
| — | §4's other Yours pages: General, Voice, Panel, Updates | Yours | M overall |

**Add before 0.8:**

| Gap | Name | Side | Size |
|---|---|---|---|
| G2 | Menu bar switch | Yours | S |
| G5 | Accent | Yours | S |
| G5 | Glass | Yours | M |
| G6 | Storage and Clean Up Old Sessions | Box | M |
| G7 | Send with ⌘Return | Yours | S |
| G8 | Export a session | Box route and menu | S–M |
| G10 | Anonymous usage switch, with #222 | Yours, and the box if boxes send | S |
| G11 | Instructions for every session | Box, Agents | S–M |
| G12 | Draft PRs | Box, Git | S |
| G13 | Remove Unused worktrees | Box | S–M |
| G17 | Remove Kuvo from this Mac | Box | M |
| — | The remaining box sections of §4 (#741) | Box | — |

**Skip:**
- G14: per-site browser rules, for now; fold them into the Rules editor.
- G15: steer or queue, as a setting.
- G16: everything in its table.
- G18: bot defaults.
- Typeface and Review findings.
- Rebinding shortcuts.
- A full backup.

**Drafts first:** every Yours page and every new row is a visible change and needs the user's pick of 2–3 drafts before it's built (CLAUDE.md). Accent is brand-adjacent.
