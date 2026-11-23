// Signing in by running an agent's own sign-in command (`claude auth login`, `codex login`,
// `grok login`): it opens its maker's page in your browser and finishes by itself. The page it
// would open is caught (its BROWSER is a little script that writes it down), so the app opens it,
// once, and can open it again. Some also print a page to use when no browser opens, which ends on
// a code to paste back (Claude Code's): that's taken too, typed into the command. The attempt says
// which it is (`mode`: `code` while the page in use is the printed one, else `auto`).

import { existsSync, lstatSync, mkdtempSync, readdirSync, readFileSync, rmSync } from "node:fs"
import { privateFolder, writeSecret } from "../shared/files"
import { join } from "node:path"
import type { SignInAttempt } from "./agent"
import { newID } from "./ids"
import { firstURL, plain } from "./proc"
import type { OAuthStatus } from "./wire"

export interface CommandSignInOptions {
  command: string
  args: string[]
  env?: Record<string, string | undefined>
  /** Where the bridge keeps its files (the URL catcher goes in `bin`). */
  home: string
  temporary: string
  /** Whether you're signed in now (asked once the command ends). */
  signedIn: () => Promise<boolean>
  /** What to do, in a line: for a sign-in that comes back by itself. */
  instructions: string
  /** The line for one that ends on a code to paste here (`mode: "code"`: its printed page, with a
   * `codePrompt`); `instructions` where it isn't given. */
  codeInstructions?: string
  /** The command's prompt for a code from its page, when it takes one: a pasted code is typed there. */
  codePrompt?: RegExp
  /** Signed in: what that changes (an ended sign-in is behind it). */
  onComplete?: () => void
  /** For a page opened on another device: the one it prints, at once (what it hands its browser
   * comes back to this computer, which another device can't reach). */
  printedPage?: boolean
  /** How long the attempt waits for its page before it's handed back without one (its status
   * gives the page once it comes). */
  pageWait?: number
  /** Where a device code is in what it prints, for showing beside the page. */
  userCode?: RegExp
  /** Why it isn't signed in though the command ended (what checking the sign-in found), in a line. */
  whyNot?: () => string | undefined
  /** Its command has ended (with `code`), and what it left (the page it handed its browser) is gone. */
  onEnd?: (code: number) => void
}

/**
 * The catcher, in the temporary folder: a path without spaces (Python splits BROWSER on them). In a
 * folder only you can open, written by the bridge itself (never one found there), and what it writes
 * down (a sign-in's page) only you can read.
 */
export function catcher(temporary: string): string {
  const path = join(privateFolder(join(temporary, "bin")), "kuvo-catch-url")
  const script = '#!/bin/sh\numask 077\nprintf \'%s\\n\' "$1" >> "$KUVO_URL_FILE"\n'
  if (!ownScript(path, script)) writeSecret(path, script, 0o700)
  return path
}

/** Whether `path` is that script, as the bridge wrote it: yours, and no one else's to change or read. */
function ownScript(path: string, script: string): boolean {
  try {
    const there = lstatSync(path)
    return there.isFile() && there.uid === process.getuid?.() && (there.mode & 0o777) === 0o700 && readFileSync(path, "utf8") === script
  } catch {
    return false
  }
}

/** An attempt's own folder, in the bridge's temporary folder, by the bridge's process: one a bridge
 * that stopped mid-way left goes as the next starts (`leftSignIns`), never another's still running. */
const SIGN_IN_FOLDER = /^kuvo-signin-(\d+)-/

/** The sign-ins' folders a bridge that's gone left in `temporary`, removed. */
export function leftSignIns(temporary: string) {
  let names: string[] = []
  try {
    names = readdirSync(temporary)
  } catch {}
  for (const name of names) {
    const pid = Number(SIGN_IN_FOLDER.exec(name)?.[1])
    if (!pid || pid === process.pid) continue
    try {
      process.kill(pid, 0)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ESRCH") rmSync(join(temporary, name), { recursive: true, force: true })
    }
  }
}

