// Local clients discover and authenticate to a resident, follow its line, and reattach on restart.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { openLine, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { parkedTimers } from "./idle-counter.mjs";

test("a pristine resident parks its idle services and creates the artifact store on use", { timeout: 95000 }, async t => {
  const fixture = await parkedTimers(t, ["bots.held", "peers.waiting", "engine.lifetime", "agents.sign-in-expiry", "backup.daily", "engine.queue-limit", "work.late-answer", "agents.tool-expiry"], 65000);
  if (!fixture) return;
  const { home, record } = fixture;
  assert.equal(fs.existsSync(path.join(home, "artifacts")), false);
  assert.equal(fs.existsSync(path.join(home, "run/work/background.json")), false);
  const artifacts = await request(record, "GET", "kuvo/artifacts/storage", undefined, { "x-kuvo-from": "app" });
  assert.equal(artifacts.status, 200, artifacts.text);
  assert.ok(fs.existsSync(path.join(home, "artifacts/key")));
});

test("idle followed lists wait for changes instead of periodic reads", { timeout: 30000 }, async t => {
  await parkedTimers(t, ["agents.background", "bots.follow", "work.follow", "work.at-work-follow", "engine.waiting-follow", "wakes.follow", "work.asks-follow", "artifacts.follow"], 6500, {
    bots: "kuvo/bots/events", work: "kuvo/work/events", background: "kuvo/work/background/events",
    waiting: "kuvo/waiting/events", wakes: "kuvo/wakes/events", asks: "kuvo/asks/events", menu: "kuvo/box/at-work/events",
    library: "kuvo/setup/library/events",
  });
});

test("followed work and background answer external records and the actual mark expiry", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("Followed records");
  const record = await here.start().ready();
  const topics = { background: "kuvo/work/background/events", marks: "kuvo/work/oversight/events" };
  const watch = Buffer.from(JSON.stringify(topics)).toString("base64url");
  const line = await openLine(record, "?watch=" + watch, "kuvo/line", { "x-kuvo-from": "app" }, 40000);
  t.after(() => line.close());
  assert.equal(line.status, 200);
  const frames = line.frames();
  const seen = new Map();
  const expect = async (topic, state, matches) => {
    while (!matches(seen.get(topic))) {
      const { value, done } = await frames.next().catch(async error => {
        const current = await request(record, "GET", topics[topic].replace(/\/events$/, ""));
        throw new Error(`follow timed out waiting for ${topic}: ${state}; last value ${JSON.stringify(seen.get(topic))}; current ${current.text}`, { cause: error });
      });
      assert.ok(!done, `follow stopped before ${topic}: ${state}`);
      if (value?.t) seen.set(value.t, value.d);
    }
  };
  await expect("background", "empty", row => row?.sessions && !Object.keys(row.sessions).length);
  await expect("marks", "empty", row => row?.marks?.length === 0);
  const work = path.join(here.home, "run/work");
  const background = path.join(work, "background.json");
  // The engine and agent runtime each own their place in this shared file.
  // Act as another writer, using the same lock and atomic replacement, rather
  // than racing the resident's initial publication of its engine place.
  const saveBackground = async place => {
    fs.mkdirSync(work, { recursive: true });
    const lock = path.join(work, ".background.lock");
    await until(() => {
      try { fs.mkdirSync(lock); return true; }
      catch (error) { if (error.code !== "EEXIST") throw error; return false; }
    }, "the background writer lock");
    try {
      const places = fs.existsSync(background) ? JSON.parse(fs.readFileSync(background, "utf8")) : {};
      if (place) places.external = place;
      else delete places.external;
      if (Object.keys(places).length) {
        write(background + ".stage", JSON.stringify(places));
        fs.renameSync(background + ".stage", background);
      } else {
        fs.rmSync(background, { force: true });
      }
    } finally {
      fs.rmdirSync(lock);
    }
  };
  await saveBackground({ sessions: { ses_followed: {
    commands: [{ id: "shl_followed", command: "build", started: Date.now() }],
  } }, folders: [] });
  await expect("background", "added command", row => row?.sessions?.ses_followed?.commands === 1);
  await saveBackground(null);
  await expect("background", "removed command", row => row?.sessions && !Object.keys(row.sessions).length);
  write(path.join(work, "oversight.json"), JSON.stringify([{
    id: "oversight-followed", bot: "bot_followed", session: "ses_followed", kind: "started", at: Date.now() - 54000,
  }]));
  await expect("marks", "added mark", row => row?.marks?.some(mark => mark.id === "oversight-followed"));
  await expect("marks", "expired mark", row => row?.marks?.length === 0);
});

test("resident publishes usable credentials for kuvo/info", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("info");
  const record = await here.start().ready();
  const denied = await request({ ...record, password: "bad password" }, "GET", "kuvo/info");
  assert.equal(denied.status, 401);
  const info = await request(record, "GET", "kuvo/info");
  assert.equal(info.status, 200);
  assert.equal(info.json.kind, "machine");
  assert.equal(info.json.chats, path.join(here.home, "chats"));
  assert.ok(info.json.features.includes("info"));
});

test("resident has one owner, streams numbered changes and publishes new credentials after restart", { timeout: 90000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("resident");
  const first = here.start();
  const record = await first.ready();
  const duplicate = here.start();
  const refused = await duplicate.exited;
  assert.equal(refused.code, 1, duplicate.log());
  const watch = Buffer.from(JSON.stringify({ example: "api/event" })).toString("base64url");
  const line = await openLine(record, "?watch=" + watch);
  try {
    assert.equal(line.status, 200);
    assert.match(line.type, /ndjson/);
    assert.equal(Number(fs.readFileSync(path.join(here.home, "run", "link.pid"), "utf8")), record.pid);
    const frames = line.frames();
    const { value: { hello } } = await frames.next();
    assert.equal(hello.fresh, true);
    assert.equal(typeof hello.line, "string");
    const { value: initial } = await frames.next();
    assert.deepEqual(initial, { t: "example", d: { type: "server.connected" }, n: 1 });
    here.engine.emit({ type: "conformance.event", data: { text: "Hello" } });
    const changed = { t: "example", d: { type: "conformance.event", data: { text: "Hello" } }, n: 2 };
    assert.deepEqual((await frames.next()).value, changed);
    const resumed = await openLine(record, `?watch=${watch}&line=${hello.line}&after=1`);
    try {
      const replay = resumed.frames();
      assert.deepEqual((await replay.next()).value.hello, { ...hello, fresh: false });
      assert.deepEqual((await replay.next()).value, changed);
    } finally {
      resumed.close();
    }
  } finally {
    line.close();
  }
  const stopped = await first.stop();
  if (process.platform === "win32") {
    // Node's Windows SIGTERM forcibly terminates the child; unlike a Unix
    // signal it cannot run the resident's shutdown handler. Reopening a home
    // with that stale discovery record is the Windows restart case here.
    assert.equal(stopped.signal, "SIGTERM", first.log());
  } else {
    assert.equal(stopped.code, 0, first.log());
    assert.equal(fs.existsSync(path.join(here.home, "run", "serve.json")), false);
  }
  const restarted = await here.start().ready();
  assert.notEqual(restarted.password, record.password);
  assert.equal((await request(restarted, "GET", "kuvo/info")).status, 200);
});
