// Secondary storage writers for the current-beta home. Every external service
// this generator needs is replaced with a local source of synthetic input.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { Database } from "bun:sqlite";
import { execFileSync } from "node:child_process";
import { gzipSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { Accounts } from "../../../box/src/agents/accounts";
import { saveMacConfig } from "../../../box/src/link/mac/link";
import { refreshOwn } from "../../../box/src/link/peers/agents";
import { setWelcomed } from "../../../box/src/link/setup/welcomed";
import { keepRecord } from "../../../box/src/link/ownership/store";
import { noteRecord, writeJournal } from "../../../box/src/link/rehome/state";
import { noteUpdate } from "../../../box/src/link/updates/update";
import { relayBackupAct } from "../../../box/src/link/backup/relay-store";
import { takeSeed } from "../../../box/src/link/add/seed";
import { addRule } from "../../../box/src/link/fence/allowed";
import { ArtifactStore } from "../../../box/src/link/artifacts/store";
import { seedRoutine } from "../../../box/src/link/areas/wakes";
import { readLink, writeLink } from "../../../box/src/link/state";
import { refreshPhoneTrust } from "../../../box/src/link/documents/phone-sync";
import { CursorClient } from "../../../box/src/usage/cursor-api";
import { CursorHistory } from "../../../box/src/usage/cursor-history";
import { makeBot } from "../../../box/src/link/bots/make";
import { keptFiles } from "../../../box/src/link/server/attached";
import { changeBrowserSettings } from "../../../box/src/link/browser/settings";
import { mergeHistory, visited } from "../../../box/src/link/browser/history";
import { writeServeRecord } from "../../../box/src/link/serve-kept";
import { writePrivate } from "../../../box/src/link/files";
import { tarOf } from "../../../box/src/link/add/files";
import { keepRelease } from "../../../box/src/link/updates/fleet";
import { verifyBoxes } from "../../../box/src/link/updates/update";
import { signWith, signingKeyOf } from "../../../box/src/link/documents/format";
import { updateWorktrees } from "../../../box/src/link/areas/worktree-records";
import { changeSettings } from "../../../box/src/link/areas/config";
import { Limits, agentAccounts, limitsRoutes } from "../../../box/src/usage/limits";
import { UsageAccounts } from "../../../box/src/usage/accounts";
import { standInEngine } from "../engine.mjs";
import { keysIn, trustHostKeys } from "../../../box/src/link/add/ssh";
import { setProjectsFolder } from "../../../box/src/link/work/settings";
import { CursorStandIn } from "../../../box/test/agents/support/cursor-standin";

const engineSchema = fileURLToPath(new URL("../../../box/test/fixtures/rehome/engine-2.2.6.sql", import.meta.url));
const put = (file: string, contents: string | Buffer) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, contents, { mode: 0o600 });
};

