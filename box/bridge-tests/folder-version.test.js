import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { layout4Build } from "./failure-bench-legacy.js";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

const runner = new URL("../src/shared/folder-version.ts", import.meta.url).href;
const link = fileURLToPath(LINK_BUNDLE);
const agents = fileURLToPath(new URL("../dist/kuvo-agents.js", import.meta.url));
const bun = process.env.KUVO_TEST_BUN || "bun";

function scratch() {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-layout-"));
  removeAtEnd(home);
  return home;
}

function launch(home, program, args, options = {}) {
  return spawnSync(program, args, {
    ...options,
    env: { ...process.env, HOME: home, KUVO_HOME: path.join(home, ".kuvo-code"), ...options.env },
    encoding: "utf8",
    timeout: 15_000,
  });
}

test("folder layout 23 upgrades layout 9 in place and the current layout opens read-only", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  const raw = '{"old":"shape"}\n';
  fs.writeFileSync(path.join(folder, "machines.json"), raw);
  const marker = path.join(folder, "folder-version.json");
  fs.writeFileSync(marker, '{"version":9}\n');
  const result = launch(home, process.execPath, [link, "folder-upgrade"]);
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
  assert.equal(fs.readFileSync(path.join(folder, "machines.json"), "utf8"), raw);
  assert.deepEqual(fs.readdirSync(folder).sort(), ["folder-version.json", "machines.json"]);
  const inode = fs.statSync(marker).ino;
  const reopened = launch(home, process.execPath, [link, "folder-upgrade"]);
  assert.equal(reopened.status, 0, reopened.stderr);
  assert.equal(fs.statSync(marker).ino, inode);
});

test("layout 11 preserves device keys, pairing IDs, and signed records through migration", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const run = path.join(folder, "run");
  fs.mkdirSync(run, { recursive: true });
  const marker = path.join(folder, "folder-version.json");
  const file = path.join(run, "link.json");
  const independent = { key: "phone", pairing: "same-phone-pairing", from: "pair" };
  const desktop = { key: "app", pairing: "same-desktop-pairing", from: "mac", kind: "desktop", mac: "primary" };
  const removed = [{ key: "old-device", pairing: "old-pairing" }];
  const signed = { signed: { d: "original-document", s: "its-original-signature" } };
  fs.writeFileSync(marker, '{"version":10,"upgradingTo":11}\n');
  fs.writeFileSync(file, JSON.stringify({
    formatVersion: 1, phones: [independent, { ...desktop, by: "member" }],
    removed, docs: signed, vouchBlocked: [{ key: "member", device: "Old" }],
  }));
  for (let attempt = 0; attempt < 2; attempt++) {
    const result = launch(home, process.execPath, [link, "folder-upgrade"]);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
    assert.deepEqual(JSON.parse(fs.readFileSync(file, "utf8")), { formatVersion: 1, phones: [independent, desktop], removed, docs: signed });
  }
});

test("failure bench: layout 17 removes only rank history from the active replica", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const run = path.join(folder, "run");
  fs.mkdirSync(run, { recursive: true });
  const marker = path.join(folder, "folder-version.json");
  const file = path.join(run, "link.json");
  const value = { successorOrder: ["pinned", "next"], successorPin: "pinned" };
  const shared = { membershipAndAccess: { sequence: 25, value }, setupNames: { sequence: 2, value: { pinned: "Desk" } } };
  const expected = { formatVersion: 1, shared, leadership: { 2: { d: "original", s: "signature" } },
    sharedPending: { operation: { d: "pending", s: "unchanged" } } };
  const replica = { ...shared, membershipAndAccess: { sequence: 25, value: { ...value, successorRevisions: [{ order: ["next", "pinned"] }] } } };
  fs.writeFileSync(marker, '{"version":16,"upgradingTo":17}\n');
  fs.writeFileSync(file, JSON.stringify({ ...expected, shared: replica }));
  for (let attempt = 0; attempt < 2; attempt++) {
    const result = launch(home, process.execPath, [link, "folder-upgrade"]);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
    assert.deepEqual(JSON.parse(fs.readFileSync(file, "utf8")), expected);
  }
});

