// Your time zone as a box's setting (PROTOCOL.md, "Your time zone"): read and changed by any client
// through the box's settings (`kuvo/settings/clock`), carried from what a Mac app gave a machine
// before (its kit, the zone above its conversations' wake-ups, its launch's KUVO_TZ), and written
// where `kuvo-wake` reads it, so a machine's bots and conversations wake on your clock, not the
// machine's. A machine's box (the real kuvo-link.js, each call a process of its own as a client's
// over SSH is, `box route`), on a clock set to UTC; the real kuvo-wake.
//
// Run with `node --test box/bridge-tests/time-zone.test.js`.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { beforeEach, test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-time-zone-")));
removeAtEnd(home);
const script = fileURLToPath(LINK_BUNDLE);
const write = (file, text) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
};
const read = file => (fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null);
const bots = path.join(home, "bots"), wakes = path.join(home, "wakes"), setting = path.join(home, "box", "time-zone.json");
// The machine's own clock: UTC, as a server's often is.
const env = { ...process.env, KUVO_HOME: home, HOME: home, TMPDIR: home, TZ: "UTC" };
for (const name of Object.keys(env)) if (name.startsWith("KUVO_") && name !== "KUVO_HOME") delete env[name];

// The box's own kuvo-wake, as a machine has it in its `bin`.
const wakeCommand = path.join(home, "bin", "kuvo-wake");
write(wakeCommand, read(new URL("../src/keeper/kuvo-wake", import.meta.url)));
fs.chmodSync(wakeCommand, 0o755);

// A route of the box's, asked as a client over SSH asks it.
function route(method, p, body) {
  const ran = spawnSync(process.execPath, [script, "box", "route"], { env, input: JSON.stringify({ method, path: p, ...(body ? { body } : {}) }), encoding: "utf8" });
  const said = JSON.parse(ran.stdout.trim().split("\n").pop());
  return { status: said.status, json: JSON.parse(said.body) };
}
const rowOf = page => page.groups.flatMap(group => group.rows).find(row => row.id === "time-zone");
const general = () => {
  const { status, json } = route("GET", "kuvo/settings/clock");
  assert.equal(status, 200, JSON.stringify(json));
  return rowOf(json);
};
// A bot's folder, as the box keeps one.
function bot(id) {
  const folder = path.join(bots, id);
  fs.mkdirSync(path.join(folder, "wakes"), { recursive: true });
  write(path.join(folder, "session"), `ses_${id}`);
  write(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id, name: "Scout", job: "", model: { id: "big-pickle", providerID: "opencode" } }));
  return folder;
}
// When a wake-up kuvo-wake keeps is due (seconds since 1970), from its file's name.
const dueOf = folder => fs.readdirSync(path.join(folder, "wakes")).map(name => Number(name.split("-")[0]));

beforeEach(() => {
  for (const each of [bots, wakes, path.join(home, "box"), path.join(home, "run")]) fs.rmSync(each, { recursive: true, force: true });
});

