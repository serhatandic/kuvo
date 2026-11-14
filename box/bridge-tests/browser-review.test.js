import "./test-safety.js";
import assert from "node:assert/strict";
import { test } from "node:test";
import { browserHost } from "./browser-host-stand-in.js";

let next = 100;
async function call(c, method, params = {}, sessionId) {
  const id = next++;
  c.send({ id, method, params, ...(sessionId ? { sessionId } : {}) });
  return c.heard.until(m => m.id === id);
}
async function page() {
  const h = await browserHost();
  const c = await h.client();
  await c.heard.until(m => m.method === "Kuvo.hello");
  h.event("Target.targetCreated", { targetInfo: { targetId: "P", type: "page", url: "about:blank", title: "Title" } });
  h.event("Target.attachedToTarget", { sessionId: "own", targetInfo: { targetId: "P", type: "page", url: "about:blank" } });
  await h.chrome.until(m => m.method === "Page.enable" && m.sessionId === "own");
  h.answer("Target.attachToTarget", () => ({ sessionId: "raw" }));
  await call(c, "Target.attachToTarget", { targetId: "P", flatten: true });
  return { ...h, c };
}

test("raw MachineBrowser metrics retain DPR and zoom; screencast limits only the picture", async () => {
  const { c, chrome, answer } = await page();
  answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
  await call(c, "Kuvo.viewing");
  await call(c, "Emulation.setDeviceMetricsOverride", { width: 500, height: 350, deviceScaleFactor: 4, mobile: false }, "raw");
  await call(c, "Emulation.setFocusEmulationEnabled", { enabled: true }, "raw");
  const started = await call(c, "Page.startScreencast", { format: "jpeg", quality: 75, maxWidth: 1000, maxHeight: 700 }, "raw");
  assert.equal(started.sessionId, "raw");
  await c.heard.until(m => m.method === "Page.screencastFrame");
  const metrics = chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride");
  assert.equal(metrics.length, 1);
  assert.deepEqual([metrics[0].params.width, metrics[0].params.height, metrics[0].params.deviceScaleFactor], [500, 350, 4]);
  assert.ok(chrome.all.some(m => m.method === "Emulation.setFocusEmulationEnabled"));
  assert.ok(chrome.all.some(m => m.method === "Page.captureScreenshot" && m.params.quality === 75 && m.params.clip.scale === 2));
  await call(c, "Page.stopScreencast", {}, "raw");
});

