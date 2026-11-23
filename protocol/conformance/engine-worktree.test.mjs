// The resident resolves a starter's root and admits only the named project's linked worktree.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { isolatedEnv, request, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { git, sessionOf } from "./engine.mjs";

test("a session started by a subagent is filed under the project of its linked worktree", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const { home, engine, start } = await computer("session-worktree");
  const project = path.join(home, "project");
  const tree = path.join(home, "linked");
  const other = path.join(home, "other");
  const env = isolatedEnv(path.dirname(home));
  write(path.join(project, "file"), "Fixture\n");
  fs.mkdirSync(other);
  git(env, project, "init", "-q");
  git(env, project, "add", "file");
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
  const accepted = await request(record, "POST", "kuvo/sessions", { ...given, folder: tree });
  assert.equal(accepted.status, 200, accepted.text);
  assert.equal(accepted.json.project, "Fixture");
  const made = engine.sessions.find(row => row.id === accepted.json.session);
  assert.equal(fs.realpathSync(made.location.directory), fs.realpathSync(tree));
  assert.equal(made.agent, "plan");
  assert.equal(made.metadata.kuvo.startedBy, root.id);
  assert.deepEqual(made.metadata.kuvo.filed, { folder: made.location.directory, project });
  assert.equal(engine.messages.get(made.id).length, 1);
  assert.match(engine.messages.get(made.id)[0].text, /ses_lead/);
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
