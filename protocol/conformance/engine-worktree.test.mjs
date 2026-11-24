// The resident resolves a starter's root and admits only the named project's linked worktree.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { isolatedEnv, readJSON, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { git, sessionOf } from "./engine.mjs";

test("a session started by a subagent is filed under the project of its linked worktree", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const { home, engine, start } = await computer("session-worktree");
  const project = path.join(home, "project");
  const tree = path.join(home, "linked");
  const other = path.join(home, "other");
  const env = isolatedEnv(path.dirname(home));
  write(path.join(project, "nested", "file"), "Fixture\n");
  fs.mkdirSync(other);
  git(env, project, "init", "-q");
  git(env, project, "add", "nested/file");
  git(env, project, "commit", "-qm", "Fixture");
  git(env, project, "worktree", "add", "--quiet", "--detach", tree);
  write(path.join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: project, name: "Fixture" }] }));
  const root = { ...sessionOf({ id: "ses_lead", directory: project }), agent: "plan" };
  const child = sessionOf({ id: "ses_child", directory: project, parent: root.id });
  engine.sessions.push(root, child);
  const record = await start().ready();
  const given = { task: "Inspect this worktree", title: "Worktree", project: "Fixture", startedBy: child.id };
  const refused = await request(record, "POST", "kuvo/sessions", { ...given, folder: other });
  assert.equal(refused.status, 400);
  assert.match(refused.json.message, /isn't in Fixture/);
  for (const folder of [tree, path.join(tree, "nested"), path.join(project, "nested")]) {
    const accepted = await request(record, "POST", "kuvo/sessions", { ...given, folder });
    assert.equal(accepted.status, 200, accepted.text);
    assert.equal(accepted.json.project, "Fixture");
    const made = engine.sessions.find(row => row.id === accepted.json.session);
    assert.equal(fs.realpathSync(made.location.directory), fs.realpathSync(folder));
    assert.equal(made.agent, "plan");
    assert.equal(made.metadata.kuvo.startedBy, root.id);
    assert.deepEqual(made.metadata.kuvo.filed, { folder: made.location.directory, project });
    assert.equal(engine.messages.get(made.id).length, 1);
    assert.match(engine.messages.get(made.id)[0].text, /ses_lead/);
  }
  assert.deepEqual(readJSON(path.join(home, "box/projects.json")).projects.map(row => row.directory), [project]);
});

test("a new worktree is recorded under its project before the conversation is returned", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const { home, engine, start } = await computer("new-session-worktree");
  const project = path.join(home, "project");
  const env = isolatedEnv(path.dirname(home));
  write(path.join(project, "file"), "Fixture\n");
  git(env, project, "init", "-q");
  git(env, project, "add", "file");
  git(env, project, "commit", "-qm", "Fixture");
  write(path.join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: project, name: "Fixture" }] }));
  const record = await start().ready();

  const response = await request(record, "POST", "kuvo/sessions", {
    project: "Fixture", worktree: true, title: "New worktree", approval: "askFirst",
  });
  assert.equal(response.status, 200, response.text);
  const { session, folder, worktree } = response.json;
  assert.equal(worktree.project, project);
  assert.equal(worktree.folder, folder);
  assert.notEqual(folder, project);
  assert.equal(fs.readFileSync(path.join(folder, "file"), "utf8"), "Fixture\n");
  const made = engine.sessions.find(each => each.id === session);
  assert.deepEqual(made.metadata.kuvo.filed, { folder, project });
  const stored = JSON.parse(fs.readFileSync(path.join(home, "box/worktrees.json"), "utf8"));
  assert.deepEqual(stored.worktrees.map(({ root, project }) => ({ root, project })), [{ root: worktree.root, project }]);
});

