import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const { kind, files: filters, pattern } = JSON.parse(Buffer.from(process.argv[2], "base64"));
const native = process.platform === "win32";
const box = kind === "box";
const sandboxReason = "Auto sandbox: Coming soon on Windows.";
// These files exercise the installed fence or its live integration. Pure sandbox policy,
// simulated namespace checks, and Windows approval routes still run.
const sandboxFiles = new Set([
  ...[
    "reviewer-own-folders",
    "sandbox-agent-session-env", "sandbox-agents-sockets", "sandbox-before-it-runs",
    "sandbox-bot-memory", "sandbox-claude-renewal", "sandbox-engine-worktree",
    "sandbox-full-access", "sandbox-git-kept", "sandbox-kept", "sandbox-leaves-nothing",
    "sandbox-runs-later", "sandbox-seat", "sandbox-session-services", "sandbox-walls",
  ].map(name => `test/${name}.test.ts`),
  "bridge-tests/sandbox-asks.test.js", "bridge-tests/sandbox.test.js",
]);

const folder = box ? "test" : "bridge-tests";
const suffix = box ? ".test.ts" : ".test.js";
const files = filters.length ? filters : fs.readdirSync(folder, { recursive: box })
  .filter(name => name.endsWith(suffix) && !name.split(path.sep).includes("node_modules"))
  .map(name => `${folder}/${name.split(path.sep).join("/")}`).sort();
for (const file of files) {
  if (!fs.existsSync(file)) throw new Error(`Test missing from tracked snapshot: ${file}`);
}

const tally = { pass: 0, fail: 0, skip: 0 };
const fileFailure = [];
let skippedFiles = 0;
let blockedFiles = 0;
let status = 0;

function command(program, args, capture = true) {
  const result = spawnSync(program, args, {
    stdio: capture ? ["ignore", "pipe", "pipe"] : ["ignore", "inherit", "inherit"],
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (result.error) console.error(result.error);
  return { code: result.status ?? 1, output: (result.stdout ?? "") + (result.stderr ?? "") };
}

for (const runtime of ["bun", "node"]) {
  const { code, output } = command(runtime, ["--version"]);
  if (code) throw new Error(`Cannot start ${runtime}: ${output.trim()}`);
  console.log(`Runtime: ${runtime} ${output.trim()}`);
}

function runFile(file) {
  console.log(`\n=== ${file} ===`);
  const fenceSecrets = file === "test/sandbox-secrets.test.ts";
  const args = box
    ? ["test", "--timeout", "120000", ...(fenceSecrets ? ["-t", "^the coding agents' sign-ins and opencode's own are among your secrets$"] : []), `./${file}`]
    : ["--test", "--test-reporter=tap", ...(pattern ? [pattern] : []), file];
  const result = command(box ? "bun" : "node", args);
  process.stdout.write(result.output);
  if (result.code) {
    status ||= result.code;
    fileFailure.push(file);
  }
  const fileCounts = {};
  for (const line of result.output.split(/\r?\n/)) {
    const counts = box ? line.match(/^\s*(\d+) (pass|fail|skip)$/) : line.match(/^# (pass|fail|skipped) (\d+)$/);
    if (counts) {
      const [label, count] = box ? [counts[2], counts[1]] : [counts[1], counts[2]];
      fileCounts[label === "skipped" ? "skip" : label] = Number(count);
    }
    if (box && line.startsWith("(skip) ")) {
      console.log(`Skip in ${file}: ${line.slice(7)} — declared by test`);
    }
    if (!box) {
      const skipped = line.match(/^\s*ok \d+ - (.+?) # SKIP(?: (.*))?$/);
      if (skipped) {
        const [, name, declared] = skipped;
        console.log(`Skip in ${file}: ${name} — ${declared || "declared by test"}`);
      }
    }
  }
  for (const [label, count] of Object.entries(fileCounts)) tally[label] += count;
  if (fenceSecrets) {
    for (const name of [
      "a secret in a folder commands write can't be moved, copied or named again to be read",
      "an agent's sign-in is hidden from commands, but not from the agent run whole with it as its own",
    ]) console.log(`Skip in ${file}: ${name} — ${sandboxReason}`);
    tally.skip += 2;
  }
}

const candidates = files.filter(file => {
  if (!sandboxFiles.has(file)) return true;
  console.log(`Skip file ${file} — ${sandboxReason}`);
  skippedFiles++;
  return false;
});

let runnable = candidates;
if (native) {
  // Pass the real guard using the suite runtime before installing dependencies or loading tests.
  if (runnable.length) {
    const guard = pathToFileURL(path.resolve("bridge-tests/test-safety.js")).href;
    const gate = command(box ? "bun" : "node", ["--eval", `import(${JSON.stringify(guard)})`]);
    process.stdout.write(gate.output);
    if (gate.code) {
      status ||= gate.code;
      for (const file of runnable) {
        console.log(`Blocked ${file} — native test guard refused to start.`);
        blockedFiles++;
      }
      runnable = [];
    }
  }
}
if (runnable.length) {
  const install = command("bun", ["install", "--frozen-lockfile"], false);
  if (install.code) {
    status ||= install.code;
    blockedFiles = runnable.length;
  } else {
    for (const file of runnable) runFile(file);
  }
}

console.log(`\nTests: ${tally.pass} pass, ${tally.fail} fail, ${tally.skip} skip`);
console.log(`Files: ${fileFailure.length} failed, ${skippedFiles} skipped, ${blockedFiles} blocked`);
for (const file of fileFailure) console.log(`Failed file: ${file}`);
process.exitCode = status;
