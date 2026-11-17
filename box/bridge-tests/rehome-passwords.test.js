// An earlier Kuvo Code's saved sign-ins, taken into this Mac's vault when you ask (#713 piece 4;
// PROTOCOL.md, "Moving this Mac's box", Saved sign-ins): the real link, moved already and still
// owing them, as its helper starts it (`serve`, its secrets on its stdin; its Keychain a stand-in
// that keeps the vault's values in memory), asked by Kuvo on this Mac as the app asks it
// (`POST kuvo/box/rehome {signIns: true}`), which starts the hand-over in a process of its own.
// `kuvo-keys` is a stand-in that says them down its pipe, as the real one does, only to the
// process that holds the move's lock; and that waits, where the test says, as the real one does
// while macOS's question goes unanswered.
// Run with `node --test box/bridge-tests/rehome-passwords.test.js`.
import "./test-safety.js";
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
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
const state = path.join(K, "updates", "rehome", "state.json");
const record = () => JSON.parse(fs.readFileSync(state, "utf8"));

/** Once `check()` holds: asked again each time something changes in `folder`, never on a timer. */
function until(folder, check) {
  return new Promise(resolve => {
    const watcher = fs.watch(folder, () => { if (check()) { watcher.close(); resolve(); } });
    if (check()) { watcher.close(); resolve(); }
  });
}
/** Once the hand-over that's running has said how it went. */
const handedOver = before => until(path.dirname(state), () => { try { return (record().signInsLast?.at ?? 0) > before; } catch { return false; } });