test("layout 18 resumes its checkpoint without rewriting live handover or legacy session data", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const marker = path.join(folder, "folder-version.json");
  const files = [
    ["run/link.json", '{"formatVersion":1,"phones":[]}'],
    ["run/agents/handover.json", '{"formatVersion":1,"from":"old","to":"new"}'],
    ["agents/sessions/old/session.json", '{"id":"old","title":"Kept"}'],
  ];
  for (const [relative, raw] of files) {
    const file = path.join(folder, relative);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, raw);
  }
  for (const raw of ['{"version":17}', '{"version":17,"upgradingTo":18}']) {
    fs.writeFileSync(marker, raw);
    const result = launch(home, process.execPath, [link, "folder-upgrade"]);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
    for (const [relative, expected] of files) {
      assert.equal(fs.readFileSync(path.join(folder, relative), "utf8"), expected);
    }
  }
});

test("failure bench: layout 19 resumes an interrupted upgrade without touching existing box data", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(path.join(folder, "run"), { recursive: true });
  const state = path.join(folder, "run", "link.json");
  const raw = '{"formatVersion":1,"phones":[],"macPeers":[],"unrecognizedLegacyField":true}';
  fs.writeFileSync(state, raw);
  const marker = path.join(folder, "folder-version.json");
  for (const from of [{ version: 18 }, { version: 18, upgradingTo: 19 }]) {
    fs.writeFileSync(marker, JSON.stringify(from));
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = launch(home, process.execPath, [link, "folder-upgrade"]);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
      assert.equal(fs.readFileSync(state, "utf8"), raw);
    }
  }
});

test("failure bench: layout 20 replays without changing saved installer invitations", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(path.join(folder, "run"), { recursive: true });
  const state = path.join(folder, "run", "link.json");
  const raw = '{"formatVersion":1,"installerInvites":{"existing":{"expires":2000000000000}},"phones":[]}';
  fs.writeFileSync(state, raw);
  const marker = path.join(folder, "folder-version.json");
  for (const from of [{ version: 19 }, { version: 19, upgradingTo: 20 }]) {
    fs.writeFileSync(marker, JSON.stringify(from));
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = launch(home, process.execPath, [link, "folder-upgrade"]);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
      assert.equal(fs.readFileSync(state, "utf8"), raw);
    }
  }
});

test("failure bench: layout 21 resumes with legacy sessions intact and preserves the local wipe record", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const session = path.join(folder, "data", "opencode", "legacy-session");
  const wipe = path.join(home, ".kuvo-code-removal-receipts", "last-setup-wipe");
  fs.mkdirSync(path.dirname(session), { recursive: true });
  fs.mkdirSync(path.dirname(wipe), { mode: 0o700 });
  fs.writeFileSync(session, "legacy session bytes");
  const record = JSON.stringify({ setup: `set_${"A".repeat(22)}`, operation: "a".repeat(32) });
  fs.writeFileSync(wipe, record, { mode: 0o600 });
  const marker = path.join(folder, "folder-version.json");
  for (const from of [{ version: 20 }, { version: 20, upgradingTo: 21 }]) {
    fs.writeFileSync(marker, JSON.stringify(from));
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = launch(home, process.execPath, [link, "folder-upgrade"]);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
      assert.equal(fs.readFileSync(session, "utf8"), "legacy session bytes");
      assert.equal(fs.readFileSync(wipe, "utf8"), record);
    }
  }
});

test("failure bench: layout 22 replays without changing legacy SSH destinations", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  const machines = path.join(folder, "machines.json");
  const list = '[{"destination":"work-dev","name":"Work"}]\n';
  fs.writeFileSync(machines, list);
  const marker = path.join(folder, "folder-version.json");
  for (const from of [{ version: 21 }, { version: 21, upgradingTo: 22 }]) {
    fs.writeFileSync(marker, JSON.stringify(from));
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = launch(home, process.execPath, [link, "folder-upgrade"]);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
      assert.equal(fs.readFileSync(machines, "utf8"), list);
    }
  }
});

