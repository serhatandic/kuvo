// Git, Changes, worktrees and projects (rust-box.md row 6.3b): the same answers from both boxes.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { scratch, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

// git without your own settings: an empty file of the test's own.
const gitIn = settings => (folder, ...args) => {
  const ran = spawnSync("git", ["-c", "init.defaultBranch=main", "-c", "user.name=Test", "-c", "user.email=test@example.com", ...args],
    { cwd: folder, encoding: "utf8", env: { ...process.env, GIT_CONFIG_GLOBAL: settings, GIT_CONFIG_NOSYSTEM: "1" } });
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
    assert.deepEqual([reply.status, reply.json], [409, {
      code: "unsupportedFormatVersion",
      message: "Project list format version 2 is unsupported (this build writes 1).",
    }]);
  });
});
