import "./safety"

// Two isolated boxes with real list, peer-mail and addressed SSH handlers. The engine's HTTP API
// and the relay's delivery (including unreachable) are stand-ins. Both provider and coding-agent
// conversations use the same list and peer protocol after the engine/bridge read.

import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { SESSION_LIST, SETUP_QUEUES, SETUP_SESSIONS, SETUP_WAITING, RUNS_ON } from "../src/shared/routes/sessions";
import { fitsShape } from "../src/shared/shape";

const features = ["line.setup-lists", "line.peer-conversation", "data.queued-device-authority"];
const worker = path.join(import.meta.dir, "fixtures", "setup-lists", "box.ts");
const keyOf = (key: Buffer) => publicOf(key).toString("base64url");
const targets = {
  sessions: "kuvo/setup/sessions/events",
  waiting: "kuvo/setup/waiting/events",
  queues: "kuvo/setup/queues/events",
  runs: "kuvo/setup/runs-on/events",
};

function pair(codingAgents = false) {
  const root = mkdtempSync(path.join(process.env.KUVO_HOME!, "setup-lists-"));
  const device = newKey();
  const boxes = ["near", "far"].map((name, i) => {
    const key = newKey(), secret = newKey(), home = path.join(root, name);
    const computer = { id: keyOf(key), name };
    const peer = { key: computer.id, name, kind: "machine", address: addressOf(secret).toString("base64url") };
    const session = `ses_${name}`;
    const model = { providerID: name, id: `model-${name}` };
    const project = path.join(home, "project");
    const time = { created: 100 + i, updated: 200 + i };
    const cases: Record<string, { id: string; title: string; time: typeof time; directory?: string; location?: { directory: string } }> = {
      [session]: { id: session, title: `Session on ${name}`, directory: project, time },
    };
    const permissions: Record<string, unknown>[] = [];
    const opened = Promise.withResolvers<ReadableStreamDefaultController<Uint8Array>>();
    const prompt = Promise.withResolvers<{ session: string; body: unknown }>();
    const engine = Bun.serve({
      hostname: "127.0.0.1", port: 0, async fetch(request) {
        const route = new URL(request.url).pathname;
        const data = (value: unknown) => Response.json({ data: value });
        if (request.method === "POST" && route === "/api/session") {
          const input = await request.json() as { title: string; location: { directory: string } };
          const id = `ses_started_${name}`;
          cases[id] = { id, title: input.title, time, location: input.location };
          return data(cases[id]);
        }
        if (route === "/api/event") return new Response(new ReadableStream<Uint8Array>({
          start(controller) {
            opened.resolve(controller);
            controller.enqueue(Buffer.from(": connected\n\n"));
          },
        }), { headers: { "content-type": "text/event-stream" } });
        if (route.endsWith("/prompt") && request.method === "POST") {
          prompt.resolve({ session: route.split("/")[3]!, body: await request.json() });
          return data({});
        }
        if (route.endsWith("/message")) return data([]);
        if (route.endsWith("/permission")) return data(permissions);
        if (route.endsWith("/inbox") || route.endsWith("/form")) return data([]);
        if (route === "/api/session") return data(Object.values(cases));
        if (route === "/api/session/active") return data([]);
        if (route.startsWith("/api/session/")) return data(cases[route.split("/")[3]!]);
        if (route === "/api/provider") return data([{ id: model.providerID }]);
        if (route === "/api/model") return data([{ ...model, name: model.id }]);
        if (route === "/api/integration") return data([{ id: model.providerID, connections: [{}] }]);
        if (route === "/api/model/default") return data(model);
        return Response.json({}, { status: 404 });
      },
    });
    const codingModel = { providerID: "codex-cli", id: "test-codex", name: "Test Codex" };
    const bridge = i === 1 && codingAgents ? Bun.serve({
      hostname: "127.0.0.1", port: 0, fetch(request) {
        const route = new URL(request.url).pathname;
        if (route === "/api/model") return Response.json({ data: { models: [codingModel] } });
        if (route === "/api/integration") return Response.json({ data: [{
          id: codingModel.providerID, name: "Codex", connections: [{}],
          metadata: { source: "agent", agent: "codex", accountID: "default", modelsComplete: true },
        }] });
        if (route === "/api/kuvo/auto") return Response.json({ data: { lists: [] } });
        return Response.json({ data: [] });
      },
    }) : null;
    mkdirSync(project, { recursive: true });
    upgradeKuvoFolder(home);
    const run = async (command: object): Promise<any> => {
      const child = Bun.spawn([process.execPath, worker, "kuvo-agents"], {
        env: { ...process.env, KUVO_HOME: home, HOME: root, TMPDIR: root },
        stdin: Buffer.from(JSON.stringify({ command, port: engine.port, bridgePort: bridge?.port })), stdout: "pipe", stderr: "pipe",
      });
      try {
        const [status, stdout, stderr] = await Promise.all([child.exited, child.stdout.text(), child.stderr.text()]);
        if (status !== 0) throw new Error(stderr || stdout);
        return JSON.parse(stdout.trim().split("\n").at(-1)!);
      } finally {
        if (child.exitCode === null) child.kill();
      }
    };
    const read = async (path: string) => run({ kind: "route", path });
    const linked = (ipc: (message: any) => void) => Bun.spawn([process.execPath, worker, "kuvo-agents"], {
      env: { ...process.env, KUVO_HOME: home, HOME: root, TMPDIR: root },
      stdin: Buffer.from(JSON.stringify({ command: { kind: "conversation" }, port: engine.port })),
      stdout: "ignore", stderr: "inherit", ipc,
    });
    return { key, secret, home, computer, peer, model, session, cases, run, read, engine, bridge, linked, opened, prompt, permissions };
  });
  const setup = "setup_" + "a".repeat(32);
  for (const box of boxes) {
    const run = path.join(box.home, "run");
    mkdirSync(run, { recursive: true });
    writeFileSync(path.join(run, "engine.json"), JSON.stringify({ port: box.engine.port, password: "test" }));
    const peers = boxes.filter(other => other !== box);
    writeFileSync(path.join(run, "link.json"), JSON.stringify({
      formatVersion: 1, key: box.key.toString("base64url"), secret: box.secret.toString("base64url"), name: box.computer.name,
      phones: [{ key: keyOf(device), name: "Desktop", from: "pair", kind: "desktop", pairing: "test" }],
      setup: { id: setup, role: "member", primary: boxes[0]!.computer.id, checkpointed: true },
      sharedSetup: setup, shared: { membershipAndAccess: { value: { boxes: boxes.map(other => other.peer) } } },
    }));
    writeFileSync(path.join(run, "peer-agents.json"), JSON.stringify(Object.fromEntries(peers.map(other =>
      [other.peer.key, { features, at: 100, list: [] }],
    ))));
  }
  const [near, far] = boxes as [typeof boxes[number], typeof boxes[number]];
  return { near, far, device: device.toString("base64url"), close() {
    for (const box of boxes) { box.engine.stop(true); box.bridge?.stop(true); }
    rmSync(root, { recursive: true, force: true });
  } };
}