test("failure bench: layout 23 resumes without changing the legacy Work Queue ledger or sessions", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const ledger = path.join(folder, "run", "work-queue", "numbers.json");
  const session = path.join(folder, "agents", "sessions", "ses_before", "session.json");
  for (const file of [ledger, session]) fs.mkdirSync(path.dirname(file), { recursive: true });
  const numbers = '{"primary":"self","next":59,"leases":[[40,50]]}\n';
  const data = '{"id":"ses_before","title":"Kept here"}\n';
  fs.writeFileSync(ledger, numbers);
  fs.writeFileSync(session, data);
  const marker = path.join(folder, "folder-version.json");
  for (const from of [{ version: 22 }, { version: 22, upgradingTo: 23 }]) {
    fs.writeFileSync(marker, JSON.stringify(from));
    for (let attempt = 0; attempt < 2; attempt++) {
      const result = launch(home, process.execPath, [link, "folder-upgrade"]);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(fs.readFileSync(marker, "utf8")), { version: 23 });
      assert.equal(fs.readFileSync(ledger, "utf8"), numbers);
      assert.equal(fs.readFileSync(session, "utf8"), data);
    }
  }
});

test("failure bench: a folder upgrade killed after step 1 resumes from step 2", () => {
  const folder = scratch();
  const run = kill => spawnSync(bun, ["--config=/dev/null", "--no-env-file", "-e", `
    import { appendFileSync } from "node:fs";
    import { upgradeFolder } from ${JSON.stringify(runner)};
    upgradeFolder(${JSON.stringify(folder)}, [
      folder => appendFileSync(folder + "/applied", "1"),
      folder => {
        if (${kill}) process.kill(process.pid, "SIGKILL");
        appendFileSync(folder + "/applied", "2");
      },
    ]);
  `], { encoding: "utf8", timeout: 15_000 });
  const marker = path.join(folder, "folder-version.json");
  const read = () => JSON.parse(fs.readFileSync(marker, "utf8"));
  const killed = run(true);
  assert.equal(killed.signal, "SIGKILL", killed.stderr);
  assert.deepEqual(read(), { version: 1, upgradingTo: 2 });
  assert.equal(fs.readFileSync(path.join(folder, "applied"), "utf8"), "1");
  const restarted = run(false);
  assert.equal(restarted.status, 0, restarted.stderr);
  assert.deepEqual(read(), { version: 2 });
  assert.equal(fs.readFileSync(path.join(folder, "applied"), "utf8"), "12");
  assert.deepEqual(fs.readdirSync(folder).sort(), ["applied", "folder-version.json"]);
});

test("failure bench: startup refuses a newer folder without changing it", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  const marker = path.join(folder, "folder-version.json");
  for (const raw of ['{"version":24}\n', '{"version":23,"upgradingTo":24}\n']) {
    fs.writeFileSync(marker, raw);
    const result = launch(home, process.execPath, [link, "serve"]);
    assert.equal(result.status, 1, result.stderr);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /Kuvo folder at .* format version 24 is unsupported \(this build writes 23\)/);
    const route = launch(home, process.execPath, [link, "box", "route"], { input: '{"method":"GET","path":"kuvo/info"}' });
    assert.equal(route.status, 0, route.stderr);
    assert.deepEqual(JSON.parse(route.stdout), {
      status: 409, routeVersion: "1", body: JSON.stringify({ code: "unsupportedFormatVersion", message: result.stderr.trim() }),
    });
    assert.equal(fs.readFileSync(marker, "utf8"), raw);
    assert.deepEqual(fs.readdirSync(folder), ["folder-version.json"]);
  }
});

test("startup refuses an unreadable marker and preserves its bytes", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  const marker = path.join(folder, "folder-version.json");
  fs.writeFileSync(marker, "{");
  const result = launch(home, process.execPath, [link, "folder-upgrade"]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Kuvo folder at .* format version invalid is unsupported/);
  assert.equal(fs.readFileSync(marker, "utf8"), "{");
  assert.deepEqual(fs.readdirSync(folder), ["folder-version.json"]);
});


test("the agents bridge refuses the newer layout before initializing its store", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  fs.mkdirSync(folder);
  fs.writeFileSync(path.join(folder, "folder-version.json"), '{"version":24}');
  const result = launch(home, bun, [agents], {
    env: { KUVO_AGENTS_HOME: path.join(folder, "agents"), OPENCODE_PASSWORD: "test" },
  });
  assert.equal(result.status, 1, result.stderr);
  assert.equal(result.stdout, "");
  assert.match(result.stderr, /Kuvo folder at .* format version 24 is unsupported \(this build writes 23\)/);
  assert.deepEqual(fs.readdirSync(folder), ["folder-version.json"]);
});

