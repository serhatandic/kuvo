// Git, Changes, worktrees and projects (rust-box.md row 6.3b): the same answers from both boxes.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, realpathSync, rmSync, writeFileSync, readFileSync, existsSync, readdirSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { scratch, until, write, openLine } from "./adapter.mjs";
import { bench } from "./bench.mjs";

// git without your own settings: an empty file of the test's own.
const gitIn = settings => (folder, ...args) => {
  const ran = spawnSync("git", ["-c", "init.defaultBranch=main", "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args],
    { cwd: folder, encoding: "utf8", env: { ...Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("KUVO_") && !key.startsWith("CLAUDE_"))), GIT_CONFIG_GLOBAL: settings, GIT_CONFIG_NOSYSTEM: "1" } });
  assert.equal(ran.status, 0, ran.stderr);
  return ran.stdout;
};

test("the projects list: as kept, other folders and worktrees filed under their project", async t => {
  const root = realpathSync(scratch());
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const settings = path.join(root, "git-config-none");
  writeFileSync(settings, "");
  const git = gitIn(settings);
  const { computer } = await bench(t);

  await t.test("none kept", async () => {
    const box = await computer("NoProjects");
    const ask = box.connect(await box.start().ready());
    const reply = await ask("GET", "kuvo/projects");
    assert.deepEqual([reply.status, reply.json], [200, { projects: [], worktrees: {} }]);
  });

  await t.test("kept projects, their other folders, records and linked worktrees", async () => {
    const app = path.join(root, "app");
    mkdirSync(path.join(app, "web"), { recursive: true });
    git(app, "init", "-q");
    writeFileSync(path.join(app, "web", "a.txt"), "a");
    git(app, "add", ".");
    git(app, "commit", "-qm", "first");
    const tried = path.join(root, "tried");
    git(app, "worktree", "add", "-q", tried);
    const box = await computer("Projects");
    write(path.join(box.home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [
      { directory: app, name: "App", folders: [path.join(app, "web"), "/shared", 7], run: "make", terminal: "old" },
      { directory: path.join(app, "web"), folders: [] },
      { directory: "relative", folders: [] },
      { directory: "/gone", folders: ["/gone-web"], removed: true },
      { directory: "/made", folders: [], addedByBox: true, madeBy: { bot: "b1", name: "Bot", at: 5 } },
      // Marked removed by hand some other way: as it's filed, removed or not.
      { directory: "/zero", folders: ["/zero-web"], removed: 0 },
      { directory: "/yes", folders: ["/yes-web"], removed: "yes" },
      // Fields the route doesn't name, kept by a newer client or by hand: answered as they're kept.
      { directory: "/extra", folders: [], color: "blue", note: { pinned: [1, null] }, name: 5 },
    ] }));
    write(path.join(box.home, "box", "worktrees.json"), JSON.stringify({ formatVersion: 1,
      worktrees: [{ root: "/trees/app-1", folder: "/trees/app-1/web", project: app }, { root: 3 }],
      restorable: [{ root: "/trees/app-2", project: app, head: "abc" }],
      handedOver: { "/trees/handed": app },
      removed: { "/trees/old": app, "/trees/odd": 4 } }));
    const ask = box.connect(await box.start().ready());
    let reply;
    // (Linked worktrees are found off the box's thread: a first answer may come before them.)
    await until(async () => {
      reply = await ask("GET", "kuvo/projects");
      return reply.status === 200 && reply.json.worktrees[path.join(tried, "web")];
    }, () => `linked worktrees found: ${JSON.stringify(reply?.json)}`, 20000, 200);
    assert.deepEqual(reply.json.projects, [
      { directory: app, name: "App", folders: [path.join(app, "web"), "/shared"], run: "make" },
      { directory: path.join(app, "web"), folders: [] },
      { directory: "/gone", folders: ["/gone-web"], removed: true },
      { directory: "/made", folders: [], addedByBox: true, madeBy: { bot: "b1", name: "Bot", at: 5 } },
      { directory: "/zero", folders: ["/zero-web"] },
      { directory: "/yes", folders: ["/yes-web"], removed: true },
      { directory: "/extra", folders: [], color: "blue", note: { pinned: [1, null] }, name: 5 },
    ]);
    assert.deepEqual(reply.json.worktrees, {
      "/shared": app,
      "/zero-web": "/zero",
      [tried]: app,
      [path.join(tried, "web")]: path.join(app, "web"),
      "/trees/app-1": app,
      "/trees/app-1/web": app,
      "/trees/app-2": app,
      "/trees/handed": app,
      "/trees/old": app,
    });
  });

  await t.test("a list from a newer box is refused, as it was written", async () => {
    const box = await computer("NewerProjects");
    write(path.join(box.home, "box", "projects.json"), JSON.stringify({ version: 2, projects: [] }));
    const ask = box.connect(await box.start().ready());
    const reply = await ask("GET", "kuvo/projects");
    const refusal = [409, {
      code: "unsupportedFormatVersion",
      message: "Project list format version 2 is unsupported (this build writes 1).",
    }];
    assert.deepEqual([reply.status, reply.json], refusal);
    const put = await ask("PUT", "kuvo/projects?directory=" + encodeURIComponent(path.join(box.home, "new")), { name: "Not saved" });
    assert.deepEqual([put.status, put.json], refusal);
    assert.deepEqual(JSON.parse(readFileSync(path.join(box.home, "box", "projects.json"), "utf8")), { version: 2, projects: [] });
  });
});

