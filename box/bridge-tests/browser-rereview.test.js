import "./test-safety.js";
import assert from "node:assert/strict";
import { test } from "node:test";
import { browserHost } from "./browser-host-stand-in.js";

let next = 1;
function call(c, method, params = {}, sessionId) {
  const id = next++;
  c.send({ id, method, params, ...(sessionId ? { sessionId } : {}) });
  return c.heard.until(m => m.id === id);
}
async function page(t) {
  const h = await browserHost({ clock: true });
  t.after(h.close);
  const c = await h.client(); await c.heard.until(m => m.method === "Kuvo.hello");
  h.event("Target.targetCreated", { targetInfo: { targetId: "P", type: "page", url: "about:blank", title: "Title" } });
  h.event("Target.attachedToTarget", { sessionId: "own", targetInfo: { targetId: "P", type: "page", url: "about:blank" } });
  await h.chrome.until(m => m.method === "Page.enable" && m.sessionId === "own");
  h.answer("Target.attachToTarget", () => ({ sessionId: "raw" }));
  await call(c, "Target.attachToTarget", { targetId: "P", flatten: true });
  return { ...h, c };
}
const captures = h => h.chrome.all.filter(m => m.method === "Page.captureScreenshot").length;
async function settle(h) { for (let i = 0; i < 3; i++) await call(h.c, "Browser.getVersion"); }
async function capture(h, advance = 2200) {
  const before = captures(h);
  await h.advance(advance);
  await h.chrome.until(m => m.method === "Page.captureScreenshot" && captures(h) > before);
  await settle(h);
}
async function still(h, raw = true) {
  h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
  if (raw) {
    await call(h.c, "Kuvo.viewing");
    await call(h.c, "Page.startScreencast", { maxWidth: 900, maxHeight: 700 }, "raw");
  } else await call(h.c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  await capture(h);
  const first = h.c.heard.all.find(m => m.method === (raw ? "Page.screencastFrame" : "Kuvo.frame"));
  await call(h.c, raw ? "Page.screencastFrameAck" : "Kuvo.frameAck",
    raw ? { sessionId: first.params.sessionId } : { frameId: first.params.frameId }, raw ? "raw" : undefined);
  for (let i = 0; i < 5; i++) await capture(h);
}

test("re-review M1: an agent with no Emulation calls retains Chrome's native DPR 2", async t => {
  const h = await page(t);
  await call(h.c, "Kuvo.setStatus", { targetId: "P", working: true });
  assert.equal(h.chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").at(-1).params.deviceScaleFactor, 2);
  await call(h.c, "Kuvo.setStatus", { working: false });
  const viewer = await h.client(); await viewer.heard.until(m => m.method === "Kuvo.hello");
  await call(viewer, "Kuvo.viewing");
  await call(viewer, "Target.attachToTarget", { targetId: "P", flatten: true });
  await call(viewer, "Emulation.setDeviceMetricsOverride", { width: 900, height: 700, deviceScaleFactor: 2 }, "raw");
  await call(h.c, "Kuvo.setStatus", { targetId: "P", working: true });
  assert.equal(h.chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").at(-1).params.deviceScaleFactor, 2);
});

for (const action of ["raw input", "agent command", "navigation", "load event"]) {
  test("re-review M2: a still class wakes immediately for " + action, async t => {
    const h = await page(t);
    await still(h);
    const before = captures(h);
    h.answer("Page.captureScreenshot", () => ({ data: "Y2hhbmdlZA==" }));
    if (action === "raw input") await call(h.c, "Input.dispatchMouseEvent", { type: "mousePressed", x: 1, y: 1 }, "raw");
    else if (action === "agent command") await call(h.c, "Runtime.evaluate", { expression: "document.body.click()" }, "raw");
    else if (action === "navigation") await call(h.c, "Kuvo.go", { targetId: "P", input: "https://example.com/new" });
    else h.event("Page.loadEventFired", {}, "own");
    await settle(h);
    await h.advance(100);
    await settle(h);
    assert.ok(captures(h) > before, "picture must not wait for the two-second still-page poll");
  });
}
