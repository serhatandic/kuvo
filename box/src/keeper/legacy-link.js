// Released beta.1 Macs run `kuvo-link.js box <verb>` over SSH with the pinned engine's Bun (their
// MachineBox). This only hands those verbs to the Rust box, and goes with the TypeScript box,
// 4 weeks after the last switch.
import { spawnSync } from "node:child_process";
import path from "node:path";

const args = process.argv.slice(2);
const [box, verb] = args;
if (box !== "box" || !["adopt", "read", "change", "key", "route", "personalities"].includes(verb) ||
    args.length !== (verb === "key" ? 3 : 2) || !path.isAbsolute(process.env.KUVO_HOME ?? "")) {
  console.error("Update Kuvo Code on this Mac to use this computer.");
  process.exit(2);
}
const executable = path.join(process.env.KUVO_HOME, "bin", process.platform === "win32" ? "kuvo-box.exe" : "kuvo-box");
const result = spawnSync(executable, args, { stdio: "inherit" });
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
