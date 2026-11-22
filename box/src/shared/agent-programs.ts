// Where each coding agent's own program is: its maker's installer's place, else your PATH. The
// bridge runs turns with it, and the usage reader asks it about its plan.
import { homedir } from "node:os"
import { join } from "node:path"
import { which } from "../agents/proc"

export const claudePlaces = [join(homedir(), ".local/bin"), join(homedir(), ".claude/local"), "/opt/homebrew/bin", "/usr/local/bin"]
export const codexPlaces = ["/opt/homebrew/bin", "/usr/local/bin", join(homedir(), ".local/bin"), join(homedir(), ".codex/bin")]
/** Google's installer puts `agy` in `~/.local/bin`. */
export const agyPlaces = [join(homedir(), ".local/bin"), "/opt/homebrew/bin", "/usr/local/bin"]
export const grokPlaces = [join(homedir(), ".grok/bin"), join(homedir(), ".local/bin"), "/opt/homebrew/bin", "/usr/local/bin"]

export const claudeProgram = () => which("claude", claudePlaces)
export const codexProgram = () => which("codex", codexPlaces)
export const grokProgram = () => which("grok", grokPlaces)
export const agyProgram = () => which("agy", agyPlaces)
