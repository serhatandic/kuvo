import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ref = fs.readFileSync(new URL("failure-bench-legacy-ref.txt", import.meta.url), "utf8").trim();
const resource = "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js";

// SHA-256 of the resource at the pinned commit. Lokum has no checkout history to verify against.
const digest = "aeb60b84f4f29590535c67ed0a3d6c5a33f448d8b8c8e566493cf6d0e598f5f2";

export function legacyBuild(checkout) {
  return buildFixture(ref, digest, ".failure-bench-legacy.mjs", checkout);
}

// The neighboring main build before versioned external backup storage.
export function recoveryNeighborBuild(checkout) {
  const neighbor = fs.readFileSync(new URL("recovery-neighbor-ref.txt", import.meta.url), "utf8").trim();
  return buildFixture(neighbor,
    "67af7fbc6a0d12105966aff1f88a338e5e928400735102aa1d829720396d60c0", ".recovery-neighbor.mjs", checkout);
}

// The last layout-4 bridge, retained for real update and keeper recovery entry points.
export function layout4Build(checkout) {
  return buildFixture("4330569c2f1c39eef166f65d2abb1b7256ef7dcb",
    "6f24ff74ca472a74045b4ebb1038ffb4ff199b331006c607b915f383f406d0b4", ".layout-4.mjs", checkout);
}

// The deployed bridge before folder markers, with its own updater and health judge.
export function layout0Build(checkout) {
  return buildFixture("11323b39316e0ec1f9b6281ab9bba9153db8e822",
    "1ee6ffd6cbfb32ad10a8fa95e40733a49dd08a494579b171c6692e1245654e18", ".layout-0.mjs", checkout);
}

// The last split launcher with the same lease, save and move protocols as resident serve.
const split = fs.readFileSync(new URL("split-launch-ref.txt", import.meta.url), "utf8").trim();
export function splitLaunchBuild() {
  return buildFixture(split, "4ab8b3a572eec08b06d18931ba8b5b44a9d2c364b1d447298bd3952517dec30f", ".split-launch.mjs");
}
export function splitLaunchKeeper() {
  return buildFixture(split, "b25c8f4465b4240a8e7cbb32ec6f8c630d60511ea0cb0129ddfaaa1563354257", ".split-keeper.sh", undefined, "box/src/keeper/kuvo-start");
}

function buildFixture(ref, digest, filename, checkout = fileURLToPath(new URL("../..", import.meta.url)), file = resource) {
  if (!/^[0-9a-f]{40}$/.test(ref)) throw new Error("Invalid pinned failure-bench revision.");
  const archived = path.join(checkout, "box", "bridge-tests", filename);
  let build;
  if (fs.existsSync(archived)) {
    build = fs.readFileSync(archived);
  } else {
    const result = spawnSync("git", ["--no-optional-locks", "-C", checkout, "show", `${ref}:${file}`], { maxBuffer: 64 * 1024 * 1024 });
    if (result.status !== 0) {
      throw new Error(`Missing pinned failure-bench build at ${ref}. Check that the pinned commit or the transferred fixture is present.`);
    }
    build = result.stdout;
  }
  if (createHash("sha256").update(build).digest("hex") !== digest) {
    throw new Error(`Wrong pinned failure-bench build for ${ref}.`);
  }
  return build;
}
