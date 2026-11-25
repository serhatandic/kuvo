// Generate from the link bundles shipped inside the tagged apps.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { isolatedEnv, readJSON, scratch, write } from "../adapter.mjs";

const repo = fileURLToPath(new URL("../../../", import.meta.url));
const asset = "Packages/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js";
const noLaunch = fileURLToPath(new URL("./no-launch.mjs", import.meta.url));
export const tags = ["v0.7.2", "v0.8.0-beta.1"];
export const at = 1760000000000;
export const fixed = n => Buffer.alloc(32, n);
export const relay = "ws://127.0.0.1:1";
const git = (env, ...args) => execFileSync("git", ["-C", repo, "-c", "core.hooksPath=/dev/null", ...args],
  { env, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });

export function runSync(command, args, env, cwd, input = "", timeout = 30000) {
  const result = spawnSync(command, args, { env, cwd, input, timeout, encoding: "utf8" });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, `${args.join(" ")}\n${result.stdout}${result.stderr}`);
  return result.stdout;
}

export async function withTag(tag, use) {
  assert.ok(tags.includes(tag));
  const root = scratch();
  const tree = path.join(root, "tree");
  const env = isolatedEnv(root);
  try {
    git(env, "worktree", "add", "--detach", tree, tag);
    try {
      await use({ root, tree, script: path.join(tree, asset),
        commit: git(env, "rev-parse", tag + "^{commit}").trim() });
    } finally {
      git(env, "worktree", "remove", "--force", tree);
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

export function taggedCommand(script, home, env, args, input) {
  return runSync(process.execPath, ["--disable-warning=MODULE_TYPELESS_PACKAGE_JSON", "--import", pathToFileURL(noLaunch).href, script, ...args],
    { ...env, KUVO_HOME: home }, home, input);
}

export function seedLink(script, home, env, link) {
  runSync(process.execPath, ["--input-type=module", "-e",
    `const input = process.argv.pop(); const { writeLink } = await import(${JSON.stringify(pathToFileURL(script).href)}); writeLink(JSON.parse(input));`, JSON.stringify(link)],
  { ...env, KUVO_HOME: home }, home);
}

export function makeMachine({ script, code }, tag, destination) {
  const home = path.join(destination, "machine");
  mkdirSync(home, { recursive: true });
  const env = isolatedEnv(destination);
  const mac = code.publicOf(fixed(2)).toString("base64url");
  const peer = { key: mac, address: code.addressOf(fixed(3)).toString("base64url"), kind: "mac", name: "Fixture Mac", id: "fixture-mac" };
  const phone = { key: code.publicOf(fixed(4)).toString("base64url"), name: "Fixture Phone" };
  const setup = { relay, name: "Fixture Machine", phones: [phone] };
  const beta = tag === tags[1];
  if (beta) Object.assign(setup, { mac, at, peers: [peer], peersAt: at, id: "fixture-machine", moved: [], tolerates: 1, macID: "fixture-mac" });
  seedLink(script, home, env, { secret: fixed(5).toString("base64url"), key: fixed(6).toString("base64url"),
    relay, name: setup.name, phones: [{ ...phone, added: at / 1000, from: "mac" }] });
  const input = JSON.stringify(setup);
  const output = taggedCommand(script, home, env, ["setup"], input);
  const record = readJSON(path.join(home, "run/link.json"));
  assert.equal(record.name, setup.name);
  assert.equal(JSON.parse(output).key, code.publicOf(fixed(6)).toString("base64url"));
  if (beta) assert.deepEqual(record.peersFrom[mac], { at, peers: [peer], moved: [] });
  return { home, env, setup: { args: ["setup"], input, output }, mac, peer };
}

export function mailVector(code, tag, peer) {
  const bytes = tag === tags[1]
    ? Buffer.from(JSON.stringify({ at: at + 1, peers: [peer], ask: "agents", moved: [] }))
    : code.noticeJSON({ title: "Fixture Agent", category: "reply", thread: "ses_fixture", body: "Ready." });
  const [sender, recipient] = tag === tags[1] ? [fixed(2), fixed(6)] : [fixed(6), fixed(4)];
  const ephemeral = fixed(7);
  const sealed = code.sealOneWay(bytes, sender, code.publicOf(recipient), code.MAIL_PROLOGUE, ephemeral).message;
  const frame = code.frame(code.Frame.mail, 0, sealed);
  assert.deepEqual(code.openOneWay(code.readFrame(frame).body, recipient, code.publicOf(sender)).payload, bytes);
  return { sender: sender.toString("hex"), recipient: recipient.toString("hex"),
    ephemeral: ephemeral.toString("hex"), plaintext: bytes.toString(), frame: frame.toString("hex") };
}

// The released wire writer, with multiple roster updates and ordinary agent
// announcements used as deterministic receipt barriers.
export function rosterVectors(code, mac) {
  const identity = (key, secret, name) => ({ key: code.publicOf(fixed(key)).toString("base64url"),
    address: code.addressOf(fixed(secret)).toString("base64url"), kind: "machine", name });
  const second = identity(8, 9, "Second Machine");
  const third = identity(10, 11, "Third Machine");
  const own = identity(6, 5, "Fixture Machine");
  const frame = (n, fields, announced) => {
    const bytes = Buffer.from(JSON.stringify({ ...fields, agents: { at: announced, list: [] } }));
    return code.frame(code.Frame.mail, 0, code.sealOneWay(bytes, fixed(n), code.publicOf(fixed(6)), code.MAIL_PROLOGUE, fixed(announced - at + 20)).message).toString("hex");
  };
  return { second, third, own, added: frame(2, { at: at + 2, peers: [mac, second, own, { key: "bad" }] }, at + 2),
    stale: frame(2, { at: at + 1, peers: [mac] }, at + 3),
    notEnroller: frame(8, { at: at + 10, peers: [third] }, at + 4),
    removed: frame(2, { at: at + 3, peers: [mac] }, at + 5) };
}

export function settingsVectors(script, home, env) {
  const cases = [
    { args: ["box", "read"], input: "" },
    { args: ["box", "adopt"], input: JSON.stringify({ settings: { instructions: ["FIXTURE.md"] } }) },
    { args: ["box", "change"], input: '{"changes":[{"path":["instructions"],"value":["UPDATED.md"]}]}' },
    { args: ["box", "key", "KUVO_KEY_NOTION"], input: "fixture-value" },
    { args: ["box", "read"], input: "" },
    { args: ["box", "route"], input: '{"method":"GET","path":"kuvo/bots"}' },
  ];
  return cases.map(item => ({ ...item, output: taggedCommand(script, home, env, item.args, item.input) }));
}

export function save(file, value) {
  write(file, JSON.stringify(value, null, 2) + "\n");
}
