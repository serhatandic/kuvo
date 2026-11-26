// A labelled stand-in model for the pinned OpenCode: an OpenAI-compatible chat endpoint on
// loopback that answers each request with the next scripted step (words, or tool calls), so the
// engine path and the OpenCode agent can be run on the same turns, byte for byte. Live proofs run a
// real model too; this one only makes the deterministic comparisons possible.
import http from "node:http";

export const STAND_IN = { providerID: "standin", id: "scripted" };

/** Settings for OpenCode naming the stand-in (merge into its `opencode.json`). */
export function standInSettings(url) {
  return {
    provider: {
      standin: {
        npm: "@ai-sdk/openai-compatible",
        name: "Stand-in (labelled test model)",
        options: { baseURL: url, apiKey: "stand-in" },
        models: Object.fromEntries(["scripted", "other"].map(id => [id, {
          name: id === "scripted" ? "Scripted stand-in" : "Other stand-in",
          attachment: true, modalities: { input: ["text", "image"], output: ["text"] },
          limit: { context: 100000, output: 8000 },
        }])),
      },
    },
  };
}

const chunk = (delta, finish = null) => ({
  id: "standin", object: "chat.completion.chunk", created: 1, model: "scripted",
  choices: [{ index: 0, delta, finish_reason: finish }],
});

/**
 * Starts the stand-in. `script`: steps, each `{ text }` or `{ tools: [{ name, args }] }` (or a
 * function of the request's messages returning one); `wait: name` holds a step until `open(name)`,
 * and `{ hang: true }` never answers (a request closed while hanging is counted in `stopped`). A
 * request past the script gets "Done.". Each request is kept in `requests`.
 */
export async function standInModel(script = []) {
  const requests = [];
  const gates = new Map();
  const gate = name => {
    if (!gates.has(name)) { let open; const opened = new Promise(resolve => { open = resolve; }); gates.set(name, { opened, open }); }
    return gates.get(name);
  };
  const counts = { stopped: 0 };
  let next = 0;
  const server = http.createServer(async (req, res) => {
    let body = "";
    for await (const part of req) body += part;
    if (req.method !== "POST" || !req.url.endsWith("/chat/completions")) {
      res.writeHead(404).end();
      return;
    }
    const asked = JSON.parse(body || "{}");
    requests.push(asked);
    // OpenCode's own small requests (titles), which offer no tools, aren't the conversation's turns.
    const titling = !asked.tools?.length;
    let step = titling ? { text: "Scripted title" } : script.shift() ?? { text: "Done." };
    if (typeof step === "function") step = step(asked.messages ?? []);
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
    const send = value => res.write(`data: ${JSON.stringify(value)}\n\n`);
    send(chunk({ role: "assistant" }));
    if (step.hang) {
      res.on("close", () => { counts.stopped++; });
      return;
    }
    if (step.wait) await gate(step.wait).opened;
    if (step.tools) {
      step.tools.forEach((tool, index) => send(chunk({ tool_calls: [{
        index, id: `call_${++next}_${index}`, type: "function",
        function: { name: tool.name, arguments: JSON.stringify(tool.args ?? {}) },
      }] })));
      send(chunk({}, "tool_calls"));
    } else {
      for (const word of String(step.text).split(/(?<= )/)) send(chunk({ content: word }));
      send(chunk({}, "stop"));
    }
    send({ ...chunk({}), choices: [], usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 } });
    res.end("data: [DONE]\n\n");
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${server.address().port}/v1`;
  return {
    url, requests, open: name => gate(name).open(),
    get stopped() { return counts.stopped; },
    close: () => { server.closeAllConnections?.(); return new Promise(resolve => server.close(resolve)); },
  };
}