type Box = ReturnType<typeof pair>["near"];

async function share(from: Box, to: Box, waiting?: string) {
  const frames = await from.run({ kind: "publish", waiting });
  expect(frames.length).toBeGreaterThan(0);
  expect(await to.run({ kind: "receive", frames })).toBe(true);
}

async function line(box: Box) {
  const watch = Buffer.from(JSON.stringify(targets)).toString("base64url");
  const { status, text } = await box.read(`kuvo/line?once=1&watch=${watch}`);
  expect(status).toBe(200);
  const [hello, ...topics] = text.trim().split("\n").map(JSON.parse as (line: string) => any);
  expect(hello.hello.fresh).toBe(true);
  const refused = topics.filter((topic: any) => topic.refused);
  expect(refused).toEqual([]);
  const state = Object.fromEntries(topics.map((row: any) => [row.t, row.d]));
  expect(fitsShape(SETUP_SESSIONS, state.sessions)).toBe(true);
  expect(fitsShape(SETUP_WAITING, state.waiting)).toBe(true);
  expect(fitsShape(SETUP_QUEUES, state.queues)).toBe(true);
  expect(fitsShape(RUNS_ON, state.runs)).toBe(true);
  return state;
}

test("one line lists both computers' sessions and offers each computer's own models and agents", async () => {
  const setup = pair(true);
  const { near, far } = setup;
  try {
    // Force a real multipart snapshot as well as an ordinary one in the reverse direction.
    const template = far.cases[far.session]!;
    const more = Array.from({ length: 360 }, (_, i) => {
      const id = `ses_extra${i}`;
      far.cases[id] = { ...template, id };
      return id;
    });
    await share(far, near, far.session);
    await share(near, far);

    const merged = await line(near);
    const rows = merged.sessions.sessions;
    expect(rows).toHaveLength(2 + more.length);
    expect(rows.filter((row: any) => row.computer.id === far.computer.id).map((row: any) => row.id).sort())
      .toEqual([far.session, ...more].sort());
    expect(rows.find((row: any) => row.id === near.session)).toMatchObject({ computer: near.computer, away: false, actions: ["open", "manage"] });
    expect(rows.find((row: any) => row.id === far.session)).toMatchObject({ computer: far.computer, away: false, actions: ["open", "manage"] });
    expect(merged.waiting.waiting).toEqual([{
      computer: far.computer, session: far.session, title: template.title, since: 700,
      asks: { kind: "permission", summary: "asks to run a command" }, away: false, actions: ["review"],
    }]);
    expect(merged.queues.queues).toEqual([{
      computer: far.computer, session: far.session, queue: { paused: true, held: [] }, away: false,
      actions: ["open", "resume", "messages"],
    }]);
    expect(merged.runs.computers.map((row: any) => [row.computer, row.offered.models.map((model: any) => [model.providerID, model.id]), row.agents]))
      .toEqual([[near.computer, [[near.model.providerID, near.model.id]], []], [far.computer, [[far.model.providerID, far.model.id], ["codex-cli", "test-codex"]], [{ id: "codex", name: "Codex" }]]]);
    expect((await line(far)).sessions.sessions.find((row: any) => row.id === near.session)).toMatchObject({ computer: near.computer });
    const local = JSON.parse((await near.read("kuvo/sessions/list")).text);
    expect(fitsShape(SESSION_LIST, local)).toBe(true);
    expect(Object.keys(local.sessions)).toEqual([near.session]);
  } finally { setup.close(); }
}, 30_000);