test("your time zone is a setting of the box: read and changed through its settings, and a machine's bot wakes on it, not on the machine's clock", () => {
  const folder = bot("bot_scout00001");
  // A machine set up with nothing from before: none yet, its own clock until you pick yours.
  const before = general();
  assert.equal(before.kind, "choice");
  assert.equal(before.value, "own-clock");
  assert.equal(before.choices[0].id, "own-clock");
  assert.match(before.choices[0].title, /'s clock \(UTC\)$/);
  assert.ok(before.choices.some(choice => choice.id === "Asia/Tokyo" && choice.title === "Asia › Tokyo"));
  assert.equal(read(setting), null, "nothing written by reading it");

  const changed = route("PATCH", "kuvo/settings/clock", { row: "time-zone", value: "Asia/Tokyo", was: before.revision });
  assert.equal(changed.status, 200, JSON.stringify(changed.json));
  const after = rowOf(changed.json);
  assert.equal(after.value, "Asia/Tokyo");
  assert.equal(after.choices.some(choice => choice.id === "own-clock"), false, "yours now: its own clock isn't offered again");
  assert.equal(general().value, "Asia/Tokyo", "kept");
  const first = JSON.parse(read(setting));
  assert.equal(first.zone, "Asia/Tokyo");
  assert.ok(first.n >= 1);
  // Where its wake-ups read it: its bots' folders, and the folder above its conversations' own.
  assert.equal(read(path.join(folder, "timezone")), "Asia/Tokyo");
  assert.equal(read(path.join(wakes, "timezone")), "Asia/Tokyo");

  // The bot wakes at 9:00 in Tokyo (midnight UTC), on a machine whose clock says UTC.
  const added = spawnSync(wakeCommand, ["bot_scout00001", "add", "2030-01-01T09:00", "Morning digest"], { env: { ...env, KUVO_BOTS: bots }, encoding: "utf8" });
  assert.equal(added.status, 0, added.stderr);
  assert.deepEqual(dueOf(folder), [Date.UTC(2030, 0, 1, 0, 0) / 1000]);

  // Changed again, from what was read before (another client changed it since): refused, with the page now.
  const stale = route("PATCH", "kuvo/settings/clock", { row: "time-zone", value: "Europe/Istanbul", was: before.revision });
  assert.equal(stale.status, 409);
  assert.equal(rowOf(stale.json.page).value, "Asia/Tokyo");
  // A zone that isn't one: refused.
  assert.equal(route("PATCH", "kuvo/settings/clock", { row: "time-zone", value: "Mars/Olympus", was: after.revision }).status, 400);
  // A new one counts on.
  assert.equal(route("PATCH", "kuvo/settings/clock", { row: "time-zone", value: "Europe/Istanbul", was: after.revision }).status, 200);
  const second = JSON.parse(read(setting));
  assert.equal(second.zone, "Europe/Istanbul");
  assert.ok(second.n > first.n);
  assert.equal(read(path.join(folder, "timezone")), "Europe/Istanbul");
});

test("a machine updated from 0.5 to 0.7 keeps the zone its Mac last gave it, nothing lost: from its kit first, else the zone above its wake-ups, else its launch's", () => {
  // Its kit, with your zone's rules (for a machine without zone files).
  const rules = Buffer.from("TZif Istanbul");
  write(path.join(bots, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "Europe/Istanbul", zoneinfo: rules.toString("base64") }));
  write(path.join(wakes, "timezone"), "America/New_York");
  write(path.join(home, "run", "launch.env"), "PATH=/usr/bin\nKUVO_TZ=Asia/Tokyo\n");
  const folder = bot("bot_scout00001");
  assert.equal(general().value, "Europe/Istanbul");
  const kept = JSON.parse(read(setting));
  // Carried: kept and gone by here, and giving way to what your other computers say (your Mac's own clock).
  assert.deepEqual([kept.zone, kept.n, kept.carried, Buffer.from(kept.zoneinfo, "base64").toString()], ["Europe/Istanbul", 0, true, "TZif Istanbul"]);
  // Kept after the kit's gone (nothing reads it now).
  fs.rmSync(path.join(bots, "kit.json"));
  assert.equal(general().value, "Europe/Istanbul");
  // Its bots' zone files, and their rules, as before (settled as a bot's next run starts, or as it arrives).
  route("POST", "kuvo/bots/bot_scout00001/arrived");
  assert.equal(read(path.join(folder, "timezone")), "Europe/Istanbul");
  assert.equal(read(path.join(folder, "zoneinfo")), "TZif Istanbul");

  // No kit: the zone its Mac put above its conversations' wake-ups.
  fs.rmSync(setting);
  assert.equal(general().value, "America/New_York");
  // Nor that: its launch's, as an older Mac started its engine.
  fs.rmSync(setting);
  fs.rmSync(path.join(wakes, "timezone"));
  assert.equal(general().value, "Asia/Tokyo");
  // None of them: none yet (it takes your other computers' as they tell it).
  fs.rmSync(setting);
  fs.rmSync(path.join(home, "run", "launch.env"));
  assert.equal(general().value, "own-clock");
  assert.equal(read(setting), null);
});

