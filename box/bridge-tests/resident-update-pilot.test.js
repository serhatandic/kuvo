import "./test-safety.js";
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { atEnd } from "./at-end.js";
import { NO_BUN } from "./bun-for-tests.js";
import { builtLink, bun, liveUpdateFixture, readJSON, runs, turns, write } from "./live-update-fixture.js";
import { relayLeaseStandIn } from "./relay-lease-stand-in.js";
import { relayStandIn } from "./relay-stand-in.js";

test("resident pilot: active sandboxed Codex survives a failed trial and a successful update", { skip: bun ? false : NO_BUN, timeout: 450_000 }, async t => {
  const link = await import(builtLink);
  const leases = relayLeaseStandIn();
  const relay = await relayStandIn(link, { request: leases.request });
  atEnd(() => relay.close());
  const secret = crypto.randomBytes(32);
  const phone = link.newKey();
  const pairing = crypto.randomBytes(16).toString("base64url");
  const boxKey = link.newKey();
  const boxPublic = link.publicOf(boxKey);
  const address = link.addressOf(secret);
  const fixture = liveUpdateFixture({ preResident: true, sandbox: true, identity: {
    id: "pilot", name: "Pilot box", key: boxKey.toString("base64url"), sign: link.newSigningSeed(), secret: secret.toString("base64url"), relay: relay.url,
    phones: [{ key: link.publicOf(phone).toString("base64url"), pairing, from: "pair", name: "Old client", added: Math.floor(Date.now() / 1000) }],
  } });
  const { home, RUN, env, box, keep, pack, bridgeUp, serveUp, linkUp, bridgeAPI, holderFor, until } = fixture;
  const { project, signals } = box();
  const installed = path.join(home, "bin", "kuvo-link.js");
  const oldClient = await import(pathToFileURL(installed).href);
  keep();
  const first = await serveUp();
  const oldLink = await linkUp();
  assert.notEqual(oldLink, first);
  await until(() => fs.existsSync(path.join(RUN, "fence", "on")), "the native sandbox to come on", 35_000);
  const bridge = await bridgeUp();
  const engine = readJSON(path.join(RUN, "engine.json")).pid;
  const { api } = bridgeAPI();
  const conversation = async title => {
    const made = await api("POST", "api/session", { title, location: { directory: project }, model: { providerID: "codex-cli", id: "gpt-test" } });
    assert.equal(made.status, 200, JSON.stringify(made.json));
    return made.json.data.id;
  };
  const session = await conversation("Pilot conversation");
  const messages = (id = session) => api("GET", `api/session/${id}/message?limit=100&order=asc`).then(result => result.json?.data ?? []);
  const prompt = async (id, text, target = session) => {
    const answer = await api("POST", `api/session/${target}/prompt`, { id, delivery: "steer", text });
    assert.equal(answer.status, 200, JSON.stringify(answer.json));
    await until(() => turns(signals).some(turn => turn.text === text), `Codex to start ${text}`, 35_000);
  };
  const accepted = "msg_residentpilot00001";
  await prompt(accepted, "hold pilot");
  const holder = await until(() => holderFor(session), "the sandboxed holder to record the turn");
  assert.equal(holder.meta.fenced, true);
  const provider = readJSON(path.join(signals, `provider-${holder.child}.json`));
  assert.deepEqual(provider, { pid: holder.child, canReadBox: false });
  const work = { engine, bridge: bridge.pid, holder: holder.pid, provider: holder.child };
  const sandboxPlugin = path.join(home, "config/opencode/plugins/kuvo-sandbox.js");
  const installedPlugin = () => {
    const source = fs.readFileSync(sandboxPlugin, "utf8");
    assert.match(source, /^const VERSION = "[a-f0-9]{16}"$/m);
    return source.replace(/^const VERSION = "[a-f0-9]{16}"$/m, "");
  };
  const pluginSource = installedPlugin();
  const runningWork = async checkpoint => {
    const now = holderFor(session);
    const pids = { engine: readJSON(path.join(RUN, "engine.json"))?.pid, bridge: readJSON(path.join(RUN, "bridge.json"))?.pid,
      holder: now?.pid, provider: now?.child };
    assert.deepEqual(pids, work, checkpoint);
    assert.equal(now.generation, holder.generation);
    assert.ok(Object.values(pids).every(runs), checkpoint);
    assert.equal(installedPlugin(), pluginSource, checkpoint);
    assert.ok(fs.statSync(path.join(home, "bin", "kuvo-fence")).mode & 0o100);
    assert.equal((await api("GET", `api/session/${session}/prompt-receipt/${accepted}`)).status, 200, checkpoint);
    assert.deepEqual((await api("GET", "api/session/active")).json?.data?.[session], { type: "busy" }, checkpoint);
    t.diagnostic(`${checkpoint} PIDs ${JSON.stringify(pids)}`);
  };
  await runningWork("split");

  const bunCommand = async (args, input = "") => {
    const stdout = await new Promise((resolve, reject) => {
      const child = execFile(bun, ["--config=/dev/null", "--no-env-file", ...args], { env, timeout: 75_000 }, (error, out, stderr) => {
        if (error) reject(new Error(stderr || error.message, { cause: error }));
        else resolve(out);
      });
      child.stdin.end(input);
    });
    return JSON.parse(stdout.trim().split("\n").at(-1));
  };
  const prepare = async (version, editLink) => {
    const { out, pinned } = pack(version, editLink);
    const release = link.verifyBoxes(fs.readFileSync(path.join(out, "boxes.json"), "utf8"), [pinned]);
    const held = await bunCommand(["-e", `
      import { receiveRelease } from ${JSON.stringify(pathToFileURL(installed).href)};
      console.log(JSON.stringify(receiveRelease(${JSON.stringify(out)}, [${JSON.stringify(pinned)}])));
    `]);
    assert.equal(held.status, 202, JSON.stringify(held));
    const args = [installed, "update", "--switch", "--version", release.version, "--sha256", release.pack.sha256, "--key", pinned];
    const ready = await bunCommand([...args, "--phase", "prepare", "--from", out]);
    assert.equal(ready.result, "waiting", JSON.stringify(ready));
    return async () => {
      const result = await bunCommand([...args, "--phase", "activate"]);
      assert.equal(result.result, "updated", JSON.stringify(result));
    };
  };

  // The phone uses the same pairing before and after both switches.
  let number = 0;
  const phoneInfo = async () => {
    const client = relay.device();
    const nonce = ++number;
    const initiator = new oldClient.IKInitiator(phone, boxPublic, oldClient.PROLOGUE);
    const send = (kind, data) => client.send(address, oldClient.frame(kind, nonce, data));
    const next = async () => oldClient.readFrame(oldClient.readRelay(await client.next(1000)).payload);
    try {
      send(oldClient.Frame.handshake1, initiator.writeMessage1(Buffer.from(JSON.stringify({ v: 1, pair: pairing }))));
      const hello = await next();
      if (hello.kind !== oldClient.Frame.handshake2) return null;
      const { payload, transport } = initiator.readMessage2(hello.body);
      send(oldClient.Frame.transport, transport.seal(oldClient.encodeMessage({ t: "req", id: 1, m: "GET", p: "kuvo/info" })));
      const first = oldClient.decodeMessage(transport.open((await next()).body));
      const chunks = [first.body];
      for (let last = first; !last.head.end;) {
        last = oldClient.decodeMessage(transport.open((await next()).body));
        chunks.push(last.body);
      }
      return first.head.s === 200 ? { info: JSON.parse(Buffer.concat(chunks)), app: JSON.parse(payload).app } : null;
    } catch { return null; }
    finally { client.leave(); }
  };
  const desktopInfo = async () => {
    const record = readJSON(path.join(RUN, "serve.json"));
    if (!record) return null;
    try {
      const answer = await fetch(`http://127.0.0.1:${record.port}/kuvo/info`, {
        headers: { authorization: "Basic " + Buffer.from(`opencode:${record.password}`).toString("base64") }, signal: AbortSignal.timeout(500),
      });
      return answer.ok ? { pid: record.pid, info: await answer.json() } : null;
    } catch { return null; }
  };
  const oldPhone = await until(phoneInfo, "the already paired phone to answer on the split link", 25_000);
  const rejectGeneration = source => {
    const broken = source.replace("serverGeneration: this.generation", 'serverGeneration: "stale-trial"');
    assert.notEqual(broken, source);
    return broken;
  };
  const broken = await prepare("0.10.0", rejectGeneration);
  await broken();
  const badPID = await serveUp(first);
  assert.equal((await desktopInfo()).info.serverGeneration, "stale-trial");
  assert.equal(runs(first), false);
  assert.equal(runs(oldLink), false);
  await runningWork("trial");
  const restored = await until(() => {
    if (!fs.existsSync(path.join(RUN, "update-cannot"))) return null;
    const record = readJSON(path.join(RUN, "serve.json"));
    return record && !record.generation && record.pid !== first && record.pid !== badPID && record.pid;
  }, "the failed generation to roll back to split launch", 155_000);
  const rolledAt = fs.statSync(path.join(RUN, "update-cannot")).mtimeMs;
  const restoredLink = await linkUp(badPID);
  assert.notEqual(restoredLink, restored);
  assert.equal(runs(badPID), false);
  assert.equal(fs.readFileSync(path.join(home, "bin", "box-version"), "utf8").trim(), "0.9.0");
  const phoneAtRollback = await until(phoneInfo, "the pairing to work after rollback", 25_000);
  assert.equal(phoneAtRollback.app, oldPhone.app);
  assert.equal((await desktopInfo())?.pid, restored);
  t.diagnostic(`split HTTP and paired relay returned within ${Math.max(0, Math.ceil(Date.now() - rolledAt))} ms of the rollback decision`);
  await runningWork("rollback");
  const afterRollback = await conversation("New work after rollback");
  await prompt("msg_residentpilot00002", "new turn after rollback", afterRollback);
  await until(async () => (await messages(afterRollback)).at(-1)?.outcome === "succeeded", "the new turn after rollback to finish", 30_000);

  const good = await prepare("0.10.1");
  const began = performance.now();
  const reached = (name, check) => until(check, `${name} to reconnect through the resident`, 60_000).then(result => {
    t.diagnostic(`${name} reconnected within ${Math.ceil(performance.now() - began)} ms of activation request`);
    return result;
  });
  const [desktop, phoneReply, ssh] = await Promise.all([
    reached("local HTTP", async () => { const answer = await desktopInfo(); return answer?.info.serverGeneration && answer; }),
    reached("paired relay", async () => { const answer = await phoneInfo(); return answer && answer.app !== oldPhone.app && answer; }),
    reached("SSH route", async () => {
      if (!readJSON(path.join(RUN, "serve.json"))?.generation) return null;
      const response = await bunCommand([installed, "box", "route"], JSON.stringify({ method: "GET", path: "kuvo/info" }));
      return response.status === 200 && JSON.parse(response.body);
    }),
    good(),
  ]);
  assert.equal(phoneReply.app, desktop.info.serverBuild);
  assert.equal(phoneReply.info.bridge, desktop.info.serverBuild);
  assert.equal(ssh.serverGeneration, desktop.info.serverGeneration);
  assert.equal(await linkUp(), desktop.pid);
  await until(() => !fs.existsSync(path.join(RUN, "previous")), "the guard to confirm the live generation", 25_000);
  await runningWork("resident");

  write(path.join(signals, "release-pilot"), "");
  await until(async () => (await messages()).at(-1)?.outcome === "succeeded", "the original active turn to finish", 30_000);
  await prompt("msg_residentpilot00003", "new turn after update");
  await until(async () => (await messages()).filter(note => note.type === "idle" && note.outcome === "succeeded").length === 2, "the new turn to finish", 30_000);
  await prompt("msg_residentpilot00004", "another sandboxed turn");
  await until(async () => (await messages()).filter(note => note.type === "idle" && note.outcome === "succeeded").length === 3, "the next turn to finish", 30_000);
  const ran = turns(signals);
  assert.deepEqual(ran.map(turn => turn.text), ["hold pilot", "new turn after rollback", "new turn after update", "another sandboxed turn"]);
  assert.equal(ran[0].pid, holder.child);
  // A new bin target changes a sandbox's readable paths. The next turn may reopen the provider
  // after the active turn has finished. Consecutive turns on the same policy share their provider.
  assert.equal(ran[2].pid, ran[3].pid);
  assert.notEqual(ran[1].pid, holder.child, "another conversation has its own sandboxed provider");
  for (const pid of new Set(ran.map(turn => turn.pid))) {
    assert.deepEqual(readJSON(path.join(signals, `provider-${pid}.json`)), { pid, canReadBox: false });
  }
});
