// Row 6.2. Use real residents and their wrapped stores. The relay and engine
// are stand-ins. Value checks use the client-only comparison route so no
// captured answer needs to hold the fixture's plaintext.
import assert from "node:assert/strict";
import { execFile, execFileSync } from "node:child_process";
import { createECDH, createHash, createHmac, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";
import { promisify } from "node:util";
import { boxBinary, isolatedEnv, launch, readJSON, request, scratch, typescriptRuntime, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const run = promisify(execFile);
const vault = "kuvo/vault";
const header = { "x-kuvo-from": "app" };
const value = () => randomBytes(24).toString("base64url");

function checkNoPlaintext(home, forbidden) {
  for (const file of fs.readdirSync(home, { recursive: true, withFileTypes: true })) {
    if (!file.isFile()) continue;
    let bytes;
    try { bytes = fs.readFileSync(path.join(file.parentPath, file.name)); }
    catch (error) {
      // SQLite can remove a transient WAL/shm file between the enumeration and
      // this open. Windows also byte-locks the transient Insights WAL index
      // while its connection is open. The index holds page locations, not page
      // contents; the database and WAL contents are still checked.
      if (error.code === "ENOENT") continue;
      if (error.code === "EBUSY" && file.name === "insights.sqlite-shm" && process.platform === "win32") continue;
      error.message += ` (${path.join(file.parentPath, file.name)})`;
      throw error;
    }
    assert.ok(forbidden.every(secret => !bytes.includes(secret)), "fixture plaintext was written into a file");
  }
}

async function create(box, secret) {
  const signed = generateKeyPairSync("ed25519");
  const mac = randomBytes(32).toString("base64url");
  const id = randomBytes(12).toString("hex");
  const link = path.join(box.home, "run/link.json");
  write(link, JSON.stringify({
    ...readJSON(link), id, macs: [mac],
    macSigns: { [mac]: signed.publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64url") },
  }));
  const proof = () => {
    const at = Date.now();
    const nonce = randomBytes(20).toString("base64url");
    const message = Buffer.from(`kuvo-vault-key\0${id}\0${at}\0${nonce}`);
    return { mac, at, nonce, proof: sign(null, message, signed.privateKey).toString("base64url") };
  };
  const proc = box.start();
  const ask = box.connect(await proc.ready());
  assert.equal((await ask("GET", vault)).status, 403);
  const signedRequest = proof();
  const made = await ask("POST", vault + "/key", signedRequest);
  assert.equal(made.status, 200);
  const key = made.json.key;
  assert.ok(Buffer.from(key, "base64url").length === 32, "the client got an unlock key");
  assert.equal((await ask("POST", vault + "/key", signedRequest)).status, 403);
  const add = await ask("POST", vault + "/items", {
    kind: "login", name: "Example login", sites: ["https://EXAMPLE.test:443/go", "example.test"],
    username: "tester", values: { password: secret },
  }, header);
  assert.equal(add.status, 200);
  assert.deepEqual(add.json.item.sites, ["example.test"]);
  assert.deepEqual(add.json.item.fields, ["password"]);
  assert.ok(!add.text.includes(secret), "item metadata hides the value");
  return { proc, ask, key, item: add.json.item };
}

test("a machine vault keeps its secret through a restart and only clients can compare it", async t => {
  const { computer } = await bench(t);
  const box = await computer("Vault");
  const secret = value();
  const { ask, proc, key, item } = await create(box, secret);
  const itemPath = vault + "/items/" + item.id;
  for (const [method, route] of [["GET", vault], ["GET", itemPath + "/values"], ["POST", itemPath + "/same"]]) {
    const answer = await ask(method, route, { values: { password: secret } });
    assert.equal(answer.status, 403);
    assert.ok(!answer.text.includes(secret), "unauthorized reply hides the value");
  }
  assert.deepEqual((await ask("POST", itemPath + "/same", { values: { password: secret } }, header)).json, { same: true });
  assert.deepEqual((await ask("POST", itemPath + "/same", { values: { password: value() } }, header)).json, { same: false });
  const activity = await ask("GET", vault + "/activity", undefined, header);
  assert.deepEqual(activity.json.entries.map(entry => [entry.what, entry.mine]), [["added", true]]);
  for (const answer of [activity, await ask("GET", vault, undefined, header)]) {
    assert.ok(!answer.text.includes(secret) && !answer.text.includes(key), "public fields hide secret material");
  }
  checkNoPlaintext(box.home, [secret, key]);
  assert.ok(!proc.log().includes(secret) && !proc.log().includes(key), "resident log hides secret material");

  await proc.stop();
  const next = box.start();
  const again = box.connect(await next.ready());
  const locked = await again("GET", vault, undefined, header);
  assert.equal(locked.json.state.locked, true);
  assert.deepEqual(locked.json.items, []);
  assert.equal((await again("POST", vault + "/unlock", { key: value() })).status, 403);
  assert.equal((await again("POST", vault + "/unlock", { key })).json.locked, false);
  assert.deepEqual((await again("POST", itemPath + "/same", { values: { password: secret } }, header)).json, { same: true });
  checkNoPlaintext(box.home, [secret, key]);
  assert.ok(!next.log().includes(secret) && !next.log().includes(key), "restart log hides secret material");
});

test("a grant put back from an earlier generation and a deleted pause file count for nothing", async t => {
  const { computer } = await bench(t);
  const box = await computer("Vault");
  const { ask, item } = await create(box, value());
  const grantsFile = path.join(box.home, "box/vault/grants.json");
  const grant = { item: item.id, who: { everyone: true }, how: "own" };
  assert.equal((await ask("POST", vault + "/grants", grant, header)).json.grants.length, 1);
  const older = fs.readFileSync(grantsFile);
  assert.deepEqual((await ask("POST", vault + "/grants", { ...grant, how: null }, header)).json.grants, []);
  fs.writeFileSync(grantsFile, older);
  assert.equal((await ask("GET", vault, undefined, header)).json.state.tampered, true);
  assert.deepEqual((await ask("GET", vault + "/grants", undefined, header)).json.grants, []);
  assert.equal((await ask("POST", vault + "/pause", { paused: true }, header)).json.paused, true);
  fs.rmSync(path.join(box.home, "box/vault/paused.json"));
  assert.equal((await ask("GET", vault, undefined, header)).json.state.paused, true);
});

test("the other implementation opens and changes the resident's wrapped vault", {
  skip: !process.env.KUVO_VAULT_OTHER_BIN,
}, async t => {
  const { computer } = await bench(t);
  const other = boxBinary(process.env.KUVO_VAULT_OTHER_BIN);
  assert.notEqual(boxBinary().typescript, other.typescript);
  const box = await computer("Vault");
  const secret = value();
  const { proc, key, item } = await create(box, secret);
  await proc.stop();

  const replaced = box.start(other);
  const ask = box.connect(await replaced.ready());
  assert.equal((await ask("POST", vault + "/unlock", { key })).status, 200);
  const listed = await ask("GET", vault, undefined, header);
  assert.equal(listed.status, 200);
  assert.deepEqual(listed.json.items, [item]);
  const itemPath = vault + "/items/" + item.id;
  assert.deepEqual((await ask("POST", itemPath + "/same", { values: { password: secret } }, header)).json, { same: true });
  const replacement = value();
  assert.equal((await ask("PATCH", itemPath, { values: { password: replacement } }, header)).status, 200);
  checkNoPlaintext(box.home, [secret, replacement, key]);
  assert.ok([secret, replacement, key].every(value => !replaced.log().includes(value)), "migration log hides secret material");
  await replaced.stop();

  const restarted = box.start();
  const original = box.connect(await restarted.ready());
  assert.equal((await original("POST", vault + "/unlock", { key })).status, 200);
  assert.deepEqual((await original("POST", itemPath + "/same", { values: { password: replacement } }, header)).json, { same: true });
  assert.equal((await original("GET", vault, undefined, header)).json.state.tampered, undefined);
});

test("the private socket proves it holds the unlock key and refuses a future request", {
  skip: process.platform === "win32",
}, async t => {
  const { computer } = await bench(t);
  const box = await computer("Vault");
  const { key, proc } = await create(box, value());
  const run = path.join(box.home, "run");
  const socketKey = fs.readFileSync(path.join(run, "socket.key"), "utf8").trim();
  const route = path.join(run, "vault.sock");
  async function exchange(given, send) {
    const socket = net.createConnection(route);
    const lines = [];
    const chunks = [];
    return new Promise((resolve, reject) => {
      socket.setTimeout(3000, () => socket.destroy(new Error("Vault socket timed out")));
      socket.setEncoding("utf8");
      socket.on("error", reject);
      socket.on("connect", () => socket.write(JSON.stringify(given) + "\n"));
      socket.on("data", chunk => {
        chunks.push(chunk);
        const text = chunks.join("");
        const end = text.lastIndexOf("\n");
        if (end < 0) return;
        for (const line of text.slice(0, end).split("\n")) {
          const reply = JSON.parse(line);
          lines.push(reply);
          if (lines.length === 1 && send) socket.write(JSON.stringify(send(reply)) + "\n");
        }
        chunks.splice(0, chunks.length, text.slice(end + 1));
      });
      socket.on("close", () => resolve(lines));
    });
  }
  assert.deepEqual(await exchange({ hello: "untrusted", key: value() }), []);
  const hello = "vault-fixture-hello";
  const auth = text => createHmac("sha256", Buffer.from(key, "base64url")).update(text).digest("base64url");
  const replies = await exchange({ hello, key: socketKey }, response => {
    assert.equal(response.formatVersion, 1);
    assert.ok(response.proof === auth(`kuvo-vault-holder\0${hello}`), "holder proved the same key");
    return { formatVersion: 90, method: "GET", path: vault };
  });
  assert.equal(replies[1].status, 409);
  assert.equal(JSON.parse(Buffer.from(replies[1].body, "base64")).code, "unsupportedFormatVersion");
  const empty = createHash("sha256").update("").digest("base64url");
  let proof;
  const accepted = await exchange({ hello, key: socketKey }, response => {
    proof = auth(`kuvo-vault-client\0${response.nonce}\0GET\0${vault}\0${empty}`);
    return { method: "GET", path: vault, headers: { "x-kuvo-vault-auth": proof }, body: "" };
  });
  assert.equal(accepted[1].status, 200);
  const replay = await exchange({ hello, key: socketKey }, () =>
    ({ method: "GET", path: vault, headers: { "x-kuvo-vault-auth": proof }, body: "" }));
  assert.equal(replay[1].status, 403);
  assert.equal((await proc.stop()).code, 0, "the resident stopped cleanly");
  assert.ok(!fs.existsSync(route), "the holder removes its socket on orderly shutdown");
});

function calling(box, session, name, input, callID) {
  box.engine.messages.set(session.id, [{
    id: "msg_vault", content: [{ type: "tool", name, callID, state: { status: "running", input } }],
  }]);
}

test("a conversation lists and answers its child's vault ask through the live holder", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const box = await computer("Session vault", boxBinary(), () => ({ KUVO_VAULT_AGENTS: "1", KUVO_TEST_LOGIN_ENV: "off" }));
  const project = path.join(box.home, "project");
  fs.mkdirSync(project);
  const root = sessionOf({ id: "ses_vaultRoot", title: "Release", directory: project });
  const child = sessionOf({ id: "ses_vaultChild", title: "Find sign-in", directory: project, parent: root.id });
  const other = sessionOf({ id: "ses_vaultUnrelated", title: "Other work", directory: project });
  box.engine.sessions.push(root, child, other);
  const secret = value();
  const { ask, item } = await create(box, secret);
  const args = { need: "site login", why: "Check the authorized fixture." };
  calling(box, child, "vault_ask", args, "call_child_ask");
  const pending = ask("POST", vault + "/tools", {
    session: child.id, name: "vault_ask", arguments: args,
  });
  pending.catch(() => {});
  const route = `kuvo/sessions/${root.id}/asks`;
  const card = await until(async () => {
    const answer = await ask("GET", route, undefined, header);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.asks.find(card => card.kind === "vault");
  }, "the owner conversation to list the child’s vault ask");
  assert.equal(card.session, root.id);
  assert.equal(card.need, "item");
  assert.ok(card.choices.some(choice => choice.item === item.id && choice.answer === "once"));
  assert.ok(!JSON.stringify(card).includes(secret));
  const direct = await ask("GET", route);
  assert.deepEqual(direct.json.asks.find(ask => ask.id === card.id).choices, []);
  await until(async () => (await ask("GET", "kuvo/waiting")).json.sessions === 1,
    "the vault question to count once for its conversation");
  const refused = await ask("POST", route + "/" + card.id, { answer: "once", item: item.id });
  assert.equal(refused.status, 403, refused.text);
  const outside = await ask("POST", `kuvo/sessions/${other.id}/asks/${card.id}`, { answer: "once", item: item.id }, header);
  assert.equal(outside.status, 404, outside.text);
  const granted = await ask("POST", route + "/" + card.id, { answer: "once", item: item.id }, header);
  assert.equal(granted.status, 200, granted.text);
  assert.equal((await pending).json.isError, false);
  assert.equal((await ask("POST", route + "/" + card.id, { answer: "once", item: item.id }, header)).status, 404);
  assert.deepEqual((await ask("GET", route, undefined, header)).json.asks, []);
  await until(async () => (await ask("GET", "kuvo/waiting")).json.sessions === 0,
    "the answered vault question to leave the waiting total");
});

test("conversation vault cards offer a lasting grant only for a project", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const box = await computer("Vault grant scope", boxBinary(), () => ({ KUVO_VAULT_AGENTS: "1", KUVO_TEST_LOGIN_ENV: "off" }));
  const project = path.join(box.home, "project");
  const account = path.join(path.dirname(box.home), "account");
  fs.mkdirSync(project);
  const { ask, item } = await create(box, value());
  for (const [id, directory, lasting] of [["ses_vaultHome", account, false], ["ses_vaultProject", project, true]]) {
    const session = sessionOf({ id, title: "Check authorized scope", directory });
    box.engine.sessions.push(session);
    const args = { command: "printf '%s' \"$FIXTURE\"", variables: { FIXTURE: item.name }, why: "Verify the approval choices." };
    calling(box, session, "vault_run", args, "call_" + id);
    const pending = ask("POST", vault + "/tools", { session: id, name: "vault_run", arguments: args });
    pending.catch(() => {});
    const route = `kuvo/sessions/${id}/asks`;
    const card = await until(async () => {
      const response = await ask("GET", route, undefined, header);
      return response.json?.asks?.find(row => row.kind === "vault");
    }, "the conversation's real vault grant request to appear");
    assert.equal(card.need, "use");
    assert.equal(card.session, id);
    assert.deepEqual(card.choices.map(row => row.answer), lasting ? ["once", "always", "reject"] : ["once", "reject"]);
    const decision = route + "/" + card.id;
    if (!lasting) {
      const refused = await ask("POST", decision, { answer: "always" }, header);
      assert.equal(refused.status, 400, refused.text);
      assert.deepEqual(refused.json, { message: "Answer once, reject." });
    }
    assert.equal((await ask("POST", decision, { answer: "reject" }, header)).status, 200);
    assert.equal((await pending).json.isError, true);
    assert.equal((await ask("POST", decision, { answer: "once" }, header)).status, 404);
  }
});

test("asks wait for the user, a grant is used once, and command output is masked", {
  skip: process.platform === "win32",
}, async t => {
  const { computer } = await bench(t);
  const box = await computer("Vault", boxBinary(), () => ({ KUVO_VAULT_AGENTS: "1", KUVO_TEST_LOGIN_ENV: "off" }));
  const secret = value();
  const { ask, proc, key, item } = await create(box, secret);
  const project = path.join(box.home, "project");
  fs.mkdirSync(project);
  const session = sessionOf({ id: "ses_asker", title: "Release", directory: project });
  box.engine.sessions.push(session);
  const args = { need: "site login", why: "Check the authorized fixture." };
  const post = (name, given, operation) => ask("POST", vault + "/tools", {
    session: session.id, name, arguments: given, ...(operation ? { operation } : {}),
  });
  calling(box, session, "vault_ask", args, "call_ask");
  const waiting = post("vault_ask", args);
  const pending = await until(async () => {
    const answer = await ask("GET", vault + "/asks", undefined, header);
    return answer.json?.asks?.[0];
  }, "vault ask did not arrive");
  assert.equal(pending.kind, "item");
  assert.equal(pending.session, session.id);
  assert.equal(pending.always, true);
  assert.equal((await ask("POST", vault + "/asks/" + pending.id, { answer: "once", item: item.id }, header)).status, 200);
  const supplied = await waiting;
  assert.equal(supplied.json.isError, false);
  assert.match(supplied.json.text, /for this once/);

  const command = { command: "printf '%s\\n' \"$VAULT_TEST\"; printf '%s' \"$VAULT_TEST\" | base64",
    variables: { VAULT_TEST: item.name }, why: args.why };
  calling(box, session, "vault_run", command, "call_run");
  assert.match((await post("vault_run", { ...command, command: "different" })).json.text, /isn't running in it/);
  const operation = "vault-run-once";
  const run = await post("vault_run", command, operation);
  assert.equal(run.json.isError, false);
  assert.ok(!run.text.includes(secret) && !run.text.includes(Buffer.from(secret).toString("base64")), "the command result hides both encodings");
  assert.match(run.json.text, /\[vault: Example login\]\n\[vault: Example login\]/);

  calling(box, session, "vault_run", command, "call_again");
  const again = post("vault_run", command);
  const next = await until(async () => (await ask("GET", vault + "/asks", undefined, header)).json?.asks?.[0],
    "one-use grant was used up");
  assert.equal(next.kind, "use");
  assert.equal((await ask("POST", vault + "/asks/" + next.id, { answer: "no" }, header)).status, 200);
  assert.equal((await again).json.isError, true);
  assert.deepEqual((await ask("GET", vault + "/asks", undefined, header)).json, { asks: [] });
  await proc.stop();
  const restarted = box.start();
  const after = box.connect(await restarted.ready());
  const replay = await after("POST", vault + "/tools", {
    session: session.id, name: "vault_run", arguments: command, operation,
  });
  assert.equal(replay.status, 200);
  assert.deepEqual(replay.json, run.json);
  const changed = await after("POST", vault + "/tools", {
    session: session.id, name: "vault_run", arguments: { ...command, why: "Different input." }, operation,
  });
  assert.equal(changed.status, 409);
  checkNoPlaintext(box.home, [secret, key]);
  assert.ok(!proc.log().includes(secret) && !proc.log().includes(key), "resident log hides secret material");
});

test("a closed gate still fills a pre-approved sign-in once for its running browser call", async t => {
  const { computer } = await bench(t);
  const box = await computer("Vault");
  const secret = value();
  const { ask, key, proc, item } = await create(box, secret);
  const session = sessionOf({ id: "ses_browser", title: "Sign in", directory: box.home });
  box.engine.sessions.push(session);
  const grant = { item: item.id, who: { everyone: true }, how: "own" };
  assert.equal((await ask("POST", vault + "/grants", grant, header)).status, 200);
  assert.deepEqual((await ask("GET", vault + "/tools")).json, { tools: [] });
  assert.equal((await ask("POST", vault + "/fill", { session: session.id, purpose: "field", item: item.id })).status, 404);
  const input = { session: session.id, origin: "https://example.test", purpose: "sign-in" };
  calling(box, session, "browser_sign_in", {}, "call_fill");
  const filled = await ask("POST", vault + "/fill", input);
  assert.equal(filled.status, 200);
  assert.ok(filled.json?.values?.password === secret, "matching tool call gave the browser its secret");
  const replay = await ask("POST", vault + "/fill", input);
  assert.equal(replay.status, 403);
  assert.ok(!replay.text.includes(secret), "replay reveals nothing");
  checkNoPlaintext(box.home, [secret, key]);
  assert.ok(!proc.log().includes(secret) && !proc.log().includes(key), "resident log hides secret material");
});

const helperScenario = {
  skip: process.platform !== "darwin" || !process.env.KUVO_VAULT_HELPER_BIN,
  timeout: 75000,
};

function macBox(t, binary) {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, binary.typescript ? "box" : ".kuvo-code");
  const keychain = path.join(root, "vault.keychain-db");
  const env = isolatedEnv(root);
  const quiet = { env, stdio: "ignore" };
  const password = "temporary-keychain-fixture";
  execFileSync("/usr/bin/security", ["create-keychain", "-p", password, keychain], quiet);
  let guard;
  t.after(async () => {
    await guard?.stop();
    execFileSync("/usr/bin/security", ["delete-keychain", keychain], quiet);
    fs.rmSync(root, { recursive: true, force: true });
  });
  const bundle = path.join(home, "bin", "Kuvo Helper.app");
  const helper = path.join(bundle, "Contents", "MacOS", "Kuvo Helper");
  write(helper, fs.readFileSync(process.env.KUVO_VAULT_HELPER_BIN), { mode: 0o700 });
  const id = "run.kuvo.code.vaulttrial.box";
  write(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>${id}</string>
<key>CFBundleExecutable</key><string>Kuvo Helper</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleName</key><string>Kuvo Helper</string>
</dict></plist>
`);
  execFileSync("/usr/bin/codesign", ["--force", "--sign", "-", "--identifier", id, bundle], quiet);
  write(path.join(home, "box/kind"), "mac\n");
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  write(path.join(home, "run/mac-link.json"), '{"phones":false,"relay":"ws://127.0.0.1:9"}\n');
  const starter = path.join(home, "bin/kuvo-start");
  if (binary.typescript) {
    const quote = text => "'" + text.replaceAll("'", "'\\''") + "'";
    write(starter, `#!/bin/sh
K="$HOME/box"
if [ "$1" = keep ]; then exec /bin/sleep 120; fi
export KUVO_HOME="$K" KUVO_TEST_RELAY=ws://127.0.0.1:9 KUVO_TEST_NO_PEERS=1 KUVO_SANDBOX=off
export BUN_BE_BUN=1 BUN_RUNTIME_TRANSPILER_CACHE_PATH=0
exec ${quote(typescriptRuntime())} --config=/dev/null --no-env-file ${quote(binary.file)} "$@"
`, { mode: 0o700 });
  } else {
    write(starter, fs.readFileSync(new URL("../../box/src/keeper/kuvo-start", import.meta.url)), { mode: 0o700 });
    fs.copyFileSync(binary.file, path.join(home, "bin/kuvo-box"));
    write(path.join(home, "box.json"), '{"launch":"rust"}\n');
    write(path.join(home, "run/launch.env"), "KUVO_TEST_RELAY=ws://127.0.0.1:9\nKUVO_TEST_NO_PEERS=1\nKUVO_SANDBOX=off\n", { mode: 0o600 });
    // The keeper's idle loop runs as shipped, with a stand-in for its power assertion.
    const tools = path.join(root, "tools");
    write(path.join(tools, "caffeinate"), "#!/bin/sh\nexit 0\n", { mode: 0o700 });
    env.PATH = tools + path.delimiter + env.PATH;
  }
  const recordFile = path.join(home, "run/serve.json");
  let lastPid;
  async function ready() {
    const record = await until(() => {
      if (guard.child.exitCode !== null) throw new Error("Test Helper left before starting its resident");
      if (!fs.existsSync(recordFile)) return;
      const record = readJSON(recordFile);
      if (record.pid !== lastPid && record.port && record.password) return record;
    }, "Test Helper did not start its resident", 30000);
    lastPid = record.pid;
    return (method, route, body, headers = header) => request(record, method, route, body, headers);
  }
  function start() {
    guard = launch(helper, ["keep", "--home", home, "--test-keychain", keychain, "--test-keychain-password", password], env, home);
    return ready();
  }
  async function restartResident() {
    // Confirm the rendezvous PID belongs to our fixture before signalling it.
    const parent = execFileSync("/bin/ps", ["-o", "ppid=", "-p", String(lastPid)], { env, encoding: "utf8" });
    assert.equal(Number(parent.trim()), guard.child.pid);
    process.kill(lastPid, "SIGTERM");
    return ready();
  }
  return { home, start, stop: () => guard.stop(), log: () => guard.log(), restartResident };
}

test("the Mac resident keeps vault values and generations in Helper's disposable Keychain", helperScenario, async t => {
  const binary = boxBinary();
  const mac = macBox(t, binary);
  const { home, start } = mac;
  let ask = await start();
  assert.equal((await ask("GET", "kuvo/box/rehome")).status, 200);
  assert.equal((await ask("POST", "kuvo/box/rehome", { signIns: true }, {})).status, 403);
  assert.equal((await ask("POST", "kuvo/box/rehome", { signIns: true }, header)).status, 409);
  const secret = value();
  const made = await ask("POST", vault + "/items", { kind: "key", name: "Fixture credential", values: { token: secret } });
  assert.equal(made.status, 200, made.json?.message);
  assert.equal(made.json.key, undefined);
  const itemPath = vault + "/items/" + made.json.item.id;
  assert.deepEqual((await ask("POST", itemPath + "/same", { values: { token: secret } })).json, { same: true });
  assert.equal(fs.existsSync(path.join(home, "box/vault/values")), false);
  if (!binary.typescript) {
    ask = await mac.restartResident();
    assert.deepEqual((await ask("POST", itemPath + "/same", { values: { token: secret } })).json, { same: true });
    const link = readJSON(path.join(home, "run/link.json"));
    assert.ok(["key", "secret", "sign"].every(field => !Object.hasOwn(link, field)),
      "the restarted resident keeps the Mac's Helper identity off disk");
  }

  const hostRecord = path.join(home, "run/host.json");
  const authentic = fs.readFileSync(hostRecord);
  const attempts = [];
  const fakePassword = value();
  const fake = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const part of request) chunks.push(part);
    const body = Buffer.concat(chunks);
    attempts.push({ path: request.url, authenticated: request.headers.authorization !== undefined, leaked: body.includes(secret) });
    let nonce;
    try { nonce = JSON.parse(body).nonce; } catch {}
    const proof = createHmac("sha256", fakePassword).update(`kuvo-host-hello\0${nonce}\0${"127.0.0.1"}:${fake.address().port}`).digest("hex");
    response.writeHead(200, { "content-type": "application/json" });
    response.end(JSON.stringify({ proof, vaultProof: proof }));
  });
  await new Promise((resolve, reject) => { fake.once("error", reject); fake.listen(0, "127.0.0.1", resolve); });
  try {
    fs.writeFileSync(hostRecord, JSON.stringify({ formatVersion: 1, url: `http://127.0.0.1:${fake.address().port}`, password: fakePassword }));
    assert.equal((await ask("PATCH", itemPath, { values: { token: secret } })).status, 503);
    assert.ok(attempts.length > 0 && attempts.every(attempt => attempt.path === "/hello" && !attempt.authenticated && !attempt.leaked),
      "a replacement port gets neither its claimed password nor the vault values");
  } finally {
    fs.writeFileSync(hostRecord, authentic);
    await new Promise(resolve => fake.close(resolve));
  }
  const old = fs.readFileSync(path.join(home, "box/vault/items.json"));
  assert.equal((await ask("PATCH", itemPath, { name: "Changed credential" })).status, 200);
  await mac.stop();
  const changed = fs.readFileSync(path.join(home, "box/vault/items.json"));
  fs.writeFileSync(path.join(home, "box/vault/items.json"), old);
  ask = await start();
  const stale = await ask("GET", vault);
  assert.equal(stale.json.state.store, "keychain");
  assert.equal(stale.json.state.tampered, true);
  assert.deepEqual(stale.json.items, []);
  fs.writeFileSync(path.join(home, "box/vault/items.json"), changed);
  assert.deepEqual((await ask("POST", itemPath + "/same", { values: { token: secret } })).json, { same: true });
  assert.equal((await ask("DELETE", itemPath)).status, 200);
  checkNoPlaintext(home, [secret]);
  assert.ok(!mac.log().includes(secret), "Helper log hides the value");
});

