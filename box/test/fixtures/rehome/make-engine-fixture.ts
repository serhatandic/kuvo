// Makes the engine's database as Kuvo Code 0.7.2's engine (opencode 2.0.32) leaves it, for the
// move's tests (`engine-2.0.32.sql`), and what the pinned engine makes of it as it starts on a copy
// (`engine-2.0.32-migrated.sql`): run once, by hand, with both real engines, never by a test.
//
//   bun box/test/fixtures/rehome/make-engine-fixture.ts <opencode 2.0.32> <the pinned opencode> [out folder]
//
// A Mac of 0.7.2 as its app laid it out (the engine's data in `<S>/opencode/data`, its chats in
// `<S>/Chats`, its worktrees in the engine's data), with real turns against a stand-in model:
// a session in a repository of yours, one in its engine worktree, a chat outside git in the app's
// folder with a file sent and a fork, one in a folder of yours outside git with a message queued
// behind a run, a shell command, a session's own permission rules and metadata. Its folders are
// written as placeholders (`@S@`, `@S_URL@`, `@HOME@`, `@ROOT@`, `@ROOT6@`, `@ID:<folder>@`),
// which a test lays out where its own world is.

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [old, pinned, out = import.meta.dir] = process.argv.slice(2);
if (!old || !pinned) throw new Error("Usage: make-engine-fixture.ts <opencode 2.0.32> <the pinned opencode> [out folder]");

const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-fixture-")));
const home = path.join(root, "h");
const S = path.join(home, "App Support");
const put = (file: string, text: string) => { fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, text); };
const git = (cwd: string, args: string[]) => {
  const ran = spawnSync("git", args, { cwd, encoding: "utf8", env: { PATH: process.env.PATH, HOME: cwd, GIT_CONFIG_NOSYSTEM: "1", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@t", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@t", GIT_AUTHOR_DATE: "2026-01-01T00:00:00Z", GIT_COMMITTER_DATE: "2026-01-01T00:00:00Z" } });
  if (ran.status !== 0) throw new Error(`git ${args.join(" ")}: ${ran.stderr}`);
  return ran.stdout.trim();
};

// The world: as `rehome.test.ts` lays its own out.
const repository = path.join(home, "proj");
fs.mkdirSync(repository, { recursive: true });
git(repository, ["init", "-q", "-b", "main"]);
git(repository, ["commit", "-q", "--allow-empty", "-m", "root"]);
put(path.join(repository, "a.txt"), "line one\nline two\n");
git(repository, ["add", "a.txt"]);
git(repository, ["commit", "-q", "-m", "a"]);
const rootCommit = git(repository, ["rev-list", "--max-parents=0", "HEAD"]);
const chat = path.join(S, "Chats", "c1");
const plain = path.join(home, "Projects", "plain");
fs.mkdirSync(chat, { recursive: true });
put(path.join(plain, "p.txt"), "plain");
put(path.join(S, "Attachments", "ses_a", "b.png"), "PNGDATA");
put(path.join(S, "Attachments", "ses_a", "with space.png"), "PNGDATA");

// A model, as a stand-in (OpenAI's chat completions): it answers each turn in words.
const model = http.createServer((request, response) => {
  let body = "";
  request.on("data", chunk => { body += chunk; });
  request.on("end", () => {
    const chunk = (delta: object, finish: string | null = null) => `data: ${JSON.stringify({ id: "c", object: "chat.completion.chunk", created: 1, model: "m", choices: [{ index: 0, delta, finish_reason: finish }], ...(finish ? { usage: { prompt_tokens: 5, completion_tokens: 5, total_tokens: 10 } } : {}) })}\n\n`;
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.end(chunk({ role: "assistant", content: "Done, as you asked." }) + chunk({}, "stop") + "data: [DONE]\n\n");
  });
});
await new Promise<void>(resolve => model.listen(0, "127.0.0.1", resolve));
const modelURL = `http://127.0.0.1:${(model.address() as { port: number }).port}/v1`;

