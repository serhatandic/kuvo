// A client's request over SSH (`kuvo-start route`, `route-to`) runs on the engine the keeper runs,
// whatever other engines are in `engine/`: the real keeper script, in a home of the test's own,
// with stand-in engines that say which one answered.
//
// Run with `node --test box/bridge-tests/keeper-route.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

const keeper = fileURLToPath(new URL("../src/keeper/kuvo-start", import.meta.url));
const write = (file, text, mode) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
  if (mode) fs.chmodSync(file, mode);
};

/** A box whose release runs engine 1.1.0, with 1.0.0 and 1.2.0 beside it. */
function box() {
  const folder = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-keeper-route-")));
  removeAtEnd(folder);
  const K = path.join(folder, ".kuvo-code");
  write(path.join(K, "bin", "kuvo-start"), fs.readFileSync(keeper, "utf8").replaceAll("@ENGINE@", "1.1.0"), 0o700);
  write(path.join(K, "bin", "kuvo-link.js"), "// stand-in\n");
  for (const version of ["1.0.0", "1.1.0", "1.2.0"])
    write(path.join(K, "engine", version, "opencode"), `#!/bin/sh\necho "${version} $* $(cat)"\n`, 0o755);
  const route = verb => spawnSync("sh", [path.join(K, "bin", "kuvo-start"), verb], {
    env: { PATH: "/usr/bin:/bin:/usr/sbin:/sbin", HOME: folder }, input: "the request", encoding: "utf8", timeout: 60_000,
  });
  return { K, route };
}

test("a request over SSH runs on the keeper's engine, not the newest one there", { skip: process.platform === "win32" }, () => {
  const { K, route } = box();
  // A newer engine waits for its swap: the keeper still runs the release's own.
  write(path.join(K, "run", "engine-ran"), "1.1.0\n");
  for (const verb of ["route", "route-to"]) {
    const answered = route(verb);
    assert.equal(answered.status, 0, answered.stderr);
    assert.equal(answered.stdout.trim(), `1.1.0 --config=/dev/null --no-env-file ${K}/bin/kuvo-link.js box ${verb} the request`);
  }
  // The release's engine didn't start, so the keeper went back to the one before: so does SSH.
  write(path.join(K, "run", "engine-back"), `1.1.0 1.0.0 ${Math.floor(Date.now() / 1000)}\n`);
  assert.match(route("route").stdout, /^1\.0\.0 /);
});

test("the selected release manifest gives SSH and the Mac helper to the Rust box", { skip: process.platform === "win32" }, () => {
  const { K, route } = box();
  const target = "versions/0.8.0-000000000000/bin";
  fs.mkdirSync(path.dirname(path.join(K, target)), { recursive: true });
  fs.renameSync(path.join(K, "bin"), path.join(K, target));
  fs.symlinkSync(target, path.join(K, "bin"));
  write(path.join(K, target, "kuvo-box"), '#!/bin/sh\nprintf "%s %s\\n" "$*" "$(cat)"\n', 0o700);
  write(path.join(K, "box", "kind"), "mac\n");
  const manifest = path.join(K, target, "..", "box.json");
  write(manifest, '{"kuvo":"box/1","files":{"launch":"rust"}}');
  assert.match(route("route").stdout, /^1\.1\.0 /);

  write(manifest, '{ "kuvo": "box/1",\n  "launch" : "rust", "files":{} }');
  for (const verb of ["route", "route-to"]) {
    const answered = route(verb);
    assert.equal(answered.status, 0, answered.stderr);
    assert.equal(answered.stdout, `${verb} the request\n`);
  }
  const served = route("serve");
  assert.equal(served.status, 0, served.stderr);
  assert.equal(fs.readFileSync(path.join(K, "run", "serve.log"), "utf8"), "serve the request\n");
});
