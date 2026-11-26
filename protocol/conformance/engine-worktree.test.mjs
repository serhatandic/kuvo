// The resident resolves a starter's root and admits only the named project's linked worktree. The
// conversations it starts, and the ones that start them, are a coding agent's (the scripted Grok,
// `acp.mjs`).
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { isolatedEnv, readJSON, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { git } from "./engine.mjs";

const headers = { "x-kuvo-from": "app" };
const model = { providerID: "grok-cli", id: "acp-test" };

/** A computer whose conversations run on its Grok, and what it reads of them. */
async function computerFor(t, name) {
  const { computer } = await bench(t, { agents: true });
  const here = await computer(name);
  const env = isolatedEnv(path.dirname(here.home));
  let record;
  const tools = {
    here, env, home: here.home,
    async start() { record = await here.start().ready(); return record; },
    begin: given => request(record, "POST", "kuvo/sessions", { model, ...given }, headers),
    async header(id) {
      const read = await request(record, "GET", `kuvo/agents/api/session/${id}`, undefined, headers);
      assert.equal(read.status, 200, read.text);
      return read.json.data ?? read.json;
    },
    /** The first thing the agent heard in conversation `id` (it names its conversation in its words here). */
    heard: (words, why) => until(() => here.agents.prompts().find(text => text.includes(words)), why, 20000),
  };
  return tools;
}

function repository(env, project, file = "file") {
  write(path.join(project, file), "Fixture\n");
  git(env, project, "init", "-q");
  git(env, project, "add", file);
  git(env, project, "commit", "-qm", "Fixture");
}

test("a session started by a subagent is filed under the project of its linked worktree", { timeout: 60000 }, async t => {
  const { here, env, home, start, begin, header, heard } = await computerFor(t, "session-worktree");
  const project = path.join(home, "project");
  const tree = path.join(home, "linked");
  const other = path.join(home, "other");
  repository(env, project, "nested/file");
  fs.mkdirSync(other);
  git(env, project, "worktree", "add", "--quiet", "--detach", tree);
  write(path.join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: project, name: "Fixture" }] }));
  here.agents.seed("ses_lead", { directory: project, agent: "plan" });
  here.agents.seed("ses_child", { directory: project, parent: "ses_lead" });
  here.agents.seed("ses_engineRun", { directory: project, metadata: { kuvo: { engineFor: "ses_lead" } } });
  await start();
  const given = { title: "Worktree", project: "Fixture", startedBy: "ses_child" };
  const refused = await begin({ ...given, task: "Inspect this worktree", folder: other });
  assert.equal(refused.status, 400);
  assert.match(refused.json.message, /isn't in Fixture/);
  let turn = 0;
  for (const [folder, startedBy] of [[tree, "ses_lead"], [path.join(tree, "nested"), "ses_child"], [path.join(project, "nested"), "ses_engineRun"]]) {
    const task = `Inspect this worktree (${++turn})`;
    const accepted = await begin({ ...given, task, folder, startedBy });
    assert.equal(accepted.status, 200, accepted.text);
    assert.equal(accepted.json.project, "Fixture");
    const made = await header(accepted.json.session);
    assert.equal(fs.realpathSync(made.location.directory), fs.realpathSync(folder));
    // (A conversation stored in Plan mode starts Build sessions: Plan mode is gone.)
    assert.equal(made.agent, "build");
    assert.equal(made.metadata.kuvo.startedBy, "ses_lead");
    assert.deepEqual(made.metadata.kuvo.filed, { folder: made.location.directory, project });
    assert.match(await heard(task, "Its task never reached its agent."), /ses_lead/);
  }
  assert.deepEqual(readJSON(path.join(home, "box/projects.json")).projects.map(row => row.directory), [project]);
});