test("mouse movement does not resize an unchanged viewer page or rebroadcast its viewport", async () => {
  const { c, chrome } = await page();
  await call(c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  const before = chrome.all.filter(m => m.method.startsWith("Emulation.")).length;
  const facts = c.heard.all.filter(m => m.method === "Kuvo.viewport").length;
  for (let i = 0; i < 10; i++) await call(c, "Kuvo.input", { targetId: "P", kind: "mouse", type: "mouseMoved", x: i, y: i });
  assert.equal(chrome.all.filter(m => m.method.startsWith("Emulation.")).length, before);
  assert.equal(c.heard.all.filter(m => m.method === "Kuvo.viewport").length, facts);
});

test("an unchanged capture class backs off, while an input wakes it immediately", async () => {
  const { framePolicy } = await import("../src/browser/frame-policy.js");
  const policy = framePolicy();
  assert.equal(policy.due(0), true);
  assert.equal(policy.captured("A", 0, 15), true);
  assert.equal(policy.due(60), false);
  assert.equal(policy.captured("A", 70, 15), false);
  assert.equal(policy.due(300), false);
  assert.equal(policy.due(320), true);
  for (let n = 0; n < 20; n++) policy.captured("A", 1000 * n, 15);
  assert.equal(policy.due(20999), false);
  policy.dirty();
  assert.equal(policy.due(20000), true);
  assert.equal(policy.captured("B", 20000, 15), true);
});

test("an identical JPEG is not sent again even after the viewer returns credit", async () => {
  const { c, chrome, answer } = await page();
  let captures = 0;
  answer("Page.captureScreenshot", () => { captures++; return { data: "anBlZw==" }; });
  await call(c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  const first = await c.heard.until(m => m.method === "Kuvo.frame");
  await call(c, "Kuvo.frameAck", { frameId: first.params.frameId });
  await call(c, "Kuvo.input", { targetId: "P", kind: "text", text: "unchanged" });
  await chrome.until(m => m.method === "Page.captureScreenshot" && captures >= 2);
  await call(c, "Browser.getVersion");
  assert.equal(c.heard.all.filter(m => m.method === "Kuvo.frame").length, 1);
  await call(c, "Kuvo.view", { targetId: null });
});

test("an oversized relay picture lowers the next class quality and tells that viewer", async () => {
  const { c, answer, chrome } = await page();
  let captures = 0;
  answer("Page.captureScreenshot", p => ({ data: Buffer.from("jpeg:" + p.quality + ":" + ++captures).toString("base64") }));
  await call(c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  const frame = await c.heard.until(m => m.method === "Kuvo.frame");
  await call(c, "Kuvo.frameDropped", { frameId: frame.params.frameId, reason: "oversize" });
  const notice = await c.heard.until(m => m.method === "Kuvo.viewQuality");
  assert.equal(notice.params.quality, 40);
  await chrome.until(m => m.method === "Page.captureScreenshot" && m.params.quality === 40);
  await call(c, "Kuvo.view", { targetId: null });
});

test("upload data must be base64 text, not an attacker-controlled array-like allocation", async () => {
  const { c, chrome } = await page();
  const result = await call(c, "Kuvo.upload", { targetId: "P", files: [{ name: "bad", data: { length: 300000000 } }] });
  assert.match(result.error.message, /base64 text/);
  assert.equal(chrome.all.filter(m => m.method === "DOM.setFileInputFiles").length, 0);
  assert.ok(!(await call(c, "Browser.getVersion")).error);
});

test("a refused upload reports failure and navigation invalidates its old chooser", async () => {
  const { c, event, answer, chrome } = await page();
  event("Page.fileChooserOpened", { backendNodeId: 42, mode: "selectSingle" }, "own");
  await c.heard.until(m => m.method === "Kuvo.fileChooser");
  answer("DOM.setFileInputFiles", () => ({ __error: { message: "No such node" } }));
  const files = [{ name: "test.txt", data: "aGk=" }];
  assert.match((await call(c, "Kuvo.upload", { targetId: "P", files })).error.message, /refused/);
  event("Page.frameNavigated", { frame: { id: "P", url: "https://example.com/new" } }, "own");
  await c.heard.until(m => m.method === "Kuvo.pages" && m.params.pages.some(p => p.url.endsWith("/new")));
  const before = chrome.all.filter(m => m.method === "DOM.setFileInputFiles").length;
  assert.match((await call(c, "Kuvo.upload", { targetId: "P", files })).error.message, /No file chooser/);
  assert.equal(chrome.all.filter(m => m.method === "DOM.setFileInputFiles").length, before);
});

test("page-initiated external URLs name their page and reach only that page's viewers", async () => {
  const { c, client, event } = await page();
  const observer = await client();
  await observer.heard.until(m => m.method === "Kuvo.hello");
  await call(c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  event("Page.frameRequestedNavigation", { url: "mailto:hello@example.com" }, "own");
  const external = await c.heard.until(m => m.method === "Kuvo.external");
  assert.equal(external.params.targetId, "P");
  await call(observer, "Browser.getVersion");
  assert.equal(observer.heard.all.filter(m => m.method === "Kuvo.external").length, 0);
  await call(c, "Kuvo.view", { targetId: null });
});

test("Page events that change no page facts do not spend relay messages", async () => {
  const { c, event } = await page();
  event("Page.domContentEventFired", {}, "own");
  await call(c, "Browser.getVersion");
  const before = c.heard.all.filter(m => m.method === "Kuvo.pages").length;
  for (let i = 0; i < 10; i++) event("Page.domContentEventFired", {}, "own");
  await call(c, "Browser.getVersion");
  assert.equal(c.heard.all.filter(m => m.method === "Kuvo.pages").length, before);
});

test("localhost and ordinary hosts with ports navigate inside the page", async () => {
  const { c, chrome } = await page();
  for (const input of ["localhost:3000", "example.com:8080"]) await call(c, "Kuvo.go", { targetId: "P", input });
  const urls = chrome.all.filter(m => m.method === "Page.navigate").map(m => m.params.url);
  assert.ok(urls.includes("http://localhost:3000"));
  assert.ok(urls.includes("https://example.com:8080"));
  assert.equal(c.heard.all.filter(m => m.method === "Kuvo.external").length, 0);
});
