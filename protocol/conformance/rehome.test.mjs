// Run the staged Mac move command against a released app's files and a private
// account. The engine and Keychain tools are the golden fixture's stand-ins.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { once } from "node:events";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { boxBinary, isolatedEnv, launch, readJSON, scratch, typescriptRuntime, until, write } from "./adapter.mjs";
import { generateRelease } from "./compat/generate.mjs";

test("a released Mac move refuses unsafe inputs and can be tried again", {
  skip: process.platform !== "darwin" && "Released Mac app folders require macOS and APFS",
  timeout: 120000,
}, async t => {
  const binary = boxBinary();
  const root = scratch();
  t.after(() => fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 }));
  const fixture = path.join(root, "release");
  await generateRelease("v0.7.2", fixture);
  const app = readJSON(path.join(fixture, "app-world.json"));
  const saved = path.join(root, "saved");
  const copy = { recursive: true, verbatimSymlinks: true, preserveTimestamps: true };
  fs.cpSync(app.home, saved, copy);
  const env = { ...isolatedEnv(root), HOME: app.home, KUVO_HOME: app.K, TMPDIR: path.join(app.home, "tmp") };
  const note = path.join(app.S, "Chats/c1/notes.md");
  const original = fs.readFileSync(note);

  function reset() {
    fs.rmSync(app.home, { recursive: true, force: true });
    fs.cpSync(saved, app.home, copy);
  }

  function command(args, exitCode, source = app, family = "rehome") {
    const account = { ...env, HOME: source.home, KUVO_HOME: source.K, TMPDIR: path.join(source.home, "tmp") };
    const result = spawnSync(binary.typescript ? typescriptRuntime() : binary.file,
      binary.typescript ? ["--no-env-file", binary.file, family, ...args] : [family, ...args], {
        env: binary.typescript ? { ...account, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" } : account,
        cwd: source.home, encoding: "utf8", timeout: 45000,
      });
    if (result.error) throw result.error;
    assert.equal(result.status, exitCode, result.stdout + result.stderr);
    return result.stdout.trim() ? JSON.parse(result.stdout.trim()) : null;
  }

  function move(exitCode, source = app) {
    return command([".kuvo-code", "--run", "--app", source.bundle, "--release", source.release, "--engine", source.engine], exitCode, source);
  }

  function untouched() {
    assert.deepEqual(fs.readFileSync(note), original);
    assert.equal(fs.existsSync(path.join(app.K, "folder-version.json")), false);
    assert.equal(fs.existsSync(path.join(app.keychain, "helper")), false);
  }

  await t.test("the installer distinguishes fresh, older, interrupted, and occupied homes", async () => {
    reset();
    fs.rmSync(app.S, { recursive: true, force: true });
    const judge = (verdict, verb) => {
      const reply = command(["--status", "--root", ".kuvo-code", "--app", app.bundle], 0, app, "install");
      assert.deepEqual({ verdict: reply.verdict, verb: reply.verb }, { verdict, verb });
      return reply;
    };
    write(path.join(app.S, "opencode/cache/models.json"), "{}");
    judge("fresh", "new");
    const making = path.join(app.K, "updates/rehome/new");
    const kind = path.join(app.K, "box/kind");
    write(making, "1\n");
    write(kind, "mac\n");
    judge("fresh", "new");
    fs.rmSync(making);
    fs.rmSync(kind);
    write(note, "old conversation");
    judge("app-07", "move");
    write(path.join(app.S, "box/run/keeper.json"), "{}");
    judge("app-box", "move");

    const journal = path.join(app.K, "updates/rehome/journal.json");
    write(journal, JSON.stringify({ v: 1, way: "move", began: 1, step: "clone", from: app.S, to: app.K, placements: [], clearing: [] }));
    assert.equal(judge("moving", "move").status.step, "clone");
    write(journal, JSON.stringify({ v: 1, way: "move", step: "clear" }));
    judge("stuck", "move");
    write(journal, "{");
    const refused = command(["--status", "--root", ".kuvo-code", "--app", app.bundle], 1, app, "install");
    assert.equal(refused.result, "refused");
    assert.match(refused.message, /can't tell how far it got/);
    fs.rmSync(journal);

    const holder = launch(process.execPath, ["-e", "console.log('ready');process.stdin.resume()", "rehome"], env, app.home, "pipe");
    try {
      await once(holder.child.stdout, "data");
      write(path.join(app.K, "updates/rehome/lock/pid"), String(holder.child.pid));
      judge("waits", null);
      holder.child.stdin.end();
      await holder.exited;
      judge("app-box", "move");
    } finally {
      await holder.stop();
    }

    const state = path.join(app.K, "updates/rehome/state.json");
    write(state, '{"state":"back","way":"back","movedBack":true}');
    judge("back", null);
    fs.rmSync(state);
    write(path.join(app.K, "bin/kuvo-start"), "#!/bin/sh\n");
    write(path.join(app.K, "run/link.json"), "{}");
    judge("two", null);
    fs.rmSync(app.S, { recursive: true, force: true });
    judge("machine", null);
    const off = path.join(app.K, "run/off");
    write(off, "");
    judge("fresh", "new");
    fs.rmSync(off);
    write(path.join(app.K, "box/kind"), "mac\n");
    write(state, '{"state":"done","way":"move","owed":["keychain","passwords"]}');
    judge("ours", null);
    for (const verb of ["--sign-ins", "--settle"]) {
      const leftover = launch(process.execPath, ["-e", "console.log('ready');process.stdin.resume()", "rehome", ".kuvo-code", verb], env, app.home, "pipe");
      try {
        await once(leftover.child.stdout, "data");
        write(path.join(app.K, "updates/rehome/lock/pid"), String(leftover.child.pid));
        assert.deepEqual(judge("ours", null).status.signIns, verb === "--sign-ins" ? { state: "asking" } : null);
      } finally {
        await leftover.stop();
      }
    }
    const signInsLast = { state: "failed", at: 123, message: "Read-back failed" };
    write(state, JSON.stringify({ state: "done", way: "move", owed: ["passwords"], signInsLast }));
    assert.deepEqual(judge("ours", null).status.signIns, signInsLast);
    write(state, JSON.stringify({ state: "done", way: "move", owed: [], signInsLast }));
    assert.equal(judge("ours", null).status.signIns, null);
  });

  await t.test("empty bot and group folders from the earlier app remain available", () => {
    reset();
    for (const name of ["bots", "groups"]) {
      const source = path.join(app.S, name);
      fs.rmSync(source, { recursive: true, force: true });
      fs.mkdirSync(source, { recursive: true, mode: 0o700 });
    }
    assert.equal(move(0).result, "done");
    for (const name of ["bots", "groups"]) {
      assert.deepEqual(fs.readdirSync(path.join(app.K, name)), []);
      assert.deepEqual(fs.readdirSync(path.join(app.S, name)), []);
    }
  });

  await t.test("new-home readiness distinguishes a collision from a cut-short install", () => {
    reset();
    const appBox = { ...app, K: path.join(app.S, "box") };
    const check = () => command([".kuvo-code", "--new", "--check"], 0, appBox);
    assert.match(check().why, /data is in its folder/);
    fs.rmSync(app.S, { recursive: true, force: true });
    assert.match(check().why, /engine isn't downloaded/);
    write(path.join(app.S, "engine/2.2.6/opencode"), "downloaded");
    assert.deepEqual(check(), { ready: true, made: false });
    write(path.join(app.K, "box/kind"), "mac\n");
    const making = path.join(app.K, "updates/rehome/new");
    write(making, "1\n");
    assert.deepEqual(check(), { ready: true, made: false });
    assert.equal(fs.readFileSync(making, "utf8"), "1\n");
    fs.rmSync(making);
    assert.deepEqual(check(), { ready: false, made: true });
    write(path.join(app.K, "box/kind"), "machine\n");
    write(path.join(app.K, "bin/kuvo-start"), "keeper");
    assert.match(check().why, /set up in this Mac's/);
    const journal = path.join(app.K, "updates/rehome/journal.json");
    write(journal, JSON.stringify({ v: 1, way: "move", step: "clone" }));
    assert.match(check().why, /is moving/);
    assert.match(fs.readFileSync(journal, "utf8"), /clone/);
  });

  await t.test("a cut-short new install can be made again and another attempt stays idle", () => {
    reset();
    fs.rmSync(app.S, { recursive: true, force: true });
    const appBox = { ...app, K: path.join(app.S, "box") };
    const install = exit => command([".kuvo-code", "--new", "--release", app.release, "--keeps-awake", "off"], exit, appBox);
    const engine = "engine/2.2.6/opencode";
    const broken = path.join(app.release, "bin/broken");
    write(path.join(app.S, engine), "fresh engine");
    fs.symlinkSync(path.join(app.home, "missing"), broken);
    assert.equal(install(1).result, "failed");
    assert.equal(fs.existsSync(path.join(app.K, "updates/rehome/new")), false);
    assert.equal(fs.existsSync(path.join(app.K, "box/kind")), false);
    fs.rmSync(broken);

    write(path.join(app.K, "updates/rehome/new"), "1\n");
    write(path.join(app.K, "bin/partial-program"), "previous program");
    write(path.join(app.K, "engine.new/partial"), "discardable scratch");
    write(path.join(app.K, "desktop/paired.json"), '{"kept":true}');
    write(path.join(app.K, "trash/earlier-box-previous/receipt"), "kept earlier");
    assert.equal(install(0).result, "done");
    assert.equal(fs.existsSync(path.join(app.K, "updates/rehome/new")), false);
    assert.equal(fs.readFileSync(path.join(app.K, "box/kind"), "utf8"), "mac\n");
    assert.deepEqual(readJSON(path.join(app.K, "box/work.json")), { keepsAwake: false });
    assert.equal(fs.readFileSync(path.join(app.K, engine), "utf8"), "fresh engine");
    assert.equal(fs.readFileSync(path.join(app.S, engine), "utf8"), "fresh engine");
    assert.deepEqual(readJSON(path.join(app.K, "desktop/paired.json")), { kept: true });
    assert.equal(fs.readFileSync(path.join(app.K, "trash/earlier-box-previous/receipt"), "utf8"), "kept earlier");
    const archive = fs.readdirSync(path.join(app.K, "trash")).find(name => name.startsWith("earlier-box-") && name !== "earlier-box-previous");
    assert.ok(archive);
    const kept = path.join(app.K, "trash", archive);
    assert.equal(fs.readFileSync(path.join(kept, "bin/partial-program"), "utf8"), "previous program");
    assert.equal(fs.existsSync(path.join(kept, "engine.new")), false);
    assert.equal(install(0).result, "nothing");
  });

  await t.test("a torn journal blocks startup with the journal preserved", () => {
    reset();
    const journal = path.join(app.K, "updates/rehome/journal.json");
    const torn = '{"v": 1, "step": "cle';
    write(journal, torn);
    const reply = move(3);
    assert.equal(reply.result, "stuck");
    assert.equal(reply.status.state, "moving");
    assert.equal(reply.status.failed.step, null);
    assert.equal(reply.message, "Kuvo was stopped part way through moving your sessions and can't tell how far it got, so it's changing nothing more. Try again; if this stays, send ~/.kuvo-code/updates/rehome/run.log to Kuvo's support.");
    assert.equal(reply.status.failed.message, reply.message);
    assert.equal(fs.readFileSync(journal, "utf8"), torn);
    untouched();
  });

  await t.test("an earlier move interrupted before placement can be started again", () => {
    reset();
    const rehome = path.join(app.K, "updates/rehome");
    write(path.join(rehome, "staging/unfinished"), "private draft");
    write(path.join(rehome, "journal.json"), JSON.stringify({
      v: 1, way: "move", began: 1, step: "clone", from: app.S, to: app.K,
      placements: [], clearing: [], kept: path.join(rehome, "kept/interrupted"),
    }));
    assert.equal(move(0).result, "done");
    assert.equal(fs.existsSync(path.join(rehome, "staging/unfinished")), false);
    assert.deepEqual(fs.readFileSync(path.join(app.K, "chats/c1/notes.md")), original);
    assert.deepEqual(fs.readFileSync(note), original);
  });

  await t.test("an earlier interrupted placement takes back its files before retrying", () => {
    reset();
    const rehome = path.join(app.K, "updates/rehome");
    const stage = path.join(rehome, "staging");
    const bin = path.join(app.K, "bin");
    const layout = path.join(app.K, "folder-version.json");
    write(path.join(bin, "partial-program"), "only the interrupted copy");
    write(layout, '{"version":23}');
    write(path.join(app.K, "box/kind"), "mac\n");
    write(path.join(rehome, "journal.json"), JSON.stringify({
      v: 1, way: "move", began: 1, step: "place", from: app.S, to: app.K,
      placements: [{ from: path.join(app.release, "bin"), to: bin, staged: path.join(stage, "0-bin"), installed: true }],
      clearing: [], kept: path.join(rehome, "kept/interrupted"),
    }));
    const actions = [
      { act: "rename", from: path.join(stage, "0-bin"), to: bin },
      { act: "rename", from: path.join(stage, "work/folder-version.json"), to: layout },
      { act: "made", path: path.join(app.K, "box"), folder: true },
      { act: "made", path: path.join(app.K, "box/kind") },
    ];
    write(path.join(rehome, "journal.acts"), actions.map(act => JSON.stringify({ ...act, formatVersion: 1 })).join("\n") + "\n");
    write(path.join(rehome, "lock/pid"), "2147483647\n");
    const resumed = command([".kuvo-code", "--resume"], 1);
    assert.equal(resumed.result, "failed");
    assert.equal(resumed.message, "It stopped part way (place), so it went back.");
    assert.equal(resumed.status.tries, 1);
    assert.equal(resumed.status.nextTry, resumed.status.failed.at + 3_600_000);
    assert.equal(resumed.status.progress, null);
    assert.equal(fs.existsSync(layout), false);
    assert.equal(move(0).result, "done");
    assert.equal(fs.existsSync(path.join(bin, "partial-program")), false);
    assert.deepEqual(fs.readFileSync(path.join(app.K, "chats/c1/notes.md")), original);
    assert.deepEqual(fs.readFileSync(note), original);
  });

  await t.test("a missing engine leaves the move ready to retry online", () => {
    reset();
    fs.rmSync(app.engine, { recursive: true });
    const reply = move(1);
    assert.equal(reply.result, "refused");
    assert.equal(reply.message, "Kuvo's engine couldn't be downloaded. Check that this Mac is online, then try again.");
    assert.equal(reply.status.tries, 1);
    assert.equal(reply.status.failed.step, "plan");
    assert.equal(reply.status.nextTry, null);
    assert.deepEqual(reply.status.progress, { step: "plan", title: "Checking this Mac", index: 1, of: 11 });
    assert.deepEqual(command([".kuvo-code", "--status"], 0), { ...reply.status, progress: null });
    untouched();
  });

  await t.test("a path in place of the home name is rejected as usage", () => {
    reset();
    assert.equal(command(["../.kuvo-code", "--status"], 2), null);
    const plist = path.join(app.bundle, "Contents/Info.plist");
    const originalPlist = fs.readFileSync(plist, "utf8");
    write(plist, originalPlist.replace(path.basename(app.S), "run.kuvo.code"));
    const refused = move(1);
    assert.equal(refused.result, "refused");
    assert.equal(refused.message, "Kuvo itself moves into ~/.kuvo-code only.");
    untouched();
  });

  await t.test("a machine's home and the app folder both keep their files", () => {
    reset();
    const machineNote = path.join(app.K, "chats/theirs/notes.md");
    write(path.join(app.K, "bin/kuvo-start"), "#!/bin/sh\n", { mode: 0o700 });
    write(path.join(app.K, "run/link.json"), "{}");
    write(machineNote, "the machine's own");
    const reply = move(1);
    assert.equal(reply.result, "refused");
    assert.equal(reply.message, "This Mac is set up as one of your machines, and has an earlier Kuvo Code's sessions too: putting them together isn't in this version yet.");
    assert.equal(reply.status.state, null);
    assert.equal(fs.readFileSync(machineNote, "utf8"), "the machine's own");
    assert.equal(fs.existsSync(path.join(app.K, "updates/rehome/state.json")), false);
    untouched();
  });

  await t.test("a removed installation is kept aside before the move starts a new home", () => {
    reset();
    write(path.join(app.K, "box/earlier.md"), "old installation");
    write(path.join(app.K, "run/off"), "");
    write(path.join(app.K, "updates/earlier.json"), "[]");
    write(path.join(app.K, "trash/earlier/note.md"), "old trash");
    write(path.join(app.K, "updates/rehome/new"), "1\n");
    write(path.join(app.K, "desktop/window.json"), '{"panel":"files"}');
    assert.equal(move(0).result, "done");
    const [kept] = fs.readdirSync(path.join(app.K, "trash"));
    assert.match(kept, /^earlier-box-/);
    const keptRoot = path.join(app.K, "trash", kept);
    assert.equal(fs.readFileSync(path.join(keptRoot, "box/earlier.md"), "utf8"), "old installation");
    assert.equal(fs.readFileSync(path.join(keptRoot, "updates/earlier.json"), "utf8"), "[]");
    assert.equal(fs.readFileSync(path.join(keptRoot, "trash/earlier/note.md"), "utf8"), "old trash");
    assert.equal(fs.readFileSync(path.join(keptRoot, "new-box-mark"), "utf8"), "1\n");
    assert.deepEqual(readJSON(path.join(app.K, "desktop/window.json")), { panel: "files" });
    assert.deepEqual(readJSON(path.join(app.K, "folder-version.json")), { version: 23 });
    assert.deepEqual(fs.readFileSync(note), original);
  });

  await t.test("a live writer is named and the move succeeds once it closes the file", async () => {
    reset();
    const log = path.join(app.S, "Chats/c1/open.log");
    const writer = launch("/bin/zsh", ["-c", 'exec 3>>"$1"; echo ready; read line', "zsh", log], env, app.home, "pipe");
    try {
      const [ready] = await once(writer.child.stdout, "data");
      assert.equal(String(ready).trim(), "ready");
      const reply = move(1);
      assert.equal(reply.result, "refused");
      assert.equal(reply.message, "“zsh” is still using your earlier Kuvo Code's files. Quit it, then try again.");
      assert.equal(writer.child.exitCode, null);
      untouched();
      writer.child.stdin.end();
      await writer.exited;
      assert.equal(move(0).result, "done");
      assert.equal(fs.readFileSync(path.join(app.K, "chats/c1/open.log"), "utf8"), "");
      assert.deepEqual(fs.readFileSync(note), original);
    } finally {
      await writer.stop();
    }
  });

  await t.test("the older app stops while a chat's program and another current app stay live", async () => {
    reset();
    const plist = fs.readFileSync(path.join(app.bundle, "Contents/Info.plist"), "utf8");
    const appProgram = /<key>CFBundleExecutable<\/key>\s*<string>([^<]+)<\/string>/;
    const current = appProgram.exec(plist)?.[1];
    assert.ok(current);
    function executable(file) {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.copyFileSync("/bin/zsh", file);
      const signed = spawnSync("/usr/bin/codesign", ["--force", "--sign", "-", file], { env, encoding: "utf8" });
      assert.equal(signed.status, 0, signed.stderr);
      return file;
    }
    function bundle(folder, program) {
      write(path.join(folder, "Contents/Info.plist"), plist.replace(appProgram, `<key>CFBundleExecutable</key><string>${program}</string>`));
      return executable(path.join(folder, "Contents/MacOS", program));
    }
    const files = [
      executable(path.join(app.S, "Chats/c1/build/server")),
      bundle(path.join(app.home, "Downloads/Kuvo.app"), current),
      bundle(path.join(app.home, "Applications/Earlier.app"), "Kuvo Code"),
    ];
    const tasks = files.map(file => launch(file, ["-c", "print ready; read -r line"], env, app.home, "pipe"));
    try {
      await Promise.all(tasks.map(task => once(task.child.stdout, "data", { signal: AbortSignal.timeout(5000) })));
      assert.equal(move(0).result, "done");
      await until(() => tasks[2].child.signalCode || tasks[2].child.exitCode !== null, "The older app did not exit", 3000);
      assert.equal(tasks[2].child.signalCode, "SIGTERM");
      for (const task of tasks.slice(0, 2)) {
        assert.equal(task.child.exitCode, null);
        assert.equal(task.child.signalCode, null);
      }
    } finally {
      await Promise.all(tasks.map(task => task.stop()));
    }
  });

  await t.test("the old engine stops while a reader from outside the app remains open", async () => {
    reset();
    const program = path.join(app.S, "engine/2.0.32/opencode");
    fs.copyFileSync("/bin/zsh", program);
    const signed = spawnSync("/usr/bin/codesign", ["-f", "-s", "-", program], { env, encoding: "utf8" });
    assert.equal(signed.status, 0, signed.stderr);
    const file = path.join(app.S, "opencode/data/opencode/opencode.db");
    const alias = path.join(app.home, "read-only.db");
    fs.symlinkSync(file, alias);
    const reader = launch("/bin/zsh", ["-c", 'exec 3<"$1"; echo ready; read line', "zsh", alias], env, app.home, "pipe");
    const old = launch(program, ["-c", "echo ready; read line"], env, app.home, "pipe");
    try {
      for (const [ready] of await Promise.all([once(reader.child.stdout, "data"), once(old.child.stdout, "data")])) {
        assert.equal(String(ready).trim(), "ready");
      }
      write(path.join(app.S, "box/run/engine.json"), JSON.stringify({ pid: reader.child.pid }));
      assert.equal(move(0).result, "done");
      assert.equal((await old.exited).signal, "SIGTERM");
      assert.equal(reader.child.exitCode, null);
      assert.equal(reader.child.signalCode, null);
      assert.deepEqual(fs.readFileSync(note), original);
    } finally {
      await old.stop();
      await reader.stop();
    }
  });

  await t.test("the earlier app's keeper shuts down through its saved loopback control", async () => {
    reset();
    const server = path.join(app.home, "keeper.mjs");
    write(server, `import {createServer} from "node:http";
const keeper = createServer((request, response) => {
  if (request.method !== "POST" || request.url !== "/stop" ||
      request.headers.authorization !== "Bearer test-password") {
    response.writeHead(404).end();
    return;
  }
  response.end("{}", () => keeper.close());
});
keeper.listen(0, "127.0.0.1", () => console.log(keeper.address().port));
`);
    const keeper = launch(process.execPath, [server, "--box"], env, app.home, "pipe");
    try {
      const [ready] = await once(keeper.child.stdout, "data");
      write(path.join(app.S, "box/run/keeper.json"), JSON.stringify({
        pid: keeper.child.pid, url: "http://127.0.0.1:" + String(ready).trim(),
        password: "test-password",
      }));
      assert.equal(move(0).result, "done");
      assert.equal((await keeper.exited).code, 0);
      assert.equal(fs.existsSync(path.join(app.K, "run/keeper.json")), false);
      assert.deepEqual(fs.readFileSync(note), original);
    } finally {
      await keeper.stop();
    }
  });

  await t.test("a damaged property list leaves settings and conversations where they were", () => {
    reset();
    const preferences = path.join(app.home, "Library/Preferences", path.basename(app.S) + ".plist");
    write(preferences, "bplist00 cut short");
    const reply = move(1);
    assert.equal(reply.result, "failed");
    assert.equal(reply.message, "Kuvo couldn't read the earlier Kuvo Code's settings, so nothing was moved. Try again.");
    assert.equal(reply.status.failed.step, "upgrade");
    assert.equal(fs.readFileSync(preferences, "utf8"), "bplist00 cut short");
    untouched();
  });

  await t.test("a newer engine schema is refused before the source database is touched", () => {
    reset();
    const file = path.join(app.S, "opencode/data/opencode/opencode.db");
    const sqlite = spawnSync("/usr/bin/sqlite3", [file,
      "INSERT INTO migration (id, time_completed) VALUES ('20270101000000_later', 1);",
    ], { env, encoding: "utf8" });
    assert.equal(sqlite.status, 0, sqlite.stderr);
    const before = fs.readFileSync(file);
    const reply = move(1);
    assert.equal(reply.result, "refused");
    assert.equal(reply.status.failed.step, "plan");
    assert.equal(reply.message, "Your sessions were last used by a version of Kuvo Code that this one doesn't know how to bring along (51 20270101000000_later). Nothing was changed. Update Kuvo if a newer version is out, then try again.");
    assert.deepEqual(fs.readFileSync(file), before);
    untouched();
  });

  await t.test("a newer app record keeps the move from starting", () => {
    reset();
    const bots = path.join(app.S, "bots.json");
    const bytes = '[{"formatVersion":2},{"id":"future-bot"}]\n';
    write(bots, bytes);
    const reply = move(1);
    assert.equal(reply.result, "failed");
    assert.match(reply.message, /Legacy Mac import list format version 2 is unsupported/);
    assert.equal(fs.readFileSync(bots, "utf8"), bytes);
    untouched();

    const rehome = path.join(app.K, "updates/rehome");
    const staged = path.join(rehome, "staging/unfinished");
    write(staged, "private draft");
    const journal = JSON.stringify({
      v: 1, way: "move", began: 1, step: "clone", from: app.S, to: app.K,
      placements: [], clearing: [], kept: path.join(rehome, "kept/interrupted"),
    });
    write(path.join(rehome, "journal.json"), journal);
    const stuck = move(3);
    assert.equal(stuck.result, "stuck");
    assert.match(stuck.message, /Legacy Mac import list format version 2 is unsupported/);
    assert.equal(fs.readFileSync(path.join(rehome, "journal.json"), "utf8"), journal);
    assert.equal(fs.readFileSync(staged, "utf8"), "private draft");
  });

  await t.test("a failed engine upgrade leaves the source intact", () => {
    reset();
    const world = path.join(app.home, "world.json");
    write(world, JSON.stringify({ ...readJSON(world), cannotMigrate: true }));
    const reply = move(1);
    assert.equal(reply.result, "failed");
    assert.equal(reply.status.failed.step, "database");
    assert.equal(reply.message, "Kuvo couldn't move your sessions this time, and left everything as it was. Try again.");
    untouched();
  });

  await t.test("there is no cutover when the destination disk is too small", () => {
    const image = path.join(root, "capacity.dmg");
    const mount = path.join(fs.realpathSync(root), "volume");
    fs.mkdirSync(mount);
    const diskImage = args => {
      const result = spawnSync("/usr/bin/hdiutil", args, { env, encoding: "utf8", timeout: 30000 });
      if (result.error) throw result.error;
      assert.equal(result.status, 0, result.stderr);
    };
    diskImage(["create", "-size", "128m", "-fs", "APFS", "-volname", "Kuvo Move Test", image]);
    diskImage(["attach", image, "-mountpoint", mount, "-nobrowse", "-noverify", "-noautoopen"]);
    try {
      const home = path.join(mount, "account");
      fs.cpSync(saved, home, copy);
      const source = Object.fromEntries(["S", "K", "bundle", "release", "engine"].map(name => [
        name, path.join(home, path.relative(app.home, app[name])),
      ]));
      const reply = move(1, { ...source, home });
      assert.equal(reply.result, "refused");
      assert.match(reply.message, /^There isn't enough room on this Mac to move your sessions\. Free up \d+ MB, then try again\.$/);
      assert.equal(reply.status.failed.step, "plan");
      assert.equal(fs.existsSync(path.join(source.K, "folder-version.json")), false);
      assert.deepEqual(fs.readFileSync(path.join(source.S, "Chats/c1/notes.md")), original);
    } finally {
      diskImage(["detach", mount]);
    }
  });

  await t.test("a legacy bot and group chat open in the moved home", () => {
    reset();
    const date = "2026-10-01T10:00:00Z";
    const local = path.join(app.S, "bots/bot_a1");
    const group = path.join(app.K, "groups/grp_team");
    write(path.join(local, "workspace/notes.md"), "bot work");
    write(path.join(app.S, "bots.json"), JSON.stringify([
      { id: "bot_a1", name: "Scout", job: "Stay watchful", folder: local, sessionID: "ses_local", created: date, agents: true },
      { id: "bot_b2", name: "Other", folder: "/remote/bot_b2", sessionID: "ses_remote", machine: "mini" },
    ]));
    write(path.join(app.S, "groups.json"), JSON.stringify([{
      id: "grp_team", name: "Team", members: ["bot_a1", "bot_b2"], created: date,
      seats: { bot_a1: "local", bot_b2: "remote" }, heard: { bot_a1: "first", bot_b2: "first" },
      agentSeats: ["remote"],
    }]));
    write(path.join(app.S, "groups/grp_team.json"), JSON.stringify([
      { id: "first", author: null, text: "Ready", time: date },
      { id: "malformed", text: "Unreadable", time: "yesterday" },
    ]));

    const reply = move(0);
    assert.equal(reply.result, "done");
    assert.ok(reply.status.dropped.includes("One message in your group chats couldn't be read, and wasn't brought along."));
    assert.deepEqual(readJSON(path.join(app.K, "bots/bot_a1/bot.json")), {
      created: date, id: "bot_a1", job: "Stay watchful", kuvo: 1, name: "Scout", updated: date,
    });
    assert.equal(fs.readFileSync(path.join(app.K, "bots/bot_a1/session"), "utf8"), "ses_local");
    assert.equal(fs.readFileSync(path.join(app.K, "bots/bot_a1/directory"), "utf8"), path.join(app.K, "bots/bot_a1/workspace"));
    assert.equal(fs.readFileSync(path.join(app.K, "bots/bot_a1/via"), "utf8"), "bridge");
    assert.equal(readJSON(path.join(app.K, "bots.json"))[0].folder, path.join(app.K, "bots/bot_a1"));
    assert.deepEqual(readJSON(path.join(group, "group.json")), {
      kuvo: 1, status: "active", group: {
        id: "grp_team", name: "Team", members: ["bot_a1", "bot_b2"], created: date,
        seats: { bot_a1: "local" }, heard: { bot_a1: "first" }, agentSeats: [],
      },
    });
    assert.deepEqual(readJSON(path.join(group, "messages.json")), [
      { id: "first", text: "Ready", time: date, kind: "message" },
    ]);
    assert.equal(fs.existsSync(path.join(app.K, "groups/grp_team.json")), false);
    assert.equal(readJSON(path.join(app.S, "groups/grp_team.json")).length, 2);
  });

  await t.test("two attachments with the same name both survive and the message finds its copy", () => {
    reset();
    write(path.join(app.S, "box/attachments/ses_a/b.png"), "the box's own b");
    assert.equal(move(0).result, "done");
    const attachment = path.join(app.K, "attachments/ses_a/b 2.png");
    assert.equal(fs.readFileSync(path.join(app.K, "attachments/ses_a/b.png"), "utf8"), "the box's own b");
    assert.equal(fs.readFileSync(attachment, "utf8"), "PNGDATA");
    const db = path.join(app.K, "data/opencode/opencode.db");
    const query = spawnSync("/usr/bin/sqlite3", [db,
      "SELECT data FROM session_message WHERE json_extract(data, '$.text') = 'see attached' LIMIT 1;",
    ], { env, encoding: "utf8" });
    assert.equal(query.status, 0, query.stderr);
    assert.equal(JSON.parse(query.stdout).files[0].source.uri, pathToFileURL(attachment).href);
    assert.equal(fs.readFileSync(path.join(app.S, "Attachments/ses_a/b.png"), "utf8"), "PNGDATA");
  });

  await t.test("old Usage preferences fill only the settings Kuvo has never saved", () => {
    reset();
    const preferences = path.join(app.home, "Library/Preferences", path.basename(app.S) + ".plist");
    write(preferences, `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>showTotalSpend</key><false/>
<key>openusage.notifications.underTenPercent</key><true/>
<key>openusage.codex.fallbackModel</key><string>app-choice</string>
</dict></plist>`);
    const settings = "agents/usage/settings.json";
    write(path.join(app.S, settings), '{"codex":{"fallback":"saved-choice"}}');
    assert.equal(move(0).result, "done");
    const saved = readJSON(path.join(app.K, settings));
    assert.deepEqual(saved.codex, { fallback: "saved-choice" });
    assert.equal(saved.imported, true);
    assert.equal(saved.kuvo, 1);
    assert.deepEqual(saved.units.totalSpend.value, { show: false });
    assert.deepEqual(saved.units.notifications.value, {
      underTenPercent: true, healthyToClose: false, closeToRunningOut: false,
    });
    assert.equal(saved.units.totalSpend.stamp.imported, true);
  });

  await t.test("pending and seen letters carry forward together with the delivery warning", () => {
    reset();
    const peer = "A".repeat(43);
    const at = 2_000_000_000_000;
    write(path.join(app.S, "phone-link.json"), JSON.stringify({ machines: { mini: { peers: true, key: peer } } }));
    write(path.join(app.S, "agent-letters.json"), JSON.stringify([
      { id: "ltr_valid", machine: "mini", from: "ses_one", to: "ses_two", text: "ready", at },
      { id: "ltr_away", machine: "offline", to: "ses_elsewhere", text: "unreachable", at },
    ]));
    write(path.join(app.S, "agent-letters-seen.json"), JSON.stringify({
      ltr_valid: at - 1, ltr_other: at, ltr_expired: 1,
    }));
    write(path.join(app.S, "box/run/letters-seen.json"), JSON.stringify({ ltr_valid: at }));
    const reply = move(0);
    assert.deepEqual(readJSON(path.join(app.K, "run/outbox/sending/ltr_valid.json")), {
      from: "ses_one", to: "ses_two", text: "ready", wait: false, at, peer,
    });
    assert.deepEqual(readJSON(path.join(app.K, "run/letters-seen.json")), { ltr_valid: at, ltr_other: at });
    assert.equal(fs.existsSync(path.join(app.K, "run/outbox/sending/ltr_away.json")), false);
    assert.ok(reply.status.dropped.includes("A message one of your agents sent to an agent on a machine that was away wasn't delivered, and isn't brought along."));
  });

  await t.test("an older pending message keeps its reply reference and valid Unicode at the size limit", () => {
    reset();
    const peer = "A".repeat(43);
    const text = "a".repeat(31_999) + "😀";
    const pending = { from: "ses_one", to: "ses_two", text, machine: "mini", at: 2_000_000_000_000, answers: "ltr_reply" };
    write(path.join(app.S, "phone-link.json"), JSON.stringify({ machines: { mini: { peers: true, key: peer } } }));
    write(path.join(app.S, "agent-messages.json"), JSON.stringify([pending]));
    assert.equal(move(0).result, "done");
    const id = "wtg_" + createHash("sha256").update(`${pending.from}\0${pending.to}\0${text}`).digest("hex").slice(0, 32);
    const { text: carried, ...letter } = readJSON(path.join(app.K, `run/outbox/sending/${id}.json`));
    assert.equal(carried.length, 31_999);
    assert.equal(/^a+$/.test(carried), true);
    assert.deepEqual(letter, {
      from: pending.from, to: pending.to, wait: false,
      at: pending.at, peer, answers: pending.answers,
    });
  });

  await t.test("settings left with the old app are named once in the move receipt", () => {
    reset();
    const preferences = path.join(app.home, "Library/Preferences", path.basename(app.S) + ".plist");
    const prop = {
      composerDrafts: { "ses_one": "draft", "ses_two": "another draft" },
      botEditsWaiting: { "bot_one": {} }, botEditOutbox: { "bot_one": {} },
      movesUnderWay: [{ stage: "switched" }, { stage: "copying" }],
      machineSignOuts: { mini: {} }, "machines.waiting": { mini: {}, tower: {} },
      projectSettings: { "ssh://mini/work": { terminal: "make shell" } },
      projectFolders: ["ssh://mini/work"], worktrees: { "ssh://mini/work/tree": {} },
      "machines.unreadable-1": {}, "kuvo.usage.connectedProviders.v1": ["Z.AI"],
      lastModel: "the-old-default", "notifications.badge": false, "analytics.enabled": false,
    };
    const encoded = Object.entries(prop).map(([key, value]) => {
      const data = typeof value === "boolean" ? `<${value}/>` : `<data>${Buffer.from(JSON.stringify(value)).toString("base64")}</data>`;
      return `<key>${key}</key>${data}`;
    }).join("\n");
    write(preferences, `<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>${encoded}</dict></plist>`);
    write(path.join(app.S, "Browser/history.json"), "[]");
    write(path.join(app.S, "projects.unreadable-1.json"), "earlier data");
    write(path.join(app.S, "Usage/proxy.json"), "earlier usage settings");
    const { status } = move(0);
    assert.deepEqual(status.dropped.filter(line => !line.startsWith("Passwords the earlier Kuvo Code")), [
      "2 messages you'd typed but not sent weren't brought along.",
      "Websites you were signed in to in Kuvo Code's browser need signing in again. Your history comes along.",
      "A change you made to a bot on a machine that was away (a name, look, personality or model) wasn't sent there. Make it again once that machine is connected.",
      "The command that opened in place of the chat for a project isn't kept.",
      "A move to another computer that hadn't finished cleaning up left its original folder where it was.",
      "You signed out of an account while a machine was away: it may still be signed in to it. Sign out there too once it's connected.",
      "2 machines that were away still have files of something you removed here (a bot, or Kuvo Code itself): remove them from there once connected.",
      "A project on your machines may have lost its name, extra folders or Run command, which the earlier Kuvo Code kept on this Mac. Set them again in each project's settings.",
      "A worktree on your machines may show its sessions under another project: the earlier Kuvo Code kept which project it belonged to on this Mac.",
      "The earlier Kuvo Code had set aside a list of machines it couldn't read. It isn't brought along: add any machine that's missing again.",
      "An API key you pasted into the usage panel (OpenRouter's or Z.ai's) isn't brought along. Paste it again in the usage panel.",
      "The model new sessions start with is chosen again in the new app.",
      "You had turned off some of Kuvo Code's notifications (or their sound or badge). Set that again in the new app.",
      "You had turned off sharing anonymous usage in the earlier Kuvo Code. That choice isn't brought along: check it in the new app's settings.",
      "The earlier Kuvo Code had set aside a list it couldn't read. It's still in its folder (a file named …unreadable…json).",
      "API keys or a proxy you set up by hand for the usage panel (files in the earlier Kuvo Code's Usage folder) aren't brought along.",
    ]);
    assert.deepEqual(readJSON(path.join(app.K, "updates/rehome/state.json")).dropped, status.dropped);
    assert.equal(fs.readFileSync(path.join(app.S, "projects.unreadable-1.json"), "utf8"), "earlier data");
  });

  await t.test("settling the copied keys and previously imported sign-ins asks for no passwords", () => {
    reset();
    const oldKey = path.join(app.keychain, "app/phone-link.key");
    const bytes = fs.readFileSync(oldKey);
    assert.equal(move(0).result, "done");
    assert.equal(fs.existsSync(oldKey), false);
    const back = command([".kuvo-code", "--back"], 1);
    assert.equal(back.result, "refused");
    assert.equal(back.message, "Kuvo on this Mac was moved from an earlier Kuvo Code, whose own folder is as it was: there's nothing to move back.");
    const state = path.join(app.K, "updates/rehome/state.json");
    write(oldKey, bytes);
    write(state, JSON.stringify({ ...readJSON(state), owed: ["keychain", "passwords"] }));
    const pref = path.join(app.home, "Library/Preferences", path.basename(app.S) + ".plist");
    write(pref, '<plist version="1.0"><dict><key>passwords.movedToVault</key><true/></dict></plist>');
    const reply = command([".kuvo-code", "--settle"], 0);
    assert.equal(reply.result, "done");
    assert.deepEqual(reply.status.owed, []);
    assert.equal(fs.existsSync(oldKey), false);
    const calls = fs.readFileSync(path.join(app.home, "keys-asked"), "utf8").split("\n");
    assert.ok(!calls.includes("passwords"));
  });

  await t.test("added coding-agent accounts keep history under the moved chat's path", () => {
    reset();
    const before = path.join(app.S, "Chats/c1");
    const after = path.join(app.K, "chats/c1");
    const grok = "agents/grok/accounts/test/sessions";
    const cursor = "agents/cursor/accounts/test/agents/agents.ndjson";
    const external = path.join(app.home, ".grok/sessions");
    const claude = path.join(app.home, ".claude/projects");
    const slug = value => value.replace(/[^A-Za-z0-9]/g, "-");
    const transcript = "thread.jsonl";
    write(path.join(app.S, grok, encodeURIComponent(before), transcript), "{}\n");
    write(path.join(app.S, grok, "session_search.sqlite"), "old cache");
    write(path.join(app.S, cursor), JSON.stringify({ cwd: before }) + "\n");
    write(path.join(external, encodeURIComponent(before), transcript), "{}\n");
    write(path.join(external, "session_search.sqlite"), "old cache");
    write(path.join(claude, slug(before), transcript), "{}\n");
    assert.equal(move(0).result, "done");
    assert.equal(fs.readFileSync(path.join(app.K, grok, encodeURIComponent(after), transcript), "utf8"), "{}\n");
    assert.equal(fs.existsSync(path.join(app.K, grok, "session_search.sqlite")), false);
    assert.equal(fs.readFileSync(path.join(app.K, cursor), "utf8"), JSON.stringify({ cwd: after }) + "\n");
    assert.equal(fs.readFileSync(path.join(app.S, grok, encodeURIComponent(before), transcript), "utf8"), "{}\n");
    assert.equal(fs.readFileSync(path.join(external, encodeURIComponent(after), transcript), "utf8"), "{}\n");
    assert.equal(fs.existsSync(path.join(external, "session_search.sqlite")), false);
    assert.equal(fs.existsSync(path.join(external, encodeURIComponent(before))), false);
    assert.equal(fs.readFileSync(path.join(claude, slug(after), transcript), "utf8"), "{}\n");
    assert.equal(fs.existsSync(path.join(claude, slug(before))), false);
  });

  await t.test("the pinned engine validates every page of active and archived sessions", {
    skip: !process.env.KUVO_TEST_ENGINE && "Supply the verified engine executable to exercise its pagination",
  }, () => {
    reset();
    const version = readJSON(path.join(app.release, "box.json")).engine;
    fs.copyFileSync(process.env.KUVO_TEST_ENGINE, path.join(app.engine, version, "opencode"));
    const sqlite = (file, sql) => {
      const result = spawnSync("/usr/bin/sqlite3", ["-json", file, sql], { env, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim() ? JSON.parse(result.stdout) : null;
    };
    const old = path.join(app.S, "opencode/data/opencode/opencode.db");
    const columns = sqlite(old, "PRAGMA table_info(session_v2);").map(column => column.name);
    const expressions = {
      id: "printf('ses_e2c1%04dffeMoreSessions%04d', n, n)",
      slug: "printf('more-%d', n)",
      time_updated: "1794556100000 + n",
      time_archived: "CASE WHEN n % 50 = 0 THEN 1794556200000 END",
    };
    sqlite(old, `WITH RECURSIVE pages(n) AS (VALUES(1) UNION ALL SELECT n + 1 FROM pages WHERE n < 250)
INSERT INTO session_v2 (${columns.map(name => `"${name}"`).join(", ")})
SELECT ${columns.map(name => expressions[name] ?? `seed."${name}"`).join(", ")}
FROM pages CROSS JOIN (SELECT * FROM session_v2 ORDER BY id LIMIT 1) AS seed;`);
    assert.equal(move(0).result, "done");
    const moved = path.join(app.K, "data/opencode/opencode.db");
    assert.deepEqual(sqlite(moved, "SELECT count(*) AS total FROM session_v2"), [{ total: 255 }]);
  });
});
