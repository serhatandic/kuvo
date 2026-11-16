// An earlier Kuvo Code's saved sign-ins, taken into this Mac's vault once its box answers (#713
// piece 4; PROTOCOL.md, "Moving this Mac's box", Saved sign-ins): the real link, moved already
// and still owing them, run as the installer runs it (`rehome --run`), against this Mac's box as
// its helper starts it (`serve`, its secrets on its stdin; its Keychain a stand-in that keeps the
// vault's values in memory). `kuvo-keys` is a stand-in that says them down its pipe, as the real
// one does, only to the process that holds the move's lock.
// Run with `node --test box/bridge-tests/rehome-passwords.test.js`.
import "./test-safety.js";
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { script, standInHost } from "./box-stand-ins.js";
import { atEnd, removeAtEnd } from "./at-end.js";

const mac = process.platform === "darwin";
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-rehome-passwords-")));
removeAtEnd(root);
const home = path.join(root, "home"), K = path.join(home, ".kuvo-code");
const APP = "run.kuvo.code.trial713";
const S = path.join(home, "Library", "Application Support", APP);
const bundle = path.join(home, "Applications", "Kuvo Code.app");
const write = (file, text, mode = 0o644) => { fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, text, { mode }); fs.chmodSync(file, mode); };
// (No password is one a file could hold by chance: the box's folder is full of random keys and
// ids in hex and base64, and three letters of those, as "abc" was here, turn up in one run in
// thirty. The short one has characters neither writes; the others are too long for chance.)
const LOGINS = [
  { host: "github.com", account: "ada", password: "hunter22" },
  { host: "github.com", account: "grace", password: "another-one" },
  { host: "example.org", account: "", password: "pass word" },
  { host: "short.example", account: "x", password: "a~%" },
];

