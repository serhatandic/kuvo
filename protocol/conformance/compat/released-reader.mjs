// A released Mac's link reading what a machine mailed it, with that release's own code: each
// frame (hex) that came for the Mac is opened with the Mac's key and the machine's, and its agents
// are taken as the release takes them. Prints what it kept, by the machine's key.
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const [script, macKey, machineKey, framesFile, folder] = process.argv.slice(2);
const code = await import(pathToFileURL(script).href);
const kept = path.join(folder, "peer-agents.json"), lock = path.join(folder, "peer.lock");
const sender = { key: machineKey, kind: "machine" };
const payloads = [];
for (const hex of JSON.parse(fs.readFileSync(framesFile, "utf8"))) {
  const packet = code.readFrame(Buffer.from(hex, "hex"));
  if (packet?.kind !== code.Frame.mail) continue;
  const opened = code.openOneWay(packet.body, Buffer.from(macKey, "hex"), Buffer.from(machineKey, "base64url"));
  const payload = JSON.parse(opened.payload.toString("utf8"));
  payloads.push(Object.keys(payload).sort());
  if (payload.agents) code.takeAgents(sender, payload.agents, kept, lock);
}
console.log(JSON.stringify({ payloads, agents: code.readPeerAgents(kept, path.join(folder, "none.json"), { macs: [] }) }));