test("keeper and engine launcher refuse the newer layout before making runtime folders", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const engine = path.join(folder, "engine", "test");
  fs.mkdirSync(engine, { recursive: true });
  fs.mkdirSync(path.join(folder, "bin"));
  fs.copyFileSync(link, path.join(folder, "bin", "kuvo-link.js"));
  // Only run the preflight. Even if the guard regresses, the stand-in cannot start a real engine.
  fs.writeFileSync(path.join(engine, "opencode"), `#!/bin/sh
[ "$4" = folder-upgrade ] || exit 77
exec ${JSON.stringify(process.execPath)} "$3" "$4"
`, { mode: 0o700 });
  fs.writeFileSync(path.join(folder, "folder-version.json"), '{"version":24}');
  for (const script of ["kuvo-start", "kuvo-launch"]) {
    const source = fs.readFileSync(new URL(`../src/keeper/${script}`, import.meta.url), "utf8");
    const staged = path.join(home, script);
    fs.writeFileSync(staged, source.replaceAll("@ENGINE@", "test"));
    // A failure to guard a startup must still fail fast and leave no detached test process.
    const result = launch(home, "/bin/sh", [staged, "status"], { input: "\n" });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /Kuvo folder at .* format version 24 is unsupported \(this build writes 23\)/);
    assert.deepEqual(fs.readdirSync(folder).sort(), ["bin", "engine", "folder-version.json"]);
  }
});