test("saved sign-ins go into the vault once this Mac's box answers: each a sign-in of its site, the one agents may use alone still theirs, one too short said, none twice", { skip: !mac && "a Mac's vault and Keychain" }, async () => {
  // Moved already (a Mac's own box in its home), with the sign-ins still owed.
  write(path.join(K, "box", "kind"), "mac\n");
  write(path.join(K, "updates", "rehome", "state.json"), JSON.stringify({ state: "done", way: "move", owed: ["passwords"], from: S, to: K, app: { bundle, id: APP } }));
  write(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${APP}</string></dict></plist>\n`);
  write(path.join(bundle, "Contents", "MacOS", "kuvo-keys"), `#!/bin/sh\nverb=$1; shift; H=""\nwhile [ $# -gt 0 ]; do case "$1" in --home) H=$2; shift 2 ;; --password) P=$2; shift 2 ;; --account) A=$2; shift 2 ;; *) shift ;; esac; done\n[ "$(cat "$H/updates/rehome/lock/pid" 2>/dev/null)" = "$PPID" ] || { echo "not the move" >&2; exit 2; }\ncase "$verb" in passwords) cat "$HOME/logins.json" ;; delete-originals) cat >/dev/null; echo "$P $A" >>"$HOME/deleted"; exit 0 ;; esac\n`, 0o755);
  write(path.join(home, "logins.json"), JSON.stringify({ v: 1, logins: LOGINS }));
  // What the earlier Kuvo Code let agents sign in to alone, and with which account (its preferences).
  write(path.join(home, "Library", "Preferences", `${APP}.plist`), `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>passwords.agentSignIn</key><array><string>github.com</string></array><key>passwords.agentSignInAccounts</key><dict><key>github.com</key><string>ada</string></dict></dict></plist>\n`);

  // This Mac's box, as its helper starts it: its secrets on its stdin, where it answers in `run/serve.json`.
  const PASSWORD = "host-password-not-a-key";
  const host = await standInHost(PASSWORD);
  host.vaultSecret = crypto.randomBytes(32).toString("hex");
  const env = { PATH: process.env.PATH, HOME: home, TMPDIR: process.env.TMPDIR ?? os.tmpdir(), KUVO_HOME: K, KUVO_HOST_URL: host.url, KUVO_HOST_PASSWORD: PASSWORD };
  const serve = spawn(process.execPath, [script, "serve"], { env, stdio: ["pipe", "pipe", "pipe"] });
  let errors = "", printed = "";
  serve.stdout.on("data", chunk => { printed += chunk; });
  serve.stderr.on("data", chunk => { errors += chunk; });
  atEnd(() => new Promise(resolve => { if (serve.exitCode !== null) return resolve(); serve.once("exit", resolve); serve.stdin.end(); }));
  serve.stdin.write(JSON.stringify({ password: "box-pw", hostVaultSecret: host.vaultSecret, vaultFileKey: crypto.randomBytes(32).toString("base64url") }) + "\n");

  // (Run beside this test, never holding it: its stand-in Keychain answers the box meanwhile.)
  const run = () => new Promise(resolve => {
    const child = spawn(process.execPath, [script, "rehome", ".kuvo-code", "--run", "--app", bundle, "--release", path.join(K, "updates", "install", "release"), "--engine", path.join(K, "updates", "install", "engine")], {
      cwd: home, env: { ...env, KUVO_TEST_UNINSTALL_TOOLS: path.join(home, "tools") }, stdio: ["ignore", "pipe", "pipe"],
    });
    let out = "", err = "";
    child.stdout.on("data", chunk => { out += chunk; });
    child.stderr.on("data", chunk => { err += chunk; });
    child.once("exit", status => resolve({ status, out, said: JSON.parse(out.trim().split("\n").pop() || "{}"), errors: err }));
  });
  const settled = await run();
  assert.equal(settled.status, 0, settled.errors + errors);
  assert.deepEqual(settled.said.status.owed, [], settled.errors + errors);
  assert.deepEqual(settled.said.status.dropped, ["A saved password shorter than four characters wasn't brought into the vault, which keeps none that short."]);

  const served = JSON.parse(fs.readFileSync(path.join(K, "run", "serve.json"), "utf8"));
  const ask = async route => (await fetch(`http://127.0.0.1:${served.port}/${route}`, { headers: { authorization: `Basic ${Buffer.from(`opencode:${served.password}`).toString("base64")}`, "x-kuvo-from": "app" } })).json();
  const vault = await ask("kuvo/vault");
  assert.deepEqual(vault.items.map(item => [item.name, item.kind, item.sites, item.username ?? null, item.signInOnly]).sort(), [
    ["example.org", "login", ["example.org"], null, true],
    ["github.com (grace)", "login", ["github.com"], "grace", true],
    ["github.com", "login", ["github.com"], "ada", true],
  ]);
  // Their passwords, where this Mac's vault keeps values (its Keychain: here the stand-in).
  const ada = vault.items.find(item => item.username === "ada");
  assert.deepEqual(host.vault.get(ada.id), { password: "hunter22" });
  assert.deepEqual([...host.vault.values()].map(values => values.password).sort(), ["another-one", "hunter22", "pass word"]);
  // The site agents signed in to alone, with that account: still theirs; the other account of it, not.
  assert.deepEqual(vault.grants.map(grant => [grant.item, grant.how]), [[ada.id, "own"]]);

  // Owed again, with the sign-in there and agents not yet let use it (as a hand-over cut short
  // between the two would leave it): nothing is added twice, and they're let use it again.
  const state = path.join(K, "updates", "rehome", "state.json");
  fs.writeFileSync(state, JSON.stringify({ ...JSON.parse(fs.readFileSync(state, "utf8")), owed: ["passwords"] }));
  const taken = await fetch(`http://127.0.0.1:${served.port}/kuvo/vault/grants`, { method: "POST", headers: { authorization: `Basic ${Buffer.from(`opencode:${served.password}`).toString("base64")}`, "x-kuvo-from": "app", "content-type": "application/json" }, body: JSON.stringify({ item: ada.id, who: { everyone: true }, how: null }) });
  assert.equal(taken.status, 200);
  assert.deepEqual((await ask("kuvo/vault")).grants, []);
  const second = await run();
  assert.deepEqual(second.said.status.owed, []);
  const again = await ask("kuvo/vault");
  assert.equal(again.items.length, 3);
  assert.deepEqual(again.grants.map(grant => [grant.item, grant.how]), [[ada.id, "own"]]);
  // Each removed from the Keychain once the vault was found to keep it; the one too short stays.
  const deleted = () => fs.readFileSync(path.join(home, "deleted"), "utf8").split("\n").filter(Boolean);
  assert.deepEqual([...new Set(deleted())].sort(), ["example.org ", "github.com ada", "github.com grace"]);

  // Owed again, with sign-ins the vault can't be found to keep as they are: one whose site and
  // account it has with another password, two in the Keychain with the same site and account, and
  // one that wasn't UTF-8 (read with U+FFFD in its place). Each stays in the Keychain as it is, and
  // the one after them still moves: the step is done, not tried again at every open.
  fs.rmSync(path.join(home, "deleted"));
  const UNSURE = [
    { host: "github.com", account: "ada", password: "changed-since" },
    { host: "twice.example", account: "t", password: "first-of-two" },
    { host: "twice.example", account: "t", password: "second-of-two" },
    { host: "bytes.example", account: "b", password: "not\uFFFDutf8" },
    { host: "later.example", account: "l", password: "after-them-all" },
  ];
  write(path.join(home, "logins.json"), JSON.stringify({ v: 1, logins: UNSURE }));
  fs.writeFileSync(state, JSON.stringify({ ...JSON.parse(fs.readFileSync(state, "utf8")), owed: ["passwords"] }));
  const third = await run();
  assert.deepEqual(third.said.status.owed, [], third.errors + errors);
  assert.ok(third.said.status.dropped.includes("4 saved passwords stay in your Keychain, not the vault: the vault has another password for that site and account, or the Keychain more than one."), JSON.stringify(third.said.status.dropped));
  assert.deepEqual(deleted(), ["later.example l"], "only the one the vault keeps as it is goes from the Keychain");
  const after = await ask("kuvo/vault");
  assert.deepEqual(host.vault.get(ada.id), { password: "hunter22" }, "the vault's own is left as it is");
  assert.deepEqual(after.items.filter(item => ["twice.example", "later.example", "bytes.example"].includes(item.sites[0])).map(item => [item.name, host.vault.get(item.id).password]).sort(), [["later.example", "after-them-all"], ["twice.example", "first-of-two"]]);
  // The vault's activity notes none of them as shown: each was checked without its value coming back.
  const activity = await ask("kuvo/vault/activity");
  assert.ok(activity.entries.some(entry => entry.what === "added"), JSON.stringify(activity));
  assert.equal(activity.entries.some(entry => entry.what === "shown"), false, JSON.stringify(activity));
  LOGINS.push(...UNSURE.filter(login => !login.password.includes("\uFFFD")));

  // After both runs, no password's text is in the box's folder.
  // (Each file that holds one is named, with what stands around it, never only "a file".)
  const files = fs.readdirSync(K, { recursive: true, encoding: "utf8" }).map(name => { try { return [name, fs.readFileSync(path.join(K, name), "latin1")]; } catch { return [name, ""]; } });
  for (const login of LOGINS) {
    const found = files.filter(([, text]) => text.includes(login.password)).map(([name, text]) => `${name}: …${text.slice(Math.max(0, text.indexOf(login.password) - 40), text.indexOf(login.password) + login.password.length + 40)}…`);
    assert.deepEqual(found, [], `${login.host}'s password isn't in a file`);
  }
  // Nor in what the move or the box said (the one too short is counted, never quoted).
  for (const login of LOGINS) assert.equal((settled.out + settled.errors + second.out + second.errors + third.out + third.errors + printed + errors).includes(login.password), false, `${login.host}'s password isn't said`);
});