test("a Linux machine without time zone data takes your pick with its rules, and keeps them where kuvo-wake reads them; not rules that aren't a zone's", () => {
  const folder = bot("bot_scout00001");
  const empty = path.join(home, "no-zoneinfo");
  fs.mkdirSync(empty, { recursive: true });
  // (A zone's rules as the box takes them: a TZif file, which it passes on as it is.)
  const rules = Buffer.concat([Buffer.from("TZif2"), Buffer.alloc(64, 1)]).toString("base64");
  // As a peer's mail brings it (`zone` beside `agents`), taken as the box takes it.
  const ran = spawnSync(process.execPath, ["--input-type=module", "-e", `
    const link = await import(${JSON.stringify(script)});
    console.log(JSON.stringify([link.takeZone({ zone: "Asia/Tokyo", n: 1, zoneinfo: ${JSON.stringify(rules)} }), link.zoneMail()]));
  `], { env: { ...env, KUVO_TEST_ZONEINFO: empty }, encoding: "utf8" });
  assert.equal(ran.status, 0, ran.stderr);
  const [taken, told] = JSON.parse(ran.stdout.trim());
  assert.equal(taken, true);
  assert.equal(told.zoneinfo, rules, "and passes them on");
  assert.equal(read(path.join(folder, "timezone")), "Asia/Tokyo");
  assert.equal(fs.readFileSync(path.join(folder, "zoneinfo")).toString("base64"), rules, "beside it, where kuvo-wake reads them");
  // The row doesn't warn: it has the rules.
  const page = spawnSync(process.execPath, [script, "box", "route"], { env: { ...env, KUVO_TEST_ZONEINFO: empty }, input: JSON.stringify({ method: "GET", path: "kuvo/settings/clock" }), encoding: "utf8" });
  const row = rowOf(JSON.parse(JSON.parse(page.stdout.trim().split("\n").pop()).body));
  assert.equal(row.tone, undefined);
  // Without them (a pick told by a box that had none, on a machine that never had them), the row says
  // why its wake-ups go by UTC.
  for (const each of [setting, path.join(wakes, "timezone"), path.join(wakes, "zoneinfo")]) fs.rmSync(each, { force: true });
  const bare = spawnSync(process.execPath, ["--input-type=module", "-e", `
    const link = await import(${JSON.stringify(script)});
    console.log(JSON.stringify(link.takeZone({ zone: "Asia/Tokyo", n: 1 })));
  `], { env: { ...env, KUVO_TEST_ZONEINFO: empty }, encoding: "utf8" });
  assert.equal(bare.stdout.trim(), "true");
  const warned = spawnSync(process.execPath, [script, "box", "route"], { env: { ...env, KUVO_TEST_ZONEINFO: empty }, input: JSON.stringify({ method: "GET", path: "kuvo/settings/clock" }), encoding: "utf8" });
  const warning = rowOf(JSON.parse(JSON.parse(warned.stdout.trim().split("\n").pop()).body));
  assert.equal(warning.tone, "warning");
  assert.match(warning.line, /tzdata/);
  // The same pick told again, now with its rules (its first mail couldn't carry them): kept.
  const later = spawnSync(process.execPath, ["--input-type=module", "-e", `
    const link = await import(${JSON.stringify(script)});
    console.log(JSON.stringify(link.takeZone({ zone: "Asia/Tokyo", n: 1, zoneinfo: ${JSON.stringify(rules)} })));
  `], { env: { ...env, KUVO_TEST_ZONEINFO: empty }, encoding: "utf8" });
  assert.equal(later.stdout.trim(), "true");
  assert.equal(fs.readFileSync(path.join(folder, "zoneinfo")).toString("base64"), rules);
  // Rules that aren't a zone's: not kept.
  for (const each of [setting, path.join(wakes, "timezone"), path.join(wakes, "zoneinfo")]) fs.rmSync(each, { force: true });
  const junk = spawnSync(process.execPath, ["--input-type=module", "-e", `
    const link = await import(${JSON.stringify(script)});
    console.log(JSON.stringify([link.takeZone({ zone: "Europe/Paris", n: 1, zoneinfo: ${JSON.stringify(Buffer.from("not rules").toString("base64"))} }), link.zoneMail()]));
  `], { env: { ...env, KUVO_TEST_ZONEINFO: empty }, encoding: "utf8" });
  const [took, mail] = JSON.parse(junk.stdout.trim());
  assert.deepEqual([took, mail.zoneinfo], [true, undefined]);
  assert.equal(read(path.join(folder, "zoneinfo")), null);
});
