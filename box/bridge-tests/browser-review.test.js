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
