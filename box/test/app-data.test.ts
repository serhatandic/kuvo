import "./safety"
import { afterAll, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { probe } from "../src/link/add/scripts";
import { appBoxThere, appDataThere, appDataProbe } from "../src/link/install/app-data";
const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-app-data-")));
afterAll(() => fs.rmSync(root, { recursive: true, force: true }));

test("the SSH probe uses the installer's old-app data policy, including caches and unfamiliar data", () => {
  const cases = [[], ["engine/opencode", "desktop/state.json", "opencode/cache/data", ".DS_Store", "box/bin/start"], ["opencode/data/opencode.db"], ["box/machines.json"], ["future-data/sign-in"], ["box/run/keeper.json"], ["box/run/link.json"]];
  for (const [index, files] of cases.entries()) {
    const folder = path.join(root, String(index));
    fs.mkdirSync(folder);
    for (const file of files) { const target = path.join(folder, file); fs.mkdirSync(path.dirname(target), { recursive: true }); fs.writeFileSync(target, "kept"); }
    const expected = appBoxThere(folder) ? "app-box" : appDataThere(folder) ? "app-07" : "";
    const ran = spawnSync("sh", ["-c", `${appDataProbe()}\nold_app_verdict`], { env: { ...process.env, A: folder }, encoding: "utf8" });
    expect(ran.status).toBe(0);
    expect(ran.stdout.trim()).toBe(expected);
  }
});

test("a current Mac may keep the previous app's sessions for going back", () => {
  const home = path.join(root, "current");
  const old = path.join(home, "Library", "Application Support", "run.kuvo.code", "opencode", "sessions.db");
  fs.mkdirSync(path.dirname(old), { recursive: true }); fs.writeFileSync(old, "kept");
  fs.mkdirSync(path.join(home, ".kuvo-code", "box"), { recursive: true });
  fs.writeFileSync(path.join(home, ".kuvo-code", "box", "kind"), "mac\n");
  const ran = spawnSync("sh", ["-c", probe("none")], { env: { ...process.env, HOME: home }, encoding: "utf8" });
  expect(ran.status).toBe(0);
  expect(ran.stdout).not.toMatch(/^app-(?:07|box)$/m);
  expect(fs.readFileSync(old, "utf8")).toBe("kept");
});
