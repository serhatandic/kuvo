// One real Antigravity turn on Gemini 4 Hydrogen through the Rust box, in a temp home with only
// read-only copies of the sign-in files needed (never refreshing a token, deleted after).
// Set KUVO_CONFORMANCE_AGY to the `agy` binary and KUVO_CONFORMANCE_AGY_PROFILE to the profile
// folder containing `antigravity-acp/antigravity-oauth-token` and `antigravity-acp/kuvo-sign-in.json`.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  boxBinary, isolatedEnv, localRelay, openLine, privateHome, request, scratch, startBox, until, write,
} from "./adapter.mjs";
import { stopFixtureHolders } from "./claude.mjs";

const program = process.env.KUVO_CONFORMANCE_AGY;
const sourceProfile = process.env.KUVO_CONFORMANCE_AGY_PROFILE;
const modelId = process.env.KUVO_CONFORMANCE_AGY_MODEL || "gemini-4-hydrogen";
const provider = "antigravity";
const headers = { "x-kuvo-from": "app" };

function digestFile(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function copyPrivate(from, to, mode = 0o400) {
  fs.mkdirSync(path.dirname(to), { recursive: true, mode: 0o700 });
  fs.copyFileSync(from, to);
  fs.chmodSync(to, mode);
}

test("a real Antigravity turn on Gemini 4 Hydrogen streams to the app's feed and ends cleanly", {
  skip: !(program && sourceProfile) && "Set KUVO_CONFORMANCE_AGY and KUVO_CONFORMANCE_AGY_PROFILE.",
  timeout: 240000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  const work = path.join(root, "work");
  const relay = await localRelay();
  let box, line;
  try {
    privateHome(home);
    fs.mkdirSync(work, { recursive: true });
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    write(path.join(home, "box", "opencode.json"), "{}\n");

    // Copy only the files `agy` and the box need, read-only, mode 0400/0600; never log any token.
    const dstProfile = path.join(home, "agents", "antigravity", "profile");
    const srcAcp = path.join(sourceProfile, "antigravity-acp");
    const dstAcp = path.join(dstProfile, "antigravity-acp");
    const srcToken = path.join(srcAcp, "antigravity-oauth-token");
    const dstToken = path.join(dstAcp, "antigravity-oauth-token");
    assert.ok(fs.existsSync(srcToken), "Source profile is missing antigravity-oauth-token.");
    assert.ok(fs.existsSync(path.join(srcAcp, "kuvo-sign-in.json")), "Source profile is missing kuvo-sign-in.json.");
    copyPrivate(srcToken, dstToken, 0o400);
    copyPrivate(path.join(srcAcp, "kuvo-sign-in.json"), path.join(dstAcp, "kuvo-sign-in.json"), 0o400);
    if (fs.existsSync(path.join(srcAcp, "settings.json"))) {
      copyPrivate(path.join(srcAcp, "settings.json"), path.join(dstAcp, "settings.json"), 0o400);
    }
    if (fs.existsSync(path.join(sourceProfile, "kuvo-models.json"))) {
      copyPrivate(path.join(sourceProfile, "kuvo-models.json"), path.join(dstProfile, "kuvo-models.json"), 0o600);
    }
    const tokenBefore = {
      digest: digestFile(dstToken),
      size: fs.statSync(dstToken).size,
      mtimeMs: fs.statSync(dstToken).mtimeMs,
    };

    const base = isolatedEnv(root);
    const bin = path.join(base.HOME, ".local", "bin");
    fs.mkdirSync(bin, { recursive: true });
    const realProgram = fs.realpathSync(program);
    if (process.platform === "win32") {
      fs.writeFileSync(path.join(bin, "agy.cmd"), `@"${realProgram}" %*\r\n`);
    } else {
      fs.writeFileSync(path.join(bin, "agy"), `#!/bin/sh\nexec "${realProgram}" "$@"\n`, { mode: 0o755 });
    }
    const system = process.platform === "win32" ? path.join(base.SystemRoot, "System32") : "/usr/bin:/bin";
    const env = {
      ...base,
      KUVO_TEST_PATH_ONLY: "1",
      KUVO_SANDBOX: "off",
      KUVO_TEST_PRICE_FEEDS: "off",
      PATH: [bin, system].join(path.delimiter),
    };

    box = startBox({ binary: boxBinary(), home, env, relay });
    const listener = await box.ready();
    const call = (method, route, body) => request(listener, method, route, body, headers, 60000);

    // Follow the app's agents feed on the line before starting the turn.
    const watch = Buffer.from(JSON.stringify({ agents: "kuvo/agents/api/event" })).toString("base64url");
    line = await openLine(listener, "?watch=" + watch);
    const feedEvents = [];
    let connected = false;
    const reader = (async () => {
      try {
        for await (const frame of line.frames()) {
          if (frame?.t !== "agents" || !frame.d?.type) continue;
          if (frame.d.type === "server.connected") connected = true;
          feedEvents.push(frame.d);
        }
      } catch {
        // Closed on teardown.
      }
    })();
    await until(() => connected, "The agents feed never connected.", 15000);

    const accounts = await call("GET", "kuvo/agents/api/integration");
    assert.equal(accounts.status, 200, accounts.text);
    const row = accounts.json.data.find((each) => each.id === provider);
    assert.ok(row && row.connections?.length > 0, `Antigravity wasn't signed in: ${JSON.stringify(row)}`);

    const offered = await call("GET", "kuvo/models/offered");
    assert.equal(offered.status, 200, offered.text);
    const model = offered.json.models.find((each) => each.providerID === provider && each.id === modelId);
    assert.ok(model, `${modelId} wasn't offered: ${JSON.stringify(offered.json.models.filter((m) => m.providerID === provider).map((m) => m.id))}`);

    const started = await call("POST", "kuvo/sessions", {
      folder: work,
      model: { providerID: provider, id: modelId },
      approval: "askFirst",
    });
    assert.equal(started.status, 200, started.text);
    const id = started.json.info?.id ?? started.json.id;
    assert.ok(id, started.text);

    const sent = await call("POST", `kuvo/sessions/${id}/send`, {
      id: `msg_${Date.now().toString(36)}agylive`,
      words: "Reply with the word OK.",
      delivery: "steer",
    });
    assert.equal(sent.status, 200, sent.text);

    let rows = [];
    await until(async () => {
      rows = (await call("GET", `kuvo/agents/api/session/${id}/message?limit=50&order=asc`)).json?.data ?? [];
      return rows.some((each) => each.type === "idle");
    }, () => `The Antigravity turn never ended: ${JSON.stringify(rows).slice(-1200)}\n${box.log()}`, 180000, 500);

    const idle = rows.filter((each) => each.type === "idle").at(-1);
    assert.equal(idle.outcome ?? "succeeded", "succeeded", JSON.stringify(idle));
    const said = rows
      .filter((each) => each.type === "assistant")
      .flatMap((each) => each.content ?? [])
      .filter((part) => part.type === "text")
      .map((part) => part.text)
      .join("");
    assert.match(said, /\bOK\b/i, `Unexpected reply: ${JSON.stringify(said)}`);

    // Prove the turn lifecycle streamed to the app's feed.
    await until(
      () => feedEvents.some((ev) => ev.type === "session.idle" && ev.data?.sessionID === id),
      () => `Feed never saw session.idle for ${id}: ${JSON.stringify(feedEvents.map((e) => [e.type, e.data?.sessionID]))}`,
      10000,
    );
    const types = feedEvents.filter((ev) => ev.data?.sessionID === id).map((ev) => ev.type);
    assert.ok(types.includes("session.execution.started"), `Missing execution.started in feed: ${JSON.stringify(types)}`);
    assert.ok(types.includes("session.execution.succeeded"), `Missing execution.succeeded in feed: ${JSON.stringify(types)}`);
    assert.ok(types.includes("session.idle"), `Missing session.idle in feed: ${JSON.stringify(types)}`);

    // Prove no token refresh modified the copied token file.
    const tokenAfter = {
      digest: digestFile(dstToken),
      size: fs.statSync(dstToken).size,
      mtimeMs: fs.statSync(dstToken).mtimeMs,
    };
    assert.deepEqual(tokenAfter, tokenBefore, "The copied OAuth token was modified during the run!");
    console.log(`Live Antigravity: ${model.id} (${model.name}), reply=${JSON.stringify(said.trim())}, feed=${JSON.stringify(types)}`);
    line.close();
    await reader;
  } finally {
    line?.close();
    await box?.stop();
    await stopFixtureHolders(home);
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
    assert.equal(fs.existsSync(root), false, `Temp home wasn't deleted: ${root}`);
  }
});
