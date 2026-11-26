// Process scenarios share the real binary adapter, an in-memory relay and an engine per computer.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { relayLeaseStandIn } from "../../box/bridge-tests/relay-lease-stand-in.js";
import { boxBinary, boxCommand, isolatedEnv, localRelay, privateHome, readJSON, request, scratch, startBox, until, write } from "./adapter.mjs";
import { agentsOn } from "./acp.mjs";
import { settingsEngine, startClaudeBridge, stopFixtureHolders } from "./claude.mjs";
import { standInEngine } from "./engine.mjs";

/**
 * With `agents`, each computer's conversations run on the scripted Grok (`acp.mjs`): `computer.agents`
 * seeds and scripts them, and the reference's bridge runs beside its resident.
 */
export async function bench(t, { handover = false, orderedMessages = false, presenceEvents = false, agents: withAgents = false } = {}) {
  const binary = boxBinary();
  const root = fs.realpathSync(scratch());
  const children = [];
  const engines = [];
  const homes = [];
  const leases = relayLeaseStandIn({ transferStartsNow: handover });
  const relay = await localRelay({ request: leases.request, presenceEvents });
  t.after(async () => {
    for (const child of children.reverse()) await child.stop();
    for (const home of homes) await stopFixtureHolders(home);
    await Promise.all([relay.close(), ...engines.map(engine => engine.close())]);
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  });
  return {
    relay, leases,
    async computer(name, implementation = binary, configure = () => ({}), { canonical = false } = {}) {
      const folder = path.join(root, name);
      const env = { ...isolatedEnv(folder), KUVO_SANDBOX: "off", ...configure(folder) };
      const home = canonical ? path.join(env.HOME, ".kuvo-code") : path.join(folder, "box");
      const agents = withAgents ? agentsOn(home, env) : undefined;
      if (agents) {
        Object.assign(env, agents.env);
        homes.push(home);
      }
      const engine = await standInEngine(folder, env);
      engine.orderedMessages = orderedMessages;
      engines.push(engine);
      // This fixture seeds records in the current layout before either resident starts.
      privateHome(home);
      write(path.join(home, "folder-version.json"), '{"version":23}\n');
      write(path.join(home, "run", "engine.json"), JSON.stringify(engine.record));
      write(path.join(home, "run", "link.json"), JSON.stringify({ phones: [], name }));
      // (The reference's bridge keeps its agents across resident restarts, as it does at home.)
      let bridge;
      const keepBridge = async runWith => {
        if (!agents || !runWith.typescript || bridge) return;
        const settings = await settingsEngine(home);
        engines.push(settings);
        bridge = await startClaudeBridge(runWith, home, env, { engine: settings });
        children.unshift(bridge);
      };
      const launchBox = (runWith, overrides) => {
        const child = startBox({ binary: runWith, home, env: { ...env, ...overrides }, relay });
        children.push(child);
        return child;
      };
      const start = (runWith = implementation, overrides = {}) => {
        if (!agents || !runWith.typescript) return launchBox(runWith, overrides);
        // (A handle that waits for the bridge before the resident starts.)
        let child;
        const started = keepBridge(runWith).then(() => (child = launchBox(runWith, overrides)));
        return {
          ready: () => started.then(each => each.ready()),
          get child() { return child.child; },
          get exited() { return child.exited; },
          stop: () => started.then(each => each.stop()),
          log: () => child?.log() ?? "",
        };
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
      const connect = record => (method, route, body, headers, timeout) => request(record, method, route, body, headers, timeout);
      const command = async (args, input) => {
        const child = boxCommand(implementation, home, env, relay, args, {}, "pipe");
        children.push(child);
        let text = "";
        child.child.stdout.setEncoding("utf8").on("data", chunk => { text += chunk; });
        child.child.stdin.end(input);
        return { ...await child.exited, text, log: child.log() };
      };
      return { home, env, engine, agents, start, join, connect, command };
    },
  };
}
