import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { NO_BUN, testBun } from "./bun-for-tests.js";

const bun = testBun();
const linkScript = LINK_BUNDLE;
const bridgeScript = new URL("../dist/kuvo-agents.js", import.meta.url);

test("Work Queue GitHub keeps complete located replies through the bridge and the link", { skip: bun ? false : NO_BUN, timeout: 30_000 }, async () => {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-queue-gh-"));
  removeAtEnd(scratch);
  const home = path.join(scratch, ".kuvo-code");
  const workspace = path.join(home, "workspace");
  const bin = path.join(home, "bin");
  const run = path.join(home, "run");
  for (const folder of [workspace, bin, run]) fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(bin, "gh"), `#!/bin/sh
case "$1/$2" in
  auth/status) exit 1 ;;
  auth/login)
    printf 'One-time code: ABCD-1234\nhttps://github.com/login/device\n' >&2
    read -r waiting
    ;;
  *) exit 2 ;;
esac
`, { mode: 0o700 });

  const env = {
    HOME: scratch, TMPDIR: scratch, PATH: `${bin}${path.delimiter}/usr/bin${path.delimiter}/bin`, BUN_BE_BUN: "1",
    KUVO_HOME: home, KUVO_TEST_PRICE_FEEDS: "off", KUVO_TEST_PATH_ONLY: "1", KUVO_AGENTS_HOME: path.join(home, "agents"),
    KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_PASSWORD: "fixture", KUVO_SANDBOX: "off",
  };
  const start = async (script, args, cwd, file) => {
    const child = spawn(bun, ["--config=/dev/null", "--no-env-file", fileURLToPath(script), ...args], { cwd, env, stdio: ["ignore", "ignore", "pipe"] });
    const done = once(child, "exit");
    let stderr = "";
    child.stderr.on("data", chunk => { stderr += chunk; });
    atEnd(async () => {
      if (child.exitCode === null && child.signalCode === null) child.kill();
      await done;
    });
    for (let tries = 0; tries < 100; tries++) {
      if (child.exitCode !== null || child.signalCode !== null) break;
      try {
        const record = JSON.parse(fs.readFileSync(path.join(run, file), "utf8"));
        if (record.pid === child.pid) return record;
      } catch {}
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    assert.fail(`No ${file}: ${stderr}`);
  };
  await start(linkScript, ["box-services"], home, "box-services.json");
  const listener = net.createServer();
  await new Promise(resolve => listener.listen(0, "127.0.0.1", resolve));
  env.KUVO_AGENTS_PORT = String(listener.address().port);
  await new Promise(resolve => listener.close(resolve));
  const bridge = await start(bridgeScript, [], workspace, "bridge.json");
  process.env.KUVO_HOME = home;
  const { engineHandler } = await import(linkScript);
  const link = engineHandler(() => { throw new Error("This route needs no engine"); }, {}, () => bridge);

  const directory = fs.realpathSync(workspace);
  const client = "a".repeat(32);
  const legacy = async (method, route, body) => {
    const response = await fetch(`http://127.0.0.1:${bridge.port}/${route}`, {
      method, body: body && JSON.stringify(body), signal: AbortSignal.timeout(8_000),
      headers: {
        authorization: `Basic ${Buffer.from(`opencode:${bridge.password}`).toString("base64")}`,
        "content-type": "application/json", "x-kuvo-sign-in-owner": `client:${client}`,
      },
    });
    return { status: response.status, json: await response.json() };
  };
  const throughLink = async (method, route, body) => {
    const parts = [];
    for await (const piece of link({ method, path: route, headers: { "x-kuvo-sign-in-client": client }, body: Buffer.from(body ? JSON.stringify(body) : "") }, {}, AbortSignal.timeout(8_000))) {
      parts.push(piece);
    }
    return { status: parts[0].head.status, json: JSON.parse(Buffer.concat(parts.filter(piece => piece.body).map(piece => piece.body)).toString()) };
  };
  const located = data => ({ status: 200, json: { location: { directory }, data } });
  const route = "api/integration/github-work-queue";
  const oauth = `${route}/connect/oauth`;
  const url = "https://github.com/login/device";
  const userCode = "ABCD-1234";
  const integration = located({ id: "github-work-queue", name: "GitHub for Work Queue", metadata: { source: "work-queue" },
    methods: [{ type: "oauth", id: "device", label: "Sign in with GitHub" }], connections: [] });
  assert.deepEqual(await legacy("GET", route), integration);
  assert.deepEqual(await throughLink("GET", route), integration);

  const started = await legacy("POST", oauth, { methodID: "device" });
  const { attemptID, time } = started.json.data;
  assert.match(attemptID, /^att_[a-zA-Z0-9]+$/);
  assert.equal(time.expires - time.created, 15 * 60_000);
  assert.deepEqual(started, located({ attemptID, url, userCode, mode: "auto", time,
    instructions: "Open GitHub, sign in, and enter the code shown here." }));
  assert.deepEqual(await throughLink("POST", oauth, { methodID: "device" }), started);

  const status = located({ status: "pending", userCode, url });
  assert.deepEqual(await legacy("GET", `${oauth}/${attemptID}`), status);
  assert.deepEqual(await throughLink("GET", `${oauth}/${attemptID}`), status);
  assert.deepEqual(await legacy("DELETE", `${oauth}/${attemptID}`), { status: 200, json: {} });
});
