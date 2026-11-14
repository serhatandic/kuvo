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

for (const reason of ["budget", "unsent", "backpressure"]) {
  test("re-review M3: an unchanged picture skipped for " + reason + " is retried", async t => {
    const h = await page(t);
    h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
    await call(h.c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
    await capture(h);
    const first = h.c.heard.all.find(m => m.method === "Kuvo.frame");
    await call(h.c, "Kuvo.frameDropped", { frameId: first.params.frameId, reason });
    await h.advance(100);
    await settle(h);
    const frames = h.c.heard.all.filter(m => m.method === "Kuvo.frame");
    assert.equal(frames.length, 2, "a skipped picture must not be remembered as displayed");
    assert.equal(frames[1].params.data, first.params.data);
  });
}

test("re-review S1: a renderer call has a deadline, and frame credits bypass its ordered queue", async t => {
  const h = await page(t);
  h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
  await call(h.c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  await capture(h);
  const first = h.c.heard.all.find(m => m.method === "Kuvo.frame");
  h.answer("Input.insertText", () => new Promise(() => {}));
  const input = next++;
  h.c.send({ id: input, method: "Kuvo.input", params: { targetId: "P", kind: "text", text: "hung" } });
  await h.chrome.until(m => m.method === "Input.insertText");
  const ack = next++;
  h.c.send({ id: ack, method: "Kuvo.frameAck", params: { frameId: first.params.frameId } });
  await settle({ ...h, c: await h.client() });
  assert.ok(h.c.heard.all.some(m => m.id === ack), "acknowledgements must bypass a blocked command");
  await h.advance(10001);
  const response = await h.c.heard.until(m => m.id === input);
  assert.match(response.error?.message ?? "", /timed out/i);
  assert.ok(!(await call(h.c, "Kuvo.pages")).error);
});

test("re-review S2: a view client can answer the dialog that is holding its input call", async t => {
  const h = await page(t);
  let release;
  h.answer("Input.insertText", () => new Promise(resolve => { release = resolve; }));
  h.answer("Page.handleJavaScriptDialog", () => { release({}); return {}; });
  const input = next++;
  h.c.send({ id: input, method: "Kuvo.input", params: { targetId: "P", kind: "text", text: "dialog" } });
  await h.chrome.until(m => m.method === "Input.insertText");
  h.event("Page.javascriptDialogOpening", { type: "prompt", message: "Name?", defaultPrompt: "" }, "own");
  await h.c.heard.until(m => m.method === "Kuvo.dialog");
  const id = next++;
  h.c.send({ id, method: "Kuvo.dialogAnswer", params: { targetId: "P", accept: true, promptText: "name" } });
  await settle({ ...h, c: await h.client() });
  const command = h.chrome.all.find(m => m.method === "Page.handleJavaScriptDialog");
  assert.ok(command, "the new vocabulary must dismiss Chrome's dialog");
  assert.equal(command.sessionId, "own");
  assert.deepEqual(command.params, { accept: true, promptText: "name" });
  assert.ok(!(await h.c.heard.until(m => m.id === id)).error);
  await h.c.heard.until(m => m.id === input);
});

for (const way of ["request", "event"]) {
  test("re-review S3: detaching a raw viewer stops its captures via " + way, async t => {
    const h = await page(t);
    let image = 0;
    h.answer("Page.captureScreenshot", () => ({ data: Buffer.from("frame" + ++image).toString("base64") }));
    await call(h.c, "Kuvo.viewing");
    await call(h.c, "Page.startScreencast", { maxWidth: 900, maxHeight: 700 }, "raw");
    await capture(h);
    const frame = h.c.heard.all.find(m => m.method === "Page.screencastFrame");
    await call(h.c, "Page.screencastFrameAck", { sessionId: frame.params.sessionId }, "raw");
    if (way === "request") await call(h.c, "Target.detachFromTarget", { sessionId: "raw" });
    else h.event("Target.detachedFromTarget", { sessionId: "raw" });
    await settle(h);
    const before = captures(h);
    await h.advance(2200); await settle(h);
    assert.equal(captures(h), before, "the detached page must consume no capture work");
  });
}

for (const raw of [false, true]) {
  test("re-review S4: a passive " + (raw ? "raw" : "Kuvo") + " viewer regains its size when the agent stops", async t => {
    const h = await page(t);
    await call(h.c, "Kuvo.setStatus", { targetId: "P", working: true });
    const viewer = await h.client(); await viewer.heard.until(m => m.method === "Kuvo.hello");
    if (raw) {
      await call(viewer, "Kuvo.viewing");
      await call(viewer, "Target.attachToTarget", { targetId: "P", flatten: true });
      await call(viewer, "Emulation.setDeviceMetricsOverride", { width: 393, height: 660, deviceScaleFactor: 2 }, "raw");
      await call(viewer, "Page.startScreencast", { maxWidth: 393, maxHeight: 660 }, "raw");
    } else await call(viewer, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
    assert.equal(h.chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").at(-1).params.width, 1280);
    await call(h.c, "Kuvo.setStatus", { working: false });
    await h.advance(5001); await settle(h);
    const restored = h.chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").at(-1);
    assert.deepEqual([restored.params.width, restored.params.height], [393, 660]);
  });
}

test("re-review S5: oversize lowers pixel scale before quality, then recovers a small displayed picture", async t => {
  const h = await page(t);
  h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
  await call(h.c, "Kuvo.view", { targetId: "P", width: 1000, height: 700, scale: 2, quality: 75 });
  await capture(h);
  const first = h.c.heard.all.find(m => m.method === "Kuvo.frame");
  await call(h.c, "Kuvo.frameDropped", { frameId: first.params.frameId, reason: "oversize" });
  const lowered = h.c.heard.all.filter(m => m.method === "Kuvo.viewQuality").at(-1).params;
  assert.deepEqual([lowered.scale, lowered.quality], [1, 75], "pixel count must fall before JPEG quality");
  await capture(h, 100);
  const retry = h.c.heard.all.filter(m => m.method === "Kuvo.frame").at(-1);
  await call(h.c, "Kuvo.frameAck", { frameId: retry.params.frameId });
  await h.advance(2200); await settle(h);
  const recovered = h.c.heard.all.filter(m => m.method === "Kuvo.viewQuality").at(-1).params;
  assert.equal(recovered.reason, "recovery");
  assert.ok(recovered.scale > 1 && recovered.scale <= 2);
  assert.equal(recovered.quality, 75);
});

test("re-review S6: raw zoom metrics and picture limits are not clamped to a 2160-high layout", async t => {
  const h = await page(t);
  h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
  await call(h.c, "Kuvo.viewing");
  await call(h.c, "Emulation.setDeviceMetricsOverride", { width: 3000, height: 4800, deviceScaleFactor: 1 }, "raw");
  const metrics = h.chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").at(-1);
  assert.deepEqual([metrics.params.width, metrics.params.height], [3000, 4800]);
  await call(h.c, "Page.startScreencast", { maxWidth: 3000, maxHeight: 4800 }, "raw");
  await capture(h);
  const frame = h.c.heard.all.find(m => m.method === "Page.screencastFrame");
  assert.deepEqual([frame.params.metadata.deviceWidth, frame.params.metadata.deviceHeight], [3000, 4800]);
  assert.equal(h.chrome.all.find(m => m.method === "Page.captureScreenshot").params.clip.scale, 1);
});

test("a reopened still view gets its first picture even when its JPEG has not changed", async t => {
  const h = await page(t);
  h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
  await call(h.c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  await capture(h);
  const first = h.c.heard.all.find(m => m.method === "Kuvo.frame");
  await call(h.c, "Kuvo.frameAck", { frameId: first.params.frameId });
  await call(h.c, "Kuvo.view", { targetId: null });
  await call(h.c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  await h.advance(100); await settle(h);
  assert.equal(h.c.heard.all.filter(m => m.method === "Kuvo.frame").length, 2);
});

test("captures wait for pending viewport changes before reading their source DPR", async t => {
  const h = await page(t);
  let release;
  h.answer("Emulation.setPageScaleFactor", () => new Promise(resolve => { release = resolve; }));
  h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
  const id = next++;
  h.c.send({ id, method: "Kuvo.view", params: { targetId: "P", width: 393, height: 660 } });
  await h.chrome.until(m => m.method === "Emulation.setPageScaleFactor");
  const observer = await h.client(); await observer.heard.until(m => m.method === "Kuvo.hello");
  await h.advance(100); await settle({ ...h, c: observer });
  assert.equal(captures(h), 0, "a frame must not mix old cached dimensions with new Chrome metrics");
  release({});
  await h.c.heard.until(m => m.id === id);
  await capture(h, 100);
  const shot = h.chrome.all.find(m => m.method === "Page.captureScreenshot");
  assert.equal(shot.params.clip.width, 393);
  assert.equal(shot.params.clip.scale, 1);
});

test("an in-flight picture is discarded if its source metrics change before delivery", async t => {
  const h = await page(t);
  let release;
  h.answer("Page.captureScreenshot", () => new Promise(resolve => { release = resolve; }));
  await call(h.c, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
  await h.advance(100);
  await h.chrome.until(m => m.method === "Page.captureScreenshot");
  const agent = await h.client(); await agent.heard.until(m => m.method === "Kuvo.hello");
  await call(agent, "Kuvo.setStatus", { targetId: "P", working: true });
  release({ data: "b2xk" });
  await settle(h);
  assert.equal(h.c.heard.all.filter(m => m.method === "Kuvo.frame").length, 0);
  h.answer("Page.captureScreenshot", () => ({ data: "bmV3" }));
  await capture(h, 100);
  assert.equal(h.c.heard.all.filter(m => m.method === "Kuvo.frame").at(-1).params.data, "bmV3");
});

for (const raw of [false, true]) {
  test("round3 MUST: consecutive tools keep a watched page at the agent size for " + (raw ? "raw" : "Kuvo") + " viewers", async t => {
    const h = await page(t);
    const viewer = await h.client(); await viewer.heard.until(m => m.method === "Kuvo.hello");
    if (raw) {
      await call(viewer, "Kuvo.viewing");
      await call(viewer, "Target.attachToTarget", { targetId: "P", flatten: true });
      await call(viewer, "Emulation.setDeviceMetricsOverride", { width: 900, height: 700, deviceScaleFactor: 2 }, "raw");
      await call(viewer, "Page.startScreencast", { maxWidth: 900, maxHeight: 700 }, "raw");
    } else await call(viewer, "Kuvo.view", { targetId: "P", width: 393, height: 660 });
    for (let tool = 0; tool < 3; tool++) {
      await call(h.c, "Kuvo.setStatus", { targetId: "P", working: true });
      await call(h.c, "Kuvo.setStatus", { working: false });
      await h.advance(1000); await settle(h);
    }
    const widths = h.chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").map(m => m.params.width);
    assert.deepEqual(widths, [raw ? 900 : 393, 1280], "tool boundaries must not reflow the page");
    await h.advance(5001); await settle(h);
    assert.equal(h.chrome.all.filter(m => m.method === "Emulation.setDeviceMetricsOverride").at(-1).params.width, raw ? 900 : 393);
  });
}

for (const failure of ["refusal", "timeout"]) {
  test("round3 S1: status facts survive a viewport " + failure, async t => {
    const h = await page(t);
    h.answer("Emulation.setDeviceMetricsOverride", () => failure === "refusal"
      ? { __error: { message: "metrics refused" } } : new Promise(() => {}));
    const id = next++;
    h.c.send({ id, method: "Kuvo.setStatus", params: { targetId: "P", working: true } });
    await h.chrome.until(m => m.method === "Emulation.setDeviceMetricsOverride");
    if (failure === "timeout") await h.advance(10001);
    const response = await h.c.heard.until(m => m.id === id);
    assert.ok(response.error);
    assert.equal(h.c.heard.all.filter(m => m.method === "Kuvo.status").at(-1)?.params.working, true);
    assert.equal(h.c.heard.all.filter(m => m.method === "Kuvo.pages").at(-1).params.pages[0].working, true);
    const later = await h.client();
    assert.equal((await later.heard.until(m => m.method === "Kuvo.hello")).params.status.working, true);
  });
}

for (const phone of [false, true]) {
  test("round3 S2: oversize preserves 1x detail before reducing " + (phone ? "phone" : "wide") + " quality", async t => {
    const h = await page(t);
    h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
    await call(h.c, "Kuvo.view", { targetId: "P", width: phone ? 393 : 1000, height: 700, scale: phone ? 1 : 2, quality: phone ? 50 : 75 });
    await capture(h);
    let frame = h.c.heard.all.filter(m => m.method === "Kuvo.frame").at(-1);
    await call(h.c, "Kuvo.frameDropped", { frameId: frame.params.frameId, reason: "oversize" });
    if (!phone) {
      await capture(h, 100);
      frame = h.c.heard.all.filter(m => m.method === "Kuvo.frame").at(-1);
      await call(h.c, "Kuvo.frameDropped", { frameId: frame.params.frameId, reason: "oversize" });
    }
    const notice = h.c.heard.all.filter(m => m.method === "Kuvo.viewQuality").at(-1).params;
    assert.equal(notice.scale, 1);
    assert.equal(notice.quality, phone ? 40 : 65);
  });
}

for (const raw of [false, true]) {
  test("round3 S3: repeated " + (raw ? "raw" : "Kuvo") + " input cannot bypass a 15 fps capture floor", async t => {
    const h = await page(t);
    h.answer("Page.captureScreenshot", () => ({ data: "anBlZw==" }));
    await call(h.c, "Kuvo.view", { targetId: "P", width: 393, height: 660, maxFps: 15 });
    await capture(h);
    const frame = h.c.heard.all.find(m => m.method === "Kuvo.frame");
    await call(h.c, "Kuvo.frameAck", { frameId: frame.params.frameId });
    const before = captures(h);
    for (let i = 0; i < 50; i++) {
      if (raw) await call(h.c, "Input.dispatchMouseEvent", { type: "mouseMoved", x: i, y: i }, "raw");
      else await call(h.c, "Kuvo.input", { targetId: "P", kind: "mouse", type: "mouseMoved", x: i, y: i });
      await h.advance(20); await settle(h);
    }
    assert.ok(captures(h) - before <= 15, "wakes must not encode still frames at input frequency");
  });
}
