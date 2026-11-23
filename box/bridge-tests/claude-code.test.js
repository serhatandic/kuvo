// Claude Code's agents on a box's own computer (PROTOCOL.md, `kuvo/claude-code`): each
// box lists what's in its own ~/.claude, named as they'd come over to its settings and marked where
// they have them. The real box, with the engine stopped.
//
// Run with `node --test box/bridge-tests/claude-code.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";

const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-"));
const user = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-user-"));
removeAtEnd(home);
removeAtEnd(user);
process.env.KUVO_HOME = home;
process.env.HOME = user;
const link = await import(LINK_BUNDLE.href);

const handle = link.engineHandler(() => { throw new Error("not running"); });

function write(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}

async function answer(handle, method, route) {
  const parts = [];
  for await (const part of handle({ method, path: route, headers: {}, body: Buffer.alloc(0) }, {}, undefined)) parts.push(part);
  const text = Buffer.concat(parts.filter(part => part.body).map(part => part.body)).toString();
  return { status: parts[0].head.status, json: text ? JSON.parse(text) : undefined };
}

const claude = path.join(user, ".claude");
write(path.join(claude, "agents", "reviewer.md"), "---\nname: Code Reviewer\ndescription: \"Reviews code.\\nThoroughly.\"\n---\nYou review code.\n");
write(path.join(claude, "agents", "plan.md"), "---\ndescription: |\n  Plans\n  ahead\n---\nPlan first.\n");
write(path.join(claude, "agents", "notes.txt"), "not an agent");
// No header at all: by its file's name.
write(path.join(claude, "agents", "Release Notes.md"), "Write release notes.\n");
// Released Macs still read the empty commands field.
write(path.join(claude, "commands", "deploy.md"), "Ship it.\n");
write(path.join(home, "box", "opencode.json"), JSON.stringify({ agents: { "code-reviewer": { mode: "subagent" } } }));

test("a computer lists its Claude Code agents with the engine stopped", async () => {
  const { status, json } = await answer(handle, "GET", "kuvo/claude-code");
  assert.equal(status, 200);
  assert.deepEqual(json.agents, [
    { name: "Code Reviewer", description: "Reviews code.\nThoroughly.", path: ["agents", "code-reviewer"], settings: { mode: "subagent", description: "Reviews code.\nThoroughly.", system: "You review code." }, imported: true },
    // (One by an engine's own agent's name comes over beside it.)
    { name: "plan", description: "Plans\nahead", path: ["agents", "plan-claude"], settings: { mode: "subagent", description: "Plans\nahead", system: "Plan first." }, imported: false },
    { name: "Release Notes", description: "", path: ["agents", "release-notes"], settings: { mode: "subagent", description: "", system: "Write release notes." }, imported: false },
  ]);
  assert.deepEqual(json.commands, []);
});

test("a computer with no ~/.claude has nothing to bring over; another's list, or a change, isn't asked here", async () => {
  const saved = process.env.HOME;
  process.env.HOME = path.join(user, "nobody");
  try {
    assert.deepEqual(await answer(handle, "GET", "kuvo/claude-code"), { status: 200, json: { agents: [], commands: [] } });
  } finally {
    process.env.HOME = saved;
  }
  assert.equal((await answer(handle, "GET", "kuvo/claude-code?machine=lab")).status, 404);
  assert.equal((await answer(handle, "POST", "kuvo/claude-code")).status, 405);
});

// What's read of ~/.claude: an agent may write files there (the sandbox lets it, outside the
// settings Claude Code starts with), and could leave a link to a file it can't read itself; so the
// box reads only Markdown files, by their own names, in your home folder but not where your keys
// and sign-ins are, and nothing that would hold it.
test("only Markdown files are offered: not a link to a key or another file, a hard link, a pipe, or a file too big", async () => {
  const saved = process.env.HOME;
  const other = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-limits-"));
  const elsewhere = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-elsewhere-"));
  removeAtEnd(other);
  removeAtEnd(elsewhere);
  process.env.HOME = other;
  try {
    const folder = path.join(other, ".claude");
    write(path.join(folder, "agents", "kept.md"), "---\nname: kept\n---\nKept.\n");
    const agents = path.join(folder, "agents");
    write(path.join(folder, "notes", "shared.md"), "Shared.\n");
    fs.symlinkSync("../notes/shared.md", path.join(agents, "shared.md"));
    write(path.join(other, ".ssh", "id_rsa"), "Fixture key.\n");
    write(path.join(folder, ".credentials.json"), '{"token":"fixture"}');
    write(path.join(other, "notes.txt"), "Notes.\n");
    fs.symlinkSync("../../.ssh/id_rsa", path.join(agents, "key.md"));
    fs.symlinkSync("../.credentials.json", path.join(agents, "signin.md"));
    fs.symlinkSync("../../notes.txt", path.join(agents, "notes.md"));

    write(path.join(other, ".ssh", "keys.md"), "My keys.\n");
    write(path.join(other, ".aws", "notes.md"), "My account.\n");
    write(path.join(other, ".password-store", "notes.md"), "My passwords.\n");
    write(path.join(elsewhere, "theirs.md"), "Someone else's.\n");
    fs.symlinkSync("../../.ssh/keys.md", path.join(agents, "keys.md"));
    fs.symlinkSync("../../.aws/notes.md", path.join(agents, "account.md"));
    fs.symlinkSync("../../.password-store/notes.md", path.join(agents, "passwords.md"));
    fs.symlinkSync(path.join(elsewhere, "theirs.md"), path.join(agents, "theirs.md"));
    fs.symlinkSync("../../.ssh", path.join(agents, "ssh.md"));
    fs.linkSync(path.join(other, ".ssh", "id_rsa"), path.join(agents, "hard-key.md"));
    write(path.join(folder, "notes", "twice.md"), "Twice.\n");
    fs.linkSync(path.join(folder, "notes", "twice.md"), path.join(agents, "twice.md"));
    execFileSync("mkfifo", [path.join(agents, "pipe.md")]);
    write(path.join(agents, "huge.md"), "x".repeat(256 * 1024 + 1));
    write(path.join(agents, "long.md"), "y".repeat(256 * 1024));
    write(path.join(agents, "folder.md", "nested.md"), "Not a top-level agent.\n");

    const { status, json } = await answer(handle, "GET", "kuvo/claude-code");
    assert.equal(status, 200);
    assert.deepEqual(json.agents.map(agent => agent.name), ["kept", "long", "shared"]);
    assert.equal(json.agents[1].settings.system.length, 256 * 1024);
    assert.equal(json.agents[2].settings.system, "Shared.");
  } finally {
    process.env.HOME = saved;
  }
});