/** An engine started as 0.7.2's app starts its own: its data, settings, state and cache in the
 * app's folder. */
async function engine(binary: string, at: string, settings: object | null) {
  const xdg = (name: string) => { const folder = path.join(at, "opencode", name); fs.mkdirSync(folder, { recursive: true }); return folder; };
  if (settings) put(path.join(xdg("config"), "opencode", "opencode.json"), JSON.stringify(settings));
  fs.mkdirSync(path.join(root, "tmp"), { recursive: true });
  const child = spawn(binary, ["serve", "--stdio", "--port", "0"], {
    cwd: xdg("workspace"), stdio: ["pipe", "pipe", "pipe"],
    env: { PATH: "/usr/bin:/bin:/usr/sbin:/sbin", HOME: home, TMPDIR: path.join(root, "tmp") + "/", XDG_DATA_HOME: xdg("data"), XDG_CONFIG_HOME: xdg("config"), XDG_STATE_HOME: xdg("state"), XDG_CACHE_HOME: xdg("cache"),
      OPENCODE_PASSWORD: "pw", OPENCODE_CLIENT: "desktop", OPENCODE_DISABLE_AUTOUPDATE: "1", OPENCODE_DISABLE_MODELS_FETCH: "1", OPENCODE_DISABLE_FREE_MODELS: "1", OPENCODE_DISABLE_CLAUDE_CODE: "1" },
  });
  let said = "", errors = "";
  child.stderr.on("data", chunk => { errors = (errors + chunk).slice(-4000); });
  const url = await new Promise<string>((resolve, reject) => {
    child.stdout.on("data", chunk => { said += chunk; const found = /"url"\s*:\s*"([^"]+)"/.exec(said); if (found) resolve(found[1]); });
    child.once("exit", status => reject(new Error(`The engine ended (${status}): ${said} ${errors}`)));
  });
  const api = async (method: string, route: string, body?: unknown, directory?: string): Promise<any> => {
    const answer = await fetch(`${url}/${route}`, { method, headers: { authorization: "Basic " + Buffer.from("opencode:pw").toString("base64"), "content-type": "application/json", ...(directory ? { "x-opencode-directory": encodeURIComponent(directory) } : {}) }, body: body === undefined ? undefined : JSON.stringify(body) });
    const text = await answer.text();
    if (!answer.ok) throw new Error(`${method} ${route}: ${answer.status} ${text.slice(0, 500)}`);
    return text ? JSON.parse(text) : null;
  };
  const stop = async () => { child.stdin.end(); await new Promise(resolve => child.once("exit", resolve)); };
  return { api, stop };
}