test("a replacement Mac imports a vault point into its own Helper Keychain", helperScenario, async t => {
  const binary = boxBinary();
  const original = macBox(t, binary);
  const replacement = macBox(t, binary);
  const disk = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(disk, { recursive: true, force: true }));
  const secret = value();
  const ask = await original.start();
  const name = "Saved site";
  const made = await ask("POST", vault + "/items", {
    kind: "login", name, sites: ["example.test"], username: "tester",
    signInOnly: true, values: { password: secret },
  });
  assert.equal(made.status, 200, made.json?.message);
  const originalItem = made.json.item.id;
  assert.equal((await ask("POST", vault + "/grants", {
    item: originalItem, who: { everyone: true }, how: "own",
  })).status, 200);
  const source = "b_" + randomBytes(16).toString("base64url");
  const computerKey = randomBytes(32).toString("base64url");
  const destination = { kind: "disk", path: disk };
  write(path.join(original.home, "backup/configuration.json"), JSON.stringify({
    version: 1, enabled: true, source, key: computerKey, dest: destination, projects: [],
  }), { mode: 0o600 });
  const backup = "kuvo/box/backups/vault";
  const point = await ask("POST", backup, { destination, source, confirm: true });
  assert.equal(point.status, 200, point.json?.message);
  const { id, recoveryKey } = point.json;
  assert.ok(Buffer.from(recoveryKey, "base64url").length === 32 && recoveryKey !== computerKey,
    "the vault point uses a separate recovery key");
  await original.stop();

  let receive = await replacement.start();
  const empty = await receive("GET", vault);
  assert.equal(empty.json.state.store, "keychain");
  assert.deepEqual(empty.json.items, []);
  const restored = await receive("POST", backup + "/import", { destination, source, id, recoveryKey, confirm: true });
  assert.equal(restored.status, 200, restored.json?.message);
  assert.deepEqual(restored.json, { imported: 1, skipped: [], grants: "approval required" });
  await replacement.stop();
  receive = await replacement.start();
  const kept = await receive("GET", vault);
  assert.equal(kept.json.state.store, "keychain");
  assert.deepEqual(kept.json.grants, []);
  const [item] = kept.json.items;
  assert.equal(item.name, name);
  assert.deepEqual(item.sites, ["example.test"]);
  assert.equal(item.signInOnly, true);
  assert.notEqual(item.id, originalItem);
  const check = await receive("POST", vault + "/items/" + item.id + "/same", { values: { password: secret } });
  assert.deepEqual(check.json, { same: true });
  const forbidden = [secret, recoveryKey];
  for (const folder of [original.home, replacement.home, disk]) checkNoPlaintext(folder, forbidden);
  for (const mac of [original, replacement]) assert.ok(forbidden.every(value => !mac.log().includes(value)));
});

