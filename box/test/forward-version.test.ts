import "./safety"
import { expect, test } from "bun:test";
import { forward } from "../src/link/server/forward";
import { versionedRoutes } from "../src/link/server/format";
import { collected } from "../src/link/server/sent-once";
import type { LinkHandler, RouteContext } from "../src/link/server/context";

test("the Box negotiates the bridge version before decoding or forwarding a reply", async () => {
  let version: string | undefined = "2";
  const asked: (string | null)[] = [];
  const bridge = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch(request) {
      asked.push(request.headers.get("x-kuvo-route-version"));
      const event = new URL(request.url).pathname === "/api/event";
      return new Response(event ? 'data: {"type":"bridge"}\n\n' : version === "2" ? "future message encoding" : '{"data":[]}', {
        headers: { "content-type": event ? "text/event-stream" : "application/json", ...(version === undefined ? {} : { "X-Kuvo-Route-Version": version }) },
      });
    },
  });
  try {
    const handle = versionedRoutes(((request, _phone, signal) => forward(
      { port: bridge.port!, password: "pw" }, request.path, request, signal, () => true, null, undefined, true,
    )) satisfies LinkHandler);
    async function answer(method: string, path: string, body?: Buffer) {
      const pieces = await Array.fromAsync(handle({ method, path, headers: {}, body }, undefined));
      return {
        head: pieces[0],
        body: Buffer.concat(pieces.flatMap(piece => "body" in piece ? [piece.body] : [])).toString(),
      };
    }
    for (const [method, path, body] of [
      ["GET", "api/event"],
      ["GET", "api/session/ses_example/message"],
      ["POST", "api/session/ses_example/prompt", Buffer.from('{"id":"msg_example","text":"Hello"}')],
    ] as const) {
      const refused = await answer(method, path, body);
      expect(refused.head).toMatchObject({ head: { status: 502 } });
      expect(JSON.parse(refused.body).code).toBe("unsupportedFormatVersion");
    }
    for (const supported of [undefined, "1"]) {
      version = supported;
      const response = await answer("GET", "api/event");
      expect(response.head).toMatchObject({ head: { status: 200, headers: { "x-kuvo-route-version": "1" } } });
      expect(response.body).toBe('data: {"type":"bridge"}\n\n');
    }
    expect(asked).toEqual(["1", "1", "1", "1", "1"]);
  } finally {
    bridge.stop(true);
  }
});

test("the Box checks the replacement bridge's route version after a handover refusal", async () => {
  const old = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch: () => new Response(null, { status: 503, headers: { "x-kuvo-bridge-handover": "1", "x-kuvo-route-version": "1" } }),
  });
  const next = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch: () => new Response("future response", { headers: { "x-kuvo-route-version": "2" } }),
  });
  try {
    const path = "api/session/ses_handover/prompt";
    const request = { method: "POST", path, headers: {}, body: Buffer.from('{"id":"msg_handover","text":"Hello"}') };
    const [head, body] = await Array.fromAsync(forward(
      { port: old.port!, password: "old" }, path, request, undefined, () => true, null, undefined, true,
      () => ({ port: next.port!, password: "next" }),
    ));
    expect(head).toMatchObject({ head: { status: 502 } });
    expect(body && "body" in body && JSON.parse(body.body.toString()).code).toBe("unsupportedFormatVersion");
  } finally {
    old.stop(true);
    next.stop(true);
  }
});

test("a permission reply cannot attach to an engine that returned after the request began", async () => {
  let dispatched = 0;
  const engine = Bun.serve({
    port: 0,
    fetch() { dispatched++; return Response.json({}); },
  });
  try {
    const path = "api/session/ses_returning/permission/per_old/reply";
    const reply = await collected(forward(null, path, {
      method: "POST", path, headers: {}, fromApp: true, body: Buffer.from('{"decision":"once"}'),
    }, undefined, () => true, null, undefined, false, undefined, () => ({ port: engine.port!, password: "test" })));
    expect(reply.status).toBe(503);
    expect(dispatched).toBe(0);
  } finally {
    engine.stop(true);
  }
});
