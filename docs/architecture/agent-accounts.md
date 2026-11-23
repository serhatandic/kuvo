# Coding-agent accounts

22 November 2026. The user's decision: "lets do it the t3 way … its okay for users to bring their
subscriptions." Code: `box/src/agents/accounts.ts`,
`roster.ts`, `signin.ts`, `agents/*.ts`; `box/src/usage/accounts.ts`, `native-usage.ts`.

## The model

**An account is one coding agent, pointed at one folder, on one box.** Every account has the same
controls (add, sign in, sign in again, remove) and the same paths for running, renewing and
reading usage. The box owns the list; clients draw it (the account picker, first run).

- **The first account is the agent's own sign-in, in its own folder**, the one Terminal uses, on
  every box: a Mac, a Linux machine, a hosted sandbox. The agent renews it itself. Kuvo never copies,
  refreshes or rewrites a credential.
- **Each added account is the same agent pointed at a folder of its own**
  (`agents/<kind>/accounts/<id>`), through the agent's supported switch. You sign in there with the
  agent's own sign-in.
- The first account keeps the provider ID `<base>`, so sessions, bots and model choices keep it.
  Added accounts are `<base>@<id>`.
- **Removing an added account** first signs it out through the agent, in its folder (`codex logout`,
  `grok logout`, `claude auth logout`), as OpenBot does. The Keychain or keyring entry tied to that
  folder goes too, and then the folder. The first account isn't removed: signing out of it signs out
  Terminal too, and the app says so.

| Agent | The switch | First account's folder | Sign-in (here / from another device) |
|---|---|---|---|
| Codex | `CODEX_HOME` | `$CODEX_HOME` or `~/.codex` | app-server `account/login/start`: browser / device code |
| Grok | `GROK_HOME` | `$GROK_HOME` or `~/.grok` | `grok login` / `grok login --device-auth` |
| Claude | `CLAUDE_CONFIG_DIR` (also names the Mac Keychain item) | `$CLAUDE_CONFIG_DIR` or `~/.claude` | `claude auth login`: browser / code paste-back |
| Cursor | the SDK's key store, one per account | `agents/cursor` (the SDK has no Terminal sign-in) | SDK browser login, or a pasted key |
| Antigravity | `GEMINI_HOME` | a private folder, like T3 (`agy` shares one keyring entry across folders) | `agy acp --login`: browser code paste-back |

**Linux is the same as a Mac** (the user, 11-22). Linux boxes used to keep the first account in
Kuvo's folder (`agents/<kind>/home`). That was for the footprint rule and to keep the sandbox
from writing `~/.codex/config.toml`. The user decided the sandbox is for accidents, not hostile
agents, so the agent's own folder is fine there too. The old folders move over on update (below).

## Where Kuvo differs from T3 Code and OpenBot

1. **Kuvo's own sign-in screen stays.** It works from another computer too, but it only drives the
   agent's *native* login. It never makes a token.
2. **One sign-in path per agent.** Codex gets no "Connect with ChatGPT" managed-renewal path.
3. **Antigravity** uses a private `GEMINI_HOME` from the first account, as T3 does.
4. **The sandbox (Auto mode only):** the agent's own folder is opened to it, read and write. Full
   Access sessions run outside the sandbox anyway. The sandbox only guards against accidents, so
   on a Mac Apple's `security` tool may write your login Keychain inside it (the keychain file and
   the copy it saves beside it, nothing else): Claude Code renews its sign-in there itself, with no
   plaintext fallback and no stale item (lead's decision, 22 November 2026). That tool could also
   remove items or the keychain: accepted for the same reason. On Linux, Landlock grants the
   agent's own folder whole (`~/.claude`'s `.credentials.json` is renewed there).

## Sign-in, renewal and usage

Sign-in runs on the box that owns the account. The client opens the page, or shows the agent's
device code or a field to paste a code back into. Each box signs in on its own, and nothing is
carried between boxes. The agent renews its sign-in as it runs. A sign-in that has ended shows
**Sign in again**.

The usage reader asks the agent itself, in the account's folder. It only fingerprints a sign-in to
notice when it changes, and never sends a token anywhere:
- **Codex:** a short `codex app-server`: `account/rateLimits/read` gives the windows, credits,
  reset credits and account. `account/rateLimitResetCredit/consume` spends a reset, with the box
  journal's request as Codex's idempotency key.
- **Grok:** a short `grok agent stdio`: ACP `_x.ai/billing`, after `authenticate` with its kept sign-in.
- **Claude:** the Agent SDK's usage call with `skipBehaviors`, on the account's sign-in (step 4).
- **Cursor:** as today, with the account's SDK key.
- **Antigravity (`agy`):** its own server while it runs (`RetrieveUserQuotaSummary`,
  `GetUserStatus` on its loopback port), with no credential sent; while it isn't running, the
  row says its usage shows once it runs. `agy` keeps its sign-in itself, so Kuvo has no token to
  read Google's quota service with. An account still on 1.3 reads as before.

## Install, updates and notices

Each agent runs from **its own normal install**. Where it isn't installed, **Install** runs the
maker's installer. Antigravity is Google's `agy` CLI (step 5, built), run as `agy acp
--gemini-dir <home> --app-data-dir antigravity-acp`, installed with
`curl -fsSL https://antigravity.google/cli/install.sh | bash` (to `~/.local/bin/agy`), updated
with `agy update`. Confirmed on a Linux test machine with 1.5.14 (22 November 2026): its
`--version` (`1.5.14`), `acp --help`, `AGY_CLI_DISABLE_AUTO_UPDATE`, the `--login` page and prompt
("Then paste the authorization code, or the full URL of the page you land on, and press Enter:"),
`initialize` (`agentInfo.version`, `oauth-personal`, `agentCapabilities.auth.logout`) and its
refusal with no sign-in (-32603 "No browser is available here…"). Not confirmed (needs a real
Google account): where the per-home sign-in file lands, and macOS.