test("a sibling project folder files both explicit and delegated starts under that project", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const { home, engine, start } = await computer("session-sibling-folder");
  const project = path.join(home, "product");
  const site = path.join(home, "website");
  fs.mkdirSync(project);
  fs.mkdirSync(site);
  const settings = path.join(home, "box", "projects.json");
  write(settings, JSON.stringify({ projects: [{ directory: project, name: "Product", folders: [site] }] }));
  const caller = sessionOf({ id: "ses_sibling", directory: site });
  engine.sessions.push(caller);
  const record = await start().ready();
  for (const fields of [{ project: "Product", folder: site }, { startedBy: caller.id }]) {
    const started = await request(record, "POST", "kuvo/sessions", { task: "Check this folder.", title: "From the website", ...fields });
    assert.equal(started.status, 200, started.text);
    assert.equal(started.json.project, "Product");
    const made = engine.sessions.find(each => each.id === started.json.session);
    assert.equal(made.location.directory, site);
    assert.deepEqual(made.metadata.kuvo.filed, { folder: site, project });
  }
  assert.deepEqual(JSON.parse(fs.readFileSync(settings, "utf8")).projects.map(row => row.directory), [project]);
});

test("an interrupted start removes an unused worktree and keeps the one an OpenCode session took", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const { home, engine, start } = await computer("failed-session-worktree");
  const project = path.join(home, "project");
  const env = isolatedEnv(path.dirname(home));
  write(path.join(project, "file"), "Fixture\n");
  git(env, project, "init", "-q");
  git(env, project, "add", "file");
  git(env, project, "commit", "-qm", "Fixture");
  write(path.join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: project, name: "Fixture" }] }));
  const record = await start().ready();
  const given = { project: "Fixture", worktree: true, title: "Interrupted start" };

  engine.failStart = "before";
  const refused = await request(record, "POST", "kuvo/sessions", given);
  assert.equal(refused.status, 502, refused.text);
  assert.equal(git(env, project, "branch", "--list", "kuvo/*"), "");
  const stored = () => JSON.parse(fs.readFileSync(path.join(home, "box/worktrees.json"), "utf8"));
  assert.deepEqual(stored().worktrees, []);
  assert.deepEqual(stored().restorable, []);

  engine.failStart = "after";
  const lost = await request(record, "POST", "kuvo/sessions", given);
  assert.equal(lost.status, 502, lost.text);
  assert.equal(engine.sessions.length, 1);
  const folder = engine.sessions[0].location.directory;
  assert.equal(fs.readFileSync(path.join(folder, "file"), "utf8"), "Fixture\n");
  assert.equal(stored().worktrees[0].folder, folder);
});

test("a cancelled client leaves a worktree only if OpenCode created its conversation", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const { home, engine, start } = await computer("cancelled-worktree-start");
  const project = path.join(home, "project");
  const env = isolatedEnv(path.dirname(home));
  write(path.join(project, "file"), "Fixture\n");
  git(env, project, "init", "-q");
  git(env, project, "add", "file");
  git(env, project, "commit", "-qm", "Fixture");
  write(path.join(home, "box/projects.json"), JSON.stringify({ projects: [{ directory: project, name: "Fixture" }] }));
  const record = await start().ready();
  const given = { project: "Fixture", worktree: true, title: "Cancelled start" };
  const stored = path.join(home, "box", "worktrees.json");
  const make = async failure => {
    const client = new AbortController();
    engine.failStart = failure;
    engine.startCutShort = () => client.abort();
    await assert.rejects(request(record, "POST", "kuvo/sessions", given, {}, AbortSignal.any([
      client.signal, AbortSignal.timeout(10000),
    ])), { name: "AbortError" });
    engine.startCutShort = null;
  };
  await make("before");
  await until(() => fs.existsSync(stored) && readJSON(stored).worktrees.length === 0, "the unused worktree record to be removed");
  assert.equal(fs.existsSync(path.join(path.dirname(home), "worktrees", "brave-otter-1")), false);
  assert.equal(git(env, project, "branch", "--list", "kuvo/*"), "");
  await make("after");
  const [made] = engine.sessions;
  const folder = made.location.directory;
  await until(() => readJSON(stored).worktrees.some(tree => tree.folder === folder), "the worktree with a conversation to be retained");
  assert.equal(fs.readFileSync(path.join(folder, "file"), "utf8"), "Fixture\n");
});