test("a project is kept by its given folder, edited and brought back", async t => {
  const { computer } = await bench(t);
  const box = await computer("EditProjects");
  const folder = path.join(box.home, "Not there");
  const other = path.join(box.home, "Other");
  write(path.join(box.home, "box", "projects.json"), JSON.stringify({ version: 1, projects: [
    { directory: folder, removed: true, folders: [], terminal: "old", addedByBox: true },
  ] }));
  const ask = box.connect(await box.start().ready());
  const route = "kuvo/projects?directory=" + encodeURIComponent(folder);
  const project = { directory: folder, name: "My project", folders: [other], run: "make" };
  const changed = await ask("PUT", route, { name: " My project ", folders: [other, other, ".", folder], run: " make " });
  assert.deepEqual([changed.status, changed.json], [200, { project }]);
  const updated = await ask("PUT", route, { name: "Not there", run: "" });
  assert.deepEqual([updated.status, updated.json], [200, { project: { directory: folder, folders: [other] } }]);
  const listed = await ask("GET", "kuvo/projects");
  assert.deepEqual(listed.json, { projects: [{ directory: folder, folders: [other] }], worktrees: { [other]: folder } });
  const added = await ask("PUT", "kuvo/projects?directory=" + encodeURIComponent(other), {});
  assert.deepEqual([added.status, added.json], [200, { project: { directory: other, folders: [] } }]);
});

