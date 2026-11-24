import { existsSync, rmSync } from "node:fs";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  boxBinary, isolatedEnv, launch, localRelay, privateHome, scratch, startBox,
  typescriptRuntime, until, write,
} from "./adapter.mjs";

// Use the TypeScript bridge or the built-in Rust fixture with the same home.
export async function withAgentFixture(t, seed, run) {
  const binary = boxBinary();
  if (binary.typescript && process.platform === "win32") {
    t.skip("The reference bridge runs on Unix.");
    return;
  }
  if (!binary.typescript && process.env.KUVO_CONFORMANCE_AGENT_FIXTURE !== "1") {
    t.skip("Use a Rust binary built with agent-runtime-fixture and KUVO_CONFORMANCE_AGENT_FIXTURE=1.");
    return;
  }
  const root = scratch();
  const home = path.join(root, "box");
  const relay = await localRelay();
  let resident, bridge;
  try {
    privateHome(home);
    const env = isolatedEnv(root);
    if (process.platform !== "win32") env.PATH = "/usr/bin:/bin";
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    await seed(home);
    if (binary.typescript) {
      const bundle = fileURLToPath(new URL("../../box/dist/kuvo-agents.js", import.meta.url));
      const port = await freePort();
      bridge = launch(typescriptRuntime(), ["--no-env-file", bundle], {
        ...env, KUVO_HOME: home, KUVO_AGENTS_REMOTE: "1", KUVO_AGENTS_PORT: String(port),
        KUVO_AGENTS_HOME: path.join(home, "agents"), KUVO_AGENTS_PASSWORD: "fixture",
        KUVO_SANDBOX: "off", BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
      }, home);
      await until(() => existsSync(path.join(home, "run", "bridge.json")),
        () => "The TypeScript bridge never published its fixture listener. " + bridge.log());
    } else {
      write(path.join(home, "run", "agent-fixture-program"), process.execPath);
    }
    resident = startBox({ binary, home, env, relay });
    await run(await resident.ready(), home);
  } finally {
    await resident?.stop();
    await bridge?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
}

async function freePort() {
  const server = net.createServer();
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const port = server.address().port;
  await new Promise(resolve => server.close(resolve));
  return port;
}
