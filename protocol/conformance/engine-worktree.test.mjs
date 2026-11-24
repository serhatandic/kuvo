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
  const run = { ...sessionOf({ id: "ses_engineRun", directory: project }), metadata: { kuvo: { engineFor: root.id } } };
  engine.sessions.push(root, child, run);
  const record = await start().ready();
  const given = { task: "Inspect this worktree", title: "Worktree", project: "Fixture", startedBy: child.id };
  const refused = await request(record, "POST", "kuvo/sessions", { ...given, folder: other });
  assert.equal(refused.status, 400);
  assert.match(refused.json.message, /isn't in Fixture/);
  for (const [folder, startedBy] of [[tree, root.id], [path.join(tree, "nested"), child.id], [path.join(project, "nested"), run.id]]) {
    const accepted = await request(record, "POST", "kuvo/sessions", { ...given, folder, startedBy });
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
  write(path.join(home, "box", "opencode.json"), JSON.stringify({ branch_prefix: " -ada:/ " }));
  const record = await start().ready();

  const response = await request(record, "POST", "kuvo/sessions", {
    project: "Fixture", worktree: true, title: "New worktree", approval: "askFirst",
  });
  assert.equal(response.status, 200, response.text);
  const { session, folder, worktree } = response.json;
  assert.equal(worktree.project, project);
  assert.equal(worktree.folder, folder);
  assert.equal(worktree.prefix, "ada/");
  assert.match(worktree.made, /^ada\/brave-otter-\d+$/);
  assert.match(git(env, folder, "symbolic-ref", "--short", "HEAD"), /^ada\//);
  assert.notEqual(folder, project);
  assert.equal(fs.readFileSync(path.join(folder, "file"), "utf8"), "Fixture\n");
  const made = engine.sessions.find(each => each.id === session);
  assert.deepEqual(made.metadata.kuvo.filed, { folder, project });
  const stored = JSON.parse(fs.readFileSync(path.join(home, "box/worktrees.json"), "utf8"));
  assert.deepEqual(stored.worktrees.map(({ root, project }) => ({ root, project })), [{ root: worktree.root, project }]);
});

test("a linked project subfolder starts the conversation in that subfolder of its new worktree", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("nested-linked-project");
  const repository = path.join(here.home, "repository");
  const sources = path.join(repository, "nested");
  const project = path.join(here.home, "project-link");
  const env = isolatedEnv(path.dirname(here.home));
  write(path.join(sources, "notes.md"), "In this project's folder.");
  git(env, repository, "init", "-q");
  git(env, repository, "add", "nested/notes.md");
  git(env, repository, "commit", "-qm", "Fixture");
  fs.symlinkSync(sources, project, process.platform === "win32" ? "junction" : "dir");
  write(path.join(here.home, "box", "projects.json"), JSON.stringify({ projects: [{ directory: project, name: "Nested project" }] }));
  const record = await here.start().ready();
  const started = await request(record, "POST", "kuvo/sessions", { project: "Nested project", worktree: true });
  assert.equal(started.status, 200, started.text);
  const { worktree, session } = started.json;
  assert.equal(worktree.project, project);
  assert.equal(started.json.folder, path.join(worktree.root, "nested"));
  assert.equal(fs.readFileSync(path.join(started.json.folder, "notes.md"), "utf8"), "In this project's folder.");
  assert.equal(here.engine.sessions.find(row => row.id === session).location.directory, started.json.folder);
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

test("a started session promises a finished notice only after it saves the starter's mark", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("start-finished-notice");
  const lead = sessionOf({ id: "ses_noticeLead", title: "Coordinator", directory: here.home });
  here.engine.sessions.push(lead);
  const record = await here.start().ready();
  const start = given => request(record, "POST", "kuvo/sessions",
    { title: "Review the release", task: "Review the files.", folder: here.home, wake: "finished", ...given });
  const work = await start({ startedBy: lead.id });
  assert.equal(work.status, 200, work.text);
  assert.deepEqual(work.json.wakes, ["finished"]);
  const details = id => here.engine.sessions.find(row => row.id === id);
  const prompt = id => here.engine.messages.get(id)[0].text;
  assert.deepEqual(details(work.json.session).metadata.kuvo.wake, ["finished"]);
  assert.match(prompt(work.json.session), /They get your last reply when you finish/);

  const outside = await start({});
  assert.equal(outside.status, 200, outside.text);
  assert.equal(outside.json.wakes, undefined);
  assert.equal(prompt(outside.json.session), "Review the files.");

  here.engine.holdSessionPatch = { id: "ses_made3", accepted() {}, release: Promise.resolve(), status: 503 };
  const unmarked = await start({ startedBy: lead.id });
  assert.equal(unmarked.status, 200, unmarked.text);
  assert.equal(unmarked.json.wakes, undefined);
  assert.equal(details(unmarked.json.session).metadata.kuvo.wake, undefined);
  assert.match(prompt(unmarked.json.session), /To answer, use send_message to ses_noticeLead/);
});

test("a session made through OpenCode is told its recorded worktree and branch", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const { home, engine, start } = await computer("worktree-instructions");
  const project = path.join(home, "project");
  const tree = path.join(home, "worktree");
  const branch = "kuvo/prepared";
  for (const folder of [project, tree]) fs.mkdirSync(folder, { recursive: true });
  write(path.join(home, "box", "worktrees.json"), JSON.stringify({ worktrees: [{ project, root: tree, folder: tree, branch }] }));
  const record = await start().ready();
  await until(() => engine.subscribers() > 0, "the resident to follow OpenCode's created sessions");
  const create = directory => request(record, "POST", "api/session", { location: { directory } });
  const made = await create(tree);
  assert.equal(made.status, 200, made.text);
  const link = `/api/experimental/session/${made.json.data.id}/instructions/entries/kuvo.worktree`;
  const instruction = await until(() => engine.asked.find(row => row.method === "PUT" && row.path === link),
    "a session created in a worktree to get its instructions");
  assert.deepEqual(instruction.json, { value: `You work in a git worktree of this project, on the branch ${branch}, so your changes stay apart from the user's own checkout at ${project} until they merge them. Make your changes here, not there.` });
  const plain = await create(project);
  assert.equal(plain.status, 200, plain.text);
  await until(async () => (await request(record, "GET", "kuvo/sessions/list")).json?.sessions?.[plain.json.data.id], "the regular project session to be observed");
  assert.equal(engine.asked.some(row => row.method === "PUT" && row.path.includes(`/${plain.json.data.id}/instructions/`)), false);
});

