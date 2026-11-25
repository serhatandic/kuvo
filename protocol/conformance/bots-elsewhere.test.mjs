// The bots of your other computers (PROTOCOL.md, `bots.elsewhere`): each computer lists the bots
// the others told it of in `kuvo/bots`, with their look, job, computer and what deleting one there
// takes; one that lives here is this computer's own, whatever another says. The box under test
// (KUVO_BOX_BIN) is Lab; Studio runs KUVO_LEAD_BIN, the TypeScript box by default.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";
import { boxBinary, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { sessionOf } from "./engine.mjs";

const otherFile = process.env.KUVO_LEAD_BIN || new URL("../../box/dist/kuvo-link.js", import.meta.url).pathname;
const other = fs.existsSync(otherFile) ? boxBinary(otherFile) : null;
const skip = !other ? "no other box (KUVO_LEAD_BIN)" : other.typescript && process.platform === "win32" ? "the TypeScript box doesn't run on native Windows" : false;

const session = (id) => `ses_${id.replace(/_/g, "")}`;

function bot({ home, engine }, id, name, look, job) {
  const folder = path.join(home, "bots", id);
  const at = "2026-10-01T09:00:00Z";
  write(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id, name, job, look, created: at, updated: at }));
  write(path.join(folder, "session"), session(id));
  write(path.join(folder, "MEMORY.md"), `# ${name}'s memory\n`);
  engine.sessions.push(sessionOf({ id: session(id), title: name, directory: folder }));
}

const elsewhere = async (ask) => {
  const answer = await ask("GET", "kuvo/bots");
  assert.equal(answer.status, 200, answer.text);
  return answer.json.elsewhere ?? [];
};

// Each listed by the other, once it was told.
const listedBy = async (ask, id, whose) => {
  let seen = [];
  await until(async () => (seen = await elsewhere(ask)).some(each => each.id === id), () => `${whose} never listed ${id}: ${JSON.stringify(seen)}`, 90000, 500);
  return seen;
};

test("each computer lists the bots the other told it of, with their look, job and what deleting one there takes; one that lives here is its own", { timeout: 180000, skip }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio", other);
  const lab = await computer("Lab");
  const far = { color: "sky", shape: "gem", face: "wink" };
  const both = { color: "lime", shape: "dome", face: "calm" };
  bot(studio, "bot_far1", "Far", far, "Reads the news");
  bot(studio, "bot_both1", "Both", both, "Is on both");
  const main = studio.connect(await studio.start().ready());
  const setup = await main("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  const invite = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const joined = await lab.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const near = { color: "coral", shape: "pebble", face: "happy" };
  bot(lab, "bot_near1", "Near", near, "Keeps the lab tidy");
  bot(lab, "bot_both1", "Both", both, "Is on both");
  const ask = lab.connect(await lab.start().ready());

  // Lab lists Studio's bot as Studio told it, and not the one it has itself.
  let seen = await listedBy(ask, "bot_far1", "Lab");
  assert.deepEqual(seen.map(each => each.id), ["bot_far1"], "one that lives here is listed as its own");
  const farThere = seen[0];
  assert.equal(farThere.name, "Far");
  assert.deepEqual(farThere.look, far);
  assert.equal(farThere.job, "Reads the news");
  assert.equal(farThere.session, session("bot_far1"));
  assert.equal(farThere.box, setup.json.you.key);
  assert.equal(farThere.computer, "Studio");
  assert.ok(["mac", "machine"].includes(farThere.kind), farThere.kind);
  assert.equal(farThere.away, undefined);
  assert.deepEqual(farThere.deletes.map(line => line.what), ["conversation", "folder", "wakes", "groups", "sessions"]);
  assert.match(farThere.deletes[1].words, /^Its folder, with its memory and what it made there, goes to (the Trash|Kuvo Code’s trash folder) on Studio\.$/);
  assert.equal(farThere.deletes[2].words, "Its wake-ups and watches are cancelled.");
  assert.equal(farThere.deletes[3].words, "It leaves the group chats on Studio.");
  assert.ok((await ask("GET", "kuvo/bots")).json.bots.some(each => each.id === "bot_both1"));

  // Studio lists Lab's bot, told with its look and job, and where a bot deleted on Lab goes.
  seen = await listedBy(main, "bot_near1", "Studio");
  assert.deepEqual(seen.map(each => each.id), ["bot_near1"], "Studio's own stays its own");
  const nearThere = seen[0];
  assert.equal(nearThere.name, "Near");
  assert.deepEqual(nearThere.look, near);
  assert.equal(nearThere.job, "Keeps the lab tidy");
  assert.equal(nearThere.computer, "Lab");
  assert.equal(nearThere.deletes[1].words, "Its folder, with its memory and what it made there, goes to Kuvo Code’s trash folder on Lab.");
});