export async function writeExtra(home: string, directory: string, own: string, setup: string, now: number) {
  saveMacConfig({ relay: "ws://127.0.0.1:1", phones: false, machines: false, name: "Golden Mac", app: "0.8.0" });
  setWelcomed(true);
  keepRecord("edit-deletions", "wqi_golden12345678901234", { id: "wqi_golden12345678901234", at: now, owner: "usr_you" });
  noteUpdate({ round: "2025-10-09/golden" });
  noteRecord(home, { state: "done", way: "move", counts: { sessions: 1 } });
  writeJournal(home, { v: 1, way: "move", began: now, step: "done", from: directory, to: home,
    placements: [], clearing: [], kept: path.join(home, "updates/rehome/kept") });
  const link = readLink()!;
  try {
    // A standby refresh writes the process-scoped trust cache. Restore the
    // primary projection before the generated home is opened.
    writeLink({ ...link, setup: { ...link.setup!, role: "member", primary: Buffer.alloc(32, 50).toString("base64url") } });
    refreshPhoneTrust({ welcomed: true, send() {} } as never);
  } finally {
    writeLink(link, true);
  }

  const source = "b_" + Buffer.alloc(16, 35).toString("base64url");
  const point = "2025-10-09T085320.000Z-daily-0000000000000001";
  const sender = { key: own, kind: "mac" as const };
  const deposit = await relayBackupAct(sender, { id: "golden", at: now, method: "POST", item: source,
    body: { action: "begin", id: point, setup, size: 16, sha256: crypto.createHash("sha256").update(Buffer.alloc(16)).digest("hex") } });
  assert.equal(deposit.status, 200, JSON.stringify(deposit.value));

  const engine = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) {
    if (new URL(request.url).pathname !== "/api/credential") return new Response(null, { status: 404 });
    return Response.json(request.method === "GET" ? [] : { id: "fixture-credential" });
  } });
  try {
    assert.equal((await takeSeed(JSON.stringify({ credentials: [{ integrationID: "fixture", value: { type: "api", key: "fixture-only" } }] }),
      () => ({ port: engine.port!, password: "fixture", pid: process.pid }))).taken, 1);
  } finally {
    engine.stop(true);
  }

  const attached = await keptFiles([{ name: "golden.txt", uri: "data:text/plain;base64,R29sZGVuIGF0dGFjaG1lbnQK" }],
    "ses_golden", async () => []);
  assert.equal((attached?.included.length ?? 0) + (attached?.paths.length ?? 0), 1);
  const botWords = fileURLToPath(new URL("../../../box/src/shared/bot-words.json", import.meta.url));
  put(path.join(home, "bin/bot-words.json"), fs.readFileSync(botWords));
  const botEngine = await standInEngine(path.dirname(home), process.env);
  try {
    const result = await makeBot({ name: "Golden Bot", project: directory, template: "news", model: { providerID: "opencode", id: "golden-model" } },
      () => botEngine.record, () => null);
    assert.equal(result.status, 201, JSON.stringify(result.value));
    await refreshOwn(() => botEngine.record);
  } finally {
    await botEngine.close();
  }
  const worktree = path.join(path.dirname(directory), "worktree");
  const git = (...args: string[]) => execFileSync("git", ["-C", directory, ...args], {
    env: { ...process.env, GIT_AUTHOR_DATE: "2025-10-09T08:53:20Z", GIT_COMMITTER_DATE: "2025-10-09T08:53:20Z" },
    stdio: "pipe",
  });
  git("init", "-q", "-b", "main");
  git("add", "README.md");
  git("commit", "-qm", "Golden project");
  git("worktree", "add", "-qb", "golden", worktree);
  updateWorktrees(() => [{ folder: worktree, project: directory, root: worktree, branch: "golden", base: "main" }]);
  assert.equal(changeSettings([{ path: ["mcp", "fixture"], value: { type: "local", command: ["false"], enabled: false } }]).status, 200);
  const accounts = new Accounts(path.join(home, "agents"));
  const account = accounts.add("codex", "Golden account", "codex/accounts/golden");
  // The provider auth file is opaque input to Kuvo.
  put(path.join(accounts.home(account)!, "auth.json"), '{"OPENAI_API_KEY":"fixture-only"}\n');

  const routine = path.join(home, "wakes/ses_golden");
  fs.mkdirSync(path.join(routine, "wakes"), { recursive: true });
  seedRoutine(routine, { title: "Golden routine", note: "Keep this schedule.", every: "1w" }, now);
  const allowed = path.join(path.dirname(directory), "output");
  fs.mkdirSync(allowed);
  assert.equal((await addRule({ project: directory, kind: "write", path: allowed, by: "fixture" })).kind, "write");

  const artifacts = path.join(home, "agents/artifacts");
  put(path.join(artifacts, "key"), Buffer.alloc(32, 36));
  const artifactStore = new ArtifactStore(artifacts, { now: () => new Date(now) });
  assert.ok(await artifactStore.shown(path.join(directory, "README.md"), { by: { kind: "you" }, via: "fixture" }));
  await artifactStore.settled();

  const cursor = new CursorStandIn();
  try {
    cursor.keys.set("fixture-cursor", "auth0|golden");
    const client = new CursorClient(() => "fixture-cursor", () => now);
    const cursorHome = path.join(home, "agents/cursor/accounts/golden");
    put(path.join(cursorHome, "auth.json"), JSON.stringify({ version: 1, backendUrl: "https://api2.cursor.sh", apiKey: "fixture-cursor", createdAtMs: now }));
    const account = new UsageAccounts(path.join(home, "agents")).source("cursor", "cursor@golden", { id: "golden", home: cursorHome, label: "Golden" });
    const limits = new Limits(() => agentAccounts([account]), path.join(home, "agents/usage/limits.json"), () => now);
    const reply = await limitsRoutes(limits, "POST", ["refresh"]);
    assert.equal(reply.status, 200);
    const { data } = await reply.json() as { data: { accounts: { plan: string }[] } };
    assert.deepEqual(data.accounts.map(row => row.plan), ["Pro"]);
    const days = await new CursorHistory(path.join(home, "agents/usage/cursor-history"), { concurrency: 1 }).read(client, now);
    assert.equal(days.size, 32);
  } finally {
    cursor.stop();
  }

  assert.deepEqual(setProjectsFolder(directory), { folder: directory });
  const sshBlob = Buffer.concat([Buffer.from("0000000b", "hex"), Buffer.from("ssh-ed25519"),
    Buffer.from("00000020", "hex"), Buffer.alloc(32, 43)]).toString("base64");
  trustHostKeys(keysIn(`golden.invalid ssh-ed25519 ${sshBlob}`));

  assert.equal(changeBrowserSettings({ offersToSave: false, never: ["golden.invalid"] }), null);
  assert.equal(mergeHistory(visited([], "https://golden.invalid/", "Golden page", now)), 1);
  writeServeRecord({ pid: 2147483647, port: 1, kept: false, password: "stale-fixture", startedAt: now });
  writePrivate(path.join(home, "run/serve.needed"), "");
  writePrivate(path.join(home, "run/fixture.log"), "fixture: previous resident stopped cleanly\n");

  // A fixture-only signed release follows the same pack and cache contracts.
  const file = Buffer.from("0.8.0-beta.1\n");
  const sha = (value: Buffer) => crypto.createHash("sha256").update(value).digest("hex");
  const meta = { version: "0.8.0-beta.1", engine: "2.2.6", files: { "bin/box-version": sha(file) } };
  const pack = gzipSync(tarOf([{ path: "box.json", mode: 0o644, data: Buffer.from(JSON.stringify(meta)) },
    { path: "bin/box-version", mode: 0o644, data: file }]));
  const bytes = Buffer.from(JSON.stringify({ kuvo: "box/1", version: meta.version, engine: meta.engine,
    pack: { url: "http://127.0.0.1:1/box.tar.gz", sha256: sha(pack), size: pack.length } }));
  const signingSeed = Buffer.alloc(32, 39).toString("base64url");
  const key = Buffer.from(signingKeyOf(signingSeed), "base64url").toString("base64");
  const signature = Buffer.from(signWith(signingSeed, Buffer.concat([Buffer.from("kuvo-box v1\n"), bytes])), "base64url").toString("base64");
  const release = JSON.stringify({ manifest: bytes.toString("base64"), signature, key });
  assert.equal(verifyBoxes(release, [key]).pack.sha256, sha(pack));
  assert.equal(keepRelease(release, pack), true);

  // These files belong to Chromium, the desktop, Grok and the retired Mac.
  // Kuvo's contract is to keep the supplied bytes in their native locations.
  put(path.join(home, "browser/profile/Default/Preferences"), '{"browser":{"check_default_browser":false}}\n');
  put(path.join(home, "desktop/settings.json"), '{"quitNoteSeen":true}\n');
  put(path.join(home, "agents/grok/accounts/golden/sessions/session_search.sqlite"), Buffer.alloc(0));
  const search = new Database(path.join(home, "agents/grok/accounts/golden/sessions/session_search.sqlite"));
  try { search.exec("CREATE VIRTUAL TABLE search USING fts5(body); INSERT INTO search VALUES ('Golden conversation');"); }
  finally { search.close(); }
  const app = path.join(process.env.HOME!, "Library/Application Support/run.kuvo.code.trial713");
  put(path.join(app, "phone-link.json"), '{"phones":[],"machines":{}}\n');

  // OpenCode owns this schema. The existing SQL fixture came from engine 2.2.6.
  const dbFile = path.join(home, "data/opencode/opencode.db");
  fs.mkdirSync(path.dirname(dbFile), { recursive: true });
  const db = new Database(dbFile);
  try { db.exec(fs.readFileSync(engineSchema, "utf8").replaceAll("@HOME@", path.dirname(home))); }
  finally { db.close(); }
}