function shortP521(file) {
  const sshString = value => {
    const bytes = Buffer.from(value);
    const size = Buffer.alloc(4);
    size.writeUInt32BE(bytes.length);
    return Buffer.concat([size, bytes]);
  };
  // A 64-byte positive mpint for a curve with a 66-byte scalar width.
  const scalar = randomBytes(64);
  scalar[0] = 0x40;
  const pair = createECDH("secp521r1");
  pair.setPrivateKey(scalar);
  const algorithm = "ecdsa-sha2-nistp521";
  const pub = Buffer.concat([algorithm, "nistp521", pair.getPublicKey()].map(sshString));
  write(file + ".pub", algorithm + " " + pub.toString("base64") + "\n");
  const parts = Buffer.concat([Buffer.alloc(8), pub, sshString(scalar), sshString("fixture")]);
  const padding = Buffer.from(Array.from({ length: (8 - parts.length % 8) % 8 }, (_, i) => i + 1));
  const raw = Buffer.concat([
    Buffer.from("openssh-key-v1\0"), ...["none", "none", ""].map(sshString),
    Buffer.from([0, 0, 0, 1]), sshString(pub), sshString(Buffer.concat([parts, padding])),
  ]);
  return "-----BEGIN OPENSSH PRIVATE KEY-----\n" + raw.toString("base64").match(/.{1,70}/g).join("\n") +
    "\n-----END OPENSSH PRIVATE KEY-----\n";
}

