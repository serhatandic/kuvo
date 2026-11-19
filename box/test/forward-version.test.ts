import "./safety"
import { expect, test } from "bun:test";
import { forward } from "../src/link/server/forward";
import { versionedRoutes } from "../src/link/server/format";
import type { LinkHandler } from "../src/link/server/context";

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
      const pieces = await Array.fromAsync(handle({ method, path, headers: {}, body }, {}));
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
