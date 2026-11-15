import "./safety"
import { expect, test } from "bun:test";
import { listComputers } from "../src/link/areas/computers";

test("computer answers carry the box's plan names, including an unknown service", async () => {
  const server = Bun.serve({ port: 0, fetch: () => Response.json({ rows: ["claude", "codex", "grok", "copilot", "openrouter", "zai", "ollama", "opencode", "future-plan", "constructor", "toString"].map(service => ({ service, boxes: [{ here: true }] })) }) });
  try {
    const computers = await listComputers("test", { port: server.port!, password: "test" });
    expect(computers[0]!.limitServices).toEqual([
      { id: "claude", title: "Claude" }, { id: "codex", title: "ChatGPT" },
      { id: "grok", title: "SuperGrok" }, { id: "copilot", title: "Copilot" },
      { id: "openrouter", title: "OpenRouter" }, { id: "zai", title: "Z.ai" },
      { id: "ollama", title: "Ollama" }, { id: "opencode", title: "OpenCode Go" },
      { id: "future-plan", title: "Future-plan" },
      { id: "constructor", title: "Constructor" }, { id: "toString", title: "ToString" },
    ]);
  } finally { server.stop(true); }
});
