// Run each mover from the same released Mac app fixture, at the same paths.
// Compare the complete stopped destination before exercising any resident routes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { boxBinary, isolatedEnv, readJSON, scratch, typescriptRuntime } from "../adapter.mjs";
import { generateRelease } from "../compat/generate.mjs";
import { runSync, tags } from "../compat/tagged.mjs";
import { compare, difference, sqliteContents } from "./compare.mjs";

const [requestFile, ...extra] = process.argv.slice(2);
assert.ok(extra.length === 0, "Usage: node protocol/conformance/differential/mac-app.mjs [REQUESTS.json]");
assert.equal(process.platform, "darwin", "The released Mac app fixtures need macOS and APFS.");
const a = boxBinary(process.env.KUVO_BOX_BIN_A || "");
const b = boxBinary(process.env.KUVO_BOX_BIN_B || "");
const engine = process.env.KUVO_TEST_ENGINE;
const releasedSchemaTip = "20261010180456_project_time_archived";

function move(binary, app) {
  const args = ["rehome", ".kuvo-code", "--run", "--app", app.bundle, "--release", app.release, "--engine", app.engine];
  const env = { ...isolatedEnv(app.home), HOME: app.home, KUVO_HOME: app.K, TMPDIR: path.join(app.home, "tmp") };
  const output = binary.typescript
    ? runSync(typescriptRuntime(), ["--no-env-file", binary.file, ...args], {
        ...env, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
      }, app.home, "", 120000)
    : runSync(binary.file, args, env, app.home, "", 120000);
  const reply = JSON.parse(output.trim().split("\n").at(-1));
  assert.equal(reply.result, "done");
  clock(reply.status, "movedAt");
  return reply;
}

function clock(record, name) {
  if (Number.isSafeInteger(record[name]) && record[name] > 0) record[name] = "<wall time>";
}

function inventory(root) {
  const files = Object.create(null);
  function walk(folder, prefix = "") {
    for (const name of fs.readdirSync(folder).sort()) {
      const file = path.join(folder, name);
      const key = prefix + name;
      const stat = fs.lstatSync(file);
      const mode = stat.mode & 0o777;
      if (stat.isDirectory()) {
        files[key] = { mode, kind: "directory" };
        walk(file, key + "/");
      } else if (stat.isSymbolicLink()) {
        files[key] = { mode, kind: "symlink", target: fs.readlinkSync(file) };
      } else {
        assert.ok(stat.isFile(), "Unexpected special file at " + key);
        const entry = { mode, kind: "file" };
        files[key] = entry;
        if (key === "data/opencode/opencode.db") {
          entry.sqlite = sqliteContents(file, root, { migrationClockAfter: engine && releasedSchemaTip });
        } else if (engine && key === "data/opencode/log/opencode.log") {
          entry.log = fs.readFileSync(file, "utf8")
            .replace(/^timestamp=\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z /gm, "timestamp=<wall time> ")
            .replace(/^(timestamp=<wall time> level=[A-Z]+ run=)[a-f0-9]{8}(?= )/gm, "$1<engine run>")
            .replace(/ durationMs=\d+(?= role=)/g, " durationMs=<elapsed time>");
        } else if (["updates/rehome/journal.json", "updates/rehome/state.json"].includes(key)) {
          const record = entry.json = readJSON(file);
          if (key.endsWith("/journal.json")) {
            clock(record, "began");
            clock(record, "clonedAt");
            record.kept = record.kept?.replace(/\d{4}-\d{2}-\d{2}T\d{2}-\d{2}-\d{2}-\d{3}Z$/, "<wall time>");
            for (const names of Object.values(record.databaseFolders)) names.sort();
          } else {
            clock(record, "movedAt");
          }
        } else {
          entry.sha256 = createHash("sha256").update(fs.readFileSync(file)).digest("hex");
        }
      }
    }
  }
  walk(root);
  return files;
}

function outside(app) {
  return {
    keychain: inventory(app.keychain),
    backlink: fs.readFileSync(path.join(app.repository, ".git/worktrees/feature-one/gitdir"), "utf8"),
    oldLink: fs.readFileSync(path.join(app.worktree, ".git"), "utf8"),
  };
}

const root = scratch();
try {
  for (const tag of tags) {
    const fixture = path.join(root, tag);
    await generateRelease(tag, fixture);
    const app = readJSON(path.join(fixture, "app-world.json"));
    if (engine) {
      const { engine: version } = readJSON(path.join(app.release, "box.json"));
      fs.copyFileSync(engine, path.join(app.engine, version, "opencode"));
    }
    const saved = path.join(fixture, "saved-account");
    const first = path.join(fixture, "moved-by-a");
    const options = { recursive: true, verbatimSymlinks: true, preserveTimestamps: true };
    fs.cpSync(app.home, saved, options);
    const replyA = move(a, app);
    const filesA = inventory(app.K);
    const outsideA = outside(app);
    fs.renameSync(app.home, first);
    fs.renameSync(saved, app.home);
    const replyB = move(b, app);
    const filesB = inventory(app.K);
    const out = [];
    difference("rehome --run reply", replyA, replyB, out);
    difference("home", filesA, filesB, out);
    difference("outside home", outsideA, outside(app), out);
    if (out.length) {
      console.error(tag + " published differences:\n" + out.join("\n"));
      process.exitCode = 1;
    } else {
      console.log(tag + " matches at all " + Object.keys(filesA).length + " published paths, in the move reply and in Git and Keychain effects.");
    }
    if (requestFile) {
      const boot = await compare(path.join(first, ".kuvo-code"), readJSON(requestFile), a, b, { secondSeed: app.K });
      if (boot.length) {
        console.error(tag + " after resident startup:\n" + boot.join("\n"));
        process.exitCode = 1;
      }
    }
  }
} finally {
  fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
}
