// A Mac that ran an earlier Kuvo Code, for the tests of the move the box runs itself (`rehome
// --run`, #713 piece 4): a home folder of the test's own, the app's folder in it laid out as that
// version's app laid it out (its engine's database as the real engine 2.0.32 made it, with its log
// still beside it), the app that carries the release (with a stand-in `kuvo-keys`), the release
// and the engine its installer staged (a stand-in Kuvo Helper; the engine a stand-in too, or the
// real pinned one where `KUVO_TEST_ENGINE` names it), and a Keychain that's two folders.
import { spawnSync } from "node:child_process"
import { createHash } from "node:crypto"
import { chmodSync, copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, rmSync, writeFileSync } from "node:fs"
import { Database } from "bun:sqlite"
import { join } from "node:path"
import { pathToFileURL } from "node:url"
import { RELEASE_FILES } from "../src/link/add/files"

export const MAIN = join(import.meta.dir, "..", "src", "link", "main.ts")
export const APP_ID = "run.kuvo.code.trial713"
export const PINNED = "2.2.6"
/** The real pinned engine, where a run names it (never fetched by a test). */
export const REAL_ENGINE = process.env.KUVO_TEST_ENGINE && existsSync(process.env.KUVO_TEST_ENGINE) ? process.env.KUVO_TEST_ENGINE : null
export const clean = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("KUVO_"))) as Record<string, string>

export const put = (file: string, text: string | Buffer, mode?: number) => {
  mkdirSync(join(file, ".."), { recursive: true })
  writeFileSync(file, text)
  if (mode !== undefined) chmodSync(file, mode)
}
export const sha = (kind: "sha1" | "sha256", text: string | Buffer) => createHash(kind).update(text).digest("hex")
export const readJSON = (file: string) => JSON.parse(readFileSync(file, "utf8"))

