import "./test-safety.js";
import assert from "node:assert/strict";
import { test } from "node:test";
import { browserHost } from "./browser-host-stand-in.js";

const { chrome, event, client, answer } = await browserHost();
answer("Target.attachToTarget", () => ({ sessionId: "agent" }));
answer("Page.captureScreenshot", () => ({ data: Buffer.from("jpeg").toString("base64") }));
const call = async (c, id, method, params = {}, sessionId) => {
  c.send({ id, method, params, ...(sessionId ? { sessionId } : {}) });
  return c.heard.until(m => m.id === id);
};

test("agent navigation reaches every viewer and a phone cannot resize its working page", async () => {
  const agent = await client(), phone = await client(), desktop = await client();
  await Promise.all([agent, phone, desktop].map(c => c.heard.until(m => m.method === "Kuvo.hello")));
  event("Target.targetCreated", { targetInfo: { targetId: "P", type: "page", url: "about:blank" } });
  event("Target.attachedToTarget", { sessionId: "keeper", targetInfo: { targetId: "P", type: "page", url: "about:blank" } });
  await chrome.until(m => m.method === "Page.enable" && m.sessionId === "keeper");
  await call(agent, 1, "Target.attachToTarget", { targetId: "P", flatten: true });
  await call(agent, 2, "Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1 }, "agent");
  await call(agent, 3, "Kuvo.setStatus", { targetId: "P", working: true, session: "ses_agent" });
  const before = chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").length;
  await call(phone, 4, "Kuvo.view", { targetId: "P", width: 393, height: 660, scale: 3 });
  await call(desktop, 5, "Kuvo.view", { targetId: "P", width: 1280, height: 800 });
  event("Page.frameNavigated", { frame: { id: "P", url: "https://example.com/agent" } }, "keeper");
  for (const viewer of [phone, desktop]) {
    const facts = await viewer.heard.until(m => m.method === "Kuvo.pages" && m.params.pages.some(p => p.url === "https://example.com/agent"));
    assert.equal(facts.params.pages[0].working, true);
  }
  assert.equal(chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").length, before);
  const frame = await phone.heard.until(m => m.method === "Kuvo.frame");
  assert.ok(frame.params.width <= 393);
  assert.ok(chrome.all.some(m => m.method === "Page.captureScreenshot" && m.params.quality === 50));
  // A third frame cannot pass without a credit. A CDP barrier observes all captures before it.
  await phone.heard.until(m => m.method === "Kuvo.frame" && m.params.frameId !== frame.params.frameId);
  await call(agent, 6, "Browser.getVersion");
  assert.equal(phone.heard.all.filter(m => m.method === "Kuvo.frame").length, 2);
  await call(phone, 7, "Kuvo.frameAck", { frameId: frame.params.frameId });
  await phone.heard.until(m => m.method === "Kuvo.frame" && m.params.frameId > phone.heard.all.find(m => m.method === "Kuvo.frame" && m.params.frameId !== frame.params.frameId).params.frameId);
});

test("raw viewer metrics and screencast calls cannot override an agent", async () => {
  const raw = await client();
  await raw.heard.until(m => m.method === "Kuvo.hello");
  await call(raw, 20, "Kuvo.viewing");
  await call(raw, 21, "Target.attachToTarget", { targetId: "P", flatten: true });
  const before = chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").length;
  await call(raw, 22, "Emulation.setDeviceMetricsOverride", { width: 320, height: 600 }, "agent");
  await call(raw, 23, "Page.startScreencast", { maxWidth: 320, maxHeight: 600 }, "agent");
  assert.equal(chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").length, before);
  assert.equal(chrome.all.filter(m => m.method === "Page.startScreencast").length, 0);
  await raw.heard.until(m => m.method === "Page.screencastFrame");
});
