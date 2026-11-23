// The compiled service describes the repeated settings flows in the same words and kinds.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { removeAtEnd } from "./at-end.js";
const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-settings-flows-"));
removeAtEnd(root);
process.env.KUVO_HOME = root;
fs.mkdirSync(path.join(root, "box"));
fs.writeFileSync(path.join(root, "box", "opencode.json"), "{}");
const link = await import(LINK_BUNDLE.href);
const handle = link.engineHandler(() => { throw Error("offline"); }, { name: "test" }, () => null);
const ask = async (method, route, body) => {
 let status, text = "";
 for await (const piece of handle({ method, path: route, headers: {}, fromApp: true, ...(body === undefined ? {} : { body: Buffer.from(JSON.stringify(body)) }) }, undefined)) {
  if (piece.head) status = piece.head.status;
  if (piece.body) text += Buffer.from(piece.body).toString();
 }
 return { status, json: text ? JSON.parse(text) : undefined };
};
test("Skills exposes its list, add form and name check in the built service", async () => {
 const answer = await ask("GET", "kuvo/settings/skills");
 assert.equal(answer.status, 200);
 assert.equal(answer.json.title, "Skills");
 const list = answer.json.groups.flatMap(group => group.rows).find(row => row.id === "skills");
 assert.equal(list.kind, "list");
 assert.equal(list.add.form.fields[0].kind, "text");
 const checked = await ask("POST", "kuvo/settings/skills/skills/add/check", {values: {name:""}});
 assert.equal(checked.status, 200);
 assert.equal(checked.json.ready, false);
 assert.ok(checked.json.fields.some(field => field.id === "name"));
});
test("Custom Auto names its unavailable source without invented lists", async () => {
 const answer = await ask("GET", "kuvo/settings/custom-auto");
 assert.equal(answer.status, 200);
 assert.equal(answer.json.title, "Custom Auto");
 assert.deepEqual(answer.json.groups, []);
 assert.equal(answer.json.notes[0].tone, "warning");
});
