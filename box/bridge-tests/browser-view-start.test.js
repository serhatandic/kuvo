// The panel's live view wakes the browser on a cold box and after a release switch.
// The box's route and keeper are real. Only Chrome's DevTools pipe is a stand-in.
import "./test-safety.js";
import { LINK_BUNDLE } from "./box-stand-ins.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { atEnd, removeAtEnd } from "./at-end.js";
import { Heard } from "./browser-host-stand-in.js";

const home = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "kv-start-")));
removeAtEnd(home);
process.env.KUVO_HOME = home;
const run = path.join(home, "run");
const pidFile = path.join(run, "browser.pid");
const browser = fileURLToPath(new URL("../dist/kuvo-browser.js", import.meta.url));
const link = LINK_BUNDLE;
const { engineConnect } = await import(link.href);

function release(version) {
  const folder = path.join(home, version);
  fs.mkdirSync(folder);
  fs.copyFileSync(browser, path.join(folder, "kuvo-browser.js"));
  fs.copyFileSync(link, path.join(folder, "kuvo-link.js"));
  fs.writeFileSync(path.join(folder, "box-version"), version);
  const staged = path.join(home, "bin.next");
  fs.symlinkSync(folder, staged);
  fs.renameSync(staged, path.join(home, "bin"));
}

function openPanel() {
  const heard = new Heard();
  const view = engineConnect()({ path: "kuvo/browser/view", headers: { upgrade: "websocket" } }, {}, {
    open() { heard.add({ opened: true }); },
    text(data) { heard.add(JSON.parse(data)); },
    binary() {},
    end(error) { heard.add({ ended: true, error }); },
  });
  atEnd(() => view.close());
  return heard;
}

async function stopBrowser() {
  if (!fs.existsSync(pidFile)) return;
  // Only the keeper started within this fixture owns this PID file. Wait for its own cleanup.
  const pid = Number(fs.readFileSync(pidFile, "utf8"));
  const stopped = new Promise(resolve => {
    const watcher = fs.watch(run, () => {
      if (fs.existsSync(pidFile)) return;
      watcher.close();
      resolve();
    });
    atEnd(() => watcher.close());
  });
  process.kill(pid, "SIGTERM");
  await stopped;
}

test("opening the Browser tab wakes the installed browser, including after a box update", { timeout: 15_000 }, async () => {
  release("test-before");
  const current = path.join(home, "browser", "current");
  fs.mkdirSync(current, { recursive: true });
  fs.mkdirSync(run, { recursive: true });
  fs.writeFileSync(path.join(run, "browser-install.json"), '{"state":"ready"}');
  fs.writeFileSync(path.join(run, "browser.next"), "0\n");
  fs.writeFileSync(path.join(home, "browser", "app-data-read"), "");
  const chrome = path.join(current, "chrome.cjs");
  fs.writeFileSync(path.join(current, "chrome"),
    `#!/bin/sh\nBUN_BE_BUN=1 exec ${JSON.stringify(process.execPath)} ${JSON.stringify(chrome)}\n`, { mode: 0o755 });
  fs.writeFileSync(chrome, `const fs = require("node:fs");
const output = fs.createWriteStream(null, { fd: 4 });
const send = message => output.write(JSON.stringify(message) + "\\0");
const page = { targetId: "page", type: "page", url: "https://example.com/", title: "Example" };
let buffer = "";
fs.createReadStream(null, { fd: 3, encoding: "utf8" }).on("data", chunk => {
  buffer += chunk;
  let end;
  while ((end = buffer.indexOf("\\0")) >= 0) {
    const message = JSON.parse(buffer.slice(0, end));
    buffer = buffer.slice(end + 1);
    if (message.method === "Browser.close") process.exit(0);
    if (message.method === "Target.setDiscoverTargets") send({ method: "Target.targetCreated", params: { targetInfo: page } });
    const result = message.method === "Target.getTargets" ? { targetInfos: [page] } : {};
    if (message.id !== undefined) send({ id: message.id, result });
  }
}).on("end", () => process.exit(0));
`);
  atEnd(stopBrowser);

  for (const version of ["test-before", "test-after"]) {
    assert.equal(fs.existsSync(pidFile), false, `Chrome is cold on ${version}`);
    const panel = openPanel();
    const first = await panel.until(message => message.opened || message.ended);
    assert.equal(first.opened, true, first.error?.message);
    const hello = await panel.until(message => message.method === "Kuvo.hello" || message.ended);
    assert.equal(hello.params?.version, version, hello.error?.message);
    const live = await panel.until(message => message.params?.pages?.length || message.ended);
    assert.deepEqual(live.params?.pages.map(page => [page.url, page.title]), [["https://example.com/", "Example"]]);
    await stopBrowser();
    await panel.until(message => message.ended);
    if (version === "test-before") release("test-after");
  }
});
