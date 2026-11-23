// A real box on the relay stand-in, for a client's own tests to meet (gpui's `kuvo_box`, in Rust:
// `gpui/kuvo_box/tests/real_box.rs`): a machine's box in a scratch home, its link running as a
// machine runs it (`kuvo-link.js run`), waiting to pair. It prints one line, `{relay, host, key,
// token, name}` (what a pairing link says), and stays until its standard input ends; then it stops
// the box it started (by its pid) and removes its folder. Nothing real is reached.
import { removeTestHomeAtExit, stopTestProcesses } from "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { testBun } from "./bun-for-tests.js";
import { relayStandIn } from "./relay-stand-in.js";

for (const name of Object.keys(process.env)) if (name.startsWith("KUVO_") && name !== "KUVO_TEST_BUN") delete process.env[name];
const script = fileURLToPath(LINK_BUNDLE);
const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-real-box-")));
removeTestHomeAtExit(home);
const box = path.join(home, ".kuvo-code");
fs.mkdirSync(path.join(box, "run"), { recursive: true, mode: 0o700 });
process.env.KUVO_HOME = box;
const link = await import(script);
const relay = await relayStandIn(link);

const key = link.newKey();
const secret = crypto.randomBytes(32);
const token = crypto.randomBytes(16).toString("base64url");
const name = "real-box";
fs.writeFileSync(path.join(box, "run", "link.json"), JSON.stringify({
  relay: relay.url, secret: secret.toString("base64url"), key: key.toString("base64url"), sign: link.newSigningSeed(), name, phones: [],
  pairing: { token, expires: Date.now() + 600_000, issuer: null },
}), { mode: 0o600 });

// As a machine runs its link: on Bun where there's one (a box's own runtime), else this Node.
const bun = testBun();
const child = spawn(bun ?? process.execPath, [script, "run"], { env: { PATH: process.env.PATH, HOME: home, KUVO_HOME: box, TMPDIR: home, ...(bun ? { BUN_BE_BUN: "1" } : {}) }, stdio: ["ignore", "ignore", "inherit"] });
let ending = false;
const end = code => {
  if (ending) return;
  ending = true;
  const done = async () => {
    try { relay.close?.(); } catch {}
    await stopTestProcesses();
    process.exit(code);
  };
  if (child.exitCode !== null || child.signalCode !== null) return done();
  child.once("exit", done);
  child.kill("SIGTERM");
};
child.once("exit", status => { if (!ending) { console.error(`the box's link ended by itself (${status})`); end(1); } });

// On the relay: only then is there anything to pair with.
const address = link.addressOf(secret);
const started = Date.now();
const waiting = setInterval(() => {
  if (relay.has(address)) {
    clearInterval(waiting);
    console.log(JSON.stringify({ relay: relay.url, host: address.toString("base64url"), key: link.publicOf(key).toString("base64url"), token, name, runtime: bun ? "bun" : "node" }));
  } else if (Date.now() - started > 30_000) {
    console.error("the box's link didn't reach the relay");
    end(1);
  }
}, 20);
process.stdin.on("end", () => end(0)).on("close", () => end(0)).resume();
