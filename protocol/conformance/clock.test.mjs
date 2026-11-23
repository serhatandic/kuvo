import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, rmSync } from "node:fs";
import { test } from "node:test";
import { boxBinary, isolatedEnv, scratch, typescriptRuntime } from "./adapter.mjs";
import { bench } from "./bench.mjs";

const vector = new URL("../../box/crates/kuvo_setup/data/time-zones.txt", import.meta.url);

test("the clock offers the reference Intl time zones and hashes the offered IDs", async t => {
  const zones = readFileSync(vector, "utf8").trimEnd().split("\n");
  if (boxBinary().typescript) {
    const home = scratch();
    try {
      const output = execFileSync(typescriptRuntime(), [
        "--no-env-file", "-e",
        'console.log([...new Set([...Intl.supportedValuesOf("timeZone"), "UTC"])].sort().join("\\n"))',
      ], { cwd: home, env: isolatedEnv(home), encoding: "utf8" });
      assert.equal(output.trimEnd(), zones.join("\n"));
    } finally {
      rmSync(home, { recursive: true, force: true });
    }
  }
  const { computer } = await bench(t);
  const box = await computer("Clock");
  const ask = box.connect(await box.start().ready());
  const reply = await ask("GET", "kuvo/settings/clock");
  assert.equal(reply.status, 200, reply.text);
  const [row] = reply.json.groups[0].rows;
  assert.equal(row.value, "own-clock");
  assert.deepEqual(row.choices.slice(1), zones.map(id => ({
    id, title: id.replaceAll("_", " ").replaceAll("/", " › "),
  })));
  const governed = { can: true, choices: ["own-clock", ...zones], kind: "choice", value: "own-clock" };
  assert.equal(row.revision, createHash("sha256").update(JSON.stringify(governed)).digest("base64url").slice(0, 16));
});
