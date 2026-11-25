// A real, signed-in Codex turn on GPT-6-Luna kept going across a Rust-to-Rust release switch.
// Set KUVO_CONFORMANCE_CODEX to the Codex CLI to run, and KUVO_CONFORMANCE_CODEX_AUTH to a Codex
// sign-in (`auth.json`): it's copied, read-only, into the case's scratch Codex home and goes with
// it. Codex is given nothing else of the computer's: a scratch HOME, CODEX_HOME and PATH.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { keptListener, stopFixtureHolders, stopPids, transcript, twoReleases } from "./claude.mjs";
import { boxBinary, boxCommand, isolatedEnv, localRelay, privateHome, request, scratch, until, write } from "./adapter.mjs";
import { provider, seedCodexSession } from "./codex.mjs";

const program = process.env.KUVO_CONFORMANCE_CODEX;
const signIn = process.env.KUVO_CONFORMANCE_CODEX_AUTH;
/** The model, by its id or name (a dry run of the case names the stand-in's). */
const wanted = new RegExp(process.env.KUVO_CONFORMANCE_CODEX_MODEL || "gpt-6-luna", "i");
const binary = boxBinary();
const headers = { "x-kuvo-from": "app" };

function heldCodex(home) {
  const folder = path.join(home, "run", "agents", "hold");
  if (!fs.existsSync(folder)) return undefined;
  return fs.readdirSync(folder).filter((name) => /^[a-f\d]{12}\.json$/.test(name)).flatMap((name) => {
    try { return [JSON.parse(fs.readFileSync(path.join(folder, name), "utf8"))]; } catch { return []; }
  }).find((record) => record.meta?.provider === provider && record.meta?.kind === "codex");
}

/** Approves the first card up, once; whether one was. */
async function approveAny(listener, route, approved) {
  const answer = await request(listener, "GET", route + "/permission", undefined, headers);
  const card = answer.json?.data?.find((each) => !approved.has(each.id));
  if (!card) return false;
  approved.add(card.id);
  const replied = await request(listener, "POST", `${route}/permission/${encodeURIComponent(card.id)}/reply`, { decision: "once" }, headers);
  assert.ok(replied.status < 300, replied.text);
  return true;
}

test("a real Codex turn on GPT-6-Luna goes on across a Rust release switch, in the same app-server", {
  skip: (!(program && signIn) && "Set KUVO_CONFORMANCE_CODEX and KUVO_CONFORMANCE_CODEX_AUTH.")
    || (binary.typescript && "A Rust-to-Rust release switch uses the Rust holder and resident."),
  timeout: 420000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  const project = path.join(root, "project");
  const proof = path.join(project, "kuvo-live-proof.txt");
  const relay = await localRelay();
  const session = "ses_codex_live";
  const route = `kuvo/agents/api/session/${session}`;
  const residents = [];
  const approved = new Set();
  let keeper;
  try {
    privateHome(home);
    fs.mkdirSync(project);
    const base = isolatedEnv(root);
    const codexHome = path.join(base.HOME, ".codex");
    fs.mkdirSync(codexHome, { recursive: true, mode: 0o700 });
    // Read-only: a refresh can't write it back.
    fs.copyFileSync(signIn, path.join(codexHome, "auth.json"));
    fs.chmodSync(path.join(codexHome, "auth.json"), 0o400);
    const bin = path.join(base.HOME, ".local", "bin");
    fs.mkdirSync(bin, { recursive: true });
    fs.writeFileSync(path.join(bin, "codex"), `#!/bin/sh\nexec "${program}" "$@"\n`, { mode: 0o755 });
    const env = { ...base, CODEX_HOME: codexHome, KUVO_TEST_PATH_ONLY: "1", KUVO_SANDBOX: "off", PATH: [bin, "/usr/bin:/bin"].join(path.delimiter) };
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    const { old, next, control } = twoReleases(binary, home, env, relay);
    try {
      await control(["select", old]);
      keeper = boxCommand(binary, home, env, relay, ["keep"]);
      const first = await keptListener(home, keeper);
      residents.push(first.pid);
      const offered = await request(first, "GET", "kuvo/models/offered?refresh=1", undefined, headers, 90000);
      assert.equal(offered.status, 200, offered.text);
      const models = offered.json.models.filter((model) => model.providerID === provider);
      const luna = models.find((model) => wanted.test(model.id) || wanted.test(model.name ?? ""));
      assert.ok(luna, wanted + " isn't offered: " + JSON.stringify(models.map((model) => [model.id, model.name])));
      seedCodexSession(home, session, { directory: project, model: luna.id });
      const started = await request(first, "POST", route + "/prompt", {
        id: "msg_codex_live",
        text: "Run exactly this shell command, and wait for it to finish: sleep 25 && touch kuvo-live-proof.txt\n"
          + "Then reply with only the word: finished",
      }, headers, 60000);
      assert.equal(started.status, 200, started.text);
      // Its command under way (or asked about): the turn is the held app-server's now.
      let seen = [];
      const initial = await until(async () => {
        await approveAny(first, route, approved);
        seen = await transcript(first, route);
        assert.notEqual(seen.at(-1)?.type, "idle", "The turn ended before the switch: " + JSON.stringify(seen));
        const held = heldCodex(home);
        const running = seen.some((row) => (row.content ?? []).some((part) => part.type === "tool" && part.state?.status === "running"));
        return running && held && Object.keys(held.meta?.turns ?? {}).length && held;
      }, () => "Codex never ran its command. " + JSON.stringify(seen), 180000);
      assert.equal(fs.existsSync(proof), false);

      await control(["select", next]);
      const second = await keptListener(home, keeper, first.pid);
      residents.push(second.pid);
      let rows = [];
      await until(async () => {
        await approveAny(second, route, approved);
        rows = await transcript(second, route);
        return rows.at(-1)?.type === "idle";
      }, () => "The replacement resident never finished the held turn. " + JSON.stringify(rows.slice(-3)), 180000);
      assert.equal(rows.at(-1).outcome, "succeeded", JSON.stringify(rows.slice(-3)));
      assert.ok(fs.existsSync(proof), "Codex's command never finished.");
      const said = rows.filter((row) => row.type === "assistant").flatMap((row) => row.content ?? [])
        .filter((part) => part.type === "text").map((part) => part.text).join(" ");
      assert.match(said, /finished/i, JSON.stringify(rows.slice(-3)));
      assert.equal(rows.filter((row) => row.type === "user").length, 1);
      const after = heldCodex(home);
      assert.ok(after, "The holder ended with its turn.");
      // The same app-server all along, its holder taken up by the next resident.
      assert.deepEqual([after.pid, after.child], [initial.pid, initial.child]);
      assert.ok(after.generation > initial.generation);
      console.log(`Live: ${luna.id} (${luna.name}), ${approved.size} approval(s), holder ${initial.pid}/${initial.child}.`);
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
