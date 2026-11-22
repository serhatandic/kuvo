# Coding-agent accounts

22 November 2026. The user's decision: "lets do it the t3 way … its okay for users to bring their
subscriptions." Research: `.claude/notes/signin-research-1122.md`. Code: `box/src/agents/accounts.ts`,
`roster.ts`, `signin.ts`, `agents/*.ts`; `box/src/usage/accounts.ts`.

## The model

**An account is one coding agent, pointed at one folder, on one box.** Every account has the same
record (`kind`, `id`, `label`, `folder`), the same controls (add, sign in, sign in again, remove) and
the same paths for running, renewing and reading usage. The box owns the list; clients draw it
(`GET kuvo/agents/accounts`, the account picker).

- **The first account is the agent's own sign-in**, in its normal folder: the one Terminal uses.
  The agent renews it itself. Kuvo never copies, refreshes or rewrites a credential.
- **Each added account is the same agent pointed at a folder of its own**
  (`agents/<kind>/accounts/<id>`), through the agent's supported switch. You sign in there with
  the agent's own sign-in.
- The first account keeps the provider ID `<base>`, so existing sessions, bots and model choices
  keep it. Added accounts are `<base>@<id>`.
- **Remove** forgets the account. A folder Kuvo made goes with it. The agent's normal folder is never
  deleted: removing the first account only signs it out through the agent's own `logout`, after the
  app says that this also signs out Terminal.

| Agent | The switch | First account's folder | Sign-in (local / another device) |
|---|---|---|---|
| Codex | `CODEX_HOME` | `$CODEX_HOME` or `~/.codex` | app-server `account/login/start`: browser / device code |
| Grok | `GROK_HOME` | `$GROK_HOME` or `~/.grok` | `grok login` / `grok login --device-auth` |
| Claude | `CLAUDE_CONFIG_DIR` (also names the Mac Keychain item) | `$CLAUDE_CONFIG_DIR` or `~/.claude` | `claude auth login`: browser / code paste-back |
| Cursor | the SDK's key store, one per account | `agents/cursor` (the SDK has no Terminal sign-in) | SDK browser login, or a pasted key |
| Antigravity | `GEMINI_HOME` | a private folder (T3: 1.5 shares one keyring entry across folders) | `agy acp --login`: browser code paste-back |

**Linux machines (decision for the user).** On Linux the box's sandbox (Landlock) grants an
agent's home *whole*: it can't let Codex rename a new `auth.json` into `~/.codex` without also
letting it write `~/.codex/config.toml`, which your own Terminal Codex then runs outside any
sandbox. Seatbelt on a Mac can (state only, settings read-only). And the footprint rule keeps a
machine's sign-ins in `~/.kuvo-code/agents`. So on a Linux box the first account's folder stays
`agents/<kind>/home`: the agent's own sign-in for that box, signed in with its own device code,
renewed by the agent. **Recommended: keep it so.** Sharing `~/.codex` with Terminal on Linux
needs the user's word on the footprint rule and a sandbox that holds settings read-only first.

## Where Kuvo differs from T3 Code and OpenBot

1. **The sandbox stays.** T3 runs agents as plain processes. Kuvo runs them whole in the box's
   sandbox, and gives each agent only its own sign-in: on a Mac, the Keychain item its folder
   names; on Linux, the folder's files (see Sandbox access below).
2. **Kuvo's own sign-in screen stays.** It works from another computer too, but it only drives the
   agent's *native* login (browser, device code or code paste-back). It never makes a token.
3. **One sign-in path per agent.** Codex gets no "Connect with ChatGPT" managed-renewal path beside
   `codex login`.
4. **Antigravity**, like T3, uses a private `GEMINI_HOME` from the first account, because `agy`
   shares one keyring entry across folders.
5. **Removing an account** first signs out through the agent (`claude auth logout`, `codex logout`,
   `grok logout` with its folder), as OpenBot does. The Keychain or keyring entry tied to that
   folder goes with it, so nothing is left behind.
6. **Hosted sandboxes and machines** have only their own accounts. There is no Terminal sign-in to
   share there.

## Sign-in on local, remote and headless boxes

Sign-in always runs on the box that owns the account, through `commandSignIn` or the agent's own
RPC. The client opens the page. On a box with no browser, or when the page is opened on another
device, the box asks for the agent's device code (Codex, Grok) or a code to paste back (Claude,
Antigravity), and the client shows it (`signsInRemotely`). Nothing is carried between boxes. Each
box signs in on its own, so no two boxes share one rotating refresh token.

## Renewal

The agent renews its own sign-in as it runs. Kuvo starts the agent with its folder and nothing
else: no `CLAUDE_CODE_OAUTH_TOKEN`, no token files of its own, no refresh calls. A sign-in that
has ended shows **Sign in again** on its account.

## Usage, through each agent's own channel

The usage reader (`box/src/usage/accounts.ts`) asks the agent itself, as the account's folder
says, and never reads a credential:

- **Codex:** a short `codex app-server`, `account/rateLimits/read` (windows, credits, reset
  credits, account ID); reset credits are spent with `account/rateLimitResetCredit/consume`.
- **Grok:** a short `grok agent stdio`, ACP `_x.ai/billing` (the billing period) and
  `_x.ai/auth/info` (who it is).
- **Claude:** the Agent SDK's usage call with `skipBehaviors`, on the account's own sign-in.
- **Cursor:** as today, with the account's SDK key.
- **Antigravity:** its quota reader, on the account's folder.

The agent may renew its sign-in during the read: that's its own renewal, written where it keeps it.
Identity for counting one account once across boxes comes from the same answer.

## Sandbox access (the minimum per agent)

The agent's process runs whole in the box's sandbox with its account's folder (`fence.ts`
`fenceAgent`): on a Mac, the folder is read, and only its state written (sign-in, sessions, logs,
locks); settings stay read-only. On Linux, the folder is written whole, so it must be Kuvo's own.

- **Codex, Grok:** their sign-in is a file in the folder (`auth.json`, renamed into place):
  state write, nothing more. Codex in `keyring` mode also needs its Keychain item: reading
  `Codex Auth` is allowed. Writing it is not yet, so a keyring-mode account renews outside the sandbox (as today).
- **Claude (Mac):** its sign-in is the Keychain item `Claude Code-credentials[-<hash>]`. Renewing
  it means writing through `securityd`, which the sandbox's children would inherit. Step 4 proves
  what the minimum is under the real fence. If no grant is safe for the commands Claude runs,
  `setup-token` doesn't come back: the account says plainly that it can't renew inside the sandbox.
- **Cursor:** the SDK key is read by the bridge, outside the agent's reach.
- **Antigravity:** its folder (`GEMINI_HOME`); on a Mac its Keychain entry, like Claude's (step 5).

## Install and update

Each agent runs from **its own normal install, which updates itself** (`claude update`,
`codex update`, `grok update`, `agy`'s updater). Where it isn't installed, **Install** runs the
maker's installer. Kuvo shows each agent's installed version in Settings › Agents and offers
**Update** where the agent doesn't update itself (`maintenance.ts`). Antigravity is the exception
today: Kuvo downloads Google's standalone ACP server pinned to 1.3.0 (`agents/antigravity.ts`),
which never updates. That's why its updates were missing. It moves to Google's `agy` CLI
(`agy acp`). Pinned copies that remain, and why:
- **Cursor's SDK** (`@cursor/sdk`): the integration *is* the library the bridge loads; Cursor's CLI
  is a different product with its own sign-in. It updates with Kuvo's releases.
- **The Claude Agent SDK**: a library that runs whichever `claude` is installed, never its own copy.

## Existing sign-ins on update (once, automatic, per box)

- **Codex, Grok on a Mac:** already the native folder. Nothing moves.
- **Codex, Grok, Claude on Linux:** `agents/<kind>/home` stays the first account's folder.
- **Claude on a Mac:** a `setup-token` token (`agents/claude/tokens/<id>.json`) or Kuvo's Console
  folder (`agents/claude/console`):
  - native `~/.claude` signed in: the first account uses it, and the old token is deleted;
  - not signed in: the old token keeps running the first account, labelled "Sign in again to see
    your plan's usage", until you sign in; then it's deleted;
  - Kuvo's Console folder: becomes an added account with that folder (label "Anthropic Console").
  - Added Claude accounts are already `CLAUDE_CONFIG_DIR` folders: only their tokens go, the same way.
- **Cursor:** unchanged. Its folder already holds the account's key.
- **Antigravity 1.3 to 1.5:** Google moved the sign-in into the OS keyring, which can't be filled
  from 1.3's file. An account keeps running on 1.3 until it's signed in on 1.5, and shows
  **Sign in again to update Antigravity**. This is the one step a user must take, because Google's change requires it.

## What gets removed

- Codex: Kuvo's own reads of ChatGPT's usage and reset-credit endpoints with the agent's token,
  and of its Keychain item (`usage/codex-credentials.ts`) for usage. The engine's `openai` provider keeps
  its own.
- Grok: the direct billing request with the raw key from `auth.json`, and the "sign-in has
  lapsed" special case: Grok renews during the read.
- Claude (Mac): `claude setup-token`, `agents/claude/tokens`, Kuvo's Console folder, the
  `CLAUDE_CODE_OAUTH_TOKEN` it set, the token-based `/api/oauth/usage` probe and its terminal-screen
  scraping in `signin.ts` (`terminal`, `codeMask`, `onLine`).
- Antigravity: the pinned 1.3.0 download, `AGY_ACP_FORCE_FILE_STORAGE`, the browser catcher's
  ACP token file.
- Accounts: the split between "the agent's own" and "managed defaults" (`managedDefaults`,
  `yourHomes` in `accounts.ts` and `usage/accounts.ts`). It becomes one rule for the first
  account's folder.