- **Private home:** every account, the first too, has its own GEMINI_HOME in Kuvo's folder. `agy`
  1.5 keeps one keyring sign-in for every home unless it believes it runs over SSH, so Kuvo starts
  it with `SSH_CONNECTION=127.0.0.1 0 127.0.0.1 0`, as T3 Code does: it then keeps the sign-in in
  a file of that home.
- **Sign-in:** `agy acp … --login` prints Google's page and takes the code it ends on (pasted in
  Kuvo's sign-in sheet, from any device). Once it exits cleanly, `agy` is asked to use the
  sign-in and list the account's models; only then is it signed in, noted in
  `antigravity-acp/kuvo-sign-in.json` (an id of Kuvo's, never a secret).
- **Sign-out and removal:** ACP `logout`, then Kuvo's note and model list go.
- **Models:** as the account's session lists them; Claude and GPT models are named
  "Claude Opus 5.5 (Antigravity)", "GPT-6.2 Sol (Antigravity)", apart from Claude Code's and Codex's.

**"Update coding agents automatically"** is a box setting, **off by default**.
- **Off:** Kuvo starts each agent with its own updater turned off where it can. Settings shows the
  installed and newest version, with **Update**. As soon as a newer version is out, the box raises
  "Codex 0.x is available", with an Update action.
- **On:** agents update themselves. Afterwards the box says "Codex updated to 0.x".
- One notice per version, cleared once it's updated or seen. The box checks and words them; the
  app's notifications bell and macOS notifications draw them.
- Using an agent in Terminal can still update a shared install. Kuvo then notices the new version.

Built (step 6): the box checks each installed agent's newest version as accounts are read and
every hour, and keeps its notices in `<agents home>/update-notices.json`, one per program and
version (`box/src/agents/update-notices.ts`, PROTOCOL.md "Updates"). Each is told once on
`kuvo/notices` (category `update`) for macOS and phones, and served at
`kuvo/agents/api/kuvo/updates/notices` for the notifications bell.

Per agent:

| Agent | Turn its updater off | Newest version from |
|---|---|---|
| Claude Code | yes: `DISABLE_AUTOUPDATER=1` | `claude update` channel (npm `@anthropic-ai/claude-code`) |
| Codex | yes: `check_for_update_on_startup=false` | npm `@openai/codex` / GitHub releases |
| Grok | yes: `GROK_DISABLE_AUTOUPDATER=1` (already set) | npm `@xai-official/grok` |
| Antigravity (`agy`) | yes: `AGY_CLI_DISABLE_AUTO_UPDATE=1` (in its binary; T3 sets it) | Google's feed: `https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/<os>_<arch>.json` (`version`), as its installer reads it |
| Cursor SDK | n/a: a library inside Kuvo | updates with Kuvo's releases; no notice |

**Two pinned libraries, in plain words.** Two of the agents are reached through a code library that
ships *inside* Kuvo rather than a program you install:
- **Cursor's SDK** (`@cursor/sdk`): Kuvo talks to Cursor through it. It has no Terminal program of
  its own to update.
- **The Claude Agent SDK**: Kuvo's way of talking to Claude Code. It always runs *your* installed
  `claude`, never a copy of its own.

They're part of Kuvo's own code, tested with it, and they update when Kuvo updates. Your agents
themselves aren't pinned.

## First run

With the agent's own sign-in as the first account, there's nothing to carry over. The box reports
an agent already signed in natively as added, with its account name or email and plan, and first
run shows it as ready. Agents not signed in get the in-place sign-in, which drives the native login.
The box's path that copied a sign-in into Kuvo's folder goes, along with any first-run step for it.

## Existing sign-ins on update (once, automatic, per box)

- **Codex, Grok on a Mac:** already the agent's own folder. Nothing moves.
- **Codex, Grok, Claude on Linux** (`agents/<kind>/home`, built, `adoptOwnFolders`): the
  conversations move into the agent's own folder, so sessions go on with their threads.
  - If the agent's own folder has no sign-in, the old sign-in moves there (moved, not copied).
  - If it has one, yours stays the first account and the old one becomes an added account,
    "Earlier sign-in".
- **Claude on a Mac:** a `setup-token` token or Kuvo's Console folder (step 4):
  - if `~/.claude` is signed in, it's used and the token is deleted;
  - if not, the token keeps working, marked "Sign in again to see your plan's usage";
  - the Console folder becomes an added account.
- **Cursor:** unchanged. **Antigravity 1.3 to 1.5:** Google moved the sign-in into the keyring.
  An account keeps running on 1.3 and shows **Sign in again to update Antigravity**: the one step
  Google's change forces.

## What gets removed

- Done: Kuvo's own ChatGPT usage and reset-credit requests for Codex accounts (the engine's `openai`
  sign-in keeps its own). The direct xAI billing request and "sign-in has lapsed" case for Grok
  accounts. The machine-only first-account folders (`managedDefaults`, `yourHomes`).
- Done: Claude on a Mac: `setup-token`, the Console folder as Kuvo's own (an earlier one is now an
  added account, `claude-compat.ts`), the `CLAUDE_CODE_OAUTH_TOKEN` it set (but an earlier token,
  until your own sign-in is there), the token-based usage read and the terminal-screen scraping in
  `signin.ts`. Claude's resets still read Claude Code's `.credentials.json` where it keeps one
  (Linux): Claude Code has no reset request to ask.
- Done (step 5): Antigravity's pinned 1.3.0 download for new sign-ins (an account still on it keeps
  running until it's signed in again, which removes its token file). Left: removing the 1.3.0
  download and its code once no account uses it.