test("however many files are there, no more than 4 MB of them is read", async () => {
  const saved = process.env.HOME;
  const other = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-many-"));
  removeAtEnd(other);
  process.env.HOME = other;
  try {
    // Twenty of 250 KB: about 5 MB together.
    for (let index = 10; index < 30; index++) write(path.join(other, ".claude", "agents", `c${index}.md`), "z".repeat(250 * 1024));
    const { status, json } = await answer(handle, "GET", "kuvo/claude-code");
    assert.equal(status, 200);
    // (In order, until the next would pass it.)
    assert.deepEqual(json.agents.map(agent => agent.name), Array.from({ length: 16 }, (_, index) => `c${index + 10}`));
  } finally {
    process.env.HOME = saved;
  }
});

test("a ~/.claude that's a link (to your dotfiles, say) is read where it leads", async () => {
  const saved = process.env.HOME;
  const other = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-dotfiles-"));
  removeAtEnd(other);
  process.env.HOME = other;
  try {
    write(path.join(other, "dotfiles", "claude", "agents", "ship.md"), "Ship it.\n");
    fs.symlinkSync(path.join(other, "dotfiles", "claude"), path.join(other, ".claude"));
    const { json } = await answer(handle, "GET", "kuvo/claude-code");
    assert.deepEqual(json.agents.map(agent => agent.name), ["ship"]);
  } finally {
    process.env.HOME = saved;
  }
});

test("a ~/.claude linked out of your home folder (dotfiles on another disk) is read there, but no further", async () => {
  const saved = process.env.HOME;
  const other = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-home-"));
  const disk = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-disk-"));
  removeAtEnd(other);
  removeAtEnd(disk);
  process.env.HOME = other;
  try {
    write(path.join(disk, "claude", "agents", "ship.md"), "Ship it.\n");
    write(path.join(disk, "other.md"), "Not Claude Code's.\n");
    fs.symlinkSync("../../other.md", path.join(disk, "claude", "agents", "other.md"));
    fs.symlinkSync(path.join(disk, "claude"), path.join(other, ".claude"));
    const { json } = await answer(handle, "GET", "kuvo/claude-code");
    assert.deepEqual(json.agents.map(agent => agent.name), ["ship"]);
  } finally {
    process.env.HOME = saved;
  }
});

// (The Nix store, where home-manager links lead, is read as your home is; it isn't stood in for here.)
test("an agents folder and an agent file linked into your dotfiles are read where they lead", async () => {
  const saved = process.env.HOME;
  const other = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-link-claude-code-stow-"));
  removeAtEnd(other);
  process.env.HOME = other;
  try {
    const agents = path.join(other, "dotfiles", "claude", ".claude", "agents");
    write(path.join(agents, "tester.md"), "---\nname: tester\n---\nTest it.\n");
    write(path.join(other, "dotfiles", "more", "lint.md"), "Lint.\n");
    const claude = path.join(other, ".claude");
    fs.mkdirSync(claude);
    fs.symlinkSync("../dotfiles/claude/.claude/agents", path.join(claude, "agents"));
    fs.symlinkSync("../../../more/lint.md", path.join(agents, "check.md"));
    // ~/.ssh can itself link into the dotfiles.
    write(path.join(other, "dotfiles", "ssh", "keys.md"), "My keys.\n");
    fs.symlinkSync(path.join(other, "dotfiles", "ssh"), path.join(other, ".ssh"));
    fs.symlinkSync("../../../ssh/keys.md", path.join(agents, "keys.md"));
    const { json } = await answer(handle, "GET", "kuvo/claude-code");
    assert.deepEqual(json.agents.map(agent => [agent.name, agent.settings.system]), [
      ["check", "Lint."], ["tester", "Test it."],
    ]);
  } finally {
    process.env.HOME = saved;
  }
});