export async function commandSignIn(options: CommandSignInOptions): Promise<SignInAttempt> {
  // The page it hands its browser, written down in a folder of this attempt's own that only you can
  // open, and gone with it.
  const browser = catcher(options.temporary)
  const own = mkdtempSync(join(options.temporary, `kuvo-signin-${process.pid}-`))
  const urlFile = join(own, "url")
  const env: Record<string, string | undefined> = { ...process.env, ...options.env, BROWSER: browser, KUVO_URL_FILE: urlFile }
  // A value left `undefined` is unset, as in `run`.
  for (const name of Object.keys(env)) if (env[name] === undefined) delete env[name]
  // What it printed, its terminal codes taken out as a whole (one can come split over two reads).
  let printed = ""
  let output = ""
  // Where its output stood as a code was typed: what it printed since may turn it away.
  let pastedAt = 0
  const turnedAway = () => /invalid code/i.test(output.slice(pastedAt))
  const take = (text: string) => {
    printed += text
    output = plain(printed)
  }
  let type: (text: string) => Promise<void>
  let child: ReturnType<typeof Bun.spawn>
  // (A command that can't start leaves nothing behind either.)
  try {
    const spawned = Bun.spawn([options.command, ...options.args], { env, stdin: "pipe", stdout: "pipe", stderr: "pipe" })
    child = spawned
    const read = async (stream: ReadableStream<Uint8Array>) => {
      const decoder = new TextDecoder()
      for await (const chunk of stream) take(decoder.decode(chunk, { stream: true }))
    }
    void read(spawned.stdout)
    void read(spawned.stderr)
    type = async (text) => {
      spawned.stdin.write(text)
      await spawned.stdin.flush()
    }
  } catch {
    rmSync(own, { recursive: true, force: true })
    throw new Error("The sign-in program couldn’t start. Try again.")
  }
  let status: OAuthStatus = { status: "pending" }
  let pasted = false
  let submitting = false
  /** Whether it was signed in as it started (signing in again): undefined until known. */
  let signedInBefore: boolean | undefined
  const succeed = () => {
    status = { status: "complete" }
    options.onComplete?.()
  }
  void child.exited.then(async (code) => {
    rmSync(own, { recursive: true, force: true })
    options.onEnd?.(code)
    if (status.status !== "pending") return
    const signedIn = await options.signedIn().catch(() => false)
    if (status.status !== "pending") return
    // Signed in before (an ended sign-in still reads as one): only the command's own success says
    // this one worked.
    const why = signedIn ? undefined : options.whyNot?.()
    if (signedIn && (code === 0 || signedInBefore === false)) succeed()
    else if (why) status = { status: "failed", message: why }
    else if (pasted) status = { status: "failed", message: "That code didn't work. Start again, and paste the code from the page this sign-in opens." }
    else {
      console.error(`The sign-in command stopped (exit code ${code}).`)
      status = { status: "failed", message: "The sign-in ended before it finished. Try again." }
    }
  })
  // Signed in while the command lingers (it waits on something after the page): done then.
  // Not when it was signed in already (signing in again, another way): its end says.
  void (async () => {
    signedInBefore = await options.signedIn().catch(() => true)
    if (signedInBefore) return
    while (status.status === "pending") {
      await Bun.sleep(3_000)
      if (status.status !== "pending") return
      if (!(await options.signedIn().catch(() => false))) continue
      if (status.status === "pending") succeed()
      child.kill()
    }
  })()

  // The page to open: what the command handed its browser, which comes back to it by itself. What
  // it printed is for when no browser opens (Claude Code's ends on a code to paste), so it's only
  // taken when nothing reaches the browser a few seconds after it.
  let url = ""
  let printedAt: number | undefined
  // Whether the page in use is the one it printed: one that ends on a code to paste here.
  let printedInUse = options.printedPage === true
  const look = (waited: number): string => {
    const printed = firstURL(output)
    if (options.printedPage) return printed ?? ""
    if (existsSync(urlFile)) return readFileSync(urlFile, "utf8").split("\n")[0].trim()
    if (printed) printedAt ??= waited
    if (!printed || waited - printedAt! < 3_000) return ""
    printedInUse = true
    return printed
  }
  let waited = 0
  for (; waited < (options.pageWait ?? 15_000) && !url; waited += 200) {
    url = look(waited)
    if (!url) await Bun.sleep(200)
    if (status.status !== "pending") break
  }
  // Not come yet (the command slow to start, or waiting on another Claude Code): looked for still,
  // and given with its status once it comes, for the app to open then.
  let late: string | undefined
  if (!url) {
    void (async () => {
      for (; status.status === "pending" && !late; waited += 200) {
        late = look(waited) || undefined
        if (!late) await Bun.sleep(200)
      }
    })()
  }
  // A device code printed with the page: a moment more for it, if it comes after.
  let userCode: string | undefined
  for (let waited = 0; options.userCode && waited < 2_000; waited += 200) {
    userCode = options.userCode.exec(output)?.[1]
    if (userCode || status.status !== "pending") break
    await Bun.sleep(200)
  }
  const codePrompt = options.codePrompt && new RegExp(options.codePrompt.source, options.codePrompt.flags.replace(/[gy]/g, ""))
  // How it ends, as it stands (a late page decides it): on a code to paste here, or by itself.
  const mode = (): "auto" | "code" => (codePrompt && printedInUse ? "code" : "auto")
  const instructions = () => (mode() === "code" ? options.codeInstructions ?? options.instructions : options.instructions)
  return {
    url,
    userCode,
    get instructions() {
      return instructions()
    },
    get mode() {
      return mode()
    },
    acceptsCode: Boolean(codePrompt),
    complete: codePrompt
      ? async (code: string) => {
          if (status.status !== "pending") throw new Error("That sign-in is over. Start it again.")
          if (submitting) throw new Error("A code is already being submitted.")
          submitting = true
          try {
            const text = code.trim()
            pasted = true
            pastedAt = output.length
            await type(text + "\n")
            // A code it can't read (part of one) is turned away at once, and it waits for another.
            for (let waited = 0; waited < 1_500 && status.status === "pending"; waited += 100) {
              await Bun.sleep(100)
              if (turnedAway()) {
                pasted = false
                throw new Error("That code couldn't be read. Copy all of it from the page, and paste it again.")
              }
            }
          } finally {
            submitting = false
          }
        }
      : undefined,
    status: () => {
      if (status.status !== "pending") return status
      const code = userCode ?? options.userCode?.exec(output)?.[1]
      return { ...status, ...(late ? { url: late, instructions: instructions() } : {}), ...(code ? { userCode: code } : {}) }
    },
    cancel() {
      if (status.status === "pending") status = { status: "failed", message: "Cancelled." }
      child.kill()
    },
  }
}