test("a new worktree is recorded under its project before the conversation is returned", { timeout: 45000 }, async t => {
  const { env, home, start, begin, header } = await computerFor(t, "new-session-worktree");
  const project = path.join(home, "project");
  repository(env, project);
  write(path.join(home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [{ directory: project, name: "Fixture" }] }));
  write(path.join(home, "box", "opencode.json"), JSON.stringify({ branch_prefix: " -ada:/ " }));
  await start();
  const response = await begin({ project: "Fixture", worktree: true, title: "New worktree", approval: "askFirst" });
  assert.equal(response.status, 200, response.text);
  const { session, folder } = response.json;
  assert.notEqual(folder, project);
  assert.match(git(env, folder, "symbolic-ref", "--short", "HEAD"), /^ada\//);
  assert.equal(fs.readFileSync(path.join(folder, "file"), "utf8"), "Fixture\n");
  assert.deepEqual((await header(session)).metadata.kuvo.filed, { folder, project });
  const stored = JSON.parse(fs.readFileSync(path.join(home, "box/worktrees.json"), "utf8"));
  assert.deepEqual(stored.worktrees.map(({ folder, project }) => ({ folder, project })), [{ folder, project }]);
});

test("a linked project subfolder starts the conversation in that subfolder of its new worktree", { timeout: 45000 }, async t => {
  const { env, home, start, begin, header } = await computerFor(t, "nested-linked-project");
  const repositoryAt = path.join(home, "repository");
  const sources = path.join(repositoryAt, "nested");
  const project = path.join(home, "project-link");
  write(path.join(sources, "notes.md"), "In this project's folder.");
  git(env, repositoryAt, "init", "-q");
  git(env, repositoryAt, "add", "nested/notes.md");
  git(env, repositoryAt, "commit", "-qm", "Fixture");
  fs.symlinkSync(sources, project, process.platform === "win32" ? "junction" : "dir");
  write(path.join(home, "box", "projects.json"), JSON.stringify({ projects: [{ directory: project, name: "Nested project" }] }));
  await start();
  const started = await begin({ project: "Nested project", worktree: true });
  assert.equal(started.status, 200, started.text);
  const { session, folder } = started.json;
  const [tree] = JSON.parse(fs.readFileSync(path.join(home, "box/worktrees.json"), "utf8")).worktrees;
  assert.equal(tree.project, project);
  assert.equal(path.resolve(folder), path.join(tree.root, "nested"));
  assert.equal(fs.readFileSync(path.join(folder, "notes.md"), "utf8"), "In this project's folder.");
  assert.equal((await header(session)).location.directory, folder);
});

test("a sibling project folder files both explicit and delegated starts under that project", { timeout: 45000 }, async t => {
  const { here, home, start, begin, header } = await computerFor(t, "session-sibling-folder");
  const project = path.join(home, "product");
  const site = path.join(home, "website");
  fs.mkdirSync(project);
  fs.mkdirSync(site);
  const settings = path.join(home, "box", "projects.json");
  write(settings, JSON.stringify({ projects: [{ directory: project, name: "Product", folders: [site] }] }));
  here.agents.seed("ses_sibling", { directory: site });
  await start();
  for (const fields of [{ project: "Product", folder: site }, { startedBy: "ses_sibling" }]) {
    const started = await begin({ task: "Check this folder.", title: "From the website", ...fields });
    assert.equal(started.status, 200, started.text);
    assert.equal(started.json.project, "Product");
    const made = await header(started.json.session);
    assert.equal(path.resolve(made.location.directory), site);
    assert.deepEqual(made.metadata.kuvo.filed, { folder: made.location.directory, project });
  }
  assert.deepEqual(JSON.parse(fs.readFileSync(settings, "utf8")).projects.map(row => row.directory), [project]);
});

test("a started session promises a finished notice only to the conversation that started it", { timeout: 45000 }, async t => {
  const { here, home, start, begin, header, heard } = await computerFor(t, "start-finished-notice");
  here.agents.seed("ses_noticeLead", { title: "Coordinator", directory: home });
  await start();
  const given = { title: "Review the release", folder: home, wake: "finished" };
  const work = await begin({ ...given, task: "Review the files.", startedBy: "ses_noticeLead" });
  assert.equal(work.status, 200, work.text);
  assert.deepEqual(work.json.wakes, ["finished"]);
  assert.deepEqual((await header(work.json.session)).metadata.kuvo.wake, ["finished"]);
  assert.match(await heard("Review the files.", "Its task never reached its agent."), /They get your last reply when you finish/);

  const outside = await begin({ ...given, task: "Review the other files." });
  assert.equal(outside.status, 200, outside.text);
  assert.equal(outside.json.wakes, undefined);
  assert.equal(await heard("Review the other files.", "Its task never reached its agent."), "Review the other files.");
});