export const git = (cwd: string, args: string[]) => {
  const ran = spawnSync("git", args, {
    cwd, encoding: "utf8",
    env: { ...clean, HOME: cwd, GIT_CONFIG_NOSYSTEM: "1", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t", GIT_AUTHOR_DATE: "2026-01-01T00:00:00Z", GIT_COMMITTER_DATE: "2026-01-01T00:00:00Z" },
  })
  if (ran.status !== 0) throw new Error(`git ${args.join(" ")}: ${ran.stderr}`)
  return ran.stdout.trim()
}

export interface Mac {
  home: string
  /** The earlier Kuvo Code's folder, and the box's home. */
  S: string
  K: string
  bundle: string
  release: string
  /** The folder the installer staged its engine in. */
  engine: string
  repository: string
  worktree: string
  rootCommit: string
  chat: string
  /** The Keychain, as two folders: the app's own items, and the helper's. */
  keychain: string
}

/** A recorded database (`fixtures/rehome/<name>.sql`) laid out where this world's folders are. */
export function recorded(name: string, { S, home, rootCommit }: { S: string; home: string; rootCommit: string }): string {
  return readFileSync(join(import.meta.dir, "fixtures", "rehome", `${name}.sql`), "utf8")
    .replaceAll("@S_URL@", pathToFileURL(S).href.slice("file://".length)).replaceAll("@S@", S).replaceAll("@HOME@", home)
    .replaceAll("@ROOT6@", rootCommit.slice(0, 6)).replaceAll("@ROOT@", rootCommit)
    .replace(/@ID:([^@]+)@/g, (_, folder: string) => sha("sha1", `directory:${folder}`))
}

/** The engine's database as an engine that was stopped where it stood leaves it: its last writes
 * still in the log beside it (`-wal`), with the log's index (`-shm`). Made elsewhere and copied
 * while it's open, since closing it would fold the log in. */
export function databaseWithLog(file: string, sql: string, scratch: string) {
  rmSync(scratch, { recursive: true, force: true })
  mkdirSync(scratch, { recursive: true })
  const made = join(scratch, "opencode.db")
  const db = new Database(made)
  db.exec("PRAGMA journal_mode = WAL; PRAGMA wal_autocheckpoint = 0;")
  db.exec(sql)
  mkdirSync(join(file, ".."), { recursive: true })
  for (const suffix of ["", "-wal", "-shm"]) copyFileSync(made + suffix, file + suffix)
  db.close()
  rmSync(scratch, { recursive: true, force: true })
}

const KEYS_STAND_IN = `#!/bin/sh
# kuvo-keys, as a stand-in: the Keychain is two folders in the test's home. Like the real one, it
# does nothing unless the move's lock names the very process that started it.
verb=$1; shift
H=""; B=""
while [ $# -gt 0 ]; do case "$1" in --home) H=$2; shift 2 ;; --helper) B=$2; shift 2 ;; *) shift ;; esac; done
KC="$HOME/keychain"
holder=$(cat "$H/updates/rehome/lock/pid" 2>/dev/null)
echo "$verb \${B##*/}" >>"$HOME/keys-asked"
[ -n "$holder" ] && [ "$holder" = "$PPID" ] || { echo "kuvo-keys works only for this Mac's box's move, run by it while it holds its lock." >&2; echo "refused $verb" >>"$HOME/keys-asked"; exit 2; }
if [ -f "$HOME/keychain-locked" ]; then echo '{"error":"The Keychain is locked.","reason":"locked"}'; exit 1; fi
case "$verb" in
copy)
    # (Unlocked, macOS keeps an item from it: it wasn't allowed, or it said something else.)
    if [ -f "$HOME/keychain-not-allowed" ]; then echo '{"error":"Your Keychain didn’t give out phone-link/key (OSStatus -25308).","reason":"not-allowed","status":-25308}'; exit 1; fi
    if [ -f "$HOME/keychain-refuses" ]; then echo '{"error":"Your Keychain didn’t give out run.kuvo.code.vault (OSStatus -25291).","reason":"refused","status":-25291}'; exit 1; fi
    [ -x "$B/Contents/MacOS/Kuvo Helper" ] || { echo "no helper" >&2; exit 2; }
    [ -z "$(ls "$KC/helper" 2>/dev/null)" ] || { echo '{"error":"Items an earlier box left.","reason":"leftovers"}'; exit 1; }
    [ ! -f "$H/updates/rehome/keychain.json" ] || { echo '{"error":"An earlier try left copies.","reason":"journal"}'; exit 1; }
    mkdir -p "$KC/helper" "$KC/app"; n=0
    for f in "$KC/app"/*; do [ -e "$f" ] || continue; cp "$f" "$KC/helper/"; n=$((n+1)); done
    echo '{"v":1,"state":"copied"}' >"$H/updates/rehome/keychain.json"
    echo "{\\"copied\\":$n}" ;;
delete-originals)
    rm -f "$KC/app"/*
    echo '{"v":1,"state":"settled"}' >"$H/updates/rehome/keychain.json"
    echo '{}' ;;
passwords)
    cat "$HOME/keychain-passwords.json" 2>/dev/null || echo '{"v":1,"logins":[]}' ;;
*) exit 2 ;;
esac
`

const HELPER_STAND_IN = `#!/bin/sh
# Kuvo Helper, as a stand-in: its short runs for the Keychain's move.
echo "$*" >>"$HOME/helper-asked"
[ "$1" = keys ] || exit 0
what=$2; shift 2
H=""
while [ $# -gt 0 ]; do case "$1" in --home) H=$2; shift 2 ;; *) shift ;; esac; done
KC="$HOME/keychain"
case "$what" in
check)
    if [ -f "$HOME/helper-cannot-read" ]; then echo '{"unreadable":["phone-link/key"]}'; exit 1; fi
    for f in "$KC/app"/*; do [ -e "$f" ] || continue; cmp -s "$f" "$KC/helper/\${f##*/}" || { echo '{"unreadable":["one"]}'; exit 1; }; done
    echo '{"unreadable":[]}' ;;
uncopy) rm -rf "$KC/helper"; rm -f "$H/updates/rehome/keychain.json"; echo '{}' ;;
clear) rm -rf "$KC/helper"; echo '{"kept":[]}' ;;
esac
`

const KEPT_ROWS = `
INSERT INTO credential VALUES('crd_1', 'anthropic', 'Work', '{"type":"api","key":"sk-test"}', NULL, NULL, 1, 1, 1);
INSERT INTO account VALUES('acc_1', 'you@example.com', 'https://example.com', 'at', 'rt', NULL, 1, 1);
INSERT INTO permission VALUES('psv_kept1', '@ROOT@', 'edit', '@S@/Chats/c1/*', 1, 1);
`
let worlds = 0
/** A Mac of Kuvo Code 0.7.2 (its app's folder with no box of its own), in `root`: sessions in a
 * repository of yours, in its engine worktree in the app's folder, in a chat there and in a
 * folder of yours outside git; files sent; what `given` adds. Nothing in its `~/.kuvo-code` but
 * what the installer staged. */
export function mac07(root: string, given: (mac: Mac) => void = () => {}, { at, engine = REAL_ENGINE }: { at?: string; engine?: string | null } = {}): Mac {
  const home = join(root, at ?? `m${String(worlds++).padStart(3, "0")}`)
  const S = join(home, "Library", "Application Support", APP_ID)
  const K = join(home, ".kuvo-code")
  mkdirSync(join(home, "tmp"), { recursive: true })
  const repository = join(home, "proj")
  mkdirSync(repository, { recursive: true })
  git(repository, ["init", "-q", "-b", "main"])
  git(repository, ["commit", "-q", "--allow-empty", "-m", "root"])
  put(join(repository, "a.txt"), "line one\nline two\n")
  git(repository, ["add", "a.txt"])
  git(repository, ["commit", "-q", "-m", "a"])
  const rootCommit = git(repository, ["rev-list", "--max-parents=0", "HEAD"])
  const data = join(S, "opencode", "data", "opencode")
  const worktree = join(data, "worktree", rootCommit.slice(0, 6), "feature one")
  mkdirSync(join(worktree, ".."), { recursive: true })
  git(repository, ["worktree", "add", "-q", "--detach", worktree])
  const chat = join(S, "Chats", "c1")
  const mac: Mac = { home, S, K, bundle: join(home, "Applications", "Kuvo Code.app"), release: join(K, "updates", "install", "release"), engine: join(K, "updates", "install", "engine"), repository, worktree, rootCommit, chat, keychain: join(home, "keychain") }
  // The engine's own: its database (2.0.32's, with its log), a snapshot of each worktree, its
  // settings, state and cache; and the engine 0.7.2 downloaded, which nothing runs again.
  // (With what the real turns never made: a provider's sign-in, an account, a rule kept for a project.)
  databaseWithLog(join(data, "opencode.db"), recorded("engine-2.0.32", { S, home, rootCommit }) + KEPT_ROWS.replaceAll("@ROOT@", rootCommit).replaceAll("@S@", S), join(home, "tmp", "make-db"))
  for (const folder of [repository, worktree]) {
    const snapshot = join(data, "snapshot", rootCommit, sha("sha1", folder))
    put(join(snapshot, "config"), `[core]\n\tbare = false\n\tworktree = ${folder}\n`)
    put(join(snapshot, "objects", "info", "alternates"), `${repository}/.git/objects\n`)
  }
  put(join(S, "opencode", "config", "opencode", "opencode.json"), JSON.stringify({ instructions: [`${S}/opencode/config/opencode/AGENTS.md`] }))
  put(join(S, "opencode", "config", "opencode", "plugins", "kuvo-code.js"), "// 0.7.2's own plugin, which it wrote at each start")
  put(join(S, "opencode", "state", "x"), "state")
  put(join(S, "opencode", "cache", "x"), "cache")
  put(join(S, "engine", "2.0.32", "opencode"), "the engine 0.7.2 ran", 0o755)
  // Yours: a chat, a folder outside git, files sent.
  put(join(chat, "notes.md"), "chat")
  put(join(home, "Projects", "plain", "p.txt"), "plain")
  put(join(S, "Attachments", "ses_a", "b.png"), "PNGDATA")
  put(join(S, "Attachments", "ses_a", "with space.png"), "PNGDATA")
  put(join(S, "integrations", "notion", "x"), "sign-in")
  // The app's own, which stays the app's.
  put(join(S, "Browser", "x"), "browser")
  // The app that carries the release, with `kuvo-keys`.
  put(join(mac.bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${APP_ID}</string><key>CFBundleExecutable</key><string>kuvo</string></dict></plist>\n`)
  put(join(mac.bundle, "Contents", "MacOS", "kuvo-keys"), KEYS_STAND_IN, 0o755)
  // This Mac's keys, in its Keychain under the app's names.
  put(join(mac.keychain, "app", "phone-link.key"), "the link's key")
  put(join(mac.keychain, "app", "integrations.KUVO_KEY_NOTION"), "a key")
  // The release its installer unpacked, every file it names as it says, and its helper.
  const files: Record<string, string> = {}
  for (const [name, mode] of RELEASE_FILES) {
    const text = name === "bin/kuvo-start" ? `#!/bin/sh\nV=${PINNED}\n` : `the release's ${name}\n`
    put(join(mac.release, name), text, mode)
    files[name] = sha("sha256", text)
  }
  put(join(mac.release, "bin", "Kuvo Helper.app", "Contents", "MacOS", "Kuvo Helper"), HELPER_STAND_IN, 0o755)
  put(join(mac.release, "box.json"), JSON.stringify({ version: "0.10.0", engine: PINNED, files }))
  // The engine its installer fetched: the real pinned one where a run names it, else a stand-in
  // that answers from the database it's started on, and brings 2.0.32's up to date as recorded.
  put(join(home, "world.json"), JSON.stringify({ S, home, rootCommit }))
  if (engine) {
    mkdirSync(join(mac.engine, PINNED), { recursive: true })
    spawnSync("/bin/cp", ["-c", engine, join(mac.engine, PINNED, "opencode")])
  } else {
    put(join(mac.engine, PINNED, "opencode"), `#!/bin/sh\nSTAND_IN_WORLD='${join(home, "world.json")}' exec '${process.execPath}' '${join(import.meta.dir, "fixtures", "rehome", "engine-stand-in.ts")}' "$@"\n`, 0o755)
  }
  given(mac)
  return mac
}

export interface Ran { status: number | null; signal: string | null; said: Record<string, any> | null; errors: string }

/** `kuvo-link.js rehome .kuvo-code …`, run as the installer runs it: in the box's home. */
export function rehome(mac: Mac, args: string[], env: Record<string, string> = {}): Ran {
  const binary = process.env.KUVO_TEST_REHOME_BIN
  const command = ["rehome", ".kuvo-code", ...args]
  const result = spawnSync(binary || process.execPath, binary ? command : [MAIN, ...command], {
    cwd: mac.home, encoding: "utf8", timeout: 120_000,
    env: { ...clean, HOME: mac.home, TMPDIR: join(mac.home, "tmp"), BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0", KUVO_HOME: mac.K, ...env },
  })
  const line = result.stdout.trim().split("\n").pop() ?? ""
  let said = null
  try { said = JSON.parse(line) } catch {}
  return { status: result.status, signal: result.signal, said, errors: result.stderr }
}
/** The whole move (`rehome --run`). */
export const run = (mac: Mac, env: Record<string, string> = {}) => rehome(mac, ["--run", "--app", mac.bundle, "--release", mac.release, "--engine", mac.engine], env)

/** Every file and folder in `folder`: its kind, mode and every byte it holds (a link's target). */
export function snapshot(folder: string, leave: (relative: string) => boolean = () => false): Record<string, string> {
  const seen: Record<string, string> = {}
  const walk = (at: string, relative: string) => {
    for (const name of readdirSync(at).sort()) {
      const file = join(at, name), here = relative ? `${relative}/${name}` : name
      if (leave(here)) continue
      const kind = lstatSync(file)
      const mode = (kind.mode & 0o777).toString(8)
      if (kind.isSymbolicLink()) seen[here] = `link ${readlinkSync(file)}`
      else if (kind.isDirectory()) { seen[here] = `folder ${mode}`; walk(file, here) }
      else if (kind.isFile()) seen[here] = `file ${mode} ${kind.size} ${sha("sha256", readFileSync(file))}`
      else seen[here] = "other"
    }
  }
  if (existsSync(folder)) walk(folder, "")
  return seen
}

/** Every row of every table of a database, as it reads. */
export function rows(file: string): Record<string, any[]> {
  const db = new Database(file, { readonly: true })
  try {
    const all: Record<string, any[]> = {}
    for (const { name } of db.query("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").all() as { name: string }[]) {
      all[name] = db.query(`SELECT * FROM "${name}" ORDER BY rowid`).all()
    }
    return all
  } finally {
    db.close()
  }
}

const xml = (text: string) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")
/** A value as a property list writes it: what the app kept as `Data` (`{ data: … }`, compact JSON) as `<data>`. */
function plistValue(value: unknown): string {
  if (typeof value === "string") return `<string>${xml(value)}</string>`
  if (typeof value === "boolean") return value ? "<true/>" : "<false/>"
  if (typeof value === "number") return Number.isInteger(value) ? `<integer>${value}</integer>` : `<real>${value}</real>`
  if (Array.isArray(value)) return `<array>${value.map(plistValue).join("")}</array>`
  const object = value as Record<string, unknown>
  if (Object.keys(object).length === 1 && "data" in object) return `<data>${Buffer.from(JSON.stringify(object.data)).toString("base64")}</data>`
  return `<dict>${Object.entries(object).map(([key, inner]) => `<key>${xml(key)}</key>${plistValue(inner)}`).join("")}</dict>`
}
/** The app's preferences, written as macOS keeps them (a binary property list). */
export function preferences(mac: Mac, values: Record<string, unknown>) {
  const file = join(mac.home, "Library", "Preferences", `${APP_ID}.plist`)
  put(file, `<?xml version="1.0" encoding="UTF-8"?>\n<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n<plist version="1.0">${plistValue(values)}</plist>\n`)
  const converted = spawnSync("/usr/bin/plutil", ["-convert", "binary1", file], { encoding: "utf8" })
  if (converted.status !== 0) throw new Error(converted.stderr)
}