test("failure bench: a broken trial can stop and restore the saved bridge for its compatible layout", () => {
  const home = scratch();
  const folder = path.join(home, ".kuvo-code");
  const saved = path.join(folder, "updates", "rollback");
  const marker = path.join(folder, "folder-version.json");
  const current = path.join(folder, "bin", "kuvo-link.js");
  const old = path.join(saved, "files", "bin", "kuvo-link.js");
  const engine = path.join(folder, "engine", "test");
  for (const dir of [path.dirname(current), path.dirname(old), engine, path.join(folder, "run")]) {
    fs.mkdirSync(dir, { recursive: true });
  }
  // This is the shipped layout-4 bridge, and the trial is the current built bridge.
  const previous = layout4Build();
  fs.writeFileSync(old, previous);
  fs.copyFileSync(link, current);
  fs.writeFileSync(path.join(engine, "opencode"), `#!/bin/sh
shift 2
exec ${JSON.stringify(process.execPath)} "$@"
`, { mode: 0o700 });
  const keeper = path.join(folder, "bin", "kuvo-start");
  fs.writeFileSync(keeper, fs.readFileSync(new URL("../src/keeper/kuvo-start", import.meta.url), "utf8").replaceAll("@ENGINE@", "test"));
  fs.writeFileSync(path.join(folder, "run", "update-trial"), "trial");
  fs.writeFileSync(path.join(saved, "restore.sh"), `#!/bin/sh
cp "${old}" "${current}" && rm "${folder}/run/update-trial"
`);
  fs.writeFileSync(marker, '{"version":4}\n');
  assert.equal(launch(home, process.execPath, [old, "here", ".probe"]).status, 0);
  const info = launch(home, process.execPath, [current, "box", "route"], { input: '{"method":"GET","path":"kuvo/info"}' });
  assert.equal(info.status, 0, info.stderr);
  const reply = JSON.parse(info.stdout);
  assert.equal(reply.status, 200, info.stdout);
  assert.ok(JSON.parse(reply.body).features.includes("devices.allow"));
  assert.equal(fs.readFileSync(marker, "utf8"), '{"version":4}\n');
  // A trial may defer a clean layout 4, but cannot defer a migration that already started.
  fs.writeFileSync(marker, '{"version":4,"upgradingTo":5}\n');
  const refused = launch(home, process.execPath, [current, "folder-upgrade"]);
  assert.equal(refused.status, 1, refused.stderr);
  assert.match(refused.stderr, /saved Kuvo Code can't recover folder layout 23/);
  assert.equal(fs.readFileSync(marker, "utf8"), '{"version":4,"upgradingTo":5}\n');

  // An incompatible marker must never let restore put back the older binary.
  fs.writeFileSync(marker, '{"version":5}\n');
  fs.writeFileSync(current, 'throw new Error("broken trial")');
  assert.equal(launch(home, "/bin/sh", [keeper, "stop"]).status, 0);
  assert.equal(launch(home, "/bin/sh", [keeper, "rollback"]).status, 1);
  assert.match(fs.readFileSync(current, "utf8"), /broken trial/);
  assert.equal(fs.readFileSync(marker, "utf8"), '{"version":5}\n');
  // Simulate the compatible case in a fresh rollback attempt.
  fs.writeFileSync(marker, '{"version":4}\n');
  const restored = launch(home, "/bin/sh", [keeper, "rollback"]);
  assert.equal(restored.status, 0, restored.stderr);
  assert.deepEqual(fs.readFileSync(current), previous);
  assert.equal(fs.readFileSync(marker, "utf8"), '{"version":4}\n');
});

test("failure bench: the legacy import seals group files and directory entries before its checkpoint", () => {
  const home = scratch();
  const upgrades = new URL("../src/link/rehome/upgrades.ts", import.meta.url).href;
  const child = launch(home, bun, ["--config=/dev/null", "--no-env-file", "-e", `
    import assert from "node:assert/strict";
    import fs from "node:fs";
    import path from "node:path";
    import { upgradeKuvoFolder } from ${JSON.stringify(runner)};
    import { runUpgrades } from ${JSON.stringify(upgrades)};
    const home = process.env.HOME;
    const appFolder = path.join(home, "app");
    const staging = path.join(home, "staging");
    const work = path.join(staging, "work");
    const placements = ["box", "run", "bots", "groups", "agents", "config", "machines.json"].map(name => ({
      from: path.join(appFolder, name), to: path.join(home, name), staged: path.join(staging, name),
    }));
    const move = { way: "move", home, appFolder, placements, work, map: [] };
    fs.mkdirSync(path.join(appFolder, "groups"), { recursive: true });
    fs.writeFileSync(path.join(appFolder, "groups.json"), JSON.stringify([{ id: "grp_team", name: "Team", members: [] }]));
    const messages = ["first", "second"].map(id => ({ id, text: id, time: "2026-01-01T00:00:00Z" }));
    fs.writeFileSync(path.join(appFolder, "groups", "grp_team.json"), JSON.stringify(messages));
    const opened = new Map(), synced = [];
    const open = fs.openSync, sync = fs.fsyncSync;
    fs.openSync = (entry, ...args) => {
      const fd = open(entry, ...args);
      opened.set(fd, entry);
      return fd;
    };
    fs.fsyncSync = fd => {
      sync(fd);
      synced.push(opened.get(fd));
    };
    const group = path.join(staging, "groups", "grp_team");
    const rename = fs.renameSync;
    fs.renameSync = (from, to) => {
      if (to === path.join(work, "folder-version.json")
        && JSON.parse(fs.readFileSync(from)).version === 1) {
        for (const expected of ["messages.json", "group.json", ".", "..", "../.."]) {
          assert.ok(synced.includes(path.resolve(group, expected)), expected + " must reach disk before the import checkpoint");
        }
      }
      rename(from, to);
    };
    upgradeKuvoFolder(work, () => runUpgrades({ release: path.join(home, "release") }, move));
    assert.deepEqual(JSON.parse(fs.readFileSync(path.join(group, "messages.json"))).map(each => each.id), ["first", "second"]);
    assert.equal(JSON.parse(fs.readFileSync(path.join(group, "group.json"))).group.name, "Team");
    const messageSync = synced.indexOf(path.join(group, "messages.json"));
    const metadataSync = synced.indexOf(path.join(group, "group.json"));
    assert.ok(messageSync >= 0 && metadataSync > messageSync, JSON.stringify(synced));
    assert.ok(synced.slice(metadataSync + 1).includes(group));
    assert.ok(synced.slice(metadataSync + 1).includes(path.dirname(group)));
    assert.ok(synced.slice(metadataSync + 1).includes(staging));
    assert.equal(JSON.parse(fs.readFileSync(path.join(work, "folder-version.json"))).version, 23);
  `]);
  assert.equal(child.status, 0, child.stderr);
});