test("a session started in a recorded worktree is told its worktree and branch", { timeout: 45000 }, async t => {
  const { home, start, begin, heard } = await computerFor(t, "worktree-instructions");
  const project = path.join(home, "project");
  const tree = path.join(home, "worktree");
  const branch = "kuvo/prepared";
  for (const folder of [project, tree]) fs.mkdirSync(folder, { recursive: true });
  write(path.join(home, "box", "worktrees.json"), JSON.stringify({ worktrees: [{ project, root: tree, folder: tree, branch }] }));
  await start();
  const made = await begin({ folder: tree, task: "Work in the tree." });
  assert.equal(made.status, 200, made.text);
  const told = await heard("Work in the tree.", "Its task never reached its agent.");
  assert.ok(told.includes(`You work in a git worktree of this project, on the branch ${branch}, so your changes stay apart from the user's own checkout at ${project} until they merge them. Make your changes here, not there.`), told);
  const plain = await begin({ folder: project, task: "Work in the project." });
  assert.equal(plain.status, 200, plain.text);
  assert.equal(await heard("Work in the project.", "Its task never reached its agent."), "Work in the project.");
});

test("session Delete and Delete All confirm which worktrees and saved changes go with them", { timeout: 45000 }, async t => {
  const { here, env, home, start, begin } = await computerFor(t, "session-delete-worktrees");
  const project = path.join(home, "project");
  const live = path.join(home, "current-tree");
  const gone = path.join(home, "removed-tree");
  for (const folder of [project, live]) fs.mkdirSync(folder);
  git(env, live, "init", "-q");
  git(env, live, "commit", "--allow-empty", "-qm", "Fixture");
  write(path.join(home, "box", "worktrees.json"), JSON.stringify({
    worktrees: [{ root: live, folder: live, project, branch: "current" }],
    restorable: [{ root: gone, folder: gone, project, branch: "old", head: "fixture", snapshot: "saved-changes" }],
  }));
  here.agents.seed("ses_live", { title: "Keep the launch", directory: live });
  here.agents.seed("ses_goneTree", { title: "Earlier work", directory: gone, metadata: { "kuvo.archived": true } });
  here.agents.seed("ses_hiddenRun", { directory: live, metadata: { kuvo: { engineFor: "ses_live" } } });
  const record = await start();
  const confirm = async id => {
    const reply = await request(record, "GET", `kuvo/sessions/${id}/actions`, undefined, headers);
    assert.equal(reply.status, 200, reply.text);
    return reply.json.actions.find(row => row.id === "delete").confirm;
  };
  const clean = await confirm("ses_live");
  assert.deepEqual(clean, { title: "Delete “Keep the launch”?", message: "Its worktree is deleted too.", label: "Delete" });
  assert.equal((await confirm("ses_goneTree")).message, "The changes kept to restore its worktree are deleted too.");
  write(path.join(live, "draft.md"), "Hasn't been committed.");
  assert.equal((await confirm("ses_live")).message, "Its worktree and changes that weren’t committed are deleted too.");
  const stale = await request(record, "DELETE", "kuvo/sessions/ses_live", {
    was: { archived: false, updated: 1, confirm: clean },
  }, headers);
  assert.equal(stale.status, 409, stale.text);
  assert.ok(fs.existsSync(path.join(home, "agents/sessions/ses_live/session.json")));

  const both = await request(record, "POST", "kuvo/sessions/deletes",
    { sessions: ["ses_live", "ses_live", "ses_goneTree", "ses_hiddenRun", "ses_missing"] }, headers);
  assert.equal(both.status, 200, both.text);
  assert.deepEqual(both.json, {
    title: "Delete 3 sessions?",
    message: "Their messages and undo snapshots go too. The worktree they were in is deleted too, with changes that weren’t committed. The changes kept to restore their worktree go too. This can’t be undone.",
    label: "Delete",
  });
  assert.equal((await request(record, "POST", "kuvo/sessions/deletes", { sessions: [] }, headers)).status, 400);
  assert.equal((await request(record, "POST", "kuvo/sessions/deletes", { sessions: ["ses_missing", "ses_hiddenRun"] }, headers)).status, 404);

  // Another conversation open in the worktree keeps it.
  const extra = await begin({ folder: live, title: "Also here" });
  assert.equal(extra.status, 200, extra.text);
  await until(async () => (await confirm("ses_live")).message === "This can’t be undone.",
    "the still-open conversation to keep its worktree");
});
