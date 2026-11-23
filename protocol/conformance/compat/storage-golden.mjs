// Produce a scratch golden home and a TypeScript oracle for its storage envelopes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isolatedEnv, scratch, typescriptRuntime } from "../adapter.mjs";
import { currentChecks } from "./current-checks.mjs";
import { generateCurrent } from "./generate.mjs";
import { runSync } from "./tagged.mjs";

const root = scratch();
const { home } = generateCurrent(root);
const absolute = file => path.join(home, file);
const directory = (folder, accept = () => true) => readdirSync(absolute(folder)).filter(accept).map(leaf => path.join(folder, leaf));

const families = currentChecks(root).map(({ family, entries }) => ({
  family,
  files: entries.map(({ file, bytes }) => ({
    file,
    hash: createHash("sha256").update(bytes).digest("hex"),
  })),
}));

const entries = [];
const envelope = (kind, paths, more = {}) => {
  for (const relative of paths) entries.push({ kind, file: absolute(relative), version: 1, ...more });
};
envelope("object", [
  "agents/sessions/ses_golden/session.json", "box/git.json", "box/worktrees.json",
  "run/link.json", "run/moves/decision-hints/mov_golden.json", "run/moves/decisions/mov_golden.json",
  "run/serve.json", "run/setup-card.json", "run/update.json",
  "run/waiting/msg_goldenqueued.json", "run/welcomed.json", "run/work-queue/labels.json",
  "run/work-queue/model.json", "run/work-queue/numbers.json", "run/work-queue/queues/usr_you.json",
  "updates/rehome/state.json", ...directory("run/owned-data/edit-deletions"),
]);
const upload = families.find(({ family }) => family === "Relay backup upload reservations").files
  .find(({ file }) => file.endsWith(".upload.json"));
assert.ok(upload);
entries.push({ kind: "object", file: path.join(root, upload.file), version: 1 });
envelope("object", ["run/queues.json"], { nested: "ses_golden" });
envelope("list", ["agents/accounts.json", "run/moves/moved.json"]);
envelope("lines", [
  ...directory("agents/artifacts/log", name => name.endsWith(".jsonl")),
]);
const bot = readdirSync(absolute("bots")).find(name => name.startsWith("bot_"));
envelope("stamp", [
  "agents/usage/settings.json", "bin/bot-words.json", "bots/personalities.json",
  `bots/${bot}/bot.json`, "box/time-zone.json", "groups/grp_golden/group.json",
], { selection: "kuvo" });
envelope("stamp", ["box/projects.json", ...directory("agents/usage/cursor-history")], { selection: "version" });
envelope("stamp", ["box/vault/key.json", "updates/rehome/journal.json"], { selection: "v" });
envelope("stamp", ["box/vault/items.json", "box/vault/grants.json", "box/vault/paused.json"], { selection: "v", version: 2 });
envelope("stamp", directory("run/setup-checkpoints"), { selection: "v", nested: "d", base64: true });
envelope("stamp", ["updates/release/boxes.json"], { selection: "kuvo", version: "box/1", nested: "manifest", base64: true });

const oracle = fileURLToPath(new URL("./storage-envelope.ts", import.meta.url));
const output = runSync(typescriptRuntime(), ["--no-env-file", oracle], isolatedEnv(root), root, JSON.stringify(entries));
const answers = JSON.parse(output);
assert.equal(answers.length, entries.length);
writeFileSync(path.join(root, "storage-oracle.json"), JSON.stringify({
  families, envelopes: entries.map(({ file, ...entry }, i) => ({
    ...entry, file: path.relative(root, file), expected: answers[i],
  })),
}));
console.log(root);