test("Changes lists, follows and edits the repository's real files", async t => {
  const { computer } = await bench(t);
  const box = await computer("Changes", undefined, folder => ({
    KUVO_CHANGES_TRASH: path.join(folder, "trash"), KUVO_TEST_LOGIN_ENV: "off",
  }));
  const root = path.join(box.home, "repo");
  mkdirSync(path.join(root, "app"), { recursive: true });
  const settings = path.join(box.home, "empty-git-config");
  writeFileSync(settings, "");
  const git = gitIn(settings);
  git(root, "init", "-q");
  git(root, "config", "user.name", "Test");
  git(root, "config", "user.email", "test@example.com");
  writeFileSync(path.join(root, "app", "a.txt"), "one\ntwo\n");
  writeFileSync(path.join(root, "app", "old.txt"), "old\n");
  writeFileSync(path.join(root, "README.md"), "read\n");
  git(root, "add", ".");
  git(root, "commit", "-qm", "first");
  const record = await box.start().ready();
  const ask = box.connect(record);
  const actualRoot = git(root, "rev-parse", "--show-toplevel").trim();
  const query = "?directory=" + encodeURIComponent(root);
  const listed = () => ask("GET", "kuvo/git/files" + query);
  const action = (route, fields = {}) => ask("POST", "kuvo/git/" + route, { directory: root, ...fields });
  assert.deepEqual((await listed()).json, { repository: true, root: actualRoot, branch: "main", files: [], uncommitted: false, commits: "all", stagedElsewhere: 0, takes: 0 });
  writeFileSync(path.join(root, "app", "a.txt"), "one\nchanged\n");
  git(root, "add", "app/a.txt");
  writeFileSync(path.join(root, "app", "a.txt"), "one\nchanged\nthree\n");
  git(root, "mv", "app/old.txt", "app/new.txt");
  rmSync(path.join(root, "README.md"));
  writeFileSync(path.join(root, "new.txt"), "draft\n");
  const first = await listed();
  assert.equal(first.status, 200, first.text);
  assert.equal(first.json.takes, 2);
  assert.deepEqual(first.json.files.map(({ stamp, ...file }) => { assert.equal(typeof stamp, "string"); return file; }), [
    { path: "app/a.txt", group: "staged", status: "modified", scope: "staged", stage: "unstage", additions: 1, deletions: 1, discard: "head", discards: { kind: "changes", undoable: false } },
    { path: "app/new.txt", oldPath: "app/old.txt", group: "staged", status: "renamed", scope: "staged", stage: "unstage", additions: 0, deletions: 0, discard: "head", discards: { kind: "undoMove", undoable: true } },
    { path: "app/a.txt", group: "unstaged", status: "modified", scope: "unstaged", stage: "stage", additions: 1, deletions: 0, discard: "unstaged", discards: { kind: "changes", undoable: false } },
    { path: "README.md", group: "unstaged", status: "deleted", scope: "unstaged", stage: "stage", additions: 0, deletions: 1, discard: "unstaged", discards: { kind: "restore", undoable: true } },
    { path: "new.txt", group: "untracked", status: "added", scope: "unstaged", stage: "stage", discard: "trash", discards: { kind: "trash", undoable: true } },
  ]);
  const diff = await action("changes", { path: "app/a.txt", scope: "unstaged" });
  assert.equal(diff.status, 200, diff.text);
  assert.match(diff.json.patch, /\+three/);
  assert.doesNotMatch(diff.json.patch, /README/);
  const fresh = await action("changes", { path: "new.txt" });
  assert.equal(fresh.status, 200, fresh.text);
  assert.match(fresh.json.patch, /\+draft/);
  const line = await openLine(record, query, "kuvo/git/files/events", {}, 20000);
  try {
    assert.equal(line.status, 200);
    const frames = line.frames();
    assert.equal((await frames.next()).value.commits, "staged");
    const stage = await action("stage", { root, path: "README.md", status: "deleted" });
    assert.equal(stage.status, 200, stage.text);
    let value;
    do { value = (await frames.next()).value; } while (value && !value.files.some(file => file.path === "README.md" && file.group === "staged"));
    assert.equal(value.takes, 3);
    const unstage = await action("unstage", { root, path: "README.md", status: "deleted" });
    assert.equal(unstage.status, 200, unstage.text);
    // A follower that starts after this change must not receive the older shared read first.
    const second = await openLine(record, query, "kuvo/git/files/events", {}, 20000);
    try {
      const current = (await second.frames().next()).value;
      assert.ok(current.files.some(file => file.path === "README.md" && file.group === "unstaged"));
    } finally { second.close(); }
  } finally { line.close(); }
  const freshRow = (await listed()).json.files.find(file => file.path === "new.txt");
  writeFileSync(path.join(root, "new.txt"), "later draft\n");
  const stale = await action("discard", { root, ...freshRow });
  assert.deepEqual([stale.status, stale.json.message], [409, "new.txt changed since it was listed. Look at it again before you discard it."]);
  const now = (await listed()).json.files.find(file => file.path === "new.txt");
  const discarded = await action("discard", { root, ...now });
  assert.equal(discarded.status, 200, discarded.text);
  assert.equal(existsSync(path.join(root, "new.txt")), false);
  const trash = box.env.KUVO_CHANGES_TRASH;
  assert.equal(readFileSync(path.join(trash, readdirSync(trash)[0], "new.txt"), "utf8"), "later draft\n");
  const committed = await action("commit", { message: "Only staged" });
  assert.equal(committed.status, 200, committed.text);
  assert.equal(git(root, "log", "-1", "--format=%s").trim(), "Only staged");
  assert.equal(git(root, "show", "HEAD:app/a.txt"), "one\nchanged\n");
  assert.equal(readFileSync(path.join(root, "app", "a.txt"), "utf8"), "one\nchanged\nthree\n");
});

