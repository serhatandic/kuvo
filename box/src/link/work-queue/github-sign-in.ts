// GitHub CLI stores the account Work Queue reads. Its GitHub.com login uses a device code.

import type { SignInAttempt } from "../../agents/agent"
import { emit } from "../../agents/events"
import { newID } from "../../agents/ids"
import { run, which } from "../../agents/proc"
import { commandSignIn } from "../../agents/signin"

import { GITHUB_SIGN_IN_BUSY, GITHUB_WORK_QUEUE } from "../../shared/github"
const PAGE = "https://github.com/login/device"
const ENV = { GH_PROMPT_DISABLED: "1", GH_NO_UPDATE_NOTIFIER: "1", NO_COLOR: "1" }

export class GitHubSignIn {
  private attempt?: { id: string; owner: string; signIn: SignInAttempt; timer: ReturnType<typeof setTimeout>; answer: Awaited<ReturnType<GitHubSignIn["start"]>> }
  private starting?: { owner: string; answer: ReturnType<GitHubSignIn["start"]> }
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

  begin(owner: string) {
    const waiting = this.starting ?? (this.attempt?.signIn.status().status === "pending" ? this.attempt : undefined)
    if (waiting) {
      if (waiting.owner !== owner) throw Object.assign(new Error(GITHUB_SIGN_IN_BUSY), { status: 409 })
      return Promise.resolve(waiting.answer)
    }
    const answer = this.start(owner).finally(() => { this.starting = undefined })
    this.starting = { owner, answer }
    return answer
  }

  private async start(owner: string) {
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
    const answer = { attemptID: id, url: signIn.userCode ? PAGE : "", instructions: signIn.instructions, userCode: signIn.userCode, mode: signIn.mode, time: { created, expires } }
    this.attempt = { id, owner, signIn, timer, answer }
    return answer
  }

  status(id: string, owner: string) {
    if (this.attempt?.id !== id || this.attempt.owner !== owner) return { status: "expired" }
    const status = this.attempt.signIn.status()
    if (status.status === "pending") {
      const userCode = status.userCode ?? this.attempt.signIn.userCode
      return { status: "pending", ...(userCode ? { userCode, url: PAGE } : {}) }
    }
    if (status.status === "failed") return { status: "failed", message: "GitHub didn’t finish signing in. Start again for a new code." }
    return status
  }

  async handle(request: Request): Promise<Response> {
    const url = new URL(request.url)
    const [, , , , connect, oauth, attempt, extra] = url.pathname.split("/")
    const method = request.method
    const response = (data: unknown) => Response.json({ data })
    if (!connect && method === "GET") return response(await this.integration())
    if (connect === "connect" && oauth === "oauth" && !extra) {
      const owner = request.headers.get("x-kuvo-sign-in-owner")
      if (!owner) return Response.json({ message: "Start GitHub sign-in from Accounts." }, { status: 400 })
      if (!attempt && method === "POST") {
        const body = await request.json().catch(() => ({})) as { methodID?: unknown }
        if (body.methodID !== "device") return Response.json({ message: "Choose Sign in with GitHub." }, { status: 400 })
        try { return response(await this.begin(owner)) }
        catch (error) {
          const status = (error as { status?: number }).status
          return Response.json({ message: status === 409 ? GITHUB_SIGN_IN_BUSY : "The sign-in couldn’t continue. Try again." }, { status: status ?? 502 })
        }
      }
      if (attempt && method === "GET") return response(this.status(attempt, owner))
      if (attempt && method === "DELETE") { this.cancel(attempt, owner); return Response.json({}) }
    }
    return Response.json({ message: "That GitHub sign-in action isn’t available." }, { status: 404 })
  }

  cancel(id = this.attempt?.id, owner?: string) {
    if (!this.attempt || this.attempt.id !== id || (owner !== undefined && this.attempt.owner !== owner)) return
    clearTimeout(this.attempt.timer)
    this.attempt.signIn.cancel()
    this.attempt = undefined
  }
}
