// Tagged SSH transcripts and golden homes opened by the selected box binary.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { boxBinary, isolatedEnv, localRelay, privateHome, readJSON, request, scratch, startBox, relayAddress, typescriptRuntime, until, write } from "./adapter.mjs";
import { currentChecks } from "./compat/current-checks.mjs";
import { generateCurrent, generateRelease } from "./compat/generate.mjs";
import { fixed, runSync, tags } from "./compat/tagged.mjs";

function command(binary, home, env, args, input) {
  const argsWithRuntime = binary.typescript ? ["--no-env-file", binary.file, ...args] : args;
  return runSync(binary.typescript ? typescriptRuntime() : binary.file, argsWithRuntime,
    { ...env, KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" }, home, input);
}

function layout(home) {
  return readJSON(path.join(home, "folder-version.json")).version;
}

for (const tag of tags) {
  test(`a machine set up by ${tag} keeps its identity and settings`, { timeout: 60000 }, async () => {
    const root = scratch();
    const binary = boxBinary();
    const relay = await localRelay();
    let proc;
    try {
      const seed = path.join(root, "release");
      await generateRelease(tag, seed, { app: false });
      const home = path.join(seed, "machine");
      privateHome(home);
      const env = isolatedEnv(root);
      const linkFile = path.join(home, "run/link.json");
      const before = readJSON(linkFile);
      const vectors = path.join(seed, "vectors.json");
      const { setup, mail } = readJSON(vectors);
      if (binary.typescript) {
        const check = fileURLToPath(new URL("./compat/check-mail.mjs", import.meta.url));
        runSync(typescriptRuntime(), ["--no-env-file", check, binary.file, vectors], { ...env, KUVO_HOME: home, BUN_BE_BUN: "1" }, home);
      }
      write(linkFile, JSON.stringify({ ...before, relay: relay.url }));
      proc = startBox({ binary, home, env, relay });
      const info = await request(await proc.ready(), "GET", "kuvo/info");
      assert.equal(info.status, 200, info.text);
      if (tag === tags[1]) {
        const recipient = relayAddress(Buffer.from(before.secret, "base64url"));
        await until(() => relay.has(recipient), "the beta.1 machine did not connect to the relay");
        const mac = relay.device(fixed(3));
        try {
          mac.send(recipient, Buffer.from(mail.frame, "hex"));
          await until(() => readJSON(linkFile).legacy?.[JSON.parse(setup.input).mac]?.at === JSON.parse(mail.plaintext).at,
            "the newer beta.1 peers list did not arrive");
        } finally {
          mac.leave();
        }
      }
      await proc.stop();
      assert.equal(layout(home), 23);
      const after = readJSON(linkFile);
      assert.equal(after.name, "Fixture Machine");
      assert.equal(after.key, before.key);
      assert.equal(after.secret, before.secret);
      const mac = JSON.parse(setup.input).mac;
      if (mac) assert.deepEqual(after.legacy[mac], { ...before.peersFrom[mac], at: JSON.parse(mail.plaintext).at });
    } finally {
      await proc?.stop();
      await relay.close();
      rmSync(root, { recursive: true, force: true });
    }
  });

  test(`the ${tag} app folder moves its conversations and attachments`,
    { timeout: 120000, skip: process.platform !== "darwin" && "The released Mac move requires APFS and plutil." }, async () => {
      const root = scratch();
      try {
        const destination = path.join(root, "release");
        await generateRelease(tag, destination);
        const mac = readJSON(path.join(destination, "app-world.json"));
        const env = { ...isolatedEnv(root), HOME: mac.home, USERPROFILE: mac.home };
        const args = ["rehome", ".kuvo-code", "--run", "--app", mac.bundle, "--release", mac.release, "--engine", mac.engine];
        mkdirSync(mac.K, { recursive: true });
        const old = path.join(mac.S, "phone-link.json");
        const saved = readFileSync(old);
        const output = command(boxBinary(), mac.K, env, args);
        const result = JSON.parse(output.trim().split("\n").at(-1));
        assert.equal(result.result, "done", output);
        assert.equal(layout(mac.K), 23);
        assert.deepEqual(readFileSync(old), saved);
        assert.equal(readFileSync(path.join(mac.K, "box/kind"), "utf8"), "mac\n");
        assert.equal(readFileSync(path.join(mac.K, "attachments/ses_a/with space.png"), "utf8"), "PNGDATA");
        assert.equal(readFileSync(path.join(mac.K, "chats/c1/notes.md"), "utf8"), "chat");
        if (tag === tags[1]) {
          const list = readJSON(old);
          assert.deepEqual(readJSON(path.join(mac.K, "run/outbox/sending/ltr_fixture.json")), {
            from: "ses_source", to: "ses_target", text: "Keep this letter.", wait: false,
            at: 1760000000000, peer: list.machines["fixture-machine"].key,
          });
        }
        const db = new DatabaseSync(path.join(mac.K, "data/opencode/opencode.db"), { readOnly: true });
        try {
          assert.deepEqual(db.prepare("SELECT title FROM session_v2 ORDER BY title").all().map(row => row.title),
            ["chat", "chat (fork #1)", "git edit", "in worktree", "plain"]);
          assert.equal(db.prepare("SELECT count(*) AS n FROM session_message").get().n, 13);
        } finally {
          db.close();
        }
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    });
}

test("the current TypeScript beta's setup records survive opening at layout 23", { timeout: 30000 }, async () => {
  const root = scratch();
  const relay = await localRelay();
  let proc;
  try {
    const binary = boxBinary();
    const { home, env } = generateCurrent(root, binary.typescript ? binary : undefined);
    const linkFile = path.join(home, "run/link.json");
    const original = readJSON(linkFile);
    proc = startBox({ binary, home, env, relay });
    const reply = await request(await proc.ready(), "GET", "kuvo/setup");
    assert.equal(reply.status, 200, reply.text);
    assert.equal(reply.json.setup, "set_8gOLfHXdUhmcVfSh9NOsAQ");
    assert.deepEqual(reply.json.computers.map(({ name, role }) => [name, role]), [["Golden Computer", "primary"]]);
    await proc.stop();
    assert.equal(layout(home), 23);
    const link = readJSON(linkFile);
    for (const key of ["key", "computerHistory", "memberRemovals", "releasedSetup", "releasedWork",
      "releasedDatabaseWork", "releasedScopedWork"]) {
      assert.deepEqual(link[key], original[key], key);
    }
  } finally {
    await proc?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("the beta.1 Mac alone replaces its peer list, even before its address is known", { timeout: 60000 }, async () => {
  const root = scratch();
  const relay = await localRelay();
  let proc;
  const mac = relay.device(fixed(3)), second = relay.device(fixed(9));
  try {
    const seed = path.join(root, "release");
    await generateRelease(tags[1], seed, { app: false });
    const home = path.join(seed, "machine"), env = isolatedEnv(root);
    privateHome(home);
    const file = path.join(home, "run/link.json");
    const before = readJSON(file), { setup, mail, roster } = readJSON(path.join(seed, "vectors.json"));
    const key = JSON.parse(setup.input).mac;
    const at = JSON.parse(mail.plaintext).at - 1;
    before.peersFrom[key].peers = [];
    write(file, JSON.stringify({ ...before, relay: relay.url }));
    const recipient = relayAddress(Buffer.from(before.secret, "base64url"));
    const send = (device, frame) => device.send(recipient, Buffer.from(frame, "hex"));
    proc = startBox({ binary: boxBinary(), home, env, relay });
    const server = await proc.ready();
    await until(() => relay.has(recipient), "machine connects");
    const peerKeysOf = async record => {
      const result = await request(record, "GET", "kuvo/computers");
      assert.equal(result.status, 200, result.text);
      return result.json.computers.map(row => row.key).sort();
    };
    const peerKeys = async () => {
      const result = await request(server, "GET", "kuvo/computers");
      assert.equal(result.status, 200, result.text);
      return result.json.computers.map(row => row.key).sort();
    };
    const waitFor = async expected => until(async () => JSON.stringify(await peerKeys()) === JSON.stringify(expected.sort()),
      () => "unexpected computer roster: " + JSON.stringify(readJSON(file)), 10000);
    const heard = async (peer, at) => until(() => {
      const entries = path.join(home, "run/peer-agents.json");
      return existsSync(entries) && readJSON(entries)[peer]?.at === at;
    }, "peer announcement did not arrive");
    await waitFor([roster.own.key]);
    send(mac, roster.added);
    await waitFor([roster.own.key, key, roster.second.key]);
    assert.deepEqual(readJSON(file).legacy[key].peers.map(row => row.key), [key, roster.second.key, roster.own.key]);
    send(mac, roster.stale);
    await heard(key, at + 3);
    assert.equal(readJSON(file).legacy[key].at, at + 2);
    send(second, roster.notEnroller);
    await heard(roster.second.key, at + 4);
    assert.deepEqual(Object.keys(readJSON(file).legacy), [key]);
    assert.deepEqual(await peerKeys(), [roster.own.key, key, roster.second.key].sort());
    send(mac, roster.removed);
    await waitFor([roster.own.key, key]);
    assert.equal(readJSON(file).legacy[key].at, at + 3);

    // A beta.1 Mac that never updates is no dead end. The relay found it gone two minutes ago.
    mac.leave();
    await proc.stop();
    write(path.join(home, "run/peer-reach.json"), JSON.stringify({ [key]: Date.now() - 120_000 }));
    proc = startBox({ binary: boxBinary(), home, env, relay });
    const again = await proc.ready();
    const rowOf = async () => (await request(again, "GET", "kuvo/computers")).json.computers.find(row => row.key === key);
    let row;
    await until(async () => (row = await rowOf())?.reach === "away", () => "the quiet Mac isn't away: " + JSON.stringify(row));
    const unlink = () => request(again, "POST", `kuvo/computers/${encodeURIComponent(key)}/unlink`, {}, { "x-kuvo-from": "app" });
    if (boxBinary().typescript) {
      // TypeScript refused, and aged such a Mac out after 30 days of running instead.
      assert.deepEqual((await request(again, "DELETE", `kuvo/computers/${encodeURIComponent(key)}`, undefined, { "x-kuvo-from": "app" })).json,
        { message: "A Mac can't be removed from here." });
      return;
    }
    // Rust offers Remove, as the app opens it, and lets the Mac go with the computers it listed.
    const remove = row.actions.find(action => action.id === "remove");
    assert.deepEqual(remove?.removal, { choices: ["unlink"] }, JSON.stringify(row.actions));
    const removed = await unlink();
    assert.equal(removed.status, 200, removed.text);
    assert.equal(removed.json.removal.state, "unlinked");
    assert.deepEqual(await peerKeysOf(again), [roster.own.key]);
    const link = readJSON(file);
    assert.equal(link.legacy?.[key], undefined);
    assert.equal((link.macs ?? []).includes(key), false);
  } finally {
    mac.leave(); second.leave();
    await proc?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
});

// The beta.1 Mac keeps its machines' records: a rename asked on one of them is made there, never saved here.
test("a rename on a beta.1 machine is handed to the Mac that keeps its records", { timeout: 60000 }, async () => {
  const root = scratch();
  const relay = await localRelay();
  let proc;
  const mac = relay.device(fixed(3));
  try {
    const wire = await import(new URL("../../box/dist/kuvo-link.js", import.meta.url).href);
    const seed = path.join(root, "release");
    await generateRelease(tags[1], seed, { app: false });
    const home = path.join(seed, "machine"), env = isolatedEnv(root);
    privateHome(home);
    const file = path.join(home, "run/link.json");
    const before = readJSON(file), { setup, mail, roster } = readJSON(path.join(seed, "vectors.json"));
    const key = JSON.parse(setup.input).mac;
    const at = JSON.parse(mail.plaintext).at;
    write(file, JSON.stringify({ ...before, relay: relay.url }));
    const recipient = relayAddress(Buffer.from(before.secret, "base64url"));
    // The Mac signs with fixed(2) and the machine's static key is fixed(6), as in the release's vectors.
    const seal = value => wire.frame(wire.Frame.mail, 0, wire.sealOneWay(Buffer.from(JSON.stringify(value)), fixed(2),
      wire.publicOf(fixed(6)), wire.MAIL_PROLOGUE).message);
    const open = bytes => {
      try { return JSON.parse(wire.openOneWay(wire.readFrame(bytes.subarray(17)).body, fixed(2), wire.publicOf(fixed(6))).payload); }
      catch { return {}; }
    };
    proc = startBox({ binary: boxBinary(), home, env, relay });
    const server = await proc.ready();
    await until(() => relay.has(recipient), "machine connects");
    mac.send(recipient, Buffer.from(roster.added, "hex"));
    await until(async () => (await request(server, "GET", "kuvo/computers")).json?.computers?.some(row => row.key === roster.second.key),
      "the Mac's peer list did not arrive");
    const rename = (computer, name) => request(server, "PATCH", `kuvo/computers/${computer}`, { name }, { "x-kuvo-from": "mac" }, 30000);
    const isMac = await rename(key, "Studio");
    assert.equal(isMac.status, 409, isMac.text);
    assert.match(isMac.json.message, /is a Mac: it goes by the name it has there\./);
    const older = await rename(roster.second.key, "Bench");
    assert.equal(older.status, 409, older.text);
    assert.match(older.json.message, /needs a newer Kuvo Code to rename your machines\./);
    mac.send(recipient, seal({ at: at + 20, agents: { at: at + 20, list: [], features: ["computers.manage"] } }));
    await until(() => {
      const told = path.join(home, "run/peer-agents.json");
      return existsSync(told) && readJSON(told)[key]?.features?.includes("computers.manage");
    }, "the Mac's features did not arrive");
    const renamed = rename(roster.second.key, "  Bench  ");
    let act;
    while (!act) act = open(await mac.next(20000)).workQueue?.act;
    assert.deepEqual({ area: act.area, item: act.item, method: act.method, body: act.body },
      { area: "computers", item: roster.second.key, method: "PATCH", body: { name: "Bench" } });
    mac.send(recipient, seal({ at: at + 21, workQueue: { acted: { id: act.id, status: 200, value: { name: "Bench" } } } }));
    const answer = await renamed;
    assert.equal(answer.status, 200, answer.text);
    assert.deepEqual(answer.json, { name: "Bench" });
    assert.ok(!readFileSync(file, "utf8").includes("Bench"), "the name stays the Mac's to keep");
  } finally {
    mac.leave();
    await proc?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
});

// The released Mac invokes these verbs over SSH independently of the relay.
test("the beta.1 Mac reads, adopts and changes machine settings over SSH", { timeout: 60000 }, async () => {
  const root = scratch();
  let proc;
  const relay = await localRelay();
  try {
    const seed = path.join(root, "release");
    await generateRelease(tags[1], seed, { app: false });
    const binary = boxBinary(), env = isolatedEnv(root);
    const target = path.join(root, "ssh");
    privateHome(target);
    const { ssh } = readJSON(path.join(seed, "vectors.json"));
    const denied = JSON.parse(command(binary, target, env, ["box", "change"], ssh[2].input));
    assert.deepEqual(denied, { status: 409, message: "This machine's settings aren't its own yet." });
    for (const item of ssh) {
      // A Rust machine answers routes through its resident. The actual settings
      // verbs must also work between resident starts, as the released Mac uses them.
      if (item.args[1] === "route") {
        proc = startBox({ binary, home: target, env, relay });
        await proc.ready();
      }
      const actual = JSON.parse(command(binary, target, env, item.args, item.input));
      const expected = JSON.parse(item.output);
      // The historical reader decodes these fields and ignores new defaults.
      const select = (found, wanted) => Object.fromEntries(Object.keys(wanted).map(key => [key, found?.[key]]));
      const decoded = select(actual, expected);
      if (expected.settings) decoded.settings = select(decoded.settings, expected.settings);
      if (expected.body) {
        expected.body = JSON.parse(expected.body);
        decoded.body = select(JSON.parse(decoded.body), expected.body);
      }
      assert.deepEqual(decoded, expected, item.args.join(" "));
    }
    assert.equal(readFileSync(path.join(target, "box/keys/KUVO_KEY_NOTION"), "utf8"), "fixture-value");
    const repeat = JSON.parse(command(binary, target, env, ["box", "adopt"], '{"settings":{"instructions":["OLD.md"]}}'));
    assert.equal(repeat.status, "own");
    assert.deepEqual(repeat.settings.instructions, ["UPDATED.md"]);
    const erased = JSON.parse(command(binary, target, env, ["box", "key", "KUVO_KEY_NOTION"], ""));
    assert.deepEqual(erased, {status: 200});
    assert.equal(existsSync(path.join(target, "box/keys/KUVO_KEY_NOTION")), false);
  } finally {
    await proc?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("the current TypeScript beta's durable records survive opening at layout 23", { timeout: 30000 }, async () => {
  const root = scratch();
  const relay = await localRelay();
  let proc;
  try {
    const binary = boxBinary();
    const { home, directory, env } = generateCurrent(root, binary.typescript ? binary : undefined);
    const families = currentChecks(root);
    const durable = [
      "folder-version.json", "box/projects.json", "box/opencode.json", "box/time-zone.json",
      "bots/personalities.json", "run/work-queue/labels.json", "run/work-queue/queues/usr_you.json",
      "agents/usage/settings.json", "agents/sessions/ses_golden/session.json",
      "box/vault/key.json", "box/vault/items.json", "box/vault/grants.json", "box/vault/paused.json",
      "box/git.json", "box/keys/KUVO_KEY_NOTION",
      "groups/grp_golden/group.json", "groups/grp_golden/messages.json", "run/queues.json",
    ];
    const bytes = durable.map(file => readFileSync(path.join(home, file)));
    const original = readJSON(path.join(home, "run/link.json"));
    assert.equal(layout(home), 23);
    proc = startBox({ binary, home, env, relay });
    const server = await proc.ready();
    const get = async route => {
      const reply = await request(server, "GET", route);
      assert.equal(reply.status, 200, reply.text);
      return reply.json;
    };
    assert.deepEqual((await get("kuvo/projects")).projects, [{ name: "Golden Project", directory, run: "npm test", folders: [] }]);
    assert.deepEqual((await get("kuvo/personalities")).personalities,
      [{ id: "custom.golden", name: "Golden Notes", description: "Keep precise notes.", symbol: "note.text", updated: 1 }]);
    const setup = await get("kuvo/setup");
    assert.equal(setup.setup, "set_8gOLfHXdUhmcVfSh9NOsAQ");
    assert.deepEqual(setup.computers.map(({ name, role }) => [name, role]), [["Golden Computer", "primary"]]);
    const { item } = await get("kuvo/work-queue/items/wqi_golden12345678901234");
    assert.equal(item.title, "Keep this item");
    assert.equal(item.number, 1);
    assert.deepEqual((await get("kuvo/bots")).bots.map(bot => bot.name), ["Golden Bot"]);
    assert.notEqual(server.pid, 2147483647);
    const agents = readJSON(path.join(home, "run/agents.json"));
    assert.ok(Array.isArray(agents.agents), "the resident publishes the agents directory");
    await until(() => !existsSync(path.join(home, "run/outbox/sending/ltr_golden.json")),
      "the expired letter remained in the outbox");
    await proc.stop();
    const link = readJSON(path.join(home, "run/link.json"));
    assert.equal(link.key, original.key);
    assert.deepEqual(link.computerHistory, original.computerHistory);
    assert.deepEqual(link.memberRemovals, original.memberRemovals);
    for (const key of ["releasedSetup", "releasedWork", "releasedDatabaseWork", "releasedScopedWork"]) {
      assert.deepEqual(link[key], original[key], key);
    }
    for (const { family, result, entries } of families) {
      if (!result.startsWith("Preserved")) continue;
      for (const { file, bytes } of entries) assert.deepEqual(readFileSync(path.join(root, file)), bytes, family + " · " + file);
    }
    durable.forEach((file, i) => assert.deepEqual(readFileSync(path.join(home, file)), bytes[i], file));
    const transcript = new DatabaseSync(path.join(home, "agents/sessions/ses_golden/messages-v1.sqlite"), { readOnly: true });
    const insights = new DatabaseSync(path.join(home, "agents/usage/insights.sqlite"), { readOnly: true });
    try {
      const messages = transcript.prepare("SELECT body FROM message ORDER BY position").all().map(row => JSON.parse(row.body));
      assert.deepEqual(messages.map(({ id, text }) => [id, text]), [["msg_golden", "Keep this conversation."]]);
      assert.equal(insights.prepare("PRAGMA user_version").get().user_version, 1);
      assert.equal(JSON.parse(insights.prepare("SELECT body FROM state WHERE key = 'fixture'").get().body).fixture, "golden");
    } finally {
      transcript.close();
      insights.close();
    }
  } finally {
    await proc?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
});
