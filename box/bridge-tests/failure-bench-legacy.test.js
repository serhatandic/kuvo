import "./test-safety.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { removeAtEnd } from "./at-end.js";
import { legacyBuild } from "./failure-bench-legacy.js";

test("failure bench: missing or substituted compatibility builds fail closed", () => {
  const checkout = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-bench-history-"));
  removeAtEnd(checkout);
  assert.throws(() => legacyBuild(checkout), /Missing pinned failure-bench build/);

  const archived = path.join(checkout, "box", "bridge-tests", ".failure-bench-legacy.mjs");
  fs.mkdirSync(path.dirname(archived), { recursive: true });
  fs.writeFileSync(archived, "a substitute build");
  assert.throws(() => legacyBuild(checkout), /Wrong pinned failure-bench build/);

  const pinned = legacyBuild();
  fs.writeFileSync(archived, pinned);
  assert.deepEqual(legacyBuild(checkout), pinned);
});
