// Tagged SSH transcripts and golden homes opened by the selected box binary.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";
import { boxBinary, isolatedEnv, localRelay, readJSON, request, scratch, startBox, relayAddress, typescriptRuntime, until, write } from "./adapter.mjs";
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
      const env = isolatedEnv(root);
      const linkFile = path.join(home, "run/link.json");
      const before = readJSON(linkFile);
      const vectors = path.join(seed, "vectors.json");
      const { setup, ssh, mail } = readJSON(vectors);
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
      if (ssh.length) {
        // Replay the Mac's setting verbs in order on a fresh settings store.
        const target = path.join(root, "ssh");
        mkdirSync(target);
        for (const item of ssh) {
          const output = command(binary, target, env, item.args, item.input);
          const actual = JSON.parse(output);
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
      }
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