test("branches, worktree merge and a pull request to a local remote", async t => {
  const { computer } = await bench(t);
  const box = await computer("Branches", undefined, () => ({ KUVO_TEST_LOGIN_ENV: "off" }));
  const root = path.join(box.home, "repo");
  const tree = path.join(box.home, "tree");
  const remote = path.join(box.home, "remote");
  mkdirSync(root);
  mkdirSync(remote);
  const settings = path.join(box.home, "git-config-none");
  writeFileSync(settings, "");
  const git = gitIn(settings);
  git(root, "init", "-q");
  git(root, "config", "user.name", "Test");
  git(root, "config", "user.email", "test@example.com");
  git(remote, "init", "--bare", "-q");
  writeFileSync(path.join(root, "a.txt"), "first\n");
  git(root, "add", ".");
  git(root, "commit", "-qm", "first");
  git(root, "remote", "add", "origin", remote);
  const ask = box.connect(await box.start().ready());
  const action = (route, fields = {}) => ask("POST", "kuvo/git/" + route, { directory: root, ...fields });
  assert.deepEqual((await action("branches")).json, { repository: true, root: git(root, "rev-parse", "--show-toplevel").trim(), current: "main", branches: ["main"], pullRequest: false });
  const pushed = await action("push");
  assert.equal(pushed.status, 200, pushed.text);
  const switched = await action("switch", { branch: "feature", create: true });
  assert.equal(switched.status, 200, switched.text);
  assert.equal((await action("branches")).json.current, "feature");
  writeFileSync(path.join(root, "a.txt"), "feature\n");
  assert.equal((await action("pull-request/draft")).status, 409);
  assert.equal((await action("commit", { message: "Improve file\n\nA description" })).status, 200);
  const draft = await action("pull-request/draft");
  assert.deepEqual([draft.status, draft.json], [200, {
    branch: "feature", base: "main", title: "Improve file", body: "A description", host: "other", creates: false,
  }]);
  const created = await action("pull-request", { title: "Improve file" });
  assert.deepEqual([created.status, created.json], [200, {}]);
  assert.equal(git(remote, "show", "feature:a.txt"), "feature\n");
  assert.equal((await action("switch", { branch: "main" })).status, 200);
  git(root, "worktree", "add", "-qb", "side", tree);
  assert.deepEqual((await action("branches", { directory: tree })).json.worktree, { into: "main" });
  writeFileSync(path.join(tree, "a.txt"), "side\n");
  assert.equal((await action("merge", { directory: tree })).status, 409);
  git(tree, "commit", "-qam", "side");
  const merged = await action("merge", { directory: tree });
  assert.deepEqual([merged.status, merged.json], [200, { into: "main" }]);
  assert.equal(readFileSync(path.join(root, "a.txt"), "utf8"), "side\n");
  writeFileSync(path.join(root, "a.txt"), "main edit\n");
  git(root, "commit", "-qam", "Main edit");
  writeFileSync(path.join(tree, "a.txt"), "side edit\n");
  git(tree, "commit", "-qam", "Side edit");
  const before = git(root, "rev-parse", "HEAD").trim();
  const conflicted = await action("merge", { directory: tree });
  assert.deepEqual([conflicted.status, conflicted.json], [409, {
    message: "main changed some of the same lines: a.txt. Nothing was merged.", conflicts: ["a.txt"], into: "main",
  }]);
  assert.equal(git(root, "rev-parse", "HEAD").trim(), before);
  assert.equal(readFileSync(path.join(root, "a.txt"), "utf8"), "main edit\n");
  assert.equal(git(root, "status", "--porcelain").trim(), "");
});

test("discard checks the listed stamp, keeps blocking entries and unstages a first commit", async t => {
  const { computer } = await bench(t);
  const box = await computer("Discard", undefined, folder => ({
    KUVO_CHANGES_TRASH: path.join(folder, "trash"), KUVO_TEST_LOGIN_ENV: "off",
  }));
  const root = path.join(box.home, "repo");
  mkdirSync(path.join(root, "app"), { recursive: true });
  const settings = path.join(box.home, "empty-git-config");
  writeFileSync(settings, "");
  const git = gitIn(settings);
  git(root, "init", "-q");
  git(root, "config", "user.name", "Test");
  git(root, "config", "user.email", "test@example.com");
  const ask = box.connect(await box.start().ready());
  const listed = () => ask("GET", "kuvo/git/files?directory=" + encodeURIComponent(root));
  const action = (route, fields) => ask("POST", "kuvo/git/" + route, { directory: root, root, ...fields });
  writeFileSync(path.join(root, "app", "file[1].txt"), "first\n");
  writeFileSync(path.join(root, "app", "file1.txt"), "not staged\n");
  const literal = { path: "app/file[1].txt", status: "added" };
  assert.equal((await action("stage", literal)).status, 200);
  assert.equal(git(root, "diff", "--cached", "--name-only").trim(), "app/file[1].txt");
  assert.equal((await action("unstage", literal)).status, 200);
  assert.equal(git(root, "diff", "--cached", "--name-only").trim(), "");
  assert.equal(readFileSync(path.join(root, "app", "file[1].txt"), "utf8"), "first\n");
  assert.equal((await action("discard", literal)).status, 409);
  assert.equal((await action("discard", { path: "../other" })).status, 400);
  assert.equal((await action("commit", { message: "first" })).status, 200);
  // The file's former parent is now a file. Discard keeps it in Trash, then restores the folder.
  rmSync(path.join(root, "app"), { recursive: true });
  writeFileSync(path.join(root, "app"), "blocking data");
  const row = (await listed()).json.files.find(file => file.path === literal.path);
  assert.equal(row.status, "deleted");
  const discard = await action("discard", row);
  assert.equal(discard.status, 200, discard.text);
  assert.equal(readFileSync(path.join(root, "app", "file[1].txt"), "utf8"), "first\n");
  const trash = box.env.KUVO_CHANGES_TRASH;
  assert.equal(readFileSync(path.join(trash, readdirSync(trash)[0], "app"), "utf8"), "blocking data");
  // Binary and large new files are not read as text diffs.
  writeFileSync(path.join(root, "binary"), Buffer.from([0,1,2]));
  const binary = await action("changes", { path: "binary" });
  assert.equal(binary.json.binary, true, binary.text);
  writeFileSync(path.join(root, "big"), Buffer.alloc(4 * 1024 * 1024 + 1));
  const big = await action("changes", { path: "big" });
  assert.deepEqual([big.status, big.json.big, big.json.patch], [200, true, ""]);
});

