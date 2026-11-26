// Debug-only proof that resident timer branches park in a *pristine* fixture.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { boxBinary, isolatedEnv, localRelay, openLine, privateHome, scratch, startBox, until, write } from "./adapter.mjs";

export async function parkedTimers(t, timers, observeMs=5500, topics=null) {
  const binary = boxBinary();
  if (binary.typescript || !["darwin", "linux"].includes(process.platform)) {
    return t.skip("The Rust debug timer reporter uses a Unix signal.");
  }
  const root = scratch();
  const home = path.join(root, "box");
  privateHome(home);
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  const relay = await localRelay();
  const box = startBox({binary, home, relay, mac: true,
    env: {...isolatedEnv(root), KUVO_TEST_TIMER_REPORT: "1"}});
  t.after(async () => { await box.stop(); await relay.close(); fs.rmSync(root, { recursive: true, force: true }); });
  const record = await box.ready();
  const reports = path.join(home, "run", "timer-fires");
  if (!fs.existsSync(reports)) return t.skip("Use a debug executable with timer reporting.");
  const report = path.join(reports, String(record.pid) + ".json");
  if (topics) {
    const watch = Buffer.from(JSON.stringify(topics)).toString("base64url");
    const line = await openLine(record, "?watch=" + watch, "kuvo/line", { "x-kuvo-from": "app" }, observeMs + 15000);
    assert.equal(line.status, 200);
    t.after(() => line.close());
    const frames = line.frames();
    const wanted = new Set(Object.keys(topics));
    while (wanted.size) {
      const { value, done } = await frames.next();
      assert.ok(!done, `missing initial topics: ${[...wanted].join(", ")}`);
      if (value?.t && value.d !== undefined) wanted.delete(value.t);
    }
  }
  const sample = async () => {
    const before = fs.existsSync(report) ? JSON.parse(fs.readFileSync(report)).sequence : 0;
    process.kill(record.pid, "SIGUSR1");
    return until(() => {
      if (!fs.existsSync(report)) return false;
      const read = JSON.parse(fs.readFileSync(report));
      return read.sequence > before && read;
    }, "the owned resident didn't report timer counts");
  };
  await new Promise(resolve => setTimeout(resolve, 500));
  const before = await sample();
  await new Promise(resolve => setTimeout(resolve, observeMs));
  const after = await sample();
  for (const timer of timers) {
    assert.equal(after.timers[timer], before.timers[timer], `${timer} must park with no work`);
  }
  return {home, relay, box, record, sample};
}