test("the conversation's SSH agent offers the vault keys and signs without releasing them", {
  skip: process.platform === "win32",
  timeout: 30000,
}, async t => {
  const root = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const { computer } = await bench(t);
  const box = await computer("V", boxBinary(), () => ({ KUVO_VAULT_AGENTS: "1", KUVO_TEST_LOGIN_ENV: "off" }));
  const { ask, proc } = await create(box, value());
  const project = path.join(root, "project");
  const other = path.join(root, "other");
  fs.mkdirSync(project);
  fs.mkdirSync(other);
  const caller = sessionOf({ id: "ses_signer", title: "Signer", directory: project });
  const stranger = sessionOf({ id: "ses_stranger", title: "Another project", directory: other });
  box.engine.sessions.push(caller, stranger);
  const socketFolder = path.join(box.home, "run/ssh-agents");
  const registrations = path.join(box.home, "run/ssh-for");
  write(path.join(registrations, caller.id), project);
  const socket = await until(() => {
    if (!fs.existsSync(socketFolder)) return;
    const socket = fs.readdirSync(socketFolder).find(name => name.endsWith(".sock"));
    return socket && path.join(socketFolder, socket);
  }, "SSH agent did not see the conversation registration");
  assert.equal(fs.lstatSync(socket).mode & 0o777, 0o600);
  const env = { ...isolatedEnv(root), SSH_AUTH_SOCK: socket };
  const secrets = [];
  const itemIds = [];
  for (const [kind, bits, format] of [
    ["ed25519", null, null], ["rsa", "2048", null], ["ecdsa", "256", null],
    ["ecdsa", "384", null], ["ecdsa", "521", null], ["rsa", "2048", "PEM"],
  ]) {
    const file = path.join(root, [kind, bits, format].filter(Boolean).join("-"));
    let secret;
    if (bits === "521") {
      secret = shortP521(file);
    } else {
      execFileSync("ssh-keygen", ["-q", "-t", kind, ...(bits ? ["-b", bits] : []),
        ...(format ? ["-m", format] : []), "-N", "", "-f", file], { env, stdio: "ignore" });
      secret = fs.readFileSync(file, "utf8");
      fs.rmSync(file);
    }
    secrets.push(secret.trim());
    const name = path.basename(file);
    const made = await ask("POST", vault + "/items", { kind: "key", name, values: { private_key: secret } }, header);
    assert.equal(made.status, 200);
    itemIds.push(made.json.item.id);
    assert.equal((await ask("POST", vault + "/grants", { item: made.json.item.id, who: { everyone: true }, how: "own" }, header)).status, 200);
    const contents = Buffer.from("Signature fixture");
    const message = path.join(root, "to-sign");
    const allowed = path.join(root, "allowed_signers");
    write(message, contents);
    write(allowed, "fixture " + fs.readFileSync(file + ".pub", "utf8"));
    await run("ssh-keygen", ["-Y", "sign", "-q", "-f", file + ".pub", "-n", "file", message], { env, timeout: 8000 });
    execFileSync("ssh-keygen", ["-Y", "verify", "-q", "-f", allowed, "-I", "fixture", "-n", "file", "-s", message + ".sig"],
      { env, input: contents, stdio: ["pipe", "ignore", "ignore"] });
    fs.rmSync(message + ".sig");
  }
  const { stdout: publicKeys } = await run("ssh-add", ["-L"], { env, timeout: 8000 });
  assert.equal(publicKeys.trim().split("\n").length, 6);

  const scoped = itemIds[0];
  assert.equal((await ask("POST", vault + "/grants", { item: scoped, who: { everyone: true }, how: null }, header)).status, 200);
  assert.equal((await ask("POST", vault + "/grants", { item: scoped, who: { project }, how: "own" }, header)).status, 200);
  const own = await run("ssh-add", ["-L"], { env, timeout: 8000 });
  assert.equal(own.stdout.trim().split("\n").length, process.platform === "darwin" ? 6 : 5);
  write(path.join(registrations, stranger.id), other);
  const otherSocket = await until(() => {
    const named = fs.readdirSync(socketFolder).find(name => path.join(socketFolder, name) !== socket && name.endsWith(".sock"));
    return named && path.join(socketFolder, named);
  }, "SSH agent did not see the second registration");
  const otherKeys = await run("ssh-add", ["-L"], { env: { ...env, SSH_AUTH_SOCK: otherSocket }, timeout: 8000 });
  assert.equal(otherKeys.stdout.trim().split("\n").length, 5);
  const activity = await ask("GET", vault + "/activity", undefined, header);
  assert.equal(activity.json.entries.filter(entry => entry.what === "used" && entry.program === "ssh").length, 6);
  checkNoPlaintext(box.home, secrets);
  assert.ok(secrets.every(secret => !proc.log().includes(secret)), "SSH agent log hides all private keys");
});

