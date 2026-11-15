# #699 Boxes on Windows: design

Status: design only, written 2026-11-15 against main at `f6279a446`. Nothing here is built, and
**nothing was run on Windows**: there's no Windows machine yet. Every Windows fact below is from
reading our code and from what's publicly documented; each one that needs a trial is marked
**(trial)** and collected in §7. Two things were checked over the network from the Mac and are
marked **(checked)**.

Read with `713-desktop-app-runs-the-box.md` (who installs and keeps a box; its §2 says "A Windows
desktop app has no box of its own", which this note replaces), `693-own-box-browser.md`, and
`protocol/PROTOCOL.md` ("Peers" :417, "Updates" :2504, "The browser on a machine" :2790, "Sandbox"
:8784). Swift paths are the retired app's, read only. `E/` is
`swift/OpenCodeKit/Sources/OpenCodeKit/Engine/`.

## 0. In short

- **Two boxes, as the user asked: one in WSL, one native.** They're different amounts of work.
  - **A WSL box is a Linux box.** Our Linux code runs in it nearly unchanged; a WSL distro reached
    over SSH would already pass the Mac's probe (`uname -sm` says `Linux x86_64`). What's missing
    is around it: starting it (WSL doesn't start by itself and stops when idle), reaching it from
    Windows and from other computers, and saying plainly what's different (projects on `/mnt/c`).
    About 25 to 32 session-hours, most of it on the machine.
  - **A native box is a port.** The box's own JavaScript mostly runs (the engine publishes Windows
    builds, and the engine is our Bun), but everything that keeps, starts, stops, installs, updates
    and sandboxes it is POSIX shell or a Unix system call. About 95 to 125 session-hours without a
    sandbox; a real sandbox is its own design, 30 to 50 more.
- **Recommended order: WSL first, native second,** sharing one groundwork piece (a three-way
  platform layer) that is built and tested on Mac and Linux before any machine exists.
- **The native port wants one change that's good for every system:** the keeper, launch, wake-ups
  and the update's restore become the box's own code (`kuvo-link.js keep`, …) instead of shell
  held in Swift strings. #715 is already moving them into `box/`; moving them as TypeScript, not as
  shell files, is what makes a native Windows box possible without a second copy in PowerShell.
  That's a decision for the user (§8, D2).
- **Equal boxes.** A WSL box offers everything a Linux box does (the sandbox subject to one kernel
  check). A native box, first, can't sandbox commands, can't keep the computer awake, and has no
  vault SSH agent; each is said plainly by the box, as a Linux box without Landlock says it today
  (`kuvo/sandbox`'s `able: false` with a reason).
- **Thin clients.** Nothing below adds logic to a client. A Windows desktop app needs two small
  transports (`wsl.exe` in place of `ssh` for a WSL box on its own computer; no shared SSH
  connections on Windows), and runs the box's installer, as on a Mac (713 §3).
- **No fallback for older boxes.** New manifest keys (`windows`), new facts (`system`, `wsl`) and
  the wire rule for paths (§2.9) are simply required of a box that lists the feature.

## 1. What a box runs today

| Part | What it is | Where |
| --- | --- | --- |
| The engine | `opencode` 2.2.6, one binary per system; also **our Bun** (`BUN_BE_BUN=1 opencode kuvo-link.js …`); nothing else is downloaded as a runtime | `E/EngineInstaller.swift:26-66`, `E/RemoteEngine.swift:1229`, `box/src/link/home.ts:244` |
| The link | `kuvo-link.js` (`box/src/link`, 69k lines): routes, relay, peers, updates, sandbox, vault | `scripts/build-box.sh` |
| The agents' bridge | `kuvo-agents.js` (`box/src/agents`, 43k lines) | `scripts/build-agents.sh` |
| The browser's keeper | `kuvo-browser.js` and Chrome for Testing | `box/src/browser`, `box/src/link/browser/install.ts` |
| The keeper | `bin/kuvo-start`, `kuvo-launch`, `kuvo-setsid`: POSIX shell, **held in Swift strings** | `E/RemoteEngine.swift:1051-1337`, `:1369-1484`, `:542-548` |
| Wake-ups | `bin/kuvo-wake`, shell, run by the keeper every minute | `E/Wakes.swift:345-`, `E/RemoteEngine.swift:1240-1252` |
| Integrations' launcher | `kuvo-npx`, `runtime/node`: shell | `E/Launcher.swift:20-143` |
| The sandbox | `bin/kuvo-fence` (the shell the engine runs each command with), Seatbelt on a Mac, Landlock and a namespace on Linux | `box/src/link/fence/files.ts:68`, `seatbelt.ts`, `landlock.ts` |
| Starting at login | a tagged `@reboot` crontab line; on a Mac also a LaunchAgent | `E/RemoteEngine.swift:933`, `:1507-1550` |
| Install | the Mac app, over SSH, sends shell and the engine's tarball on standard input | `swift/KuvoCode/Stores/Machines.swift:570-712` |

Two facts that shape everything:

- **There is no `win32` anywhere in `box/src`** except one spawn option
  (`box/src/link/run-command.ts:103`). About 70 lines test `process.platform`, and nearly all are
  `=== "darwin" ? mac : linux`: on Windows they'd take the Linux side. The clearest is
  `box/src/link/fence/state.ts:101`, `const platform: "darwin" | "linux" = process.platform ===
  "darwin" ? "darwin" : "linux"`, which would try Landlock on Windows.
- **Injecting the platform is already how the box tests the other system:**
  `loginShell(env, platform)` (`box/src/shared/platform.ts:92`), `systemName(platform, read)`
  (`box/src/link/areas/computers.ts:47`), `runtimeHere(platform, arch)`
  (`box/src/link/browser/install.ts:102`), `thisComputer(platform, read, …)`
  (`box/src/agents/usage/usage.ts:583`), `listeningPorts(groups, {proc, platform})`
  (`box/src/link/work/services.ts:11`), `readKit(system)`, `suggestedFolders(…, platform)`,
  `discoverAntigravity(profile, platform, procRoot, runner)`. That pattern, with `path.win32` for
  pure path logic, is what lets most of this be tested on a Mac.

## 2. Inventory of Unix assumptions

Each row: where, what breaks natively, how WSL fares, what a native port needs. "WSL: same as
Linux" means nothing to do.

### 2.1 The keeper, launch and start at login

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `E/RemoteEngine.swift:1051-1337` (`kuvo-start`: `box`, `keep`, `engine`, `serve`, `start`, `stop`, `restart`, `off`) | `sh`; `&`, `$!`, `$$`; `trap`; `sleep 15` loop; `env -i`; `setsid`; `date +%s`; `stat -c/-f`; `find -mmin/-newer/-user`; `od /dev/urandom`; `ln` hard-link lock (:981-1048) | same as Linux | The keeper as the box's own code: `kuvo-link.js keep` and the rest (D2). No `sh` exists; a PowerShell copy would be a second copy of box logic |
| `E/RemoteEngine.swift:531-533`, `:988-990` | "is that pid ours": `kill -0`, `/proc/$pid/cmdline`, else `ps -ww -o command=`; owner by `find /proc/$1 -user` | same | §2.3 |
| `E/RemoteEngine.swift:1085-1097` | stop: `kill -- -$pgid`, wait, `kill -9` | same | §2.3 |
| `E/RemoteEngine.swift:1152` | `pkill -f "watchdog …"` (the engine's own) | same | by PID, in code (and it's a pattern kill today; worth replacing everywhere) |
| `E/RemoteEngine.swift:1369-1484` (`kuvo-launch`) | reads `KEY=VALUE` on stdin; `(umask 077; …)` for `keys/KUVO_KEY_*`; `opencode serve … >engine.log &`; polls with `grep`; EXIT trap | same | in code, with the keeper |
| `E/RemoteEngine.swift:542-548` (`kuvo-setsid`) | `perl -e 'POSIX::setsid()'` | same | not needed: a detached process on Windows has no session to leave |
| `E/RemoteEngine.swift:1074`, `:352-361` | `K=$HOME/.kuvo-code`; `chmod 700`; `XDG_*`, `TMPDIR`, `npm_config_cache`, `BUN_INSTALL` under `$K`; `PATH="$K/bin:$PATH"` | same | `%USERPROFILE%\.kuvo-code`; `TEMP`/`TMP` as well as `TMPDIR`; `;` in PATH; whether the engine honours `XDG_*` on Windows **(trial)** |
| `E/RemoteEngine.swift:1070-1071`, `gpui/kuvo_box/src/ssh.rs:87` | newest engine by glob, `[ -x ]`, `sort -t.` | same | `opencode.exe`; no execute bit; the choice made in code, once |
| `E/RemoteEngine.swift:933`, `:1501-1521` | `@reboot` line through `crontab -l`/`crontab -`, under `perl -e 'alarm 15'` | cron may not run, and WSL itself isn't started at boot: §3.3 | a per-user start at sign-in (§4.2); no crontab |
| `E/RemoteEngine.swift:1528-1554`, `box/src/link/helper.ts:47-119`, `box/src/link/uninstall.ts:34-62,139-153` | `launchctl`, LaunchAgents, `crontab` | Linux side same | Windows' own: uninstall must take out what §4.2 adds |
| `box/src/link/keeper.ts:49`, `box/src/link/areas/engine.ts:79`, `box/src/link/uninstall.ts:165`, `box/src/link/updates/update.ts:494,514,534` | `spawn("sh", [kuvo-start, …])` | same | calls into the box's own keeper code |
| `box/src/link/keeper.ts:41` | `runs(keeper, "kuvo-start")` by command line | same | the keeper is `kuvo-link.js keep`; §2.3 |

### 2.2 Wake-ups and the integrations' launcher

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `E/Wakes.swift:345-1058` (`kuvo-wake`) | shell; GNU `date -d` or BSD `date -j`; `timeout -k` or a perl fork; `cksum`, `od`, `awk`, `sed`; `curl -K -` or `wget` to the engine on 127.0.0.1; `mkdir` locks freed by `find -mmin`; `${x%/*}` path cutting; zone from `/usr/share/zoneinfo` | same; the clock can lag after Windows sleeps **(trial)**, which makes a wake-up late | in code (#715 already moves it): timers and `fetch` in the link; nothing Windows-specific left |
| `box/src/link/areas/wakes.ts:4,21` | runs `WAKE_COMMAND` (`kuvo-wake`) with `execFile` | same | calls the same code directly |
| `box/src/link/fence/files.ts:59-64` | the sandbox launcher makes `wakes`, `watches`, `inbox` with `umask 077` | same | §2.5 |
| `E/Launcher.swift:20-110` (`kuvo-npx`, `runtime/node`) | `#!/bin/sh`; `npm install --prefix`; `exec node`; `mkdir` lock with `kill -0`; `PATH="$(dirname "$0")/runtime:$PATH"` | same; **Windows' `node`/`npm` can be on WSL's PATH** (`/mnt/c/Program Files/nodejs`) and would be picked up: §3.6 | in code, or `.cmd` shims; `npm` on Windows is `npm.cmd`, which `spawn` won't run without a shell |
| `box/src/agents/toolrelay.ts:733-737` | an integration's server started as `/bin/sh -c 'BUN_INSTALL_CACHE_DIR=… exec "$0" "$@"'` | same | set the variable in `env`, spawn directly |
| `box/src/agents/toolrelay.ts:776,858` | `detached: true`, `process.kill(-pid, "SIGTERM")` | same | §2.3 |
| `E/CommandEnvironment.swift:75-88` | the engine's plugin written with a here-doc and `cmp -s` | same | written by the box's code |

### 2.3 Processes, signals and "is that still ours"

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `box/src/shared/platform.ts:30-76` (`processRuns`, `commandLineOf`, `runsAs`) | a pid's command line from `/proc/<pid>/cmdline`, else `/bin/ps -ww -o command= -p` | same | Windows has neither. A pid's command line needs `Get-CimInstance Win32_Process` (slow, hundreds of ms) or a native call. **Better: stop needing it.** Record each process's start time beside its pid and compare (one cheap call), or hold a named mutex per role |
| `box/src/link/compat/launch-lock.ts:44-45`, `box/src/link/browser/install.ts:150-160`, `box/src/link/permissions.ts:29`, `box/src/agents/usage/antigravity-plan.ts:111-120`, `gpui/kuvo_box/src/place.rs:114-164` | the same, and a scan of all of `/proc` or `ps -axww`; `lsof` for a listener | same | as above; `place.rs:161` already answers `None` off Mac and Linux |
| `box/src/shared/platform.ts:359-364` (`processUse`) | `/proc/<pid>/stat`, else `ps -A -o pid=,ppid=,state=,time=,command=` | same | CPU and memory of a process tree: PowerShell or native; or say "not shown on Windows" first |
| `box/src/link/work/services.ts:11-25` | listening ports by process group: `/proc`, else `lsof -g` | same | `Get-NetTCPConnection` by pid plus the tree; later |
| `box/src/agents/proc.ts:84-97`, `box/src/agents/sessions.ts:198-261,3104-3114`, `box/src/link/run-command.ts:92-115`, `box/src/link/areas/engine.ts:168`, `box/src/shared/platform.ts:187-202` | `detached: true` makes a process group; `process.kill(-pid, sig)` signals all of it; `kill(-group, 0)` asks if any is left | same | **No process groups in Node or Bun on Windows.** `process.kill(-pid)` fails. Needs a Job Object per started command (kill-on-close), which Node doesn't expose: through `bun:ffi` to `kernel32` **(trial)** or a small native helper; `taskkill /T /F` only follows parents and misses a child whose parent has gone |
| `box/src/agents/main.ts:576-577`, `box/src/link/main.ts:364-365,647,752-753` | `process.on("SIGTERM"/"SIGINT")` to clean up and stop children | same | **On Windows `kill(pid, "SIGTERM")` ends the process at once and no handler runs.** A graceful stop needs its own channel: a `stop` route on the loopback server, or closing a pipe the process reads |
| `box/src/link/main.ts:331,983,998,1168`, `box/src/link/serve-kept.ts:42`, `box/src/link/uninstall.ts:171`, `box/src/shared/hang.ts:211-219` | "ask with SIGTERM, then SIGKILL" | same | the channel above, then terminate |
| `box/src/shared/platform.ts:125` (`WATCHED`) | a shell watch that kills a login shell's group when a pipe closes | same | not needed natively (§2.4: no login shell to read) |
| `box/src/link/main.ts:619`, `box/src/link/areas/integrations.ts:290`, `box/src/link/updates/update.ts:863`, `box/src/link/browser/install.ts:655`, `box/src/link/github/graphql.ts:352`, `box/src/link/keeper.ts:66` | `spawn(process.execPath, …, {detached: true, stdio: "ignore"}).unref()` outlives its parent | same | works on Windows, but **each opens a console window** unless `windowsHide: true` is set, at every `spawn` in the box (about 60) **(trial)**: one `spawn` of our own in `shared/` |
| `box/src/link/fence/finished.ts:15`, `box/src/agents/sessions.ts:136`, `box/src/agents/agents/claude.ts:1300` | a command's end reads as a signal name | same | Windows gives an exit code only |
| `box/src/shared/files.ts:78-100` | locks are folders (`mkdir`), stale by mtime | same | works on NTFS |
| `box/src/link/home.ts:106`, `box/src/link/compat/launch-lock.ts:106`, `box/src/browser/kuvo-browser.js:130,394` | `fs.linkSync` to put a file in place only if none is there | same | hard links work on NTFS; not on FAT/exFAT or some network drives (the code already falls back) |

### 2.4 Shells, PATH and the commands agents run

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `box/src/agents/sessions.ts:247-248` | a conversation's command: `$SHELL -c <command>` (else zsh or `/bin/sh`), detached | same | **which shell?** (D3). Recommended: Git for Windows' `bash.exe`, as Claude Code itself requires on Windows; then `-c` holds |
| `box/src/link/run-command.ts:102-103` | `$SHELL -lc` | same | as above |
| `box/src/agents/maintenance.ts:128` | an agent's installer: `/bin/sh -c <command>` (`curl … \| bash`) | same | each agent's Windows installer is PowerShell or npm: §5 |
| `box/src/link/fence/files.ts:68-262` (`bin/kuvo-fence`) | **the engine's shell is our `sh` script**, which finds the conversation, then runs the real shell inside the sandbox | same | with Git Bash it can stay a script for the no-sandbox path; whether the engine on Windows takes a `SHELL` of ours **(trial)** |
| `box/src/shared/platform.ts:92-121` (`loginShell`, `accountShell`) | `$SHELL`, `/etc/passwd`, `getent` | same | none: Windows has no login shell |
| `box/src/shared/platform.ts:145-243` (`loginPath`, `loginEnvironment`) | runs the login shell `-ilc` to read PATH as a terminal has it | works; the PATH it reads **includes Windows' folders** (`/mnt/c/…`, with spaces and parentheses) unless `appendWindowsPath=false`: §3.6 | PATH comes from the registry (`HKCU\Environment` and the system's), which a process started at sign-in already has; re-read it there when an install changes it |
| `box/src/shared/platform.ts:129`, and 12 `split(":")` / 4 `join(":")` in `link` and `shared` | `:` separates PATH | same | `path.delimiter`; a drive letter makes `:` wrong at once (`C:\…`) |
| `box/src/link/work/reviewer.ts:100,158-167,400-466,677-701` (Auto's reviewer) and `box/src/agents/sessions.ts:500-600` | they read commands as POSIX shell: wrappers (`sudo`, `env`, `nohup`…), startup files (`/etc/profile`, `.bashrc`), `crontab`, `systemctl`, `launchctl` | same | with Git Bash, the grammar holds; what's missing is Windows' own persistence and danger (`schtasks`, `reg add …\Run`, `sc create`, `powershell -enc`, `Start-Process -Verb RunAs`, the Startup folder) and commands handed to `cmd /c` or `powershell -c`, which it can't read: those ask |
| `box/src/link/fence/policy.ts:148,298,400-491` | secrets and "runs later" places by Unix path (`.ssh`, `.config/systemd`, `~/Library/…`), sockets in `/tmp`, `/run`, `/var/run` | same | Windows' lists (`%APPDATA%`, `%LOCALAPPDATA%`, credential stores, the Startup folder); only matters once there's a sandbox or for the reviewer |
| `box/src/agents/signin.ts:62-71` | a `#!/bin/sh` script as `BROWSER`, checked `uid` and mode `0700`, to catch a sign-in's URL | same | a `.cmd` or the box's own program as `BROWSER`; the check becomes §2.5's |

### 2.5 Private files, permissions and keys

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| 217 lines with `chmod`, `mode: 0o600/0o700` or a mode test in `link`, `shared`, `browser` (most in `kuvo-browser.js`, `updates/update.ts`, `browser/install.ts`, `shared/files.ts`, `rehome/`, `fence/state.ts`); more in `agents` | a file is private because its mode says so | same on the Linux filesystem. **On `/mnt/c` modes are made up** unless the drive is mounted with `metadata`; `K` is never there, but a project can be | **On Windows `mode` is ignored, `chmod` only flips read-only, and `stat.mode` is invented.** Privacy is the folder's ACL: `%USERPROFILE%` is the user's, SYSTEM's and Administrators' by default, and `.kuvo-code` inherits it |
| `box/src/shared/files.ts:44-54` (`privateFolder`), `:59-70` (`placeWhole`) | `stat.uid === getuid()`, `mode & 0o077`, then `chmod` | same | **one** definition of "private" in `shared/files.ts` with a Windows side: checks the ACL once for `K` (`icacls`, or native), refuses a `K` that others can read, and makes the per-file checks pass by it |
| `box/src/link/vault/ssh.ts:45`, `box/src/link/vault/holder.ts:159`, `box/src/agents/signin.ts:71`, `box/src/agents/agents/claude.ts:619`, `gpui/kuvo_box/src/ssh.rs:235-249` | strict: refuse unless owner is me and no group/other bits | same | these **refuse** on Windows as written (`getuid` is undefined, `uid` is 0, mode is `0o666`); they take the definition above |
| `E/RemoteEngine.swift:1369-`, `keys/KUVO_KEY_*`, the vault (`box/src/link/vault/store.ts`) | keys in `0600` files (a Mac: Keychain through Kuvo Helper) | same as Linux: files | files under the profile's ACL first; DPAPI (tied to the Windows account) later, as the Mac has the Keychain. Say which in `privacy.md` |
| `box/src/shared/account.ts:19-37` (`accountEntry`) | `id -P` on a Mac, else `getent passwd`, else `/etc/passwd` | same | neither exists: falls through to `os.userInfo().homedir` with **an empty account name**. Needs a Windows branch (`os.userInfo()`, `USERPROFILE`) |
| `box/src/shared/account.ts:12` | `HOME`, else `os.homedir()` | same | `HOME` is unset on Windows, but set (in MSYS form) for anything started from Git Bash: take `USERPROFILE` first |
| `box/src/link/fence/state.ts:654`, `box/src/link/home.ts:85`, `box/src/link/fence/policy.ts:400,487` | `getuid()` or `os.userInfo().uid` in path names (`/tmp/kuvo-ssh-<uid>`) | same | Mac-only today; keep it so |

### 2.6 Sockets

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `box/src/browser/kuvo-browser.js:38,1760`, `box/src/link/areas/asks.ts:74,130`, `box/src/link/browser/view.ts:19`, `box/src/link/browser/keeper-client.ts:34` | `run/browser.sock`, a Unix socket in a private folder | same | Node and Bun on Windows listen on **named pipes** (`\\.\pipe\…`), not socket files. One function names "the box's socket X": a path on Unix, a pipe named from a hash of `K` on Windows |
| `box/src/link/vault/holder.ts:19,140,159`, `box/src/link/fence/files.ts:403` | `run/vault.sock`, checked to be a socket, mine, mode `0700` | same | a pipe; **any local account can try to open a pipe**, so the key every caller already gives first (`run/socket.key`, `box/src/link/home.ts:74-110`) is what protects it. Confirm the pipe's default ACL **(trial)** |
| `box/src/link/vault/ssh.ts:184`, `box/src/link/vault/ssh-agent.ts:262`, `box/src/link/home.ts:79-86`, `box/src/link/fence/files.ts:247` | a per-conversation SSH agent on a Unix socket, handed to commands as `SSH_AUTH_SOCK`; path length limits (104/108 bytes) | same | Windows' own `ssh` talks to a named pipe; Git for Windows' `ssh` talks to an MSYS socket file. Which one a conversation's `git` uses decides the shape **(trial)**. First: not offered, said plainly |
| `box/src/link/main.ts:739,797` | the box's server and the kept server on `127.0.0.1:0` (TCP) | same; **WSL2 forwards `localhost` to Windows**, so a Windows app can reach it (§3.2) | same code |
| `E/RemoteEngine.swift:125-137`, `gpui/kuvo_box/src/ssh.rs:75,252-262` | the *client's* shared SSH connections: `ControlPath=/tmp/kuvo-ssh-<uid>/%C` | n/a (client) | a Windows desktop app: Windows' OpenSSH has no `ControlMaster`; `ssh.rs:258` already answers "none" (every request a new connection: slow, so the kept server's tunnel matters more there) |
| `gpui/notes/kuvo-box-machines.md` (planned `-L <socket>:…`) | forwarding to a Unix socket | n/a | a Windows client forwards to a TCP port instead |

### 2.7 The sandbox

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `box/src/link/fence/state.ts:101,451-475` (`able`) | Mac: `/usr/bin/sandbox-exec`. Anything else: Landlock (`/sys/kernel/security/landlock`), `unshare` (`/usr/bin/unshare`), unprivileged user namespaces | WSL2 only (WSL1 has no Linux kernel: no Landlock, no namespaces). WSL2's kernel must list `landlock` in `/sys/kernel/security/lsm` **(trial; older WSL kernels didn't)**; user namespaces are normally allowed there. Where it isn't able, the existing `reason: "kernel"` or `"namespaces"` text says so, already correct | a third answer: `able: false, reason: "windows"`, "Windows can't sandbox commands yet." (a new word in `box/src/shared/routes/sandbox.ts:22`) |
| `box/src/link/fence/landlock.ts:100-440` | `mount --bind`, tmpfs, `/proc/self/mountinfo`, `uid_map`, `mkfifo`, `timeout`, `find`, `awk`, `sed` | same as Linux | none of it exists. A Windows sandbox is AppContainer or a restricted token, set up with Win32 calls from a native program; allowing a project folder means **adding an ACL entry to the user's folder**, which is a footprint question. Its own design (§6, piece N9) |
| `box/src/link/fence/seatbelt.ts`, `box/src/link/fence/outside.ts:93-288` | Seatbelt profiles; `/usr/bin/log show` to see what was refused | n/a | n/a |
| `box/src/link/work/approval.ts:105-129`, `box/src/link/fence/state.ts:712` | what a box does with no sandbox: Auto asks first, bots allow no command, the vault stays shut | as a Linux box that can't | exactly this, by the existing code path, once `able` says so. **This is a real difference from a Linux box and must be drawn in the UI from `kuvo/sandbox`, which clients already do** |
| `docs/footprint.md` (#429: `.git/commondir` and empty folders added to repositories on Linux) | Linux only | applies in WSL as on Linux; on `/mnt/c` repositories those writes cross to NTFS **(trial)** | none |

### 2.8 Install, update and uninstall

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `E/RemoteEngine.swift:178-180`, `gpui/kuvo_box/src/ssh.rs:85-102` | every remote command is `exec sh -c '… printf %s <base64> \| base64 -d …; eval'` in the account's login shell | works if SSH lands in the distro (§3.2); through Windows' SSH server it lands in `cmd.exe` and fails | a second form of the one command for a Windows box, or better, a fixed entry the box provides (`bin/kuvo-box route`, `bin\kuvo-box.cmd route`) so the client holds no script (today `ssh.rs:87` chooses the engine in shell, which is logic in a client) |
| `swift/KuvoCode/Stores/Machines.swift:395-462`, `E/RemoteEngine.swift:293` | probe `uname -sm`, `ldd --version`; accepts Linux glibc and Darwin; else "The engine runs on macOS, and on Linux with glibc, on x86-64 or ARM64; this machine is …" | passes as Linux | a probe that works in `cmd`/PowerShell (`ver`, `%PROCESSOR_ARCHITECTURE%`); the refusal's words change |
| `E/EngineInstaller.swift:26-66`, `swift/KuvoCode/Stores/BoxRelease.swift:51-54`, `box/src/link/updates/update.ts:456-466` | engine builds for `linux` and `darwin` × `x86_64`/`aarch64`; the manifest has `manifest.darwin` and `manifest.linux`; unpacks `package/bin/opencode` with `tar`; `chmod 0o755` | Linux builds | **(checked)** npm has `@opencode/cli-windows-x64` and `@opencode/cli-windows-arm64` at 2.2.6. A `windows` key in the manifest; the file is `opencode.exe` (name inside the package **(trial)**); `tar.exe` ships with Windows 10 and later; no `chmod` |
| `swift/KuvoCode/Stores/Machines.swift:494-504` | the Mac downloads the engine and pipes it to `tar xzf -` over SSH; then `find … -exec rm -rf` | works | binary stdin through Windows' SSH server and `cmd` **(trial)**. Better: the machine downloads its own engine and release (`curl.exe` and `tar.exe` are in Windows), verifies them, and the box's installer does the rest in code (713 §4.1's installer, which a Windows app or a one-line bootstrap runs) |
| `box/src/link/updates/update.ts:249-292` (`keepForRollback` writes `restore.sh`) | the way back from a failed update is a **generated shell script** (`cp -p`, `mv -f`, `kill -0`, `/proc` or `ps`, `sh kuvo-start stop`), run by the update or later by the keeper | same | a plan as data (`restore.json`) run by the box's code (`kuvo-link.js update restore`), with the keeper (D2) |
| `box/src/link/updates/update.ts:411,465,476,312` | `tar xzf`, `sh -n kuvo-start` to check the keeper reads, `df -Pk` for room | same | `tar.exe`; no `sh -n`; `fs.statfs` for room (also on Unix) |
| `box/src/link/updates/update.ts:502-513`, 76 `renameSync` in `link`/`shared` | a file is replaced by renaming a new one over it, running or not | same | **Windows won't replace or delete a running `.exe`, nor rename a folder a process has open or as its working folder** (every box process starts with `cwd: K`). Engines are already per-version folders, so a new engine never overwrites a running one; removing the old one must wait until it's stopped. `.js` files only being read can be replaced. Uninstall and `rehome` need the processes gone first **(trial)** |
| `box/src/link/updates/update.ts:437` | helper bundles only on `darwin` | none | a native starter program, if D4 chooses one, travels the same way |
| `scripts/pack-box.sh`, `scripts/build-box.sh`, `scripts/sign-box.mjs` | built on a Mac with `sh`, `tar`, `codesign` | none | the pack is the same for all systems (JavaScript and data); only a native starter would need a Windows build and Authenticode signing (a certificate: D4) |
| `box/src/link/uninstall.ts:34-188` | stops by pid and SIGTERM, removes the crontab line and LaunchAgent, leaves `K` | same, plus the Windows-side task (§3.3) | removes what §4.2 adds; `uninstall.md` gains a Windows section |
| `box/src/link/moves/bundle.ts:172-188` (Move to Computer) | `tar -czf`/`-xzf`, `--no-mac-metadata` on a Mac | same | `tar.exe` (bsdtar) works; a bundle from a Mac or Linux box holding names illegal on Windows (`:`, trailing dots, `CON`) must be refused with the name said |
| `box/src/link/rehome/*` | Mac only (`NOTHING_TO_MOVE` elsewhere) | n/a | n/a |

### 2.9 Paths, names, links and text

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `box/src/link/home.ts:15`, `gpui/kuvo_box/src/place.rs:35` | `K = ~/.kuvo-code` | the distro's home | `%USERPROFILE%\.kuvo-code`, same name (D1). Not `%APPDATA%`: one name on every system keeps docs, uninstall and support the same |
| 22 `startsWith("/")`, 48 `split("/")`, 29 `"~/"` in `link`/`shared`/`browser`; more in `agents` (§5); `swift/KuvoCode/Stores/Machines.swift:1953-1960,2025-2029` (clients: a path is absolute if it starts with `/`) | `/` separates, absolute starts with `/`, `~/` is home | same | **A rule for the wire, in PROTOCOL.md: a Windows box says and takes paths with forward slashes and the drive letter kept (`C:/Users/me/app`).** Windows accepts them; every client's `split("/")` keeps working; only "is it absolute" widens (a drive letter). Inside the box, `path.*` for the system's own form, converted at the routes. The alternative (backslashes on the wire) makes every client learn two path grammars |
| `box/src/link/areas/claude-code.ts:101`, `box/src/link/work/reviewer.ts:112-115,263`, `box/src/link/work/reviewing.ts:460` | names fold case on a Mac only | Linux filesystem: case-sensitive. `/mnt/c`: case-insensitive by default, per folder | Windows folds case too: one `foldsCase()` in the platform layer, three callers |
| `box/src/link/fence/lookalike.ts`, `box/src/link/work/reviewer.ts` (what a path really names) | a path names one file once links are followed | same | Windows adds aliases: 8.3 short names (`PROGRA~1`), `file::$DATA` and other streams, trailing dots and spaces, `\\?\` and UNC forms, junctions. Any rule that allows or refuses by path must canonicalize first; needed by the reviewer even with no sandbox |
| `box/src/link/browser/install.ts:358,430,631`, `box/src/link/browser/zip.ts:189` | `browser/current` is a **symlink** to the build; the zip's own symlinks are made | same | making a symlink needs Developer Mode or an administrator. Use a junction for folders (no privilege), or a one-line pointer file; Chrome's Windows zip has no links |
| `box/src/browser/kuvo-browser.js:509,519` | Chrome's `SingletonLock` and `SingletonSocket` are symlinks naming the owner | same | Windows' Chrome locks its profile another way (a `lockfile`); the "whose profile is it" check needs its Windows form **(trial)** |
| `box/src/link/areas/claude-code.ts:115-145`, `box/src/shared/git.ts:196-235`, `box/src/link/work-queue/ops.ts:107`, `box/src/link/work/project-trees.ts:47`, `box/src/link/browser/zip.ts:163` | `O_NOFOLLOW`, `O_NONBLOCK`, `/proc/self/fd/<n>` to read a file without following links or blocking on a FIFO | same | the constants are undefined on Windows (some sites already write `?? 0`, some don't: `project-trees.ts:47` would pass `NaN`). `lstat` first, and no FIFOs to fear |
| `box/src/link/home.ts:244` (`OWN_RUNTIME = ["--config=/dev/null", "--no-env-file"]`), `box/src/link/updates/update.ts:474`, `box/src/link/fence/files.ts:93`, `gpui/kuvo_box/src/ssh.rs:92` | `/dev/null` as "no config file", on every start of our own code | same | `os.devNull` (`\\.\nul`); whether Bun takes it for `--config` **(trial)**, else an empty file of ours |
| `box/src/shared/git.ts:23` (`core.hooksPath=/dev/null`), `box/src/link/areas/changes.ts:254` (`git diff --no-index -- /dev/null`) | git reads `/dev/null` | same | Git for Windows takes `/dev/null` in `diff --no-index`; for `hooksPath` an empty folder of ours is surer **(trial)** |
| `box/src/shared/zone.ts:30` | the zone from `/etc/localtime`'s link | WSL keeps it in step with Windows | falls to `Intl`'s zone, which is right on Windows |
| `box/src/link/areas/computers.ts:47-62` (`systemName`) | macOS's plist, `/etc/os-release` | says "Ubuntu 24.04", nothing about Windows: add the fact (§3.5) | `os.version()` ("Windows 11 Pro") |
| `box/src/agents/usage/usage.ts:583-618` (`thisComputer`) | `ioreg` UUID, `/etc/machine-id` | `/etc/machine-id` exists with systemd; without it, a fallback to the name. A WSL box and a native box on one PC are two boxes, rightly | `MachineGuid` from the registry (`reg query`) |
| `box/src/link/peers/computer-name.ts`, 19 `hostname()` | the computer's name | **the same as Windows' name**, so a WSL box and a native box on one PC would be named alike: §3.5 | fine |
| `box/src/agents/snapshots.ts:60` | sets `core.autocrlf=false`, `core.symlinks=true`, `core.longpaths=true` for its own snapshots | same | already right for Windows; `core.symlinks=true` without the privilege makes git write link files as text **(trial)** |
| every `split("\n")` over a file a person or another tool wrote (10 places already allow `\r\n`) | LF | same on the Linux filesystem; a project on `/mnt/c` checked out by Windows' git has CRLF | CRLF is the default checkout with Git for Windows (`core.autocrlf=true`). Diffs and Changes follow the user's git as they do now; what needs a look is each parser of a user's file (`.gitignore`, `.env`, config, skills, `CLAUDE.md` front matter) for a trailing `\r`, and never writing our scripts with CRLF |
| filenames the box makes | any character but `/` | same | no `:`, `*`, `?`, `"`, `<`, `>`, `\|`, no trailing dot or space, no `CON`/`NUL`/`COM1`; a path over 260 characters fails in tools that aren't long-path aware (`K\data\opencode\worktree\<id>\<name>\…` is deep already). Audit of names built from titles, branch names, times and ids |
| `box/src/link/work-queue/ops.ts:93`, `E/RemoteEngine.swift:1074` | `/tmp`, `os.tmpdir()`, `TMPDIR=$K/tmp` | same | `os.tmpdir()` only, and `TEMP`/`TMP` set to `K\tmp` for what the box starts |
| `box/src/shared/watch.ts:48`, `box/src/agents/artifacts/folders.ts:39`, `box/src/link/areas/changes.ts:1099`, `box/src/shared/git.ts:38` | recursive `fs.watch` on a Mac; a watch per folder on Linux (inotify); git's fsmonitor on a Mac | Linux side same. **On `/mnt/c`, changes made from Windows don't reach inotify** | recursive watching works on Windows (as on a Mac): the `linux` flag becomes "not recursive here"; git's fsmonitor exists on Windows too |

### 2.10 What a box asks the system for, tool by tool

`git` (18 call sites and `shared/git.ts`), `sh` (13), `tar` (4), `ps` (5), `df` (4), `du`
(`box/src/link/git.ts:94`), `lsof` (2), `getent` (2), `id`, `cp` (Mac's `rehome`), `mount`,
`unshare`, `crontab`, `launchctl`, `security`, `ioreg`, `pmset`, `caffeinate`, `plutil`, `log`,
`osascript`, `mdfind`, `trash`, `gh`.

- **WSL:** all the Linux ones are there in a normal distro. `unshare` is util-linux (present on
  Ubuntu). `git` is the distro's.
- **Native:** `tar.exe`, `curl.exe` and usually `ssh.exe` ship with Windows. **`git` doesn't**, and
  the box can't work without it. `df`/`du` become `fs.statfs` and a walk in code; `ps`, `lsof`,
  `getent`, `id` go with §2.3 and §2.5.
- **Mac-only features and what a Windows box says** (as a Linux box does today):
  `box/src/link/work/awake.ts:23-86` (keep awake: "Only a Mac can be kept awake" until a native
  call exists), `box/src/link/work/apps.ts:102-247` (apps to open: none), `box/src/link/areas/changes.ts:566`
  (the system's trash: the box's own `trash` folder), `box/src/link/browser/from-app.ts:45`,
  `box/src/link/helper.ts:47`, `box/src/link/permissions.ts:51`.

### 2.11 The browser

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `box/src/link/browser/install.ts:102-110` (`runtimeHere`) | "The browser runs on Linux and macOS; this machine is win32." | Linux build | a `windows` runtime. **(checked)** Chrome for Testing 155.0.8059.79 has `win64` and `win32` zips and **no `win-arm64`**: on ARM Windows the x64 build under emulation **(trial)**, or said plainly; in WSL on ARM the `linux-arm64` build exists |
| `box/src/link/browser/linux.json`, `rpm.ts`, `xz.ts` | Chrome's libraries and fonts brought along as pinned Rocky 8 packages, so nothing is installed on the system | same: works without WSLg, since Chrome runs headless (`kuvo-browser.js:573`) **(trial)** | none needed: Windows has what Chrome needs; a `windows.json` with only `chromium` |
| `box/src/browser/kuvo-browser.js:661` | Chrome driven over inherited pipes (`stdio[3]`, `stdio[4]`) | same | extra stdio pipes to a Windows child under Bun **(trial)**; the fallback is a loopback debugging port, which any local process could open, so not without a decision |
| `box/src/browser/kuvo-browser.js:601` | `--no-sandbox` when Chrome's own sandbox can't run (no user namespaces) | Chrome's sandbox needs user namespaces: fine on WSL2 | Chrome's sandbox works on Windows as is |
| `box/src/link/browser/install.ts:122-128` (`hasBrowser`) | `chrome` with the execute bit | same | `chrome.exe`, no bit |
| `box/src/link/browser/install.ts:389` | `df -Pk` for room | same | `fs.statfs` |
| `docs/footprint.md` (Chrome's own leftovers) | on a Mac, `CFFIXED_USER_HOME` | Linux: none | Chrome on Windows writes under `%LOCALAPPDATA%` and the registry unless told otherwise **(trial)**: footprint |

### 2.12 The coding agents' bridge

Paths under `box/src/agents/`. The bridge has **no Unix sockets**: every agent is driven over
standard input and output or a pseudo-terminal, and its own channels are HTTP on 127.0.0.1
(`main.ts:402-404`, `toolrelay.ts:245`). What's Unix in it:

| Where | What it assumes | WSL | Native port needs |
| --- | --- | --- | --- |
| `proc.ts:12-25` (`which`) | a program is `<folder>/<name>` with the execute bit; no `PATHEXT` | same; finds Windows' programs on `/mnt/c` (§3.6) | `.exe`, `.cmd`, `.bat` by `PATHEXT`; **a `.cmd` (every npm-installed agent) can't be spawned without a shell**, and going through `cmd.exe` brings its quoting: run the `.js` it points to with our Bun instead |
| `proc.ts:36-109`, `rpc.ts:44,150-156`, `toolrelay.ts:776,858`, `sessions.ts:196-279,3101-3117` | groups and signals (§2.3); `rpc.ts` stops an agent by closing its input, then SIGTERM | same | closing input works; the rest is §2.3. **An agent's own children (its shells, MCP servers) outlive a terminate** unless in a Job Object |
| `signin.ts:164-170` | sign-ins that need a terminal use Bun's pseudo-terminal (`Bun.spawn(…, {terminal})`) | same | whether Bun has it on Windows (ConPTY) **(trial)**; if not, those sign-ins (Claude Code's `setup-token`, Grok's `login`) need another way or are said to be done in a terminal by hand |
| `signin.ts:56-71` | `BROWSER` is our `#!/bin/sh` script, `0700`, mine; no spaces in its path | same | a `.cmd` or our Bun with a script; `C:\Users\First Last\…` **has spaces** more often than a Unix home: the path rule needs a real fix |
| `signin.ts:82-96` | others' leftovers found by pid and `kill(pid, 0)` → `ESRCH` | same | `kill(pid, 0)` works on Windows; the error code differs **(trial)** |
| `machine.ts:74-86` | `own-folders.sh` (`export X='…'`) sourced by Claude Code as `CLAUDE_ENV_FILE` | same | sourced by Git Bash if D3 holds; values need MSYS-safe quoting |
| `maintenance.ts:128,94,191-256` | install: `/bin/sh -c 'curl … \| bash'`; update: by the install's real path (`/lib/node_modules/`, `/Cellar/`, `/.local/share/claude/`), `PATH` joined with `:` (`:210`) | same | each maker's Windows installer (PowerShell `irm … \| iex`, or npm); npm's layout on Windows is `%APPDATA%\npm\node_modules\…` with `.cmd` shims; no Homebrew (winget and scoop exist: leave those updates to the user, as `mise` is today, `:236`) |
| `sessions.ts:305-688,836-841` (permission rules read commands) | POSIX grammar, `sh -c`, `sudo`, `env`, `xargs`, `find -exec`; `apt`, `systemctl`, `launchctl` | same | holds for Git Bash; `cmd /c` and `powershell -c` aren't followed, so what's inside them can't match an "always allow" and must ask (the safe side); add them to `RUNS_ANOTHER` |
| `sessions.ts:750-751,735,491,604`, `fence.ts:133,164,273,282`, `acp.ts:418`, `toolrelay.ts:709,726`, `artifacts/routes.ts:171`, `diff.ts:141`, `claude.ts:616-620,667`, about 45 in all (`reviews.ts:1273` already knows `^[A-Za-z]:`) | absolute means `/…`; `~/`; names split on `/` | same | §2.9's rule, and `path.isAbsolute`. **`fence.ts:133,164` and `acp.ts:418` silently drop any path that doesn't start with `/`**: on Windows every allowed folder would vanish from the policy (harmless while there's no sandbox, wrong the day there is one) |
| `codex.ts:2588-2610` (`shadowHome`), `skills.ts:213` | **symlinks** for a second Codex account's home and Cursor's skills | same | junctions for folders, copies or hard links for files (§2.9) |
| about 40 `mkdir 0o700` / write `0o600` sites (`fence.ts`, `skills.ts`, `subagents.ts`, `carry.ts`, `groups/store.ts`, `artifacts/store.ts:527-536` `fchmod`, usage caches) | §2.5 | same | §2.5; nothing per site if `K` is private |
| `main.ts:451-459,555,563-570` | `run/bridge.json` with pid, port, password; the engine watched with `kill(pid, 0)` every 5 s; exits when its input closes | same | works; "is that pid the engine" is §2.3 |
| `main.ts:498`, `shared/hang.ts:232` | the parent's command line (`commandLineOf(process.ppid)`), `includes("kuvo-agents")` | same | §2.3 |
| `main.ts:516-517`, `cursor.ts:683-694` | `--config=/dev/null`; `PATH ?? "/usr/bin:/bin"` | same | §2.9 |
| `artifacts/folders.ts:39-60` | Linux: a watch per folder (inotify, up to 4000); elsewhere one recursive watch | Linux's; blind to Windows-side edits on `/mnt/c` | the recursive one, as a Mac |
| `usage/antigravity-plan.ts:92-127` | Antigravity's quota port found in `/proc` (Linux) or `ps`/`lsof` (Mac); elsewhere nothing | Linux's | its limits aren't shown on Windows until there's a way |
| `usage/usage.ts:631-640,713-724`, `usage/*-history.ts` | agents' logs under `~/.claude`, `~/.codex`, `~/.config/claude`; SQLite opened as `"file:" + path + "?immutable=1"` | same | the same folders exist under the profile on Windows **(trial)**; a `file:` URI needs `/C:/…` with the path percent-encoded |

## 3. The WSL path

**What it is:** a Linux box in a WSL2 distro, in that distro's `~/.kuvo-code`. Same pack, same
engine (`cli-linux-x64` or `-arm64`), same code. WSL1 is refused with the reason (no Linux kernel:
no sandbox, slow files); the box can tell from `/proc/version` and `WSL_DISTRO_NAME`.

### 3.1 Install

- **From a Windows desktop app:** it runs the box's installer inside the distro, as the Mac app
  runs it from its bundle (713 §4.1): `wsl.exe -d <distro> --exec sh` with the bootstrap on
  standard input. The app only picks the distro (a list from `wsl.exe -l -q`) and draws the
  installer's lines.
- **From another computer (a Mac adding it as a machine):** over SSH, as any Linux machine, once
  SSH lands in the distro (§3.2).
- **By hand:** the same one-line bootstrap pasted into the distro's terminal. Needed anyway for
  machines; this is the first place it's the main way in.
- **If WSL isn't installed,** the app says so and offers the native box or `wsl --install` (which
  needs an administrator and a restart: the user's to run, never ours).
- Nothing is installed in the distro beyond `~/.kuvo-code`. `git` must be there (it is on Ubuntu's
  image; otherwise the existing "needs a working git" refusal, reworded for Linux).

### 3.2 Reaching it

- **From the Windows desktop app on the same PC:**
  - `box route`: `wsl.exe -d <distro> --exec …` in place of `ssh <host> …`. One more transport
    beside `SshBox` in `gpui/kuvo_box` (the stand-in in tests is the same shape).
  - The kept server (`run/serve.json`, 127.0.0.1): WSL2 forwards `localhost` to Windows by default,
    and in "mirrored" networking shares it outright, so the app connects to `127.0.0.1:<port>`
    directly **(trial: both modes, and with a VPN on)**. It reads `serve.json` through
    `\\wsl.localhost\<distro>\home\<user>\.kuvo-code\run\serve.json` or with `box route`.
  - It attaches as Kuvo Code on the box's own computer (`x-kuvo-from: mac`, PROTOCOL.md's
    "The app attached…"), as a Linux desktop app would (713 §2). The header's name is historical.
- **From the user's other boxes:** peers through the relay, end to end. The box dials out; nothing
  about WSL matters. This is the way that needs no setup at all.
- **From a Mac or phone as a machine over SSH:** three ways, all **(trial)**:
  1. Windows' OpenSSH server with the account's default shell set to WSL (`DefaultShell` in the
     registry, an administrator's one line): then our `exec sh -c '…'` lands in the distro as is.
     It changes what every SSH login to that PC gets, so it's the user's choice, documented, never
     done by us.
  2. Windows' OpenSSH server as it is, and the client wraps each command: `wsl.exe -d <distro>
     --exec sh -c …`. Quoting through `cmd.exe` is the risk; the script already travels as base64,
     which helps.
  3. An SSH server inside the distro on its own port, with the port opened on Windows (mirrored
     networking, or a `netsh` proxy). Most setup; cleanest once done.
  Recommended: (2) as what we support, since it needs nothing changed on the PC but the SSH server
  being on; (1) noted in the docs as also working.
- **Sign-in callbacks and the engine's tunnel** (`-L <port>:127.0.0.1:<port>`): the same forwards,
  ending in Windows; they reach the distro through the same `localhost` forwarding **(trial)**.

### 3.3 Starting at sign-in, and staying up

Three separate problems:

1. **WSL isn't running until something asks for it.** A crontab `@reboot` line inside it never
   fires at Windows' boot. Something on the Windows side has to start it.
2. **Inside, `@reboot` needs cron running.** With systemd on (the default for Ubuntu installed
   from the Store since late 2022; `systemd=true` in `/etc/wsl.conf`) cron runs if installed.
   Without systemd, nothing starts services. So the box can't count on the `@reboot` line in WSL
   **(trial: a fresh Ubuntu, and one with systemd off)**.
3. **WSL stops the distro when no Windows-started session is left,** and background processes
   alone don't always hold it **(trial: with and without systemd, current WSL)**.

**Design:** one per-user start on the Windows side, which also holds the distro open:
`wsl.exe -d <distro> --exec sh ~/.kuvo-code/bin/kuvo-start keep-here` (the keeper in the
foreground of that session: a small new subcommand, or the existing `keep` not backgrounded).
Registered as the native box's start is (§4.2: a value under `HKCU\…\Run`, or a task at sign-in),
with a hidden window.

- **Who registers it: the box,** from inside WSL, through WSL's interop (`reg.exe`/`schtasks.exe`
  on `/mnt/c/Windows/System32`), so a client stays out of it and a box installed by hand or over
  SSH gets it too. If interop is off (`/etc/wsl.conf`), the box says: "Windows won't start this
  box when you sign in. Turn on interop, or start it yourself."
- **Footprint:** this is one entry outside `~/.kuvo-code`, on the Windows side: the WSL box's
  `@reboot` line. It's tagged, listed in `docs/footprint.md` and `uninstall.md`, and removed by
  `uninstall` (through interop) — and it must be removable when the distro is gone (documented by
  name).
- **Before sign-in** (a PC used as a server, nobody at the screen): only a task set to run whether
  or not the user is signed in, which needs an administrator once. Said plainly as a limit; the
  docs give the one command. Same for the native box.
- **Sleep:** Windows sleeping stops everything. `keepsAwake` is Mac-only today
  (`box/src/link/work/awake.ts:23`); a WSL box says it can't keep the PC awake.

### 3.4 Files across `\\wsl.localhost` and `/mnt/c`

- **The box's home and, by default, projects live in the Linux filesystem.** Fast, modes and
  links real, watching works. The Windows app shows and opens them as `\\wsl.localhost\<distro>\…`
  (`\\wsl$` is the old name for the same thing); paths on the wire stay the box's POSIX paths, and
  only "open in Explorer / in an editor" in the app converts, which is drawing.
- **A project on `/mnt/c`** (the user's existing Windows checkout) works but differently, and the
  box should state it as a fact on the project (`kuvo/projects`: `slowFiles: "windows-drive"` or
  similar), drawn as one quiet line:
  - file access is several times slower (git status, installs);
  - changes made from Windows aren't seen by watchers (Changes and artifacts fall to their timed
    reads);
  - modes aren't real, so the sandbox's read-only promises and anything `0600` there aren't
    either: **the sandbox should refuse Also Allowed folders and projects on a Windows drive, or
    say what it can't keep (trial, then a decision: D6)**;
  - names fold case; line endings are whatever Windows' git chose.
- **Windows programs from inside** (`explorer.exe`, `code`, `cmd.exe /c`): interop lets a command
  in WSL start Windows programs, **which run outside our sandbox entirely**. Landlock stops the
  exec only if `/mnt/c` and `/init`'s interop are out of reach of sandboxed commands **(trial:
  can a sandboxed command start `/mnt/c/Windows/System32/cmd.exe`?)**. If it can, the sandbox on
  WSL must keep `/mnt/*/Windows` and the `WSLInterop` binfmt entry from commands, or it isn't a
  sandbox. This is the one WSL finding that could block "equal to Linux".

### 3.5 What the box says about itself

- **Facts, in `kuvo/info` (or `computers`):** `system: "linux"`, `wsl: { distro: "Ubuntu", version: 2 }`,
  and Windows' name and version (read once through interop, optional). Clients draw
  "Ubuntu 24.04 on Windows" and can tell two boxes on one PC apart.
- **Names:** the distro's host name is the PC's. The box's suggested name becomes
  "<PC> (Ubuntu)" where a WSL fact is set, in `box/src/link/peers/computer-name.ts` (the box
  decides; a rename by the user wins as now).
- **Ports:** `localhost` is shared with Windows, so a port the box means for itself is reachable
  by Windows programs of the same user, and the other way round. All are random and
  password-protected already; `kuvoPorts()` in the Mac's sandbox policy has no WSL counterpart to
  worry about.

### 3.6 The agents, the browser and the sandbox inside WSL

- **Agents:** all five are Linux programs there, installed and signed in as on any Linux machine
  (§5). One trap: **WSL puts Windows' PATH after Linux's**, so `claude`, `node`, `npm`, `git` or
  `code` installed on Windows can be found in `/mnt/c/…` when the Linux one is missing, and then
  fail in odd ways. Wherever the box looks for a program (`loginPath`, the agents' finders,
  `kuvo-npx`), it skips folders on a Windows drive when `wsl` is set, and "not installed" stays
  true and clear.
- **Sign-in:** the bridge already catches the URL an agent wants opened
  (`box/src/agents/signin.ts:62`) and the client opens it; the callback comes back over the
  forward. No Windows browser is started from inside.
- **The browser:** the Linux runtime with its bundled libraries, headless; no WSLg needed
  **(trial)**. Chrome's sandbox needs user namespaces: fine on WSL2.
- **The sandbox:** Landlock and a namespace, as Linux, subject to the kernel check (§2.7) and the
  interop finding (§3.4). Memory: WSL caps the VM at half the PC's memory by default; a busy box
  with Chrome and several agents may need `.wslconfig` raised, which is the user's file: a
  troubleshooting note, not ours to write.

## 4. The native path

**The minimum:** the engine and the box's three JavaScript files running under
`%USERPROFILE%\.kuvo-code`, kept running from sign-in, reachable by the relay and by a Windows
desktop app on the same PC, running conversations with the engine's models and at least Claude
Code and Codex, updating itself from the signed release. No sandbox, said plainly.

### 4.1 What "minimum" has to replace

In dependency order (sizes in §6):

1. **A platform layer with three systems** (§1): `system(): "mac" | "linux" | "windows"`, and
   every `darwin ? … : …` made a three-way choice that **fails closed** on Windows ("not on
   Windows yet") instead of running Linux's code.
2. **The keeper, launch, wake-ups, `kuvo-npx` and the update's restore as the box's code** (D2).
3. **Processes:** start hidden, stop gracefully through a channel, kill a whole tree (Job Object),
   and "is it still ours" without command lines (§2.3).
4. **Private files and the box's sockets** (§2.5, §2.6).
5. **Paths:** home, the wire rule, case, `/dev/null`, names (§2.9).
6. **Install, update, uninstall, start at sign-in** (§2.8, §4.2).
7. **The shell for commands** (D3) and PATH.
8. **The browser** (§2.11) and **each agent** (§5).

### 4.2 A service or a sign-in task

| | A. Windows service | B. Scheduled task at sign-in | C. `HKCU\…\Run` value |
| --- | --- | --- | --- |
| Needs an administrator to install | yes | sometimes (a sign-in trigger from the command line asks for elevation on some setups) **(trial)** | no |
| Runs before anyone signs in | yes | only with "run whether signed in or not" (administrator, once) | no |
| Runs as the user, with their profile, PATH, credentials and DPAPI | only if given the user's password | yes | yes |
| Restarts it if it dies | yes | can | no (the keeper restarts the rest; nothing restarts the keeper) |
| Footprint outside `.kuvo-code` | a service entry | one task | one registry value |
| Like today's | — | the Mac's LaunchAgent | the `@reboot` line |

**Recommend C first, B as the documented "start before sign-in" option, never A.** A service is
the wrong shape: it runs as another identity (agents' sign-ins, the user's SSH keys, their PATH
and git config all belong to the user), and it needs an administrator, which the footprint rule
forbids ("no `sudo`"). C is one tagged value, removed by uninstall, the exact analogue of the
`@reboot` line. `docs/footprint.md` gains it as a listed exception.

**The window problem.** A console program started from `Run` opens a console window, and so does
every child it spawns without `windowsHide`. Options for the first process: `conhost.exe
--headless <engine> …` (ships with Windows 10 1809 and later) **(trial)**, a `.vbs` or PowerShell
shim (flashes, and looks like malware to some scanners), or a tiny native starter of ours
(`kuvo-start.exe`, a few dozen lines of Rust built without a console, signed). The starter is the
Windows cousin of Kuvo Helper: it carries no logic, only "start the box hidden, as me". D4.

### 4.3 Paths

`%USERPROFILE%\.kuvo-code`, the same name and layout inside (D1). Reasons: one name in docs,
support, uninstall and the client's `place.rs` (`home.join(".kuvo-code")` is already right on
Windows); `%USERPROFILE%` is private to the user by default; `%APPDATA%` roams on domain PCs,
which would copy keys and gigabytes of browser to a server. The box checks once that `K` isn't
readable by other accounts and refuses to keep keys in one that is.

### 4.4 The sandbox

**Nothing equivalent exists that we can use as we use Seatbelt and Landlock** (unprivileged, per
command, by path, set up by the program itself):

- **AppContainer** is the nearest: deny by default for files, registry and network. But a command
  inside can only reach folders whose ACL names the container, so allowing a project means editing
  the user's folders' ACLs (and undoing it), many developer tools don't run inside one, and it's
  set up with Win32 calls from a native program.
- **A restricted token at low integrity** stops writes outside labelled folders but not reads: it
  can't hide secrets, which is half of what our sandbox promises.
- **Job Objects** limit processes and resources, not files.
- **Windows Sandbox / Hyper-V** are a whole second Windows: Pro editions only, heavy, not per
  command.
- **Codex's own Windows sandbox** (restricted tokens and ACLs, with an elevated setup step for
  its stronger mode) shows it's possible and how much it is.
- **WSL** is the sandbox that already works on that PC: which is the WSL box.

**So the first native box says:** `kuvo/sandbox` → `able: false, reason: "windows"`, "Windows
can't sandbox commands yet. On this PC, a box in WSL can." Everything that follows is the existing
no-sandbox behaviour (`box/src/link/work/approval.ts:105-129`): Auto asks first, bots allow no
command, the vault stays shut, Full Access is the user's own confirmation. **That makes a native
box much less useful for bots and Auto than a Linux or WSL box, and the app must say so where the
user chooses between the two** (a design to draft: D5).

A real sandbox (AppContainer through a native helper) is piece N9: its own design note, a trial on
the machine first, a separate security review.

### 4.5 Update

The same signed pack and manifest, with `windows` engine builds beside `darwin` and `linux`. The
update's steps are already code except three: `tar` (use `tar.exe`), `sh -n` on the keeper (gone
with D2), and `restore.sh` (a plan as data, D2). Windows-only rules: a running `.exe` can't be
replaced (engines are per version, so only cleanup waits); everything is stopped before the swap,
which the update already does. If there's a native starter (D4), it's a file in the pack that's
replaced while it's running by rename-aside-then-delete-later **(trial)**.

### 4.6 Reaching a native box

- **A Windows desktop app on the same PC:** `run/serve.json` and 127.0.0.1, exactly as on a Mac
  (`gpui/kuvo_box/src/place.rs`, `local.rs`); `place.rs:161` (no command line on Windows) needs the
  start-time check instead (§2.3).
- **Other boxes:** the relay. Nothing to do.
- **As a machine over SSH** (Windows' OpenSSH server; `cmd.exe` is its default shell): the box
  provides a fixed entry (`.kuvo-code\bin\kuvo-box.cmd route`) so the client sends one plain
  command with no quoting to get wrong; requests and answers on standard input and output as now
  **(trial: binary-clean through `cmd`)**. Administrators' keys live in
  `C:\ProgramData\ssh\administrators_authorized_keys`, not the profile: a docs line.

## 5. Each coding agent

What our code does is from reading it. **What each maker ships for Windows is from memory of
their public docs, not checked, and every cell in the last two columns is (trial).** In WSL each
is its Linux self.

| Agent | How we find, install and run it today | Sign-in kept | In WSL | Native: what's known and what's needed |
| --- | --- | --- | --- | --- |
| **Claude Code** | `claude` in `~/.local/bin`, `~/.claude/local`, Homebrew's folders (`agents/claude.ts:39,237`); install `curl -fsSL https://claude.ai/install.sh \| bash` (`:184`); run by its SDK over stdio, through the sandbox launcher (`:1481,1547`) | on a Mac, Kuvo's own token file and the Keychain item's date via `/usr/bin/security` (`:175,382,2621`); elsewhere `<config>/.credentials.json` (`:379`) | works as on Linux; Anthropic documents WSL | Anthropic ships a native Windows build that **requires Git for Windows** (which is why D3 leans that way), with a PowerShell installer. Needs: `claude.exe` and its Windows folders in `places`; the install line; `ownsPath`'s `/tmp/claude-<uid>` (`:619`) on Windows; credentials are a file as on Linux |
| **Codex** | `codex` in Homebrew's folders, `~/.local/bin`, `~/.codex/bin` (`agents/codex.ts:35,276`); install `curl … chatgpt.com/codex/install.sh \| sh` (`:127`); `codex app-server` over stdio (`:458`) | `<CODEX_HOME or ~/.codex>/auth.json` (`:168,794`); extra accounts through a home of symlinks (`:2588-2610`) | works as on Linux; OpenAI has recommended WSL on Windows | a native Windows build exists (npm `@openai/codex`), with a Windows sandbox of its own. Needs: the npm `.cmd` problem (§2.12); `unwrapShell`'s `/bin/(zsh\|bash\|sh) -lc` (`:2614`) learning what Codex wraps commands in on Windows (PowerShell); `shadowHome` without symlinks |
| **Cursor** | no program: `@cursor/sdk@1.0.35` installed by our Bun into the bridge's folder (`agents/cursor.ts:42,113-128`); in the bridge's process, or a worker under the sandbox launcher (`:683-694`) | `<bridge folder>/auth.json` or `CURSOR_API_KEY` (`:101,131`) | works as on Linux if the SDK's Linux build does | whether the SDK has a Windows build and runs under Bun there. If it does, this is the least work of the five (no finder, no installer) |
| **Grok** | `grok` in `~/.grok/bin`, `~/.local/bin` (`agents/grok.ts:22,105`); install `curl … x.ai/cli/install.sh \| bash` (`:52`); `grok agent stdio`, ACP (`:300-309`); `grok login` in a pseudo-terminal | `<GROK_HOME or ~/.grok>/auth.json` (`:148-153,199`) | works as on Linux | **unknown whether xAI ships a Windows build.** If not: "Grok isn't made for Windows. A box in WSL can run it.", from the box, as Antigravity says today for an unknown system |
| **Antigravity** | a zip per system from a pinned map with keys `darwin-arm64`, `darwin-x64`, `linux-x64`, `linux-arm64` only (`agents/antigravity.ts:32-53,425`); unpacked by `unzip` or `python3` (`:799-807`); `chmod 0o755`; `agy_acp_server.par` run directly (`:534`) | `profile/antigravity-acp/acp_token.json` (`:397`) | works as on Linux; needs `unzip` or `python3` in the distro (use the box's own `zip.ts` instead: also right for Linux) | **no Windows key**: today's text, "Google doesn't make Antigravity's agent for this computer.", is what a native box says until Google's Windows agent is found, pinned and tried. A `.par` can't be run directly on Windows |

**Order for N7:** Claude Code, Codex, Cursor, then Grok and Antigravity only if their makers ship
for Windows. **Equal boxes:** an agent its maker doesn't ship for Windows is listed as not
available there, with the reason and "a box in WSL can run it", never hidden.

**The agents' installers break the footprint rule on purpose** (`docs/footprint.md`, the user,
2026-10-23). On Windows the same exception applies, to the makers' Windows installers; it needs
the same line there, and the folders they write (`%USERPROFILE%\.local\bin`, `%APPDATA%\npm`, …)
named in `uninstall.md` once tried.

## 6. Pieces, in order

Sizes are session-hours for building with tests and a review; "machine" is what needs the Windows
PC. **G** pieces serve both paths, **W** is WSL, **N** is native.

| # | Piece | Size | Needs the machine for | Buildable and testable before it exists |
| --- | --- | --- | --- | --- |
| G0 | **Trials on the machine** (§7): a scripted checklist, results written up as `699-piece0-results.md` | 5–7 | all of it | the checklist script itself |
| G1 | **Three-system platform layer**: `system()`, the `wsl` fact, every two-way platform choice made three-way and failing closed, `sandbox` reason `windows`, system name, computer id, account and home on Windows | 7–9 | nothing | all, with injected `platform`, `env` and `read`, as the existing tests do |
| G2 | **Paths**: the wire rule in PROTOCOL.md with vectors, conversion at the routes, `foldsCase`, `os.devNull`, `path.delimiter`, legal names, `O_NOFOLLOW ?? 0` | 8–10 | a last check | all, under `path.win32`; the clients' "is absolute" in `KuvoClient` and `gpui/kuvo_box/paths.rs` with shared vectors |
| W1 | **A WSL box knows it's one**: refuses WSL1, facts and naming (§3.5), skips Windows' PATH when finding programs, the `/mnt/c` fact on projects | 4–5 | checking the detection | all, with stand-in `/proc/version` and PATH |
| W2 | **Starting and holding it** (§3.3): `keep-here`, the Windows-side entry through interop, its removal, the words when interop is off | 5–6 | 3 of them | the box side, with stand-in `reg.exe` |
| W3 | **Reaching it**: the `wsl.exe` transport in `gpui/kuvo_box`; the SSH wrap for a Mac adding it; install by bootstrap | 6–8 | 4 of them | the transport against a stand-in `wsl`; the wrap's quoting as vectors |
| W4 | **Sandbox and interop on WSL** (§3.4): keep Windows' programs and drives from sandboxed commands; D6 | 4–6 | most | the policy change, on Linux |
| W5 | **Docs and footprint**: machines, install, uninstall, privacy, troubleshooting in kuvo-web; `docs/footprint.md`; CHANGELOG | 3–4 | screenshots | all |
| | **WSL box ready** | **≈ 42–55 with G0–G2; 22–29 of it W** | | |
| N1 | **The keeper as the box's code** (D2, with #715): `keep`, launch, start, stop, the lock, back-off; wake-ups; `kuvo-npx`; the update's restore as a plan. On Unix a three-line `kuvo-start` remains that execs it | 18–24 | 2–3, for stops and restarts | nearly all, on Mac and Linux, against the keeper's existing scenario tests (`KuvoStartServeTests`, `RebootTests`, `WakesTests` are the spec) |
| N2 | **Processes on Windows**: one `spawn` of our own (hidden, tree-kill by Job Object), the stop channel, start-time instead of command lines | 10–12 | 4–5: Job Objects and windows can't be faked | the callers' move to the one `spawn`; the stop channel; start-time records |
| N3 | **Private files and sockets**: the ACL check, pipes named from `K` with the socket key | 7–9 | 3 | the naming and the single definition; pipe behaviour only there |
| N4 | **Install, update, uninstall, start at sign-in**: bootstrap for PowerShell, `windows` in the manifest and `box.json`, `opencode.exe`, the `Run` value, removal | 10–13 | 5 | manifest, selection and the plan; stand-in `reg.exe` |
| N5 | **The shell and PATH** (D3): finding Git for Windows, the "install Git" refusal, commands through it, the reviewer's Windows list | 7–9 | 3 | finding and refusing; the reviewer's rules as vectors |
| N6 | **The browser**: `windows.json`, install without links, `chrome.exe`, the profile lock, pipes | 8–10 | 5 | the runtime choice and install against the stand-in source |
| N7 | **Agents**, one at a time (§5) | 3–5 each, 15–22 | all, with an account for each | each finder with injected platform |
| N8 | **Reaching it over SSH; the desktop app attaching in place; docs and footprint** | 7–9 | 4 | the fixed entry; docs |
| N9 | **A native sandbox** (AppContainer through a helper): design, trial, build, separate review | 30–50 | most | the design |
| | **Native box ready without N9** | **≈ 95–125 with G0–G2** | | |

**Order:** G1 and G2 now (no machine). G0 the day the machine arrives. Then W1–W5 (a full box on
Windows soonest), with N1 alongside since it needs the machine least and #715 is moving the same
code. Then N2–N8. N9 on its own decision.

**What can't be tested without the machine**, however carefully simulated: process trees and
windows, pipes and ACLs, replacing files in use, Chrome, each agent's real program, WSL's start
and stop, SSH into Windows. A `path.win32` test proves a path rule; it proves nothing about what
Windows does.

**A test lane for it:** `scripts/test-windows.sh <ssh-host> [--wsl]`, the cousin of
`scripts/test-linux.sh`: this checkout's code to a folder of its own (`.kuvo-code-test`), run on
the machine's engine as Bun, removed afterwards, and a list of what appeared in the profile and
`%TEMP%`. Part of G0. (GitHub's Windows runners could run the box's unit tests too, once CI is on
again: the lead's call.)

## 7. Trials for the day the machine arrives (G0)

WSL:

1. `cat /sys/kernel/security/lsm` on the stock WSL2 kernel: is `landlock` there? `unshare -Urm true`?
2. The whole box test lane and the bridge's sandbox tests in the distro.
3. Can a sandboxed command start `/mnt/c/Windows/System32/cmd.exe` (interop)? With `/mnt/c` kept out?
4. Does the distro stay up with only the keeper running, systemd on and off? After the terminal closes? After sleep?
5. `@reboot` with systemd on, off; cron present or not on a fresh Ubuntu.
6. `reg.exe`/`schtasks.exe` from inside; the entry starting the keeper at sign-in with no window.
7. The kept server reached from Windows on 127.0.0.1: NAT and mirrored networking; with a VPN.
8. SSH from the Mac: Windows' server wrapping `wsl.exe --exec sh -c` (quoting, binary stdin for the engine's tarball), and `DefaultShell` set to WSL.
9. The browser: install and a page, without WSLg; on ARM if the PC is.
10. Each agent installed and signed in; Windows' `node`/`claude` on PATH as the trap.
11. A project on `/mnt/c`: git speed, watching, modes, the #429 git files.
12. The clock after sleep; a wake-up due during it.

Native:

13. `opencode.exe --version`; `BUN_BE_BUN=1 opencode.exe kuvo-link.js selftest`; the name inside the npm package; `--config=` with `NUL`; whether it honours `XDG_*`; which shell it runs commands with and whether ours can be set.
14. A console window: from `Run`, with `conhost --headless`, with and without `windowsHide` on children.
15. `bun:ffi` to `kernel32` from the engine as Bun: a Job Object that kills a tree; a process's start time. Bun's pseudo-terminal (`Bun.spawn`'s `terminal`); `kill(pid, 0)` on a pid that's gone; spawning an npm `.cmd`.
16. `net.listen` on a pipe: who can open it; the socket key in front.
17. Extra stdio pipes to `chrome.exe`; the profile lock's form; what Chrome leaves outside the profile folder.
18. Replacing `kuvo-link.js` while read; removing an old engine folder; renaming `K` with nothing running.
19. `HKCU\…\Run` and a sign-in task without elevation; the box up after sign-in, and after a restart with nobody signed in (task with stored credentials).
20. Windows' SSH server: a `.cmd` entry, binary-clean stdin/stdout, a forward to 127.0.0.1.
21. Git for Windows: `bash.exe -c`, `/dev/null` in `diff --no-index` and `hooksPath`, `core.symlinks` without the privilege, CRLF in Changes, which `ssh` git uses and its agent.
22. Each agent's native program: found, signed in, a turn run, stopped mid-turn (does its tree die?).
23. Long paths: a worktree under `K\data\opencode\worktree\…` past 260 characters with git, node and a compiler.
24. Defender: does it slow or quarantine the engine, the unsigned starter, Chrome for Testing?
25. ARM64 Windows, if the PC is one: the arm64 engine; Chrome x64 under emulation.

## 8. Decisions for the user

| # | Decision | Recommended | Why |
| --- | --- | --- | --- |
| D1 | Where a native box lives | `%USERPROFILE%\.kuvo-code` | one name everywhere; private by default; doesn't roam |
| D2 | The keeper, launch, wake-ups and restore: stay shell (moved into `box/` as files by #715) or become the box's code | **the box's code, on every system**, #715 doing the move once | a native box can't run shell; two keepers (shell and PowerShell) would be the same logic twice; the shell held in strings is already the hardest part of the box to test. Cost: the most proven code in the box is rewritten; its scenario tests are the safety net. **Needs settling before #715 goes further** |
| D3 | The shell for commands on a native box | require **Git for Windows** (the user installs it; the box finds it and says "Install Git for Windows" when it's missing) | git is needed anyway; Claude Code requires the same; POSIX commands keep the reviewer and every prompt working. Alternative: bring a portable Git inside `.kuvo-code` (about 300 MB, no system install, ours to update) |
| D4 | The first process on a native box | try `conhost --headless` in G0; a tiny signed starter if it isn't clean | a starter means a Windows build, a code-signing certificate and SmartScreen reputation: a cost, and a brand surface (its name and icon in Task Manager and Startup apps: **a brand decision**) |
| D5 | How the app presents the two boxes on a Windows PC | drafts first (two or three, light and dark): "WSL (recommended: sandboxed)" beside "Windows (no sandbox yet)" | it's a new visible choice; the box states the facts, the app draws them |
| D6 | Projects on a Windows drive in a WSL box | allow, with the fact shown; the sandbox refuses to promise what it can't there | people's existing checkouts are on `C:` |
| D7 | Is a native box without a sandbox acceptable for the release, with N9 after? | yes, said plainly, if the WSL box ships beside it | "both as options" is met; a native sandbox is the largest and riskiest piece |
| D8 | Start before sign-in (a task with stored credentials, an administrator once) | documented as the user's own step, never done by us | the footprint rule: no elevation |

## 9. What I need from the user (the machine)

A short list to send:

- **A Windows 11 PC or VM** (23H2 or later, Pro if possible), x64; 16 GB of memory and 80 GB free.
  An ARM one as well only if we mean to support ARM at release.
- **Reachable over SSH like lokum:** Windows' OpenSSH Server on, my Mac's key in the account's
  `authorized_keys` (for an administrator account: `C:\ProgramData\ssh\administrators_authorized_keys`),
  reachable on the home network and by Tailscale.
- **Two accounts:** a standard (non-administrator) one for tests, and an administrator one I use
  only for setup steps I'll list first.
- **WSL2 installed with Ubuntu 24.04**, left as it installs (systemd as it comes). Virtualization
  on (in a VM: nested virtualization). I'll make a second distro myself for the "no systemd" case.
- **Git for Windows installed** for the test account (and nothing else: no Node, no agents; I
  install those inside test folders or say when one needs the real profile).
- **It can sleep, restart and be signed out by me** (remote restart allowed), and stays on
  otherwise.
- **Sign-ins for the agents on it when we reach N7 / W-trial 10:** Claude Code and Codex first;
  Cursor, Grok and Antigravity after. You sign in there once; I never see the passwords.
- **Whether Defender stays on** (it should: it's what users have).
- Later, only if D4 picks a starter: **a Windows code-signing certificate**.
