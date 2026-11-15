// Your time zone as a box's setting (PROTOCOL.md, "Your time zone"): read and changed by any client
// through the box's settings (`kuvo/settings/general`), carried from what a Mac app gave a machine
// before (its kit, the zone above its conversations' wake-ups, its launch's KUVO_TZ), and written
// where `kuvo-wake` reads it, so a machine's bots and conversations wake on your clock, not the
// machine's. A machine's box (the real kuvo-link.js, each call a process of its own as a client's
// over SSH is, `box route`), on a clock set to UTC; the real kuvo-wake.
//
// Run with `node --test box/bridge-tests/time-zone.test.js`.
import "./test-safety.js";
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
const script = fileURLToPath(new URL("../../swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js", import.meta.url));
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
const swift = read(new URL("../../swift/OpenCodeKit/Sources/OpenCodeKit/Engine/Wakes.swift", import.meta.url));
const opening = 'public static let script = #"""\n';
const from = swift.indexOf(opening) + opening.length;
const wakeCommand = path.join(home, "bin", "kuvo-wake");
write(wakeCommand, swift.slice(from, swift.indexOf('\n    """#', from)).split("\n").map(line => line.replace(/^ {4}/, "")).join("\n") + "\n");
fs.chmodSync(wakeCommand, 0o755);

// A route of the box's, asked as a client over SSH asks it.
function route(method, p, body) {
  const ran = spawnSync(process.execPath, [script, "box", "route"], { env, input: JSON.stringify({ method, path: p, ...(body ? { body } : {}) }), encoding: "utf8" });
  const said = JSON.parse(ran.stdout.trim().split("\n").pop());
  return { status: said.status, json: JSON.parse(said.body) };
}
const rowOf = page => page.groups.flatMap(group => group.rows).find(row => row.id === "time-zone");
const general = () => {
  const { status, json } = route("GET", "kuvo/settings/general");
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
  assert.match(before.choices[0].title, /Clock \(UTC\)$/);
  assert.ok(before.choices.some(choice => choice.id === "Asia/Tokyo" && choice.title === "Asia › Tokyo"));
  assert.equal(read(setting), null, "nothing written by reading it");

  const changed = route("PATCH", "kuvo/settings/general", { row: "time-zone", value: "Asia/Tokyo", was: before.revision });
  assert.equal(changed.status, 200, JSON.stringify(changed.json));
  const after = rowOf(changed.json);
  assert.equal(after.value, "Asia/Tokyo");
  assert.equal(after.choices.some(choice => choice.id === "own-clock"), false, "yours now: its own clock isn't offered again");
  assert.equal(general().value, "Asia/Tokyo", "kept");
  assert.deepEqual([JSON.parse(read(setting)).zone, JSON.parse(read(setting)).n], ["Asia/Tokyo", 1]);
  // Where its wake-ups read it: its bots' folders, and the folder above its conversations' own.
  assert.equal(read(path.join(folder, "timezone")), "Asia/Tokyo");
  assert.equal(read(path.join(wakes, "timezone")), "Asia/Tokyo");

  // The bot wakes at 9:00 in Tokyo (midnight UTC), on a machine whose clock says UTC.
  const added = spawnSync(wakeCommand, ["bot_scout00001", "add", "2030-01-01T09:00", "Morning digest"], { env: { ...env, KUVO_BOTS: bots }, encoding: "utf8" });
  assert.equal(added.status, 0, added.stderr);
  assert.deepEqual(dueOf(folder), [Date.UTC(2030, 0, 1, 0, 0) / 1000]);

  // Changed again, from what was read before (another client changed it since): refused, with the page now.
  const stale = route("PATCH", "kuvo/settings/general", { row: "time-zone", value: "Europe/Istanbul", was: before.revision });
  assert.equal(stale.status, 409);
  assert.equal(rowOf(stale.json.page).value, "Asia/Tokyo");
  // A zone that isn't one: refused.
  assert.equal(route("PATCH", "kuvo/settings/general", { row: "time-zone", value: "Mars/Olympus", was: after.revision }).status, 400);
  // A new one counts on.
  assert.equal(route("PATCH", "kuvo/settings/general", { row: "time-zone", value: "Europe/Istanbul", was: after.revision }).status, 200);
  assert.deepEqual([JSON.parse(read(setting)).zone, JSON.parse(read(setting)).n], ["Europe/Istanbul", 2]);
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
  // Counted 0: your Mac's own clock (counted 1 as its box carries it) wins over what it once gave.
  assert.deepEqual([kept.zone, kept.n, Buffer.from(kept.zoneinfo, "base64").toString()], ["Europe/Istanbul", 0, "TZif Istanbul"]);
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

test("the box's other clocks go by your zone too: the night's window it updates in", () => {
  write(setting, JSON.stringify({ kuvo: 1, zone: "Asia/Tokyo", n: 3 }));
  const ran = spawnSync(process.execPath, ["--input-type=module", "-e", `
    const link = await import(${JSON.stringify(script)});
    // 2030-01-01T18:30Z is 03:30 on the 2nd in Tokyo: the night's window there, not in UTC.
    console.log(JSON.stringify(link.updateNight(new Date(Date.UTC(2030, 0, 1, 18, 30)))));
  `], { env, encoding: "utf8" });
  assert.equal(ran.status, 0, ran.stderr);
  assert.equal(JSON.parse(ran.stdout.trim()), "2030-01-02");
});
