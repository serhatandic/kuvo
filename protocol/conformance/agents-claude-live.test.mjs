// A real, signed-in Claude Code turn held open across a Rust-to-Rust release switch.
// Set KUVO_CONFORMANCE_CLAUDE to an executable that runs Claude Code for the scratch HOME it's
// given, signed in to a test account, isolated from the computer's own Claude sign-in and limited
// to Haiku. The case installs it as that HOME's `claude` and passes it no credential itself.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import {
  headers, heldClaude, keptListener, seedSession, stopFixtureHolders, stopPids, transcript, twoReleases,
} from "./claude.mjs";
import { boxBinary, boxCommand, isolatedEnv, localRelay, privateHome, request, scratch, until, write } from "./adapter.mjs";

const launcher = process.env.KUVO_CONFORMANCE_CLAUDE;
const binary = boxBinary();

test("a real Claude Code turn on Haiku waits on an approval across a Rust release switch", {
  skip: (!launcher && "Set KUVO_CONFORMANCE_CLAUDE to an isolated, signed-in Claude Code launcher.")
    || (binary.typescript && "A Rust-to-Rust release switch uses the Rust holder and resident."),
  timeout: 300000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  const project = path.join(root, "project");
  const proof = path.join(project, "kuvo-live-proof.txt");
  const relay = await localRelay();
  const session = "ses_claude_live";
  const route = `kuvo/agents/api/session/${session}`;
  const residents = [];
  let keeper;
  try {
    privateHome(home);
    fs.mkdirSync(project);
    const base = isolatedEnv(root);
    const bin = path.join(base.HOME, ".local", "bin");
    if (process.platform === "win32") {
      write(path.join(bin, "claude.cmd"), `@"${launcher}" %*\r\n`);
    } else {
      fs.mkdirSync(bin, { recursive: true });
      fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\nexec "${launcher}" "$@"\n`, { mode: 0o755 });
    }
    const system = process.platform === "win32" ? path.join(base.SystemRoot, "System32") : "/usr/bin:/bin";
    const env = { ...base, KUVO_TEST_PATH_ONLY: "1", KUVO_SANDBOX: "off", PATH: [bin, system].join(path.delimiter) };
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    // Ask First, as the clients set it: a command is a card before it runs.
    const askFirst = ["edit", "write", "patch", "shell"].map(action => ({ action, resource: "*", effect: "ask" }));
    seedSession(home, session, { directory: project, model: "haiku", permissions: askFirst });
    const { old, next, control } = twoReleases(binary, home, env, relay);
    try {
      await control(["select", old]);
      keeper = boxCommand(binary, home, env, relay, ["keep"]);
      const first = await keptListener(home, keeper);
      residents.push(first.pid);
      const started = await request(first, "POST", route + "/prompt", {
        id: "msg_claude_live",
        text: "Use the Bash tool to run exactly this command: touch kuvo-live-proof.txt\n"
          + "Then reply with only the word: finished",
      }, headers, 60000);
      assert.equal(started.status, 200, started.text);
      let seen = [];
      const asked = await until(async () => {
        const answer = await request(first, "GET", route + "/permission", undefined, headers);
        assert.equal(answer.status, 200, answer.text);
        if (answer.json.data[0]) return answer.json.data[0];
        seen = await transcript(first, route);
        assert.notEqual(seen.at(-1)?.type, "idle", "The turn ended without asking: " + JSON.stringify(seen));
      }, () => "Claude never asked to run the command. " + JSON.stringify(seen), 120000);
      assert.equal(fs.existsSync(proof), false);
      const initial = await until(() => heldClaude(home, session), "Claude's holder was never recorded.");
      assert.equal(initial.meta.turns.main.turn, "msg_claude_live");

      await control(["select", next]);
      const second = await keptListener(home, keeper, first.pid);
      residents.push(second.pid);
      const again = await until(async () => {
        const answer = await request(second, "GET", route + "/permission", undefined, headers);
        assert.equal(answer.status, 200, answer.text);
        return answer.json.data.find(card => card.id === asked.id);
      }, "The replacement resident never showed Claude's open approval again.", 60000);
      const replied = await request(second, "POST", `${route}/permission/${encodeURIComponent(again.id)}/reply`,
        { decision: "once" }, headers);
      assert.equal(replied.status, 200, replied.text);
      const rows = await until(async () => {
        const rows = await transcript(second, route);
        return rows.at(-1)?.type === "idle" && rows;
      }, "The replacement resident never finished the held turn.", 180000);
      assert.equal(rows.at(-1).outcome, "succeeded", JSON.stringify(rows));
      assert.ok(fs.existsSync(proof), "Claude's approved command never ran.");
      const said = rows.filter(row => row.type === "assistant").flatMap(row => row.content ?? [])
        .filter(part => part.type === "text").map(part => part.text).join(" ");
      assert.match(said, /finished/i, JSON.stringify(rows));
      const after = heldClaude(home, session);
      assert.ok(after, "The holder ended with its turn.");
      assert.deepEqual([after.pid, after.child, after.generation], [initial.pid, initial.child, initial.generation + 1]);
      const saved = JSON.parse(fs.readFileSync(path.join(home, "agents/sessions", session, "agent.json"), "utf8"));
      assert.match(saved.threads["claude-code"].state.claudeSession, /^[a-f0-9-]{36}$/i);
    } finally {
      await control(["off"]);
    }
  } finally {
    await keeper?.stop();
    await stopPids(residents);
    await stopFixtureHolders(home);
    await relay.close();
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  }
});
