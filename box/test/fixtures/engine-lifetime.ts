import assert from "node:assert/strict";
import { once } from "node:events";
import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { K, RUN } from "../../src/link/home";
import { serveBox } from "../../src/link/server/serve";
import { keepEngineOnDemand } from "../../src/link/engine/lifetime";
import { engineRuns } from "../../src/shared/platform";

const root = path.resolve(import.meta.dir, "../../..");
const version = "2.2.6";
const keeper = path.join(K, "bin", "kuvo-start");
const record = path.join(RUN, "engine.json");
const busy = path.join(RUN, "fixture-busy");
for (const folder of ["bin", "run", "workspace", `engine/${version}`]) fs.mkdirSync(path.join(K, folder), { recursive: true });
for (const name of ["kuvo-start", "kuvo-launch", "kuvo-setsid"]) {
  const file = fs.readFileSync(path.join(root, "box/src/keeper", name), "utf8");
  fs.writeFileSync(path.join(K, "bin", name), file.replaceAll("@ENGINE@", version), { mode: 0o755 });
}
fs.copyFileSync(path.join(root, "box/dist/kuvo-link.js"), path.join(K, "bin", "kuvo-link.js"));
const runtime = JSON.stringify(process.execPath);
fs.writeFileSync(path.join(K, "engine", version, "opencode"), `#!/bin/bash
export BUN_BE_BUN=1
exec -a "$0" ${runtime} "$@"
`, { mode: 0o755 });
fs.writeFileSync(path.join(K, "workspace", "serve"), `
import fs from "node:fs";
const [port] = process.argv.slice(process.argv.indexOf("--port") + 1);
let next = 0;
Bun.serve({
port: Number(port), hostname: "127.0.0.1",
fetch(request) {
  const route = new URL(request.url).pathname;
  if (request.method === "POST" && route === "/api/session") {
    return Response.json({ data: { id: "ses_fixture" + (++next), title: "New conversation" } });
  }
  if (route === "/api/session/active") {
    return Response.json({ data: fs.existsSync(${JSON.stringify(busy)}) ? ["ses_fixture1"] : [] });
  }
  return Response.json({ data: [] });
},
});
console.log("server listening");
`);
fs.writeFileSync(path.join(RUN, "launch.env"), `PATH=${process.env.PATH}\n`);
const env = { ...process.env, HOME: path.dirname(K) };
// The keeper's one-shot command launches the engine. The normal keeper loop is
// unnecessary here, so a process we own holds its start lock until cleanup.
const keeperSlot = spawn("/bin/sh", ["-c", "read done", keeper], { env, stdio: ["pipe", "ignore", "ignore"] });
const slotExited = once(keeperSlot, "exit");
fs.writeFileSync(path.join(RUN, "box.pid"), String(keeperSlot.pid));
let now = 0;
const lifetime = keepEngineOnDemand(() => now)!;
const server = serveBox("fixture");
server.listen(0, "127.0.0.1");
await once(server, "listening");
const { port } = server.address() as { port: number };
const headers = { authorization: "Basic " + Buffer.from("opencode:fixture").toString("base64"), "content-type": "application/json", "x-kuvo-from": "app" };
const info = async () => (await fetch(`http://127.0.0.1:${port}/kuvo/info`, { headers })).json();
const create = async () => {
  const result = await fetch(`http://127.0.0.1:${port}/kuvo/sessions`, {
    method: "POST", headers, body: JSON.stringify({ title: "New conversation", folder: path.join(K, "workspace"), model: { providerID: "opencode", id: "big-pickle" } }),
  });
  assert.equal(result.status, 200, await result.clone().text());
  return (await result.json() as { session: string }).session;
};
const pid = () => JSON.parse(fs.readFileSync(record, "utf8")).pid as number;
try {
  assert.equal((await info() as { engineState?: string }).engineState, "idle");
  assert.equal(fs.existsSync(record), false);
  assert.deepEqual((await Promise.all([create(), create()])).sort(), ["ses_fixture1", "ses_fixture2"]);
  assert.equal((await info() as { engineState?: string }).engineState, "running");
  const first = pid();
  now += 600_001;
  fs.writeFileSync(busy, "");
  await lifetime.look();
  assert.equal(pid(), first);
  assert.equal(engineRuns(first, K), true);
  fs.rmSync(busy);
  now += 600_001;
  await lifetime.look();
  assert.equal((await info() as { engineState?: string }).engineState, "idle");
  assert.equal(engineRuns(first, K), false);
  assert.equal(fs.existsSync(path.join(RUN, "stopped")), false);
  assert.equal(await create(), "ses_fixture1");
  assert.notEqual(pid(), first);
} finally {
  server.closeAllConnections();
  server.close();
  await once(spawn("/bin/sh", [keeper, "engine-halt"], { env, stdio: "ignore" }), "exit");
  keeperSlot.stdin!.end();
  await slotExited;
}