test("a computer going away keeps its rows with the same last-seen time and no actions", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near, far.session);
    expect(await near.run({ kind: "away", address: far.peer.address })).toBe(true);
    const merged = await line(near);
    const session = merged.sessions.sessions.find((row: any) => row.id === far.session);
    expect(session).toMatchObject({ computer: far.computer, away: true, lastSeen: expect.any(Number), actions: [] });
    const away = { computer: far.computer, session: far.session, away: true, lastSeen: session.lastSeen, actions: [] };
    expect(merged.waiting.waiting).toEqual([expect.objectContaining(away)]);
    expect(merged.queues.queues).toEqual([expect.objectContaining(away)]);
    expect(merged.runs.computers.map((row: any) => row.computer)).toEqual([near.computer]);
  } finally { setup.close(); }
}, 30_000);

test("an empty session starts on the computer chosen in Runs on over addressed SSH", async () => {
  const setup = pair();
  const { near, far, device } = setup;
  try {
    const wrong = await near.run({ kind: "start", device, target: near.computer.id, bodyTarget: far.computer.id });
    const right = await far.run({ kind: "start", device, target: far.computer.id, bodyTarget: far.computer.id });
    expect({ wrong: wrong.status, right: right.status }).toEqual({ wrong: 409, right: 200 });
    expect(JSON.parse(right.body)).toMatchObject({ session: "ses_started_far", computer: far.computer });
    expect(Object.keys(near.cases)).toEqual([near.session]);
    expect(Object.keys(far.cases)).toEqual([far.session, "ses_started_far"]);
  } finally { setup.close(); }
}, 30_000);