test("a failed push keeps the commit, and GitLab's pull-request page names the branch and base", async t => {
  const { computer } = await bench(t);
  const box = await computer("GitLab", undefined, () => ({ KUVO_TEST_LOGIN_ENV: "off" }));
  const root = path.join(box.home, "repo");
  const remote = path.join(box.home, "remote");
  mkdirSync(root);
  const settings = path.join(box.home, "empty-git-config");
  writeFileSync(settings, "");
  const git = gitIn(settings);
  git(root, "init", "-q");
  git(root, "config", "user.name", "Test");
  git(root, "config", "user.email", "test@example.com");
  writeFileSync(path.join(root, "a.txt"), "first\n");
  git(root, "add", ".");
  git(root, "commit", "-qm", "first");
  write(path.join(box.home, "box", "opencode.json"), JSON.stringify({ pull_request_instructions: "Use the ticket title." }));
  const ask = box.connect(await box.start().ready());
  const action = (route, fields = {}) => ask("POST", "kuvo/git/" + route, { directory: root, ...fields });
  assert.equal((await action("switch", { branch: "fix/typo", create: true })).status, 200);
  writeFileSync(path.join(root, "a.txt"), "fixed\n");
  const commit = await action("commit", { message: "Fix typo", push: true });
  assert.deepEqual([commit.status, commit.json.committed, commit.json.pushed], [422, true, false]);
  assert.match(commit.json.message, /^Committed, but the push didn’t go through:/);
  assert.equal(git(root, "log", "-1", "--format=%s").trim(), "Fix typo");
  mkdirSync(remote);
  git(remote, "init", "--bare", "-q");
  git(root, "remote", "add", "origin", "https://gitlab.com/example/fixture.git");
  // No network: push uses the test's own bare repository, only the draft/page uses the real URL.
  git(root, "config", "remote.origin.pushurl", remote);
  const draft = await action("pull-request/draft", { summary: true, prompt: true });
  assert.equal(draft.status, 200, draft.text);
  assert.equal(draft.json.host, "gitlab");
  assert.match(draft.json.summary, /Fix typo[\s\S]*\+fixed/);
  assert.match(draft.json.prompt, /Use the ticket title\.$/);
  const pulled = await action("pull-request", { title: "Fix typo", body: "A description" });
  assert.deepEqual([pulled.status, pulled.json], [200, {
    compareURL: "https://gitlab.com/example/fixture/-/merge_requests/new?merge_request%5Bsource_branch%5D=fix%2Ftypo&merge_request%5Btarget_branch%5D=main&merge_request%5Btitle%5D=Fix+typo&merge_request%5Bdescription%5D=A+description",
  }]);
  assert.equal(git(remote, "show", "fix/typo:a.txt"), "fixed\n");
});

test("Git and project directory queries accept Windows names without accepting parent traversal", async t => {
  const { computer } = await bench(t);
  const box = await computer("DirectoryQueries");
  const ask = box.connect(await box.start().ready());
  const directory = "C:\\Kuvo-Conformance-Directory-That-Is-Not-There";
  const query = "?directory=" + encodeURIComponent(directory);
  for (const route of ["kuvo/git/files", "kuvo/git/files/events"]) {
    assert.equal((await ask("GET", route + query)).status, 404);
  }
  const project = await ask("PUT", "kuvo/projects" + query, {});
  assert.equal(project.status, process.platform === "win32" ? 200 : 400);
  const parent = "?directory=" + encodeURIComponent(directory + "\\..\\elsewhere");
  assert.equal((await ask("GET", "kuvo/git/files" + parent)).status, 403);
  assert.equal((await ask("PUT", "kuvo/projects" + parent, {})).status, 403);
});
