// Keep the reference bridge and both resident binaries on the same private, local CLI fixture.
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { standInClaude } from "../../box/bridge-tests/claude-stand-in.js";
import { launch, typescriptRuntime, until } from "./adapter.mjs";

export function claudeEnvironment(binary, home, env, fixture) {
  const bin = standInClaude({
    root: env.HOME,
    agents: binary.typescript ? path.join(home, "agents") : undefined,
    ...fixture,
  });
  const system = process.platform === "win32" ? path.join(env.SystemRoot, "System32") : "/usr/bin:/bin";
  return { ...env, KUVO_TEST_PATH_ONLY: "1", PATH: [bin, system].join(path.delimiter) };
}

export async function startClaudeBridge(binary, home, env) {
  if (!binary.typescript) return undefined;
  fs.rmSync(path.join(home, "run", "bridge.json"), { force: true });
  const bundle = fileURLToPath(new URL("../../box/dist/kuvo-agents.js", import.meta.url));
  const bridge = launch(typescriptRuntime(), ["--config=/dev/null", "--no-env-file", bundle], {
    ...env, KUVO_HOME: home, KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_PORT: String(await freePort()),
    KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PASSWORD: "fixture",
    KUVO_SANDBOX: "off", BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
  }, home);
  try {
    await until(() => fs.existsSync(path.join(home, "run", "bridge.json")),
      () => "The TypeScript bridge never published its fixture listener. " + bridge.log());
    return bridge;
  } catch (error) {
    await bridge.stop();
    throw error;
  }
}

async function freePort() {
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}
