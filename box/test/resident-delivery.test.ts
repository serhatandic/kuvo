import "./safety";
import { expect, test } from "bun:test";
import { once } from "node:events";
import { processRuns } from "../src/shared/platform";
import { keepList, listPlaceHeard } from "../src/link/lists/sessions";
import { createBoxDispatcher } from "../src/link/server/router";
import { serveBox } from "../src/link/server/serve";

test("the resident HTTP box delivers from kept routes to the engine and each coding agent", async () => {
  const engines = ["opencode", "claude", "codex", "antigravity", "grok", "cursor"];
  const session = (agent: string) => ({ id: `ses_DeliveryRoute_${agent}`, title: agent, metadata: { kuvo: { agent } } });
  let perSessionReads = 0;
  const received: string[] = [];
  const server = (agents: string[]) => Bun.serve({
    hostname: "127.0.0.1", port: 0, async fetch(request) {
      const route = new URL(request.url).pathname;
      if (route === "/api/session") return Response.json({ data: agents.map(session) });
      if (route === "/api/session/active") return Response.json({ data: [] });
      const match = /^\/api\/session\/([^/]+)(?:\/(prompt))?$/.exec(route);
      const agent = agents.find(each => session(each).id === match?.[1]);
      if (!agent) return new Response(null, { status: 404 });
      if (!match?.[2]) {
        perSessionReads++;
        return Response.json({ data: session(agent) });
      }
      const message = await request.json() as { id: string; text: string };
      received.push(`${agent}:${message.id}:${message.text}`);
      return Response.json({ data: {} });
    },
  });
  const engine = server(engines.slice(0, 1)), bridge = server(engines.slice(1));
  // The dispatcher verifies the bridge's process name in production too.
  const child = Bun.spawn([process.execPath, "-e", "setTimeout(() => {}, 30000)", "kuvo-agents"], {
    stdout: "ignore", stderr: "ignore",
  });
  const stop = keepList();
  const engineRecord = { port: engine.port!, password: "engine-fixture" };
  const bridgeRecord = { port: bridge.port!, password: "bridge-fixture", pid: child.pid };
  const dispatcher = createBoxDispatcher({ readEngine: () => engineRecord, readBridge: () => bridgeRecord });
  const box = serveBox("test", dispatcher.entry(), () => engineRecord);
  try {
    await Promise.all([listPlaceHeard("engine", engineRecord), listPlaceHeard("bridge", bridgeRecord)]);
    expect(processRuns(child.pid, "kuvo-agents")).toBe(true);
    box.listen(0, "127.0.0.1");
    await once(box, "listening");
    const address = box.address();
    if (!address || typeof address === "string") throw new Error("Missing fixture address");
    for (const agent of engines) {
      const response = await fetch(`http://127.0.0.1:${address.port}/kuvo/sessions/${session(agent).id}/send`, {
        method: "POST", headers: { authorization: "Basic " + Buffer.from("opencode:test").toString("base64"),
          "content-type": "application/json", "x-kuvo-from": "app" },
        body: JSON.stringify({ id: `msg_fixture${agent}`, words: "Send from the resident box" }),
      });
      expect(response.status).toBe(200);
      await response.arrayBuffer();
    }
    expect(received).toEqual(engines.map(agent => `${agent}:msg_fixture${agent}:Send from the resident box`));
    expect(perSessionReads).toBe(0);
  } finally {
    box.closeAllConnections();
    box.close();
    stop();
    engine.stop(true);
    bridge.stop(true);
    child.kill();
    await child.exited;
  }
});
