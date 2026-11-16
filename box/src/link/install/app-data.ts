// The installer's old-app data policy, also rendered for a read-only SSH probe. Keep the
// directory exclusions here so adding a computer and installing locally judge the same data.
import fs from "node:fs";
import path from "node:path";
import { quote } from "../../shared/text";
import { OWN_BOX_RAN } from "../setup/judge";

const DATA_RULES = [
  { folder: "", except: ["engine", "box", "desktop", "Caches", "cache", "tmp", "logs", "Logs", "opencode"] },
  { folder: "opencode", except: ["cache"] },
  { folder: "box", except: ["run", "bin"] },
];
const listOf = (folder: string): string[] => { try { return fs.readdirSync(folder); } catch { return []; } };
const exists = (file: string): boolean => { try { fs.lstatSync(file); return true; } catch { return false; } };
export const appBoxThere = (folder: string): boolean => OWN_BOX_RAN.some(file => exists(path.join(folder, "box", "run", file)));
export const appDataThere = (folder: string): boolean => DATA_RULES.some(rule => listOf(path.join(folder, rule.folder)).some(name => !name.startsWith(".") && !rule.except.includes(name)));

/** $A is the old app folder. No tools from that computer's Kuvo are run. */
export const appDataProbe = (): string => `old_app_verdict() {
${OWN_BOX_RAN.map(file => `  [ ! -e "$A/box/run/${file}" ] && [ ! -L "$A/box/run/${file}" ] || { echo app-box; return; }`).join("\n")}
${DATA_RULES.map(rule => `  for entry in "$A${rule.folder ? `/${rule.folder}` : ""}"/*; do
    [ -e "$entry" ] || [ -L "$entry" ] || continue
    case "\${entry##*/}" in ${rule.except.map(quote).join("|")}) continue ;; esac
    echo app-07; return
  done`).join("\n")}
}`;