test("the nearest box opens another computer’s bot chat, follows its turn and sends to its owner", async () => {
  const setup = pair();
  const { near, far } = setup;
  let a: ReturnType<Box["linked"]> | undefined;
  let b: ReturnType<Box["linked"]> | undefined;
  const bot = "bot_far";
  const folder = path.join(far.home, "bots", bot);
  const readyA = Promise.withResolvers<void>();
  const readyB = Promise.withResolvers<void>();
  const live = Promise.withResolvers<Record<string, any>>();
  const queues = Promise.withResolvers<any>();
  const asks = Promise.withResolvers<any>();
  const page = Promise.withResolvers<any>();
  const sent = Promise.withResolvers<any>();
  try {
    mkdirSync(folder, { recursive: true });
    writeFileSync(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Remote bot" }));
    writeFileSync(path.join(folder, "session"), far.session);
    await share(far, near);
    a = near.linked(message => {
      if (message.failure) live.reject(new Error(message.failure));
      if (message.ready) readyA.resolve();
      if (message.frame) b!.send({ frame: message.frame });
      if (message.line?.refused) live.reject(new Error(JSON.stringify(message.line.refused)));
      const line = message.line;
      if (line?.t === "turn" && line.d) live.resolve(line.d);
      if (line?.t === "queue" && line.d) queues.resolve(line.d);
      if (line?.t === "asks" && line.d) asks.resolve(line.d);
      if (message.read) page.resolve(message.read);
      if (message.sent) sent.resolve(message.sent);
    });
    b = far.linked(message => {
      if (message.ready) readyB.resolve();
      if (message.frame) a!.send({ frame: message.frame });
    });
    await Promise.all([readyA.promise, readyB.promise]);

    const base = `kuvo/peer-session/${far.computer.id}/${far.session}`;
    far.permissions.push({ id: "per_fixture", sessionID: far.session, action: "edit", resources: ["README.md"] });
    a.send({ watch: { turn: base + "/events", queue: base + "/queue/events", asks: base + "/asks/events" } });
    a.send({ read: base + "/session" });
    const saw = await Promise.race([far.opened.promise, live.promise.then(() => { throw new Error("The engine did not open its event stream."); })]);
    const event = { type: "session.text.delta", data: { sessionID: far.session, delta: "A live turn from B" } };
    saw.enqueue(Buffer.from(`data: ${JSON.stringify({ ...event, data: { ...event.data, sessionID: "ses_unrelated" } })}\n\n`));
    saw.enqueue(Buffer.from(`data: ${JSON.stringify(event)}\n\n`));
    expect(await live.promise).toEqual(event);
    expect(await queues.promise).toMatchObject({ paused: false, items: [] });
    expect(await asks.promise).toMatchObject({ asks: [{ id: "per_fixture", kind: "permission", session: far.session, title: "Allow editing README.md?" }] });
    const original = await page.promise;
    expect(original.status).toBe(200);
    expect(JSON.parse(original.text).data.id).toBe(far.session);

    const id = "msg_computerroute";
    a.send({ send: { path: `kuvo/data/messages?session=${far.session}&computer=${far.computer.id}`, body: { id, words: "Send to the remote bot" } } });
    const accepted = await sent.promise;
    expect(accepted.status).toBe(202);
    expect(JSON.parse(accepted.text)).toMatchObject({ status: "pending", owner: far.computer.id });
    expect(await far.prompt.promise).toMatchObject({ session: far.session, body: { id, text: expect.stringContaining("Send to the remote bot") } });
  } finally {
    a?.kill();
    b?.kill();
    await Promise.all([a?.exited, b?.exited]);
    setup.close();
  }
}, 40_000);
