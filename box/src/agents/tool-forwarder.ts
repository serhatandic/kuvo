// Runs in the agent's sandbox. Only the relay's address and this session's key are given to it.
export const toolForwarder = String.raw`
const { randomUUID } = require("node:crypto");
const { createInterface } = require("node:readline");
const { setTimeout: delay } = require("node:timers/promises");
const { KUVO_TOOL_RELAY: url, KUVO_TOOL_RELAY_KEY: key, KUVO_TOOL_TIMEOUT: timeout } = process.env;
const calls = new Map();
let session;
const write = message => process.stdout.write(JSON.stringify(message) + "\n");
const send = (body, signal) => fetch(url, {
  method: "POST",
  headers: { "Content-Type": "application/json", Authorization: key, "X-Kuvo-Tool-Forwarder": "1", ...(session ? { "Mcp-Session-Id": session } : {}) },
  body, signal, redirect: "error",
});
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on("line", async line => {
  let message;
  try { message = JSON.parse(line); }
  catch { return write({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } }); }
  if (message.id == null) {
    if (message.method === "notifications/initialized") {
      const response = await send(line, AbortSignal.timeout(5000)).catch(() => null);
      await response?.body?.cancel();
    }
    if (message.method === "notifications/cancelled") {
      const call = calls.get(message.params?.requestId);
      if (call) {
        call.abort();
        await send(line, AbortSignal.timeout(5000)).catch(() => {});
      }
    }
    return;
  }
  const controller = new AbortController();
  calls.set(message.id, controller);
  const name = message.params?.name;
  const replay = message.method !== "tools/call" ||
    typeof name === "string" && !name.startsWith("browser_");
  if (message.method === "tools/call") {
    message.params = { ...message.params, _meta: { ...message.params?._meta, "kuvo.operation": randomUUID() } };
  }
  const body = JSON.stringify(message);
  let retryBy;
  try {
    const signal = AbortSignal.any([controller.signal, AbortSignal.timeout(Number(timeout) + 80_000)]);
    while (!signal.aborted) {
      try {
        const response = await send(body, signal);
        if (![502, 503].includes(response.status)) {
          if (!response.ok) {
            await response.body?.cancel();
            throw new Error("Kuvo Code's tools said HTTP " + response.status + ".");
          }
          const answer = await response.json();
          if (message.method === "initialize") session = response.headers.get("mcp-session-id");
          if (!answer.error?.data?.kuvoRelayRestart || !replay) return write(answer);
        } else {
          await response.body?.cancel();
          if (response.status === 502 && !replay) break;
        }
      } catch (error) {
        const code = error?.cause?.code ?? error?.code;
        if (!code && error.message.startsWith("Kuvo Code's tools said HTTP")) throw error;
        if (!replay && code !== "ECONNREFUSED" && code !== "ConnectionRefused") break;
      }
      retryBy ??= Date.now() + 80_000;
      if (Date.now() >= retryBy) break;
      await delay(200, undefined, { signal });
    }
    throw new Error("Kuvo Code's tools didn't answer.");
  } catch (error) {
    if (!controller.signal.aborted) write({ jsonrpc: "2.0", id: message.id, error: { code: -32603, message: error.message } });
  } finally {
    calls.delete(message.id);
  }
});
input.on("close", () => process.exit(0));
`;