const data = path.join(S, "opencode", "data", "opencode");
const database = path.join(data, "opencode.db");
const sqlite = (file: string, sql: string) => {
  const ran = spawnSync("/usr/bin/sqlite3", [file, sql], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  if (ran.status !== 0) throw new Error(ran.stderr);
  return ran.stdout;
};

{
  const { api, stop } = await engine(old, S, {
    provider: { lab: { name: "Lab", npm: "@ai-sdk/openai-compatible", options: { baseURL: modelURL, apiKey: "none" }, models: { m: { name: "M", tool_call: true } } } },
    model: "lab/m",
  });
  let n = 0;
  const messageID = () => `msg_${"0".repeat(12)}${String(++n).padStart(14, "0")}`;
  const idle = async (session: string) => {
    for (let tries = 0; tries < 600; tries++) {
      const active = await api("GET", "api/session/active");
      const inbox = await api("GET", `api/session/${session}/inbox`).catch(() => ({ data: [] }));
      if (!(session in (active.data ?? {})) && !(inbox.data ?? []).length) return;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw new Error(`${session} never came to rest`);
  };
  const create = async (title: string, directory: string) => (await api("POST", "api/session", { title, location: { directory } })).data;
  const say = async (session: string, text: string, files?: { uri: string; name?: string }[], delivery = "steer") => {
    await api("POST", `api/session/${session}/prompt`, { id: messageID(), text, ...(files ? { files } : {}), delivery });
  };

  // In a repository of yours.
  const inGit = await create("git edit", repository);
  await say(inGit.id, "In a.txt, replace line two.");
  await idle(inGit.id);
  // A chat in the app's folder, with files sent (one named with a space), its own rules and metadata.
  const inChat = await create("chat", chat);
  await say(inChat.id, "see attached", [{ uri: pathToFileURL(path.join(S, "Attachments", "ses_a", "b.png")).href, name: "b.png" }, { uri: pathToFileURL(path.join(S, "Attachments", "ses_a", "with space.png")).href, name: "with space.png" }]);
  await idle(inChat.id);
  await api("POST", `api/session/${inChat.id}/shell`, { command: "pwd" });
  await api("PATCH", `api/session/${inChat.id}`, { permissions: [{ action: "edit", resource: `${chat}/*`, effect: "allow" }] });
  await api("PATCH", `api/session/${inChat.id}`, { metadata: { kuvo: { folder: chat } } });
  // A fork of it.
  const messages = (await api("GET", `api/session/${inChat.id}/message?limit=50`)).data as { id: string; type?: string; role?: string }[];
  const last = messages.at(-1)!;
  await api("POST", `api/session/${inChat.id}/fork`, { before: last.id }).catch(async () => api("POST", `api/session/${inChat.id}/fork`, {}));
  // In the engine's worktree of the repository.
  const worktree = (await api("POST", "api/worktree", { projectID: inGit.projectID, name: "feature one" })).directory as string;
  const inWorktree = await create("in worktree", worktree);
  await say(inWorktree.id, "Append a line to a.txt.");
  await idle(inWorktree.id);
  // In a folder of yours outside git: one answered, then one left waiting behind nothing running.
  const inPlain = await create("plain", plain);
  await say(inPlain.id, "hello");
  await idle(inPlain.id);
  await stop();
  console.log(JSON.stringify({ worktree, sessions: { inGit: inGit.id, inChat: inChat.id, inWorktree: inWorktree.id, inPlain: inPlain.id } }));
}

/** A database's dump with this world's folders as placeholders. */
function dump(file: string): string {
  sqlite(file, "PRAGMA wal_checkpoint(TRUNCATE)");
  let sql = sqlite(file, ".dump");
  const id = (folder: string) => createHash("sha1").update(`directory:${folder}`).digest("hex");
  for (const folder of [chat, plain]) sql = sql.replaceAll(id(folder), `@ID:${folder}@`);
  sql = sql.replaceAll(pathToFileURL(S).href.slice("file://".length), "@S_URL@").replaceAll(S, "@S@").replaceAll(home, "@HOME@")
    .replaceAll(rootCommit, "@ROOT@").replaceAll(rootCommit.slice(0, 6), "@ROOT6@");
  // (The engine names its temporary folder in what it tells the model: nowhere of this world's.)
  sql = sql.replaceAll(path.join(root, "tmp"), "/tmp/kuvo-0.7");
  if (sql.includes(root)) throw new Error("The dump still names this run's own folder.");
  return sql;
}

fs.writeFileSync(path.join(out, "engine-2.0.32.sql"), dump(database));
// What the pinned engine makes of a copy as it starts on it.
const copy = path.join(root, "copy");
fs.cpSync(S, copy, { recursive: true });
const moved = path.join(copy, "opencode", "data", "opencode", "opencode.db");
{
  const { stop } = await engine(pinned, copy, null);
  await stop();
}
fs.writeFileSync(path.join(out, "engine-2.0.32-migrated.sql"), dump(moved).replaceAll(copy, "@S@"));
console.log(sqlite(database, "SELECT count(*) || ' ' || max(id) FROM migration").trim(), "→", sqlite(moved, "SELECT count(*) || ' ' || max(id) FROM migration").trim());
model.close();
fs.rmSync(root, { recursive: true, force: true });
