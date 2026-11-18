// GitHub CLI stores the account Work Queue reads. Its GitHub.com login uses a device code.

import type { SignInAttempt } from "./agent"
import { emit } from "./events"
import { newID } from "./ids"
import { run, which } from "./proc"
import { commandSignIn } from "./signin"

import { GITHUB_WORK_QUEUE } from "../shared/github"
const PAGE = "https://github.com/login/device"
const ENV = { GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" }

export class GitHubSignIn {
  private attempt?: { id: string; signIn: SignInAttempt; timer: ReturnType<typeof setTimeout> }
  private starting?: ReturnType<GitHubSignIn["start"]>
  private account?: { until: number; check: Promise<boolean> }

  constructor(private readonly folders: { home: string; temporary: string }) {}

  async integration() {
    const gh = which("gh")
    if (!gh) return null
    if (!this.account || this.account.until <= Date.now()) {
      const check = run(gh, ["auth", "status", "--hostname", "github.com"], { env: ENV, cwd: this.folders.temporary, timeout: 5_000 })
        .then(result => result.code === 0, () => false)
      this.account = { until: Date.now() + 30_000, check }
    }
    const signedIn = await this.account.check
    return {
      id: GITHUB_WORK_QUEUE, name: "GitHub for Work Queue", metadata: { source: "work-queue" },
      methods: [{ type: "oauth", id: "device", label: "Sign in with GitHub" }],
      connections: signedIn ? [{ type: "external", method: "oauth", status: { status: "connected", message: "Signed in to GitHub" } }] : [],
    }
  }

  begin() {
    return this.starting ??= this.start().finally(() => { this.starting = undefined })
  }

  private async start() {
    const gh = which("gh")
    if (!gh) throw new Error("Install GitHub CLI on the computer that runs the Work Queue, then try again in Settings → Accounts.")
    if (process.env.GH_TOKEN || process.env.GITHUB_TOKEN) throw new Error("The Work Queue uses a GitHub token set on this computer. Remove that setting before signing in here.")
    if (this.attempt) this.cancel(this.attempt.id)
    const signIn = await commandSignIn({
      ...this.folders, command: gh, args: ["auth", "login", "--hostname", "github.com", "--web"],
      env: ENV, signedIn: async () => true, printedPage: true, pageWait: 5_000,
      instructions: "Open GitHub, sign in, and enter the code shown here.",
      userCode: /One-time code\s*:\s*([A-Z0-9]{4}-[A-Z0-9]{4})/i,
      onComplete: () => {
        this.account = undefined
        emit("integration.updated", { integrationID: GITHUB_WORK_QUEUE })
      },
    })
    const id = newID("att")
    const created = Date.now()
    const expires = created + 15 * 60_000
    const timer = setTimeout(() => this.cancel(id), expires - created).unref()
    this.attempt = { id, signIn, timer }
    return { attemptID: id, url: signIn.userCode ? PAGE : "", instructions: signIn.instructions, userCode: signIn.userCode, mode: signIn.mode, time: { created, expires } }
  }

  status(id: string) {
    if (this.attempt?.id !== id) return { status: "expired" }
    const status = this.attempt.signIn.status()
    if (status.status === "pending") {
      const userCode = status.userCode ?? this.attempt.signIn.userCode
      return { status: "pending", ...(userCode ? { userCode, url: PAGE } : {}) }
    }
    if (status.status === "failed") return { status: "failed", message: "GitHub didn’t finish signing in. Start again for a new code." }
    return status
  }

  waiting(): boolean {
    return this.starting !== undefined || this.attempt?.signIn.status().status === "pending"
  }

  cancel(id = this.attempt?.id) {
    if (!this.attempt || this.attempt.id !== id) return
    clearTimeout(this.attempt.timer)
    this.attempt.signIn.cancel()
    this.attempt = undefined
  }
}
