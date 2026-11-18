import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ref = fs.readFileSync(new URL("failure-bench-legacy-ref.txt", import.meta.url), "utf8").trim();
const resource = "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js";

// SHA-256 of the resource at the pinned commit. Lokum has no checkout history to verify against.
const digest = "aeb60b84f4f29590535c67ed0a3d6c5a33f448d8b8c8e566493cf6d0e598f5f2";

export function legacyBuild(checkout = fileURLToPath(new URL("../..", import.meta.url))) {
  const archived = path.join(checkout, "box", "bridge-tests", ".failure-bench-legacy.mjs");
  let build;
  if (fs.existsSync(archived)) {
    build = fs.readFileSync(archived);
  } else {
    const result = spawnSync("git", ["--no-optional-locks", "-C", checkout, "show", `${ref}:${resource}`], { maxBuffer: 64 * 1024 * 1024 });
    if (result.status !== 0) {
      throw new Error(`Missing pinned failure-bench build at ${ref}. Run scripts/failure-bench.sh --prepare.`);
    }
    build = result.stdout;
  }
  if (createHash("sha256").update(build).digest("hex") !== digest) {
    throw new Error(`Wrong pinned failure-bench build for ${ref}.`);
  }
  return build;
}
