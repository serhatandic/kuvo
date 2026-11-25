// What agents ask you for while they browse (PROTOCOL.md, "Per area", `asks`): each listed in the
// conversation it's for, by that one's name, and an answer that fits it given to the browser over
// its socket, the box's socket key first. (The fixtures in `agents.json` say the rest.)
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import test from "node:test";
import { launch, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

// (The browser listens on a Unix socket, and doesn't run on native Windows yet.)
const skip = process.platform === "win32" ? "the browser doesn't run on native Windows" : false;

test("an ask is listed in the conversation it's for, by its name, and an answer that fits it reaches the browser once", { timeout: 60000, skip }, async t => {
  const { computer } = await bench(t);
  const lab = await computer("Lab");
  const { home, engine, env } = lab;
  const run = path.join(home, "run");
  // A subagent asks: it's filed in the conversation it works for.
  engine.sessions.push(sessionOf({ id: "ses_lead", title: "Fixer", directory: path.join(home, "..", "work") }));
  engine.sessions.push({ ...sessionOf({ id: "ses_sub", title: "A helper", directory: path.join(home, "..", "work") }), parentID: "ses_lead" });
  const ask = lab.connect(await lab.start().ready());
  // The browser's socket: what it's given, in order.
  const said = [];
  const browser = net.createServer(socket => {
    let buffer = "";
    socket.on("data", chunk => {
      buffer += chunk;
      let end;
      while ((end = buffer.indexOf("\n")) >= 0) {
        const message = JSON.parse(buffer.slice(0, end));
        buffer = buffer.slice(end + 1);
        said.push(message);
        if (message.method === "Kuvo.answer") socket.write(JSON.stringify({ id: message.id, result: {} }) + "\n");
      }
    });
  });
  await new Promise(done => browser.listen(path.join(run, "browser.sock"), done));
  t.after(() => browser.close());
  const stand = launch(process.execPath, ["-e", "process.stdin.resume()", "kuvo-browser"], env, home);
  t.after(() => stand.stop());
  write(path.join(run, "browser-asks.json"), JSON.stringify({ asks: [
    { id: "ask-1", kind: "signIn", host: "github.com", session: "ses_sub", username: true, password: true },
    { id: "ask-2", kind: "code", host: "github.com", prompt: "The code GitHub sent", passkey: true },
  ] }));
  write(path.join(run, "browser.pid"), String(stand.child.pid));

  const listed = await until(async () => {
    const answer = await ask("GET", "kuvo/asks");
    return answer.json?.asks?.length === 2 && answer;
  }, "The asks were never listed.");
  assert.deepEqual({ ...listed.json.asks[0], machine: undefined }, { id: "ask-1", kind: "signIn", host: "github.com", machine: undefined, session: "ses_lead", asker: "Fixer", username: true, password: true, saved: [], offersToSave: false });
  assert.deepEqual([listed.json.asks[1].prompt, listed.json.asks[1].passkey, "session" in listed.json.asks[1]], ["The code GitHub sent", true, false]);

  assert.equal((await ask("POST", "kuvo/asks/ask-1", { answer: "signIn", saved: "me" })).status, 400, "saved passwords are the Mac's");
  assert.equal((await ask("POST", "kuvo/asks/ask-1", { answer: "code", code: "1" })).status, 400, "not what it asks");
  assert.equal((await ask("POST", "kuvo/asks/ask-9", { answer: "declined" })).status, 404);
  assert.deepEqual(said, [], "nothing that didn't fit reached the browser");
  assert.equal((await ask("POST", "kuvo/asks/ask-1", { answer: "signIn", username: "me", password: "secret" })).status, 200);
  assert.equal((await ask("POST", "kuvo/asks/ask-2", { answer: "code", code: " 123456 " })).status, 200);
  const key = fs.readFileSync(path.join(run, "socket.key"), "utf8").trim();
  assert.deepEqual(said, [
    { method: "Kuvo.key", params: { key } },
    { id: 1, method: "Kuvo.answer", params: { id: "ask-1", answer: { type: "signIn", username: "me", password: "secret" } } },
    { method: "Kuvo.key", params: { key } },
    { id: 1, method: "Kuvo.answer", params: { id: "ask-2", answer: { type: "code", code: "123456" } } },
  ]);

  // The browser gone: nothing is asked of anyone now.
  await stand.stop();
  assert.deepEqual((await until(async () => {
    const answer = await ask("GET", "kuvo/asks");
    return answer.json?.asks?.length === 0 && answer;
  }, "What the browser left was still listed.")).json, { asks: [] });
});
