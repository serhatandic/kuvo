// The repository is real, while the coding agent's replies are scripted. The
// test modifies files only in the fixture's own temporary project.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { test } from "node:test";
import { boxBinary, request, until } from "./adapter.mjs";
import { headers, result, scriptedClaude, streams, texts } from "./claude.mjs";

function gitOnFixture(claude) {
  if (process.platform === "win32") {
    const executable = execFileSync("where.exe", ["git"], { encoding: "utf8" }).trim().split(/\r?\n/)[0];
    claude.env.PATH += path.delimiter + path.dirname(executable);
  }
}

const skip = boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;

test("Undo and Redo restore the turn's repository files without changing the Git index or unrelated edits", { skip, timeout: 120000 }, async () => {
  const claude = await scriptedClaude();
  gitOnFixture(claude);
  const id = "ses_undo_repo";
  try {
    const folder = path.join(claude.home, "projects", id);
    fs.mkdirSync(folder, { recursive: true });
    const git = (...args) => execFileSync("git", ["-C", folder, ...args], { env: claude.env, encoding: "utf8" });
    const write = (name, text) => fs.writeFileSync(path.join(folder, name), text);
    const read = name => fs.readFileSync(path.join(folder, name), "utf8");
    git("init", "-q");
    write("a.txt", "before\n");
    write("other.txt", "unrelated\n");
    write(".gitignore", "ignored.txt\n");
    write("ignored.txt", "untouched\n");
    git("add", "a.txt", "other.txt", ".gitignore");
    const index = git("ls-files", "--stage");
    claude.seed(id, { directory: folder, model: "default" });
    claude.script("Change files", [
      ...streams("$UUID", [{ type: "text", text: "Working." }]),
      { until: "finish-files" },
      ...streams("$UUID", [{ type: "text", text: "Done." }]),
      result(["$UUID"]),
    ]);
    await claude.start();
    await claude.prompt(id, "Change files");
    await until(async () => texts(await claude.rows(id)).join("").includes("Working."), "The turn did not start.");
    write("a.txt", "after\n");
    write("made.txt", "new\n");
    claude.go("finish-files");
    const rows = await claude.idles(id);
    const messageID = rows.find(row => row.type === "user").id;
    await until(() => {
      const file = path.join(claude.home, "agents", "sessions", id, "agent.json");
      try { return Array.isArray(JSON.parse(fs.readFileSync(file, "utf8")).snapshots?.[messageID]?.files); }
      catch { return false; }
    }, "The turn's file snapshot was not finalized.", 30000);
    write("other.txt", "edited independently\n");
    const ask = (method, route, body) => request(claude.listener, method, claude.route(id) + route, body, headers, 45000);
    const undo = await ask("POST", "/revert/stage", { messageID });
    assert.equal(undo.status, 200, undo.text);
    assert.equal(undo.json.data.messageID, messageID);
    assert.equal(read("a.txt"), "before\n");
    assert.equal(fs.existsSync(path.join(folder, "made.txt")), false);
    assert.equal(read("other.txt"), "edited independently\n");
    assert.equal(read("ignored.txt"), "untouched\n");
    assert.equal(git("ls-files", "--stage"), index);
    const redo = await ask("DELETE", "/revert");
    assert.equal(redo.status, 200, redo.text);
    assert.equal(read("a.txt"), "after\n");
    assert.equal(read("made.txt"), "new\n");
    assert.equal(read("other.txt"), "edited independently\n");
    assert.equal(git("ls-files", "--stage"), index);

    // The next prompt makes the staged Undo permanent and starts a fresh native conversation.
    const again = await ask("POST", "/revert/stage", { messageID });
    assert.equal(again.status, 200, again.text);
    claude.script("Continue here", [
      ...streams("$UUID", [{ type: "text", text: "Started from here." }]),
      result(["$UUID"]),
    ]);
    await claude.prompt(id, "Continue here");
    const final = await until(async () => {
      const messages = await claude.rows(id);
      return messages.at(-1)?.type === "idle" && texts(messages).join("").includes("Started from here.") && messages;
    }, "The conversation did not continue after Undo.", 30000);
    assert.deepEqual(final.filter(row => row.type === "user").map(row => row.text.split("\n")[0]), ["Continue here"]);
    assert.equal(read("a.txt"), "before\n");
    assert.equal(fs.existsSync(path.join(folder, "made.txt")), false);
    assert.equal(read("other.txt"), "edited independently\n");
  } catch (error) {
    throw new Error([error.message, claude.log(), JSON.stringify(claude.heard().slice(-8))].join("\n"), { cause: error });
  } finally {
    await claude.close();
  }
});

test("Undo in a repository subfolder leaves ignored files, the rest of the repository and nested repositories alone", { skip, timeout: 120000 }, async () => {
  const claude = await scriptedClaude();
  gitOnFixture(claude);
  const id = "ses_undo_subfolder";
  try {
    const root = path.join(claude.home, "projects", id);
    const folder = path.join(root, "sub");
    const nested = path.join(folder, "nested");
    fs.mkdirSync(nested, { recursive: true });
    const git = (directory, ...args) => execFileSync("git", ["-C", directory, ...args], { env: claude.env, encoding: "utf8" });
    const write = (where, text) => fs.writeFileSync(path.join(root, where), text);
    const read = where => fs.readFileSync(path.join(root, where), "utf8");
    git(root, "init", "-q");
    git(nested, "init", "-q");
    write("sub/nested/kept", "nested\n");
    git(nested, "add", "kept");
    git(nested, "-c", "user.name=Fixture", "-c", "user.email=fixture@localhost", "commit", "-qm", "Start");
    write("sub/.gitignore", "ignored.txt\n");
    write("sub/ignored.txt", "kept before\n");
    write("sub/change.txt", "old\n");
    write("outside.txt", "outside\n");
    claude.seed(id, { directory: folder, model: "default" });
    claude.script("Change locally", [
      ...streams("$UUID", [{ type: "text", text: "Working." }]),
      { until: "subfolder-finish" },
      result(["$UUID"]),
    ]);
    await claude.start();
    await claude.prompt(id, "Change locally");
    await until(async () => texts(await claude.rows(id)).join("").includes("Working."), "The turn did not start.");
    write("sub/change.txt", "new\n");
    write("sub/.gitignore", "");
    write("sub/ignored.txt", "kept after\n");
    write("sub/nested/kept", "nested after\n");
    write("outside.txt", "outside after\n");
    claude.go("subfolder-finish");
    const rows = await claude.idles(id);
    const messageID = rows.find(row => row.type === "user").id;
    await until(() => {
      try {
        return Array.isArray(JSON.parse(fs.readFileSync(path.join(claude.home, "agents", "sessions", id, "agent.json"), "utf8")).snapshots?.[messageID]?.files);
      } catch { return false; }
    }, "The turn's file snapshot was not finalized.", 30000);
    const undo = await request(claude.listener, "POST", claude.route(id) + "/revert/stage", { messageID }, headers, 45000);
    assert.equal(undo.status, 200, undo.text);
    assert.equal(read("sub/change.txt"), "old\n");
    assert.equal(read("sub/.gitignore"), "ignored.txt\n");
    assert.equal(read("sub/ignored.txt"), "kept after\n");
    assert.equal(read("sub/nested/kept"), "nested after\n");
    assert.equal(read("outside.txt"), "outside after\n");
  } catch (error) {
    throw new Error([error.message, claude.log(), JSON.stringify(claude.heard().slice(-8))].join("\n"), { cause: error });
  } finally {
    await claude.close();
  }
});
