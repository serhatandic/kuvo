import { createInterface } from "node:readline";
import { mcpResult } from "./fixture.mjs";

for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.id !== undefined) {
    console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: mcpResult(request) }));
  }
}
