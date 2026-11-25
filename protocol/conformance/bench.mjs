// Process scenarios share the real binary adapter, an in-memory relay and an engine per computer.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { relayLeaseStandIn } from "../../box/bridge-tests/relay-lease-stand-in.js";
import { boxBinary, boxCommand, isolatedEnv, localRelay, privateHome, readJSON, request, scratch, startBox, until, write } from "./adapter.mjs";
import { standInEngine } from "./engine.mjs";

export async function bench(t, { orderedMessages = false, presenceEvents = false } = {}) {
  const binary = boxBinary();
  const root = fs.realpathSync(scratch());
  const children = [];
  const engines = [];
  const leases = relayLeaseStandIn();
  const relay = await localRelay({ request: leases.request, presenceEvents });
  t.after(async () => {
    for (const child of children.reverse()) await child.stop();
    await Promise.all([relay.close(), ...engines.map(engine => engine.close())]);
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  });
  return {
    relay, leases,
    async computer(name, implementation = binary, configure = () => ({})) {
      const folder = path.join(root, name);
      const home = path.join(folder, "box");
      const env = { ...isolatedEnv(folder), KUVO_SANDBOX: "off", ...configure(folder) };
      const engine = await standInEngine(folder, env);
      engine.orderedMessages = orderedMessages;
      engines.push(engine);
      // This fixture seeds records in the current layout before either resident starts.
      privateHome(home);
      write(path.join(home, "folder-version.json"), '{"version":23}\n');
      write(path.join(home, "run", "engine.json"), JSON.stringify(engine.record));
      write(path.join(home, "run", "link.json"), JSON.stringify({ phones: [], name }));
      const start = (runWith = implementation, overrides = {}) => {
        const child = startBox({ binary: runWith, home, env: { ...env, ...overrides }, relay });
        children.push(child);
        return child;
      };
      const join = async (code, { afterStart, resident, joinWith = implementation } = {}) => {
        // The TS installer starts its old link if nothing is running. Own that
        // process explicitly, then stop it before starting the resident.
        let legacy;
        if (joinWith.typescript && !resident) {
          // A link waiting for its first pairing stays alive through enrollment.
          // The incoming setup replaces this pending token.
          const initialize = boxCommand(joinWith, home, env, relay, ["setup-local"]);
          children.push(initialize);
          assert.equal((await initialize.exited).code, 0, initialize.log());
          const link = path.join(home, "run", "link.json");
          write(link, JSON.stringify({ ...readJSON(link), pairing: { token: "fixture", expires: Date.now() + 120000 } }));
          legacy = boxCommand(joinWith, home, env, relay, ["run"]);
          children.push(legacy);
        }
        try {
          if (legacy) {
            const pid = path.join(home, "run", "link.pid");
            await until(() => fs.existsSync(pid) && Number(fs.readFileSync(pid, "utf8")) === legacy.child.pid,
              () => `Installer's link did not start\n${legacy.log()}`);
          }
          const args = joinWith.typescript ? ["join"] : ["install", "--join"];
          const child = boxCommand(joinWith, home, env, relay, args);
          children.push(child);
          child.child.stdin.end(code + "\n");
          await afterStart?.(child);
          return { ...await child.exited, log: child.log() };
        } finally {
          await legacy?.stop();
        }
      };
      const connect = record => (method, route, body, headers) => request(record, method, route, body, headers);
      const command = async (args, input) => {
        const child = boxCommand(implementation, home, env, relay, args, {}, "pipe");
        children.push(child);
        let text = "";
        child.child.stdout.setEncoding("utf8").on("data", chunk => { text += chunk; });
        child.child.stdin.end(input);
        return { ...await child.exited, text, log: child.log() };
      };
      return { home, env, engine, start, join, connect, command };
    },
  };
}
