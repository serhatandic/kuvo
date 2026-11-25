// Edits held for another computer (PROTOCOL.md, `kuvo/bots`, "Held for another computer"): a bot
// that lives on a computer that's away is changed or deleted on another, which holds the edit (202),
// lists it (`held`, the bot `waiting` with the name you gave it), and hands it on once that computer
// is back; what that computer refuses is let go and said (`refused`). The box under test
// (KUVO_BOX_BIN) is Lab; Studio runs KUVO_LEAD_BIN, the TypeScript box by default: each holds for
// the other, and each keeps what the other held.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { boxBinary, readJSON, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const otherFile = process.env.KUVO_LEAD_BIN || new URL("../../box/dist/kuvo-link.js", import.meta.url).pathname;
const other = fs.existsSync(otherFile) ? boxBinary(otherFile) : null;
const skip = !other ? "no other box (KUVO_LEAD_BIN)" : other.typescript && process.platform === "win32" ? "the TypeScript box doesn't run on native Windows" : false;
const words = new URL("../../box/src/shared/bot-words.json", import.meta.url);

const session = (id) => `ses_${id.replace(/_/g, "")}`;

function bot({ home, engine }, id, name) {
  const folder = path.join(home, "bots", id);
  const at = "2026-10-01T09:00:00Z";
  write(path.join(home, "bin", "bot-words.json"), fs.readFileSync(words));
  write(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id, name, look: { color: "sky", shape: "gem", face: "wink" }, created: at, updated: at }));
  write(path.join(folder, "session"), session(id));
  write(path.join(folder, "MEMORY.md"), `# ${name}'s memory\n`);
  engine.sessions.push(sessionOf({ id: session(id), title: name, directory: folder }));
}

const listed = async (ask) => {
  const answer = await ask("GET", "kuvo/bots");
  assert.equal(answer.status, 200, answer.text);
  return answer.json;
};
// (As the app asks: what only reaches the box's port can't hold an edit as you.)
const asApp = (connect) => (method, route, body) => connect(method, route, body, { "x-kuvo-from": "app" });
const nameOf = (home, id) => readJSON(path.join(home, "bots", id, "bot.json")).name;

test("an edit to a bot whose computer is away is held, listed, and taken there once it's back; what it refuses is said", { timeout: 300000, skip }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio", other);
  const lab = await computer("Lab");
  bot(studio, "bot_far1", "Far");
  let studioBox = studio.start();
  const studioAsk = studio.connect(await studioBox.ready());
  let main = asApp(studioAsk);
  const setup = await main("GET", "kuvo/setup");
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const invite = await studioAsk("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const joined = await lab.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  bot(lab, "bot_near1", "Near");
  let labBox = lab.start();
  let ask = asApp(lab.connect(await labBox.ready()));
  await until(async () => (await listed(ask)).elsewhere?.some(each => each.id === "bot_far1"), "Lab never listed Studio's bot.", 90000, 500);
  await until(async () => (await listed(main)).elsewhere?.some(each => each.id === "bot_near1"), "Studio never listed Lab's bot.", 90000, 500);

  // Studio away: Lab holds a change for its bot, and lists it waiting, with the name you gave it.
  await studioBox.stop();
  const held = await ask("PATCH", "kuvo/bots/bot_far1", { name: "Far Two" });
  assert.equal(held.status, 202, held.text);
  assert.equal(held.json.held.bot, "bot_far1");
  assert.equal(held.json.held.computer, "Studio");
  assert.deepEqual(held.json.held.change, { name: "Far Two" });
  assert.equal((await ask("PATCH", "kuvo/bots/bot_far1", { work: { limits: {} } })).status, 409, "work settings change only where it lives");
  assert.equal((await ask("PATCH", "kuvo/bots/bot_far1", { name: "" })).status, 400);
  let now = await listed(ask);
  assert.deepEqual(now.held.map(each => [each.bot, each.change]), [["bot_far1", { name: "Far Two" }]]);
  const far = now.elsewhere.find(each => each.id === "bot_far1");
  assert.equal(far.name, "Far Two");
  assert.equal(far.waiting, true);
  assert.ok(fs.existsSync(path.join(lab.home, "run", "bots-held", "bot_far1.json")));

  // Back: Studio takes it, and Lab lets it go.
  studioBox = studio.start();
  main = asApp(studio.connect(await studioBox.ready()));
  await until(() => nameOf(studio.home, "bot_far1") === "Far Two", "Studio never took the held change.", 120000, 500);
  await until(async () => (await listed(ask)).held?.length === 0, "Lab still held the change.", 60000, 500);

  // One Studio refuses (a personality there isn't): let go, and said.
  const refusedNow = await ask("PATCH", "kuvo/bots/bot_far1", { personality: "preset.nothing-like-it" });
  assert.equal(refusedNow.status, 202, refusedNow.text);
  now = await until(async () => {
    const said = await listed(ask);
    return said.refused?.some(each => each.bot === "bot_far1") && said;
  }, "Studio's refusal was never said.", 120000, 500);
  assert.deepEqual(now.held, []);
  const refusal = now.refused.find(each => each.bot === "bot_far1");
  assert.equal(refusal.computer, "Studio");
  assert.match(refusal.message, /^Studio didn’t take the changes you made while it was away\./);
  assert.equal(now.elsewhere.find(each => each.id === "bot_far1").refused, true);

  // The other way: Lab away, Studio holds a delete for Lab's bot; Lab, back, deletes it.
  await labBox.stop();
  const deleting = await main("DELETE", "kuvo/bots/bot_near1");
  assert.equal(deleting.status, 202, deleting.text);
  assert.equal(deleting.json.held.delete, true);
  labBox = lab.start();
  ask = asApp(lab.connect(await labBox.ready()));
  await until(() => !fs.existsSync(path.join(lab.home, "bots", "bot_near1")), "Lab never deleted its bot as Studio held.", 120000, 500);
  await until(async () => (await listed(main)).held?.length === 0, "Studio still held the delete.", 60000, 500);
});
