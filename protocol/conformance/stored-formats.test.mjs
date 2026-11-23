// Exercise the layout gate at the resident process boundary on both binaries.
import assert from "node:assert/strict";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, localRelay, request, scratch, startBox, write } from "./adapter.mjs";

for (const [kind, marker, error] of [
  ["newer", '{"version":24}\n', /format version 24 is unsupported/],
  ["malformed", '{"version":\n', /format version invalid is unsupported/],
  ["invalid checkpoint", '{"version":22,"upgradingTo":24}\n', /format version 24 is unsupported/],
]) {
  test(`startup refuses the ${kind} layout before touching the home`, { timeout: 10000 }, async () => {
    const root = scratch();
    const home = path.join(root, "box");
    const file = path.join(home, "folder-version.json");
    const binary = boxBinary();
    const relay = await localRelay();
    write(file, marker);
    const proc = boxCommand(binary, home, isolatedEnv(root), relay, ["serve", ...(binary.typescript ? ["--resident"] : [])]);
    try {
      const { code } = await proc.exited;
      assert.notEqual(code, 0);
      assert.match(proc.log(), error);
      assert.equal(readFileSync(file, "utf8"), marker);
      assert.deepEqual(readdirSync(home), ["folder-version.json"]);
    } finally {
      await proc.stop();
      await relay.close();
      rmSync(root, { recursive: true, force: true });
    }
  });
}

test("an empty home opens at layout 23 and its committed marker survives a restart", { timeout: 30000 }, async () => {
  const root = scratch();
  const home = path.join(root, "box");
  const marker = path.join(home, "folder-version.json");
  const binary = boxBinary();
  const relay = await localRelay();
  const env = isolatedEnv(root);
  let proc;
  try {
    mkdirSync(home);
    proc = startBox({ binary, home, env, relay, mac: true });
    assert.equal((await request(await proc.ready(), "GET", "kuvo/info")).status, 200);
    assert.equal(readFileSync(marker, "utf8"), '{"version":23}\n');
    await proc.stop();
    proc = startBox({ binary, home, env, relay, mac: true });
    assert.equal((await request(await proc.ready(), "GET", "kuvo/info")).status, 200);
    await proc.stop();
    assert.equal(readFileSync(marker, "utf8"), '{"version":23}\n');
    assert.equal(existsSync(path.join(home, ".folder-upgrade.lock")), false);
  } finally {
    await proc?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
});
