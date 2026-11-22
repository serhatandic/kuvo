// The coding agents' update notices (docs/architecture/agent-accounts.md, "Install, updates and
// notices"; PROTOCOL.md, "Update notices"): with "Update coding agents automatically" off, "Codex
// 0.161.0 is available" as soon as the box knows of a newer version than the one installed, with
// an Update action where Kuvo Code can update it; with it on, "Codex updated to 0.161.0" once the
// installed version moved on. One per program and version, kept in `<agents home>/update-notices.json`
// across restarts: served until a client says it was seen, or until it no longer holds (the program
// updated past it). A seen one is kept as seen, so the same version is never raised twice.
// The box decides and words them; clients draw them (the notifications bell, macOS notifications).

import { readFileSync } from "node:fs"
import { join } from "node:path"
import { writeDurablePrivate } from "../shared/files"
import { compareVersions, type Maintenance } from "./maintenance"

export interface UpdateNotice {
  /** `<program>-<kind>-<version>`: the same notice whenever it's decided again. */
  id: string
  /** The agent's program (`codex`), one for every account with it. */
  program: string
  /** Its name as the app shows it ("Codex"). */
  name: string
  kind: "available" | "updated"
  /** The version that's out (`available`) or that it updated to (`updated`). */
  version: string
  /** The version it had: installed then (`available`), or before it updated (`updated`). */
  from?: string
  /** When it was decided (ms). */
  at: number
  seen?: true
}

/** A notice as a client gets it: the box's words, and what can be done about it. */
export interface ServedNotice {
  id: string
  program: string
  /** The agent's own sign-in, whose update route updates its program. */
  provider: string
  kind: "available" | "updated"
  version: string
  title: string
  body: string
  at: number
  /** `update`: `POST kuvo/agents/api/kuvo/updates/<provider>`. None where Kuvo Code can't update it. */
  action?: { id: "update"; label: string }
}

interface Kept {
  notices: UpdateNotice[]
  /** Each program's version when last read, to tell an update that happened since. */
  versions: Record<string, string>
}

export const updateNoticesFile = (home: string) => join(home, "update-notices.json")

/** What a program is now, as the box reads it to decide. */
export interface ProgramNow {
  program: string
  name: string
  provider: string
  maintenance: Maintenance
  /** Kuvo Code can update it (its installer is known). */
  updates: boolean
}

/**
 * The box's update notices, kept in `file`. `note` is called with each program as it's read (as
 * clients ask for the accounts, and every hour); it returns a notice decided just now, once, to
 * be told to macOS and your phones.
 */
export class UpdateNotices {
  /** What Kuvo Code knows of each program now, for the words and the action. */
  private readonly programs = new Map<string, ProgramNow>()

  constructor(
    private readonly file: string,
    private readonly now = Date.now,
  ) {}

  private read(): Kept {
    try {
      const kept = JSON.parse(readFileSync(this.file, "utf8"))
      return {
        notices: Array.isArray(kept?.notices) ? kept.notices.filter((each: any) => typeof each?.id === "string" && typeof each?.program === "string" && typeof each?.version === "string") : [],
        versions: kept?.versions && typeof kept.versions === "object" ? kept.versions : {},
      }
    } catch {
      return { notices: [], versions: {} }
    }
  }

  private write(kept: Kept) {
    writeDurablePrivate(this.file, JSON.stringify(kept))
  }

  /**
   * `program` as just read, with the agents updating themselves (`automatic`) or not: a new
   * notice, if this makes one (once a version), and whether what's served changed.
   */
  note(now: ProgramNow, automatic: boolean): { told?: ServedNotice; changed: boolean } {
    this.programs.set(now.program, now)
    const installed = now.maintenance.installed
    if (!installed) return { changed: false }
    const kept = this.read()
    const before = JSON.stringify(kept)
    const was = kept.versions[now.program]
    kept.versions[now.program] = installed
    const mine = (notice: UpdateNotice) => notice.program === now.program
    // What it's updated to (or past) holds no more; nor does an earlier update, once it's moved on.
    kept.notices = kept.notices.filter((notice) => !mine(notice) || (notice.kind === "available" ? compareVersions(installed, notice.version) < 0 : notice.version === installed))
    let made: UpdateNotice | undefined
    const latest = now.maintenance.latest
    if (automatic) {
      // It updates itself: what's out isn't yours to do, so it's let be (and not raised again).
      for (const notice of kept.notices) if (mine(notice) && notice.kind === "available") notice.seen = true
      if (was && compareVersions(installed, was) > 0 && !kept.notices.some((notice) => mine(notice) && notice.kind === "updated" && notice.version === installed)) {
        made = { id: `${now.program}-updated-${installed}`, program: now.program, name: now.name, kind: "updated", version: installed, from: was, at: this.now() }
      }
    } else if (latest && compareVersions(installed, latest) < 0) {
      // A newer one than an earlier notice's replaces it; the same one is never raised twice.
      kept.notices = kept.notices.filter((notice) => !mine(notice) || notice.kind !== "available" || compareVersions(notice.version, latest) >= 0)
      if (!kept.notices.some((notice) => mine(notice) && notice.kind === "available" && notice.version === latest)) {
        made = { id: `${now.program}-available-${latest}`, program: now.program, name: now.name, kind: "available", version: latest, from: installed, at: this.now() }
      }
    }
    if (made) kept.notices.push(made)
    const changed = JSON.stringify(kept) !== before
    if (changed) {
      try {
        this.write(kept)
      } catch {
        // (Not kept, so not told: it's decided again the next time it's read.)
        return { changed: false }
      }
    }
    return { told: made && this.served(made), changed: Boolean(made) || changed }
  }

  /** What clients show now, newest first. */
  list(): ServedNotice[] {
    return this.read()
      .notices.filter((notice) => !notice.seen)
      .sort((a, b) => b.at - a.at)
      .map((notice) => this.served(notice))
  }

  /** It was seen (for every client): no longer served, and not raised again. Whether it was served. */
  seen(id: string): boolean {
    const kept = this.read()
    const notice = kept.notices.find((each) => each.id === id && !each.seen)
    if (!notice) return false
    notice.seen = true
    this.write(kept)
    return true
  }

  private served(notice: UpdateNotice): ServedNotice {
    const program = this.programs.get(notice.program)
    const name = program?.name ?? notice.name
    const base = { id: notice.id, program: notice.program, provider: program?.provider ?? notice.program, kind: notice.kind, version: notice.version, at: notice.at }
    if (notice.kind === "updated") {
      return { ...base, title: `${name} updated to ${notice.version}`, body: notice.from ? `It was ${notice.from}. It updates itself, as set in Settings › Computer.` : "It updates itself, as set in Settings › Computer." }
    }
    const installed = program?.maintenance.installed ?? notice.from
    const have = installed ? `You have ${installed}.` : ""
    if (program?.updates) return { ...base, title: `${name} ${notice.version} is available`, body: have, action: { id: "update", label: "Update" } }
    const how = program?.maintenance.shown ? `Update it with “${program.maintenance.shown}”.` : `Update ${name} the way you installed it.`
    return { ...base, title: `${name} ${notice.version} is available`, body: [have, how].filter(Boolean).join(" ") }
  }
}