test("session Delete and Delete All confirm which worktrees and saved changes go with them", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("session-delete-worktrees");
  const project = path.join(here.home, "project");
  const live = path.join(here.home, "current-tree");
  const gone = path.join(here.home, "removed-tree");
  for (const folder of [project, live]) fs.mkdirSync(folder);
  const env = isolatedEnv(path.dirname(here.home));
  git(env, live, "init", "-q");
  git(env, live, "commit", "--allow-empty", "-qm", "Fixture");
  write(path.join(here.home, "box", "worktrees.json"), JSON.stringify({
    worktrees: [{ root: live, folder: live, project, branch: "current" }],
    restorable: [{ root: gone, folder: gone, project, branch: "old", head: "fixture", snapshot: "saved-changes" }],
  }));
  const current = sessionOf({ id: "ses_live", title: "Keep the launch", directory: live });
  const removed = sessionOf({ id: "ses_goneTree", title: "Earlier work", directory: gone, archived: true });
  const helper = { ...sessionOf({ id: "ses_hiddenRun", directory: live }), metadata: { kuvo: { engineFor: current.id } } };
  here.engine.sessions.push(current, removed, helper);
  const record = await here.start().ready();
  const confirm = async id => {
    const reply = await request(record, "GET", `kuvo/sessions/${id}/actions`);
    assert.equal(reply.status, 200, reply.text);
    return reply.json.actions.find(row => row.id === "delete").confirm;
  };
  const clean = await confirm(current.id);
  assert.deepEqual(clean, { title: "Delete “Keep the launch”?", message: "Its worktree is deleted too.", label: "Delete" });
  assert.equal((await confirm(removed.id)).message, "The changes kept to restore its worktree are deleted too.");
  write(path.join(live, "draft.md"), "Hasn't been committed.");
  assert.equal((await confirm(current.id)).message, "Its worktree and changes that weren’t committed are deleted too.");
  const stale = await request(record, "DELETE", `kuvo/sessions/${current.id}`, {
    was: { archived: false, updated: current.time.updated, confirm: clean },
  });
  assert.equal(stale.status, 409, stale.text);
  assert.ok(here.engine.sessions.some(row => row.id === current.id));

  const both = await request(record, "POST", "kuvo/sessions/deletes",
    { sessions: [current.id, current.id, removed.id, helper.id, "ses_missing"] });
  assert.equal(both.status, 200, both.text);
  assert.deepEqual(both.json, {
    title: "Delete 3 sessions?",
    message: "Their messages and undo snapshots go too. The worktree they were in is deleted too, with changes that weren’t committed. The changes kept to restore their worktree go too. This can’t be undone.",
    label: "Delete",
  });
  assert.equal((await request(record, "POST", "kuvo/sessions/deletes", { sessions: [] })).status, 400);
  assert.equal((await request(record, "POST", "kuvo/sessions/deletes", { sessions: ["ses_missing", helper.id] })).status, 404);
  assert.equal(here.engine.sessions.length, 3);

  const extra = sessionOf({ id: "ses_otherOpen", directory: live });
  here.engine.sessions.push(extra);
  here.engine.emit({ type: "session.created", data: { sessionID: extra.id, info: extra } });
  await until(async () => (await confirm(current.id)).message === "This can’t be undone.",
    "the still-open conversation to keep its worktree");
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
