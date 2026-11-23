import { cpSync, mkdirSync, mkdtempSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { boxBinary, isolatedEnv, readJSON, scratch, typescriptRuntime } from "../adapter.mjs";
import { saveAppSnapshot } from "./app-snapshot.mjs";
import { runSync, save, tags, withTag } from "./tagged.mjs";

const worker = fileURLToPath(new URL("./generate-tag.mjs", import.meta.url));
const currentWorld = fileURLToPath(new URL("./current-world.ts", import.meta.url));
const appWorld = fileURLToPath(new URL("./app-world.ts", import.meta.url));

export async function generateRelease(tag, destination, { app = process.platform === "darwin" } = {}) {
  mkdirSync(destination, { mode: 0o700 });
  await withTag(tag, world => {
    runSync(process.execPath, ["--disable-warning=MODULE_TYPELESS_PACKAGE_JSON", worker, tag, world.commit, world.script, destination],
      isolatedEnv(world.root), world.root);
    if (!app) return;
    const env = { ...isolatedEnv(world.root), BUN_BE_BUN: "1" };
    const mac = JSON.parse(runSync(typescriptRuntime(), ["--no-env-file", appWorld, destination], env, world.root));
    const vectors = path.join(destination, "vectors.json");
    const { setup } = readJSON(vectors);
    saveAppSnapshot(world, tag, JSON.parse(setup.output), JSON.parse(setup.input), path.join(mac.S, "phone-link.json"), vectors);
    const appLink = path.join(mac.S, "box", "bin", "kuvo-link.js");
    mkdirSync(path.dirname(appLink), { recursive: true });
    cpSync(world.script, appLink);
    save(path.join(destination, "app-world.json"), mac);
  });
}

export function generateCurrent(destination, binary = boxBinary(process.env.KUVO_CONFORMANCE_TS_BIN ?? "box/dist/kuvo-link.js")) {
  if (!binary.typescript) throw new Error("The golden home generator needs the TypeScript reference bundle.");
  const home = path.join(destination, "machine");
  const directory = path.join(destination, "project");
  mkdirSync(home, { recursive: true, mode: 0o700 });
  const env = { ...isolatedEnv(destination), KUVO_HOME: home, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0" };
  const bun = typescriptRuntime();
  runSync(bun, ["--no-env-file", binary.file, "folder-upgrade"], env, home);
  runSync(bun, ["--no-env-file", currentWorld, home, directory], env, home);
  return { home, directory, env };
}

export function generateHoldTranscript(cache) {
  const binary = boxBinary(process.env.KUVO_CONFORMANCE_TS_BIN ?? "box/dist/kuvo-link.js");
  const env = {
    ...isolatedEnv(cache), KUVO_BOX_BIN: binary.file, KUVO_CONFORMANCE_BUN: typescriptRuntime(),
    KUVO_CONFORMANCE_HOLDER_TRANSCRIPT: path.join(cache, "holder.json"),
  };
  const test = fileURLToPath(new URL("../holder.test.mjs", import.meta.url));
  runSync(process.execPath, ["--test", test], env, cache);
}

// Each invocation creates a new cache entry. It never writes over an existing fixture.
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const cache = process.argv[2] ? mkdtempSync(path.join(path.resolve(process.argv[2]), "kuvo-golden-")) : scratch();
  for (const tag of tags) await generateRelease(tag, path.join(cache, tag));
  generateCurrent(path.join(cache, "current"));
  generateHoldTranscript(cache);
  console.log(cache);
}