test("saved sign-ins come over when you ask, never before: macOS's question waited on holds nothing up; each a sign-in of its site, the one agents may use alone still theirs, one too short said, none twice", { skip: !mac && "a Mac's vault and Keychain" }, async () => {
  // Moved already (a Mac's own box in its home), with the sign-ins still owed.
  write(path.join(K, "box", "kind"), "mac\n");
  write(path.join(K, "updates", "rehome", "state.json"), JSON.stringify({ state: "done", way: "move", owed: ["passwords"], from: S, to: K, app: { bundle, id: APP } }));
  write(path.join(bundle, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>\n<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${APP}</string></dict></plist>\n`);
  // (Asked for them, it notes so; where the test holds it, it waits as macOS's question does until
  // it's answered; where you clicked Deny, it says so.)
  write(path.join(bundle, "Contents", "MacOS", "kuvo-keys"), `#!/bin/sh\nverb=$1; shift; H=""\nwhile [ $# -gt 0 ]; do case "$1" in --home) H=$2; shift 2 ;; --password) P=$2; shift 2 ;; --account) A=$2; shift 2 ;; *) shift ;; esac; done\n[ "$(cat "$H/updates/rehome/lock/pid" 2>/dev/null)" = "$PPID" ] || { echo "not the move" >&2; exit 2; }\ncase "$verb" in passwords) echo passwords >>"$HOME/keys-asked"; [ -p "$HOME/question" ] && read answer <"$HOME/question"; [ -f "$HOME/denied" ] && { echo '{"error":"You denied it.","reason":"not-allowed","status":-128}'; exit 1; }; cat "$HOME/logins.json" ;; delete-originals) cat >/dev/null; [ "$P" = changed.example ] && { echo '{"reason":"copy-missing"}'; exit 1; }; echo "$P $A" >>"$HOME/deleted"; exit 0 ;; esac\n`, 0o755);
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

  // (Once it says where it answers, its `run/serve.json` is there: it writes that first.)
  await new Promise(resolve => { const said = () => { if (printed.includes('"url"')) { serve.stdout.off("data", said); resolve(); } }; serve.stdout.on("data", said); said(); });
  const served = JSON.parse(fs.readFileSync(path.join(K, "run", "serve.json"), "utf8"));
  const authorization = `Basic ${Buffer.from(`opencode:${served.password}`).toString("base64")}`;
  const ask = async route => (await fetch(`http://127.0.0.1:${served.port}/${route}`, { headers: { authorization, "x-kuvo-from": "app" } })).json();
  /** Kuvo on this Mac asking for them (or, without its word, anything else on it). */
  const bring = async ({ app = true } = {}) => {
    const answer = await fetch(`http://127.0.0.1:${served.port}/kuvo/box/rehome`, { method: "POST", headers: { authorization, "content-type": "application/json", ...(app ? { "x-kuvo-from": "app" } : {}) }, body: JSON.stringify({ signIns: true }) });
    return { status: answer.status, value: await answer.json() };
  };
  /** The installer, as the app runs it as it opens, after an update or not. */
  const installer = (verb, more = []) => {
    const ran = spawnSync(process.execPath, [script, "install", verb, "--root", ".kuvo-code", "--app", bundle, ...more], { cwd: K, env: { PATH: process.env.PATH, HOME: home, TMPDIR: env.TMPDIR, KUVO_HOME: K }, encoding: "utf8", timeout: 60_000 });
    return { status: ran.status, said: JSON.parse(ran.stdout.trim().split("\n").pop() || "{}"), errors: ran.stderr };
  };

  // Nothing's read until you ask: the box ran, and kuvo-keys was asked nothing.
  assert.deepEqual((await ask("kuvo/box/rehome")).owed, ["passwords"]);
  assert.equal(fs.existsSync(path.join(home, "keys-asked")), false);
  // Only Kuvo on this Mac asks (macOS asks you there): a phone, or anything else, can't.
  assert.equal((await bring({ app: false })).status, 403);

  // Asked, with macOS's question left unanswered.
  assert.equal(spawnSync("mkfifo", [path.join(home, "question")]).status, 0);
  atEnd(() => { try { const held = fs.openSync(path.join(home, "question"), fs.constants.O_WRONLY | fs.constants.O_NONBLOCK); fs.writeSync(held, "no\n"); fs.closeSync(held); } catch {} });
  let before = Date.now() - 1;
  const asked = await bring();
  assert.equal(asked.status, 202, JSON.stringify(asked.value));
  assert.deepEqual(asked.value.signIns, { state: "asking" });
  await until(home, () => fs.existsSync(path.join(home, "keys-asked")));
  // Meanwhile Kuvo opens and updates as ever: this Mac's own, nothing to run, nothing waited for;
  // the move asked all the same (an older app would) runs nothing.
  const status = installer("--status");
  assert.deepEqual({ verdict: status.said.verdict, verb: status.said.verb, signIns: status.said.status?.signIns }, { verdict: "ours", verb: null, signIns: { state: "asking" } }, status.errors);
  const move = installer("--move", ["--release", path.join(K, "updates", "install", "release"), "--engine", path.join(K, "updates", "install", "engine")]);
  assert.deepEqual({ status: move.status, result: move.said.result }, { status: 0, result: "nothing" }, move.errors);
  // Asked again, it's the one that runs: none started beside it.
  assert.equal((await bring()).status, 200);
  assert.deepEqual(fs.readFileSync(path.join(home, "keys-asked"), "utf8").trim().split("\n"), ["passwords"]);
  // Answered.
  fs.writeFileSync(path.join(home, "question"), "allowed\n");
  fs.rmSync(path.join(home, "question"));
  await handedOver(before);
  const settled = { said: { status: await ask("kuvo/box/rehome") } };
  assert.deepEqual(settled.said.status.owed, [], errors);
  assert.deepEqual(settled.said.status.signIns, { state: "done", at: record().signInsLast.at, added: 3, kept: 1 });
  assert.deepEqual(settled.said.status.dropped, ["A saved password shorter than four characters wasn't brought into the vault, which keeps none that short."]);
  /** Asked again, as a hand-over cut short would leave them owed. */
  const run = async () => {
    before = Date.now() - 1;
    assert.equal((await bring()).status, 202);
    await handedOver(before);
    return { said: { status: await ask("kuvo/box/rehome") } };
  };

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
  // account it has with another password, two in the Keychain with the same site and account (the
  // real kuvo-keys reads both by them, so both say the one password), one that wasn't UTF-8 (read
  // with U+FFFD in its place), one whose site the vault refuses, and one that changed in the
  // Keychain meanwhile. Each stays in the Keychain as it is, and the one after them still moves:
  // the step is done, not tried again at every open.
  fs.rmSync(path.join(home, "deleted"));
  const UNSURE = [
    { host: "github.com", account: "ada", password: "changed-since" },
    { host: "twice.example", account: "t", password: "one-of-two" },
    { host: "twice.example", account: "t", password: "one-of-two" },
    { host: "bytes.example", account: "b", password: "not\uFFFDutf8" },
    { host: "bad_host.example", account: "u", password: "refused-site" },
    { host: "changed.example", account: "c", password: "changed-meanwhile" },
    { host: "later.example", account: "l", password: "after-them-all" },
  ];
  write(path.join(home, "logins.json"), JSON.stringify({ v: 1, logins: UNSURE }));
  fs.writeFileSync(state, JSON.stringify({ ...JSON.parse(fs.readFileSync(state, "utf8")), owed: ["passwords"] }));
  const third = await run();
  assert.deepEqual(third.said.status.owed, [], errors);
  assert.ok(third.said.status.dropped.includes("6 saved passwords stay in your Keychain: Kuvo couldn't be sure the vault would keep them exactly. Add any you use to the vault yourself."), JSON.stringify(third.said.status.dropped));
  assert.deepEqual(deleted(), ["later.example l"], "only the one the vault keeps as it is goes from the Keychain");
  const after = await ask("kuvo/vault");
  assert.deepEqual(host.vault.get(ada.id), { password: "hunter22" }, "the vault's own is left as it is");
  assert.deepEqual(after.items.filter(item => !["github.com", "example.org"].includes(item.sites[0])).map(item => [item.name, host.vault.get(item.id).password]).sort(), [["changed.example", "changed-meanwhile"], ["later.example", "after-them-all"]]);
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
  for (const login of LOGINS) assert.equal((printed + errors + fs.readFileSync(path.join(K, "updates", "rehome", "run.log"), "utf8")).includes(login.password), false, `${login.host}'s password isn't said`);

  // Owed again, and you click Deny: said in words to act on, still owed, nothing taken.
  fs.writeFileSync(state, JSON.stringify({ ...record(), owed: ["passwords"] }));
  write(path.join(home, "denied"), "");
  before = Date.now() - 1;
  assert.equal((await bring()).status, 202);
  await handedOver(before);
  const denied = await ask("kuvo/box/rehome");
  assert.deepEqual({ owed: denied.owed, signIns: denied.signIns }, { owed: ["passwords"], signIns: { state: "failed", at: record().signInsLast.at, message: "macOS didn't let Kuvo read them. Try again: when macOS asks whether kuvo-keys may use each one, enter your Mac's password and click Always Allow." } });
});