test("a vault point needs its own recovery key and restores fresh items without grants", async t => {
  const { computer } = await bench(t);
  const original = await computer("Original");
  const target = await computer("Target");
  const disk = fs.realpathSync(scratch());
  t.after(() => fs.rmSync(disk, { recursive: true, force: true }));
  const secret = value();
  const { ask, key, item } = await create(original, secret);
  const source = "b_" + randomBytes(16).toString("base64url");
  const computerKey = randomBytes(32).toString("base64url");
  const destination = { kind: "disk", path: disk };
  write(path.join(original.home, "backup/configuration.json"), JSON.stringify({
    version: 1, enabled: true, source, key: computerKey, dest: destination, projects: [],
  }), { mode: 0o600 });
  assert.equal((await ask("POST", vault + "/grants", { item: item.id, who: { everyone: true }, how: "own" }, header)).status, 200);
  const backup = "kuvo/box/backups/vault";
  const exported = await ask("POST", backup, { destination, source, confirm: true }, header);
  assert.equal(exported.status, 200, exported.json?.message);
  const { id, recoveryKey } = exported.json;
  assert.ok(Buffer.from(recoveryKey, "base64url").length === 32 && ![key, computerKey].includes(recoveryKey), "vault point has a separate key");
  const receive = target.connect(await target.start().ready());
  assert.equal((await receive("POST", vault + "/key", {}, header)).status, 200);
  for (const [key, status] of [[computerKey, 409], [recoveryKey, 200]]) {
    const reply = await receive("POST", backup + "/import", { destination, source, id, recoveryKey: key, confirm: true }, header);
    assert.equal(reply.status, status, reply.json?.message);
    if (status === 200) assert.deepEqual(reply.json, { imported: 1, skipped: [], grants: "approval required" });
  }
  const restored = await receive("GET", vault, undefined, header);
  assert.deepEqual(restored.json.grants, []);
  const restoredItem = restored.json.items[0];
  assert.equal(restoredItem.name, item.name);
  assert.notEqual(restoredItem.id, item.id);
  assert.deepEqual((await receive("POST", vault + "/items/" + restoredItem.id + "/same", {
    values: { password: secret },
  }, header)).json, { same: true });
  checkNoPlaintext(original.home, [secret]);
  checkNoPlaintext(target.home, [secret]);
  checkNoPlaintext(disk, [secret, recoveryKey]);
});
