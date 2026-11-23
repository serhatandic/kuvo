import "./safety"

// Two isolated boxes with real list, peer-mail and addressed SSH handlers. The engine's HTTP API
// and the relay's delivery (including unreachable) are stand-ins. Both provider and coding-agent
// conversations use the same list and peer protocol after the engine/bridge read.

import { expect, test } from "bun:test";
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { SESSION_LIST, SETUP_QUEUES, SETUP_SESSIONS, SETUP_WAITING, RUNS_ON } from "../src/shared/routes/sessions";
import { fitsShape } from "../src/shared/shape";

const features = ["data.ownership", "line.pending-computers", "search.setup", "work-queue.forward.memory", "line.merged-archives", "line.merged-outputs", "line.merged-notices", "line.merged-library", "work-queue.elsewhere", "bots.peer-activity", "line.setup-lists", "line.peer-conversation", "data.queued-device-authority", "typed.draft", "typed.delivery"];
const worker = path.join(import.meta.dir, "fixtures", "setup-lists", "box.ts");
const keyOf = (key: Buffer) => publicOf(key).toString("base64url");
const targets = {
  sessions: "kuvo/setup/sessions/events",
  listing: "kuvo/setup/sessions/list/events",
  waiting: "kuvo/setup/waiting/events",
  queues: "kuvo/setup/queues/events",
  runs: "kuvo/setup/runs-on/events",
};

const agentProviders = ["claude-code", "codex-cli", "antigravity", "grok-cli", "cursor"] as const;
const agentNames = ["claude", "codex", "antigravity", "grok", "cursor"] as const;
const codingSession = (agent: string) => `ses_fixture${agent}`;

function pair(codingAgents = false, bridgeDelivery = false, { withLibrary = false, managed = false, withOutputs = false } = {}) {
  const root = mkdtempSync(path.join(process.env.KUVO_HOME!, "setup-lists-"));
  const device = newKey();
  const boxes = ["near", "far"].map((name, i) => {
    const key = newKey(), secret = newKey(), home = path.join(root, name);
    const computer = { id: keyOf(key), name };
    const peer = { key: computer.id, name, kind: "machine", address: addressOf(secret).toString("base64url") };
    const session = `ses_${name}`;
    const model = { providerID: name, id: `model-${name}` };
    const project = path.join(home, "project");
    const time: { created: number; updated: number; archived?: number } = { created: 100 + i, updated: 200 + i };
    const cases: Record<string, { id: string; title: string; time: typeof time; directory?: string; location?: { directory: string } }> = {
      [session]: { id: session, title: `Session on ${name}`, directory: project, time },
    };
    const transcript: Record<string, object[]> = {};
    const permissions: Record<string, unknown>[] = [];
    const opened = Promise.withResolvers<ReadableStreamDefaultController<Uint8Array>>();
    const prompt = Promise.withResolvers<{ session: string; body: unknown }>();
    const prompts: { session: string; body: unknown }[] = [];
    // (Down: the engine not answering yet, as just after the box starts.)
    const engineState = { down: false };
    const engine = Bun.serve({
      hostname: "127.0.0.1", port: 0, async fetch(request) {
        if (engineState.down) return Response.json({}, { status: 503 });
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
          const sent = { session: route.split("/")[3]!, body: await request.json() };
          prompts.push(sent);
          prompt.resolve(sent);
          return data({});
        }
        if (route.endsWith("/message")) return data(transcript[route.split("/")[3]!] ?? []);
        if (route.endsWith("/permission")) return data(permissions);
        if (route.endsWith("/inbox") || route.endsWith("/form")) return data([]);
        if (route === "/api/session") return data(Object.values(cases));
        if (route === "/api/session/active") return data([]);
        if (request.method === "DELETE" && route.startsWith("/api/session/")) {
          delete cases[route.split("/")[3]!];
          return data(true);
        }
        if (route.startsWith("/api/session/")) return data(cases[route.split("/")[3]!]);
        if (route === "/api/provider") return data([{ id: model.providerID }]);
        if (route === "/api/model") return data([{ ...model, name: model.id }]);
        if (route === "/api/integration") return data([{ id: model.providerID, connections: [{}] }]);
        if (route === "/api/model/default") return data(model);
        return Response.json({}, { status: 404 });
      },
    });
    const models = (bridgeDelivery ? agentProviders : ["codex-cli"]).map(providerID => ({
      providerID, id: `test-${providerID.replace(/-cli$/, "")}`, name: providerID === "codex-cli" ? "Test Codex" : providerID,
    }));
    const bridgeCases = Object.fromEntries(bridgeDelivery ? agentNames.map((agent, index) => [
      codingSession(agent), { id: codingSession(agent), title: `Session on ${agent}`, time, directory: project, model: models[index] },
    ]) : []);
    const bridgePrompts: { session: string; body: unknown }[] = [];
    const bridge = i === 1 && codingAgents ? Bun.serve({
      hostname: "127.0.0.1", port: 0, async fetch(request) {
        const route = new URL(request.url).pathname;
        const data = (value: unknown) => Response.json({ data: value });
        if (route === "/api/model") return data({ models });
        if (route === "/api/integration") return data(models.map((model, index) => ({
          id: model.providerID, name: model.providerID === "codex-cli" ? "Codex" : model.providerID, connections: [{}],
          metadata: { source: "agent", agent: bridgeDelivery ? agentNames[index] : "codex", accountID: "default", modelsComplete: true },
        })));
        if (route === "/api/kuvo/auto") return data({ lists: [] });
        if (route === "/api/session") return data(Object.values(bridgeCases));
        if (route.endsWith("/prompt") && request.method === "POST") {
          bridgePrompts.push({ session: route.split("/")[3]!, body: await request.json() });
          return data({});
        }
        if (route === "/api/session/active" || route.endsWith("/inbox") || route.endsWith("/message")) return data([]);
        if (route.startsWith("/api/session/")) {
          const session = bridgeCases[route.split("/")[3]!];
          return session ? data(session) : Response.json({}, { status: 404 });
        }
        return data([]);
      },
    }) : null;
    mkdirSync(project, { recursive: true });
    upgradeKuvoFolder(home);
    const run = async (command: object): Promise<any> => {
      const child = Bun.spawn([process.execPath, worker, "kuvo-agents"], {
        env: { ...process.env, KUVO_HOME: home, HOME: root, TMPDIR: root },
        stdin: Buffer.from(JSON.stringify({ command, port: engine.port, bridgePort: bridge?.port, withLibrary, withOutputs })), stdout: "pipe", stderr: "pipe",
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
      stdin: Buffer.from(JSON.stringify({ command: { kind: "conversation" }, port: engine.port, bridgePort: bridge?.port })),
      stdout: "ignore", stderr: "inherit", ipc,
    });
    return { key, secret, home, computer, peer, model, session, cases, run, read, engine, engineState, bridge, linked, opened, prompt, prompts, bridgePrompts, permissions, transcript };
  });
  const setup = (managed ? "set_" : "setup_") + "a".repeat(32);
  for (const box of boxes) {
    const run = path.join(box.home, "run");
    mkdirSync(run, { recursive: true });
    writeFileSync(path.join(run, "engine.json"), JSON.stringify({ port: box.engine.port, password: "test" }));
    const peers = boxes.filter(other => other !== box);
    writeFileSync(path.join(run, "link.json"), JSON.stringify({
      formatVersion: 1, key: box.key.toString("base64url"), secret: box.secret.toString("base64url"), name: box.computer.name,
      phones: [{ key: keyOf(device), name: "Desktop", from: "pair", kind: "desktop", pairing: "test" }],
      setup: { id: setup, role: "member", primary: boxes[0]!.computer.id, checkpointed: true, ...(managed ? { managed: true, epoch: 1 } : {}) },
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

test("a new conversation opens before it appears in the list", async () => {
  const setup = pair();
  try {
    // The engine has created the conversation, but has sent no list or event since it did.
    await share(setup.near, setup.far);
    const { started, location } = await setup.near.run({ kind: "start-and-locate" });
    expect(started).toBe(200);
    expect(location.status).toBe(200);
    expect(JSON.parse(location.text)).toEqual({ place: "engine", running: false });
  } finally { setup.close(); }
}, 30_000);

test("a conversation only known from a peer list opens on its computer", async () => {
  const setup = pair(true, true);
  const { near, far } = setup;
  let a: ReturnType<Box["linked"]> | undefined;
  let b: ReturnType<Box["linked"]> | undefined;
  const nearReady = Promise.withResolvers<void>();
  const farReady = Promise.withResolvers<void>();
  const replies = new Map<string, ReturnType<typeof Promise.withResolvers<{ status: number; text: string }>>>();
  try {
    await share(far, near);
    a = near.linked(message => {
      if (message.ready) nearReady.resolve();
      if (message.frame) b!.send({ frame: message.frame });
      if (message.failure) for (const reply of replies.values()) reply.reject(new Error(message.failure));
      if (message.replyTo) replies.get(message.replyTo)?.resolve(message.routeAnswer);
    });
    b = far.linked(message => {
      if (message.ready) farReady.resolve();
      if (message.frame) a!.send({ frame: message.frame });
    });
    await Promise.all([nearReady.promise, farReady.promise]);
    for (const id of [far.session, ...agentNames.map(codingSession)]) {
      const read = (path: string) => {
        const reply = Promise.withResolvers<{ status: number; text: string }>();
        replies.set(path, reply);
        a!.send({ call: { path }, replyTo: path });
        return reply.promise;
      };
      const found = await read(`kuvo/sessions/${id}/location`);
      expect(found.status).toBe(200);
      const { on } = JSON.parse(found.text);
      expect(on).toEqual({ ...far.computer, away: false, actions: ["open", "manage"] });
      const runtime = await read(`kuvo/peer-session/${on.id}/${id}/location`);
      expect(runtime.status).toBe(200);
      expect(JSON.parse(runtime.text)).toEqual({ place: id === far.session ? "engine" : "agents", running: false });
    }
  } finally {
    a?.kill();
    b?.kill();
    await Promise.all([a?.exited, b?.exited]);
    setup.close();
  }
}, 30_000);

test("a conversation on an away computer is still known", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near);
    await awayAWhile(near, far);
    const found = await near.read(`kuvo/sessions/${far.session}/location`);
    expect(found.status).toBe(200);
    expect(JSON.parse(found.text)).toEqual({ on: { ...far.computer, away: true, lastSeen: expect.any(Number), actions: [] } });
  } finally { setup.close(); }
}, 30_000);

test("a conversation absent from the setup is unknown", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near);
    await share(near, far);
    const found = await near.read("kuvo/sessions/ses_unknown/location");
    expect(found.status).toBe(404);
    expect(JSON.parse(found.text)).toEqual({ message: "That conversation isn’t there any more." });
  } finally { setup.close(); }
}, 30_000);

/** `far` went off a while ago, as `near` knows it: the relay found it not there two minutes ago,
 * and its last copies came then. (Quiet under a minute, a computer is just live.) */
async function awayAWhile(near: Box, far: Box, at = Date.now() - 2 * 60_000) {
  const folder = path.join(near.home, "run", "peers", far.computer.id);
  for (const name of ["lists-v1.json", "library-v1.json", "notices-v1.json", "outputs-v1.json", "archives-v1.json"]) {
    const file = path.join(folder, name);
    if (existsSync(file)) writeFileSync(file, JSON.stringify({ ...JSON.parse(readFileSync(file, "utf8")), seen: at }));
  }
  return near.run({ kind: "away", address: far.peer.address, at });
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
    const farProject = far.computer.id + ":" + template.directory;
    expect(Object.keys(merged.listing.sessions).sort()).toEqual([near.session, far.session, ...more].sort());
    expect(merged.listing.sessions[near.session].computer).toEqual(near.computer);
    expect(merged.listing.sessions[far.session]).toMatchObject({ computer: far.computer, on: { ...far.computer, actions: ["open", "manage"] }, project: farProject });
    expect(merged.listing.projects[farProject]).toMatchObject({ name: "project", computer: far.computer });
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
    // Quiet under a minute, it's just live: nothing greys, and no row says "seen now".
    expect(await near.run({ kind: "away", address: far.peer.address })).toBe(true);
    const blip = (await line(near)).sessions.sessions.find((row: any) => row.id === far.session);
    expect(blip).toMatchObject({ computer: far.computer, away: false, actions: ["open", "manage"] });
    expect(blip.lastSeen).toBeUndefined();
    // Last heard from over a minute ago: away.
    const lists = path.join(near.home, "run", "peers", far.computer.id, "lists-v1.json");
    writeFileSync(lists, JSON.stringify({ ...JSON.parse(readFileSync(lists, "utf8")), seen: Date.now() - 2 * 60_000 }));
    rmSync(path.join(near.home, "run", "peer-reach.json"));
    expect(await near.run({ kind: "away", address: far.peer.address, at: Date.now() - 2 * 60_000 })).toBe(true);
    const merged = await line(near);
    const session = merged.sessions.sessions.find((row: any) => row.id === far.session);
    expect(session).toMatchObject({ computer: far.computer, away: true, lastSeen: expect.any(Number), actions: [] });
    expect(session.updating).toBeUndefined();
    const away = { computer: far.computer, session: far.session, away: true, lastSeen: session.lastSeen, actions: [] };
    expect(merged.waiting.waiting).toEqual([expect.objectContaining(away)]);
    expect(merged.queues.queues).toEqual([expect.objectContaining(away)]);
    // Runs on still lists it, away since it was last seen, so it's shown and not picked.
    expect(merged.runs.computers.map((row: any) => [row.computer, row.here ?? false, row.away ?? false, row.lastSeen]))
      .toEqual([[near.computer, true, false, undefined], [far.computer, false, true, session.lastSeen]]);
    expect(fitsShape(RUNS_ON, merged.runs)).toBe(true);
  } finally { setup.close(); }
}, 30_000);

test("the sidebar pages a peer's worktree sessions under their project, even while that peer is away", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    const template = far.cases[far.session]!;
    const project = template.directory!;
    const worktree = path.join(far.home, "worktree");
    const git = (...args: string[]) => {
      const result = Bun.spawnSync(["git", "-C", project, ...args]);
      if (result.exitCode !== 0) throw new Error(result.stderr.toString());
    };
    git("init");
    git("-c", "user.name=Test", "-c", "user.email=test@example.test", "commit", "--allow-empty", "-m", "Initial");
    git("worktree", "add", "--detach", worktree);
    const ids = Array.from({ length: 17 }, (_, i) => {
      const id = `ses_worktree${i}`;
      far.cases[id] = { ...template, id, directory: worktree, time: { ...template.time, updated: 1000 - i } };
      return id;
    }).concat(far.session);
    const archived = "ses_archived_worktree";
    far.cases[archived] = { ...template, id: archived, directory: worktree, time: { ...template.time, archived: 900 } };
    await share(far, near);
    expect(await awayAWhile(near, far)).toBe(true);

    const { listing } = await line(near);
    const key = `${far.computer.id}:${project}`;
    expect(listing.sessions[ids[0]!]).toMatchObject({ directory: worktree, project: key, on: { ...far.computer, away: true } });
    expect(listing.projects[key]).toMatchObject({ shown: ids.slice(0, 6), total: 18, more: { count: 10 } });
    expect(listing.projects[`${far.computer.id}:${worktree}`]).toBeUndefined();
    expect(listing.archivedGroups).toContainEqual({ directory: key, title: "project", kind: "project", sessions: [archived] });
    const page = async (cursor: string) => {
      const answer = await near.read(`kuvo/sessions/page?${new URLSearchParams({ project: key, cursor })}`);
      expect(answer.status).toBe(200);
      return JSON.parse(answer.text);
    };
    const next = await page(listing.projects[key].more.cursor);
    expect(next).toMatchObject({ project: key, sessions: ids.slice(0, 16), total: 18, more: { count: 2 } });
    expect(await page(next.more.cursor)).toEqual({ project: key, sessions: ids, total: 18, more: null });
  } finally { setup.close(); }
}, 30_000);

test("the sidebar shows a peer's saved sessions while this computer has no local list yet", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near);
    near.engineState.down = true;
    const watch = Buffer.from(JSON.stringify({ listing: targets.listing })).toString("base64url");
    const answer = await near.read(`kuvo/line?once=1&watch=${watch}`);
    expect(answer.status).toBe(200);
    const listing = JSON.parse(answer.text.trim().split("\n")[1]!).d;
    expect(Object.keys(listing?.sessions ?? {})).toEqual([far.session]);
    expect(fitsShape(SESSION_LIST, listing)).toBe(true);
    expect(listing.startProject).toBeNull();
  } finally { setup.close(); }
}, 30_000);

test("Runs on lists this computer's models and its peers' at once after a restart, while its engine and peers are silent", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near);
    const runsOn = async () => {
      const watch = Buffer.from(JSON.stringify({ runs: targets.runs })).toString("base64url");
      const { status, text } = await near.read(`kuvo/line?once=1&watch=${watch}`);
      expect(status).toBe(200);
      const runs = text.trim().split("\n").map((line: string) => JSON.parse(line)).find((row: any) => row.t === "runs")!.d;
      expect(fitsShape(RUNS_ON, runs)).toBe(true);
      return runs.computers.map((row: any) => [row.computer.name, row.here ?? false, row.pending ?? false, row.offered.models.map((model: any) => model.id)]);
    };
    expect(await runsOn()).toEqual([["near", true, false, [near.model.id]], ["far", false, false, [far.model.id]]]);
    // Started again (each read is a new process), its engine not answering yet and the other
    // computer silent: both are still listed with their models, its own as last read.
    near.engineState.down = true;
    expect(await runsOn()).toEqual([["near", true, true, [near.model.id]], ["far", false, false, [far.model.id]]]);
  } finally { setup.close(); }
}, 30_000);

test("a computer that comes back after being away has its rows live again at once, and is listed connected", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near, far.session);
    // Off for two hours, with its bot Mira: last heard from then, and found away by the relay.
    const told = path.join(near.home, "run", "peer-agents.json");
    const heard = Date.now() - 2 * 3600_000;
    const mira = { kind: "bot", id: "bot_mira", name: "Mira", session: "ses_mira" };
    writeFileSync(told, JSON.stringify({ [far.peer.key]: { features, at: 100, heard, list: [mira] } }));
    expect(await near.run({ kind: "away", address: far.peer.address, at: heard })).toBe(true);
    const lists = path.join(near.home, "run", "peers", far.computer.id, "lists-v1.json");
    writeFileSync(lists, JSON.stringify({ ...JSON.parse(readFileSync(lists, "utf8")), seen: heard }));
    const away = (await line(near)).sessions.sessions.find((row: any) => row.id === far.session);
    expect(away).toMatchObject({ away: true, actions: [] });
    const reachOf = async () => (JSON.parse((await near.read("kuvo/computers")).text).computers as any[]).find(computer => computer.key === far.peer.key)?.reach;
    const botOf = async () => (JSON.parse((await near.read("kuvo/bots")).text).elsewhere as any[]).find(bot => bot.id === mira.id);
    expect(await reachOf()).toBe("away");
    expect(await botOf()).toMatchObject({ computer: "far", away: true, lastSeen: heard });
    // It's back: the first thing it mails says so, with nothing asked of you.
    await share(far, near, far.session);
    const back = (await line(near)).sessions.sessions.find((row: any) => row.id === far.session);
    expect(back).toMatchObject({ computer: far.computer, away: false, actions: ["open", "manage"] });
    expect(back.lastSeen).toBeUndefined();
    expect(await reachOf()).toBe("connected");
    const live = await botOf();
    expect(live).toMatchObject({ computer: "far" });
    expect(live.away).toBeUndefined();
  } finally { setup.close(); }
}, 30_000);

test("a computer switching to a new release isn't away: its rows stay live, said to be updating", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near, far.session);
    // It said it was switching to the release, then went quiet to restart into it.
    const told = path.join(near.home, "run", "peer-agents.json");
    writeFileSync(told, JSON.stringify({ [far.peer.key]: { features, at: 100, list: [], computer: {
      kuvo: "0.8.0-beta.43", fleetUpdate: { state: "updating", stage: "switching", current: "0.8.0-beta.43", target: "0.8.0-beta.44", canRetry: false },
    } } }));
    expect(await near.run({ kind: "away", address: far.peer.address })).toBe(true);
    const merged = await line(near);
    const row = merged.sessions.sessions.find((row: any) => row.id === far.session);
    expect(row).toMatchObject({ computer: far.computer, away: false, updating: true, actions: ["open", "manage"] });
    expect(row.lastSeen).toBeUndefined();
    expect(merged.waiting.waiting).toEqual([expect.objectContaining({ session: far.session, away: false, updating: true })]);
    // Its conversation is still located there, and opens: its kept copy, said to be updating.
    const found = await near.read(`kuvo/sessions/${far.session}/location`);
    expect(JSON.parse(found.text)).toEqual({ on: { ...far.computer, away: false, updating: true, actions: ["open", "manage"] } });
    // Its bots too: as they are, said to be updating, never greyed as away.
    writeFileSync(told, JSON.stringify({ [far.peer.key]: { features, at: 100, heard: Date.now() - 3 * 60_000, list: [{ kind: "bot", id: "bot_mira", name: "Mira", session: "ses_mira" }], computer: {
      kuvo: "0.8.0-beta.43", fleetUpdate: { state: "updating", stage: "switching", current: "0.8.0-beta.43", target: "0.8.0-beta.44", canRetry: false },
    } } }));
    const mira = (JSON.parse((await near.read("kuvo/bots")).text).elsewhere as any[]).find(bot => bot.id === "bot_mira");
    expect(mira).toMatchObject({ computer: "far", updating: true });
    expect(mira.away).toBeUndefined();
    expect(mira.lastSeen).toBeUndefined();
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

test("the nearest box serves a remote bot’s own activity on its bots line", async () => {
  const setup = pair();
  const { near, far } = setup;
  const bot = "bot_far";
  const folder = path.join(far.home, "bots", bot);
  try {
    mkdirSync(folder, { recursive: true });
    writeFileSync(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Remote bot" }));
    writeFileSync(path.join(folder, "session"), far.session);
    const directory = path.join(near.home, "run", "peer-agents.json");
    writeFileSync(directory, JSON.stringify({ [far.peer.key]: { features, at: 100, list: [
      { kind: "bot", id: bot, name: "Remote bot", session: far.session },
    ] } }));
    await share(far, near, far.session);
    const botTopic = Buffer.from(JSON.stringify({ bots: "kuvo/bots/events" })).toString("base64url");
    const bots = await near.read(`kuvo/line?once=1&watch=${botTopic}`);
    const state = bots.text.trim().split("\n").map(JSON.parse as (line: string) => any).find(row => row.t === "bots");
    expect(state.d.elsewhere).toEqual([expect.objectContaining({ id: bot, session: far.session, box: far.computer.id,
      now: expect.objectContaining({ state: "needsYou", asking: 1 }) })]);
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

    const location = await near.read(`kuvo/sessions/${far.session}/location`);
    expect(location.status).toBe(200);
    const owner = JSON.parse(location.text).on.id;
    const base = `kuvo/peer-session/${owner}/${far.session}`;
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

test("a followed conversation stays readable from the nearest box while its computer is away, and catches up when it's back", async () => {
  const setup = pair();
  const { near, far } = setup;
  let a: ReturnType<Box["linked"]> | undefined;
  let b: ReturnType<Box["linked"]> | undefined;
  const readyA = Promise.withResolvers<void>(), readyB = Promise.withResolvers<void>();
  const failed = Promise.withResolvers<never>();
  void failed.promise.catch(() => {});
  const reads: ReturnType<typeof Promise.withResolvers<any>>[] = [];
  let published = Promise.withResolvers<void>();
  // Away: what the near box mails it finds nothing there, and the relay says so.
  let away = false;
  let tried = 0;
  try {
    const picture = "data:image/png;base64," + "A".repeat(64 * 1024);
    // (Newest first, as the engine pages them.)
    const transcript = far.transcript[far.session] = [] as object[];
    transcript.push(
      { id: "msg_2", role: "assistant", text: "The build is green.", files: [{ mime: "image/png", url: picture }] },
      { id: "msg_1", role: "user", text: "Is the build green?" },
    );
    await share(far, near);
    a = near.linked(message => {
      if (message.failure) failed.reject(new Error(message.failure));
      if (message.ready) readyA.resolve();
      if (message.frame) {
        if (away) {
          tried++;
          a!.send({ unreachable: far.peer.address });
        } else b!.send({ frame: message.frame });
      }
      if (message.read) reads.shift()?.resolve(message.read);
    });
    b = far.linked(message => {
      if (message.failure) failed.reject(new Error(message.failure));
      if (message.ready) readyB.resolve();
      if (message.frame) a!.send({ frame: message.frame });
      if (message.published) published.resolve();
    });
    await Promise.all([readyA.promise, readyB.promise]);
    const read = async () => {
      const answer = Promise.withResolvers<any>();
      reads.push(answer);
      a!.send({ read: `kuvo/peer-session/${far.computer.id}/${far.session}/messages?limit=100&order=desc` });
      const { status, text } = await Promise.race([answer.promise, failed.promise]);
      return { status, page: JSON.parse(text) };
    };

    const live = await read();
    expect(live.status).toBe(200);
    expect(live.page.data.map((message: any) => message.text)).toEqual(["The build is green.", "Is the build green?"]);
    expect(live.page.away).toBeUndefined();

    // It restarts: the same transcript, kept without its inline picture, said as its last copy.
    away = true;
    const kept = await read();
    expect(kept.status).toBe(200);
    expect(kept.page.data.map((message: any) => message.text)).toEqual(["The build is green.", "Is the build green?"]);
    expect(kept.page.data[0].files).toEqual([{ mime: "image/png", url: "data:image/png;base64," }]);
    // (Quiet under a minute: no "seen now".)
    expect(kept.page.away).toEqual({ computer: { id: far.computer.id, name: "far" } });
    expect(tried).toBeGreaterThan(0);
    // Known away now: answered at once, nothing sent to wait on.
    const sent = tried;
    expect((await read()).page).toEqual(kept.page);
    expect(tried).toBe(sent);

    // Back, with a new answer: its mail says so, and the next read is its transcript as it is now.
    transcript.unshift({ id: "msg_3", role: "assistant", text: "Deployed too." });
    away = false;
    b.send({ publish: true });
    await Promise.race([published.promise, failed.promise]);
    const back = await read();
    expect(back.status).toBe(200);
    expect(back.page.data.map((message: any) => message.text)).toEqual(["Deployed too.", "The build is green.", "Is the build green?"]);
    expect(back.page.away).toBeUndefined();
  } finally {
    a?.kill();
    b?.kill();
    await Promise.all([a?.exited, b?.exited]);
    setup.close();
  }
}, 40_000);

test("five drafts wait two minutes for their computer and go once in order when it is back", async () => {
  const setup = pair(true, true);
  const { near, far } = setup;
  const replies = new Map<string, ReturnType<typeof Promise.withResolvers<any>>>();
  let a: ReturnType<Box["linked"]> | undefined;
  let b: ReturnType<Box["linked"]> | undefined;
  const readyA = Promise.withResolvers<void>(), readyB = Promise.withResolvers<void>();
  let away = true;
  let dropped = 0;
  let dropReply = false;
  const lostReceipt = Promise.withResolvers<void>();
  const failed = Promise.withResolvers<never>();
  // Each IPC request races the harness’s explicit error, so broken dispatch fails immediately.
  void failed.promise.catch(() => {});
  try {
    await share(far, near);
    const nearMessages = (ready: ReturnType<typeof Promise.withResolvers<void>>) => (message: any) => {
      if (message.failure) failed.reject(new Error(message.failure));
      if (message.ready) ready.resolve();
      if (message.frame) {
        if (away) dropped++;
        else b!.send({ frame: message.frame });
      }
      if (message.replyTo) replies.get(message.replyTo)?.resolve(message.routeAnswer);
    };
    a = near.linked(nearMessages(readyA));
    const farMessages = (ready: ReturnType<typeof Promise.withResolvers<void>>) => (message: any) => {
      if (message.ready) ready.resolve();
      if (message.failure) failed.reject(new Error(message.failure));
      if (message.frame && dropReply) {
        dropReply = false;
        lostReceipt.resolve();
      } else if (message.frame) a!.send({ frame: message.frame });
    };
    b = far.linked(farMessages(readyB));
    await Promise.all([readyA.promise, readyB.promise]);
    let request = 0;
    const ask = async (command: object) => {
      const replyTo = String(++request);
      const reply = Promise.withResolvers<any>();
      replies.set(replyTo, reply);
      a!.send({ ...command, replyTo });
      try { return await Promise.race([reply.promise, failed.promise]); }
      finally { replies.delete(replyTo); }
    };
    const route = `kuvo/sessions/${far.session}/delivery?computer=${far.computer.id}`;
    const clock = 1_791_000_000_000;
    const receipts = [];
    for (let n = 1; n <= 5; n++) {
      const draft = { id: `msg_wait${n}`, text: `Message ${n}`, message: { delivery: "queue" } };
      const accepted = await ask({ call: { path: route, body: draft } });
      expect(accepted.status).toBe(202);
      receipts.push(JSON.parse(accepted.text).operation);
      expect(await ask({ call: { path: route, body: draft } })).toEqual(accepted);
    }
    const draft = { id: "msg_drop", text: "Remove this draft", message: { delivery: "queue" } };
    expect((await ask({ call: { path: route, body: draft } })).status).toBe(202);
    const base = route.split("?")[0];
    const selected = `${base}/msg_drop?computer=${far.computer.id}&receipt=edit-fixture`;
    const removal = await ask({ call: { method: "DELETE", path: selected } });
    expect(removal.status).toBe(200);
    expect(JSON.parse(removal.text).removed).toEqual({ id: draft.id, draft: draft.text, words: draft.text, files: [], receipt: "edit-fixture" });
    expect(await ask({ call: { method: "DELETE", path: selected } })).toEqual(removal);
    expect((await ask({ call: { path: `${base}/msg_drop/ack?computer=${far.computer.id}&receipt=edit-fixture`, body: {} } })).status).toBe(200);
    expect((await ask({ call: { path: route, body: draft } })).status).toBe(409);
    expect((await ask({ call: { path: `${base}/msg_wait3/steer?computer=${far.computer.id}`, body: {} } })).status).toBe(200);

    for (let elapsed = 0; elapsed < 60_000; elapsed += 5_000) await ask({ round: clock + elapsed });
    a.kill();
    await a.exited;
    const restarted = Promise.withResolvers<void>();
    a = near.linked(nearMessages(restarted));
    await restarted.promise;
    const waiting = await ask({ call: { path: route } });
    expect(JSON.parse(waiting.text).items.map((item: any) => item.id)).toEqual(Array.from({ length: 5 }, (_, i) => `msg_wait${i + 1}`));
    for (let elapsed = 60_000; elapsed < 120_000; elapsed += 5_000) await ask({ round: clock + elapsed });
    expect(dropped).toBe(24);
    expect(far.prompts).toEqual([]);
    const queueTopic = Buffer.from(JSON.stringify({
      queue: `${base}/events?computer=${far.computer.id}`,
    })).toString("base64url");
    const followed = await ask({ call: { path: `kuvo/line?once=1&watch=${queueTopic}` } });
    const queueState = followed.text.trim().split("\n").map(JSON.parse as (line: string) => any).find((row: any) => row.t === "queue");
    expect(queueState.d.items.map((item: any) => item.id))
      .toEqual(Array.from({ length: 5 }, (_, i) => `msg_wait${i + 1}`));
    away = false;
    dropReply = true;
    await ask({ round: clock + 120_000 });
    await Promise.race([lostReceipt.promise, failed.promise]);
    expect(far.prompts).toHaveLength(1);
    // The owner also restarts after accepting the first draft, before it hears the retry.
    b.kill();
    await b.exited;
    const ownerBack = Promise.withResolvers<void>();
    b = far.linked(farMessages(ownerBack));
    await ownerBack.promise;
    for (const [i, operation] of receipts.entries()) {
      const receipt = await ask({ round: clock + 125_000 + i * 5_000, receipt: operation });
      expect(receipt).toMatchObject({ operation, status: "committed", owner: far.computer.id });
    }
    await ask({ round: clock + 150_000 });
    expect(far.prompts.map(({ session, body }: any) => [session, body.id, body.text.trim(), body.delivery]))
      .toEqual(Array.from({ length: 5 }, (_, i) => [far.session, `msg_wait${i + 1}`, `Message ${i + 1}`, i === 2 ? "steer" : "queue"]));

    // The owner picks its native adapter box-side. All five coding agents use the bridge API,
    // while the five prompts above prove the same path through the engine.
    for (const [i, agent] of agentNames.entries()) {
      const session = codingSession(agent);
      const id = `msg_agent${i}`;
      const accepted = await ask({ call: {
        path: `kuvo/sessions/${session}/delivery?computer=${far.computer.id}`, body: { id, text: agent, message: { delivery: "steer" } },
      } });
      expect(accepted.status).toBe(202);
      const { operation } = JSON.parse(accepted.text);
      expect(await ask({ round: clock + 160_000 + i * 5_000, receipt: operation }))
        .toMatchObject({ operation, status: "committed", owner: far.computer.id });
    }
    expect(far.bridgePrompts.map(({ session, body }: any) => [session, body.id, body.text.trim()]))
      .toEqual(agentNames.map((agent, i) => [codingSession(agent), `msg_agent${i}`, agent]));
    expect(far.prompts).toHaveLength(5);

    const local = await ask({ call: {
      path: `kuvo/sessions/${near.session}/delivery`, body: { id: "msg_local", text: "Keep this here", message: { delivery: "queue" } },
    } });
    expect(local.status).toBe(202);
    await ask({ round: clock + 190_000 });
    expect(await Promise.race([near.prompt.promise, failed.promise])).toMatchObject({
      session: near.session, body: { id: "msg_local", text: expect.stringContaining("Keep this here") },
    });
  } finally {
    a?.kill();
    b?.kill();
    await Promise.all([a?.exited, b?.exited]);
    setup.close();
  }
}, 40_000);

test("the Library’s first state includes an away computer’s kept files even on a never-opened bot tab", async () => {
  const setup = pair(false, false, { withLibrary: true });
  const { near, far } = setup;
  try {
    await share(far, near);
    const library = "kuvo/setup/library/events?source=bot_ada&kind=document&q=brief";
    const watch = Buffer.from(JSON.stringify({ library })).toString("base64url");
    await awayAWhile(near, far);
    const { status, text } = await near.run({ kind: "route", path: `kuvo/line?once=1&watch=${watch}` });
    expect(status).toBe(200);
    const [, first] = text.trim().split("\n").map(JSON.parse as (line: string) => any);
    expect(first).toMatchObject({ t: "library", d: {
      items: [
        { name: "brief.pdf", there: false, computer: { key: far.computer.id, name: "far" } },
        { name: "brief.pdf", there: true, computer: { key: near.computer.id, name: "near", here: true } },
      ],
      computers: [
        { key: near.computer.id, state: "here" },
        { key: far.computer.id, state: "away", seen: expect.any(Number) },
      ],
    } });
  } finally { setup.close(); }
});

test("the rail’s first waiting count includes a peer’s kept ask", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near, far.session);
    const topic = "kuvo/setup/waiting/counts/events";
    const watch = Buffer.from(JSON.stringify({ waiting: topic })).toString("base64url");
    const answer = await near.read(`kuvo/line?once=1&watch=${watch}`);
    expect(answer.status).toBe(200);
    const [, first] = answer.text.trim().split("\n").map(JSON.parse as (line: string) => any);
    expect(first).toMatchObject({ t: "waiting", d: { sessions: 1, bots: 0, queue: 0 } });
  } finally { setup.close(); }
});

test("the sidebar’s first Needs you list includes a peer’s kept ask", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await share(far, near, far.session);
    const listing = await line(near);
    expect(listing.listing.needsYou).toEqual([{
      session: far.session, asks: { kind: "permission", summary: "asks to run a command" },
    }]);
  } finally { setup.close(); }
});

test("the Work Queue’s first Deleted list keeps items from a computer that is away", async () => {
  const setup = pair(false, false, { managed: true });
  const { near, far } = setup;
  try {
    const id = "wqi_labitemlabitemlabi02";
    const at = Date.now();
    const stamp = { at, by: { user: true } };
    const queues = path.join(far.home, "run", "work-queue", "queues");
    mkdirSync(queues, { recursive: true });
    writeFileSync(path.join(queues, `${far.session}.json`), JSON.stringify({ owner: far.session, items: [{
      id, owner: far.session, title: "Drop the old importer", status: "todo", order: 0, priority: "normal",
      labels: [], notes: [], files: [], links: [], sessions: [], created: stamp, updated: stamp, removed: stamp,
    }] }));
    const frames = await far.run({ kind: "publish-queue" });
    expect(frames.length).toBeGreaterThan(0);
    expect(await near.run({ kind: "receive", frames })).toBe(true);
    const watch = Buffer.from(JSON.stringify({ queue: "kuvo/work-queue/events?owner=all&glance=1&counts=1" })).toString("base64url");
    const answer = await near.run({ kind: "route", path: `kuvo/line?once=1&watch=${watch}`, away: far.peer.address });
    expect(answer.status).toBe(200);
    const [, first] = answer.text.trim().split("\n").map(JSON.parse as (line: string) => any);
    expect(first).toMatchObject({ t: "queue", d: {
      items: [], counts: { removed: 1 }, computers: [{ key: far.computer.id }], removed: [{
        id, title: "Drop the old importer", computer: { key: far.computer.id, name: "far" },
        removed: stamp, can: { restore: false, edit: false, delete: false },
      }],
    } });
  } finally { setup.close(); }
});

test("setup attention lists the original computer of a kept notice and counts its ask once", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    const folder = path.join(far.home, "run", "notices");
    mkdirSync(folder, { recursive: true });
    writeFileSync(path.join(folder, "told.json"), JSON.stringify([{
      id: "ntc_far", at: Date.now(), notice: { category: "finished", data: { session: far.session } },
    }]));
    await share(far, near, far.session);
    const watch = Buffer.from(JSON.stringify({
      notices: "kuvo/setup/notices/events", waiting: "kuvo/setup/waiting/counts/events",
    })).toString("base64url");
    await awayAWhile(near, far);
    const answer = await near.run({ kind: "route", path: `kuvo/line?once=1&watch=${watch}` });
    expect(answer.status).toBe(200);
    const [, ...frames] = answer.text.trim().split("\n").map(JSON.parse as (line: string) => any);
    const states = Object.fromEntries(frames.map(frame => [frame.t, frame.d]));
    expect(states.notices?.notices).toEqual([expect.objectContaining({
      id: "ntc_far", computer: far.computer, away: true, lastSeen: expect.any(Number),
      notice: { category: "finished", data: { session: far.session } },
    })]);
    expect(states.waiting).toMatchObject({ sessions: 1, bots: 0 });
  } finally { setup.close(); }
});

test("a delegated session stays in Activity as last seen when its computer goes away", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    const kept = path.join(near.home, "run", "activity");
    mkdirSync(kept, { recursive: true });
    // (Last heard two minutes ago: away, by the one rule.)
    const heard = Date.now() - 2 * 60_000;
    writeFileSync(path.join(kept, far.peer.key + ".json"), JSON.stringify({ at: heard, heard, rows: [{
      session: far.session, state: "working", title: "Delegated work", line: [near.session],
    }] }));
    await awayAWhile(near, far, heard);
    const answer = await near.run({ kind: "route", path: `kuvo/work/background/${near.session}` });
    expect(answer.status).toBe(200);
    expect(JSON.parse(answer.text).activity).toEqual({
      working: 0, asking: 0, sessions: [],
      lastSeenSessions: [{ session: far.session, title: "Delegated work", state: "working", computer: far.peer.key, lastSeen: heard }],
    });
  } finally { setup.close(); }
}, 30_000);

test("Activity's first Outputs state includes the kept file made by a delegated session on another computer", async () => {
  const setup = pair(false, false, { withOutputs: true });
  const { near, far } = setup;
  try {
    Object.assign(far.cases[far.session]!, { parentID: near.session });
    await share(far, near);
    const outputs = `kuvo/setup/outputs/events?session=${near.session}`;
    const watch = Buffer.from(JSON.stringify({ outputs })).toString("base64url");
    await awayAWhile(near, far);
    const answer = await near.run({ kind: "route", path: `kuvo/line?once=1&watch=${watch}` });
    expect(answer.status).toBe(200);
    const [, first] = answer.text.trim().split("\n").map(JSON.parse as (line: string) => any);
    expect(first).toMatchObject({ t: "outputs", d: { artifacts: [
      { name: "delegated.txt", computer: far.computer, away: true, lastSeen: expect.any(Number) },
    ], next: null } });
  } finally { setup.close(); }
}, 30_000);

test("opening a remote bot's New tab gets the bot's folder and tools from its owner", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    const bot = "bot_far";
    const folder = path.join(far.home, "bots", bot);
    mkdirSync(folder, { recursive: true });
    writeFileSync(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: bot, name: "Remote bot" }));
    writeFileSync(path.join(folder, "session"), far.session);

    // Its owner advertises live browser, filesystem, Changes and Terminal support over peer mail.
    const directory = path.join(far.home, "project");
    writeFileSync(path.join(near.home, "run", "peer-agents.json"), JSON.stringify({
      [far.computer.id]: { list: [], at: 100, features: [...features, "browser.view", "folders.files", "git.status", "pty.socket"] },
    }));
    await share(far, near);
    const found = await near.read(`kuvo/sessions/${far.session}/location?tools=1`);
    expect(found.status).toBe(200);
    expect(JSON.parse(found.text)).toMatchObject({
      on: far.computer,
      tools: { directory, ready: { files: true, browser: true, changes: true, terminal: true } },
    });
  } finally { setup.close(); }
}, 30_000);

test("Settings Archived Sessions shows the kept owner rows with that computer's confirmation", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    far.cases[far.session]!.time.archived = 300;
    await share(far, near);
    const owner = JSON.parse((await far.read("kuvo/settings/archived")).text);
    const confirm = owner.groups[0].rows[0].actions[0].confirm;
    expect(confirm).toEqual({ title: "Delete “Session on far”?", body: "This can’t be undone.", button: "Delete" });
    const watch = Buffer.from(JSON.stringify({ archive: "kuvo/settings/archived/events" })).toString("base64url");
    const answer = await near.read(`kuvo/line?once=1&watch=${watch}`);
    expect(answer.status).toBe(200);
    const [, first] = answer.text.trim().split("\n").map(JSON.parse as (line: string) => any);
    const rows = first.d.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])]));
    expect(rows).toContainEqual(expect.objectContaining({
      id: `session-${far.computer.id}:${far.session}`,
      title: "Session on far", computer: far.computer,
      actions: expect.arrayContaining([expect.objectContaining({ id: "delete", confirm: expect.objectContaining({ button: "Delete" }) })]),
    }));
    expect(rows).toContainEqual(expect.objectContaining({
      id: `delete-all-${far.computer.id}`, computer: far.computer,
      actions: expect.arrayContaining([expect.objectContaining({ id: "delete-all", confirm })]),
    }));
  } finally { setup.close(); }
}, 30_000);

test("search finds words on a reachable computer without copying its transcript", async () => {
  const setup = pair();
  const { near, far } = setup;
  const aReady = Promise.withResolvers<void>();
  const bReady = Promise.withResolvers<void>();
  const answer = Promise.withResolvers<{ status: number; text: string }>();
  let a: ReturnType<Box["linked"]> | undefined;
  let b: ReturnType<Box["linked"]> | undefined;
  try {
    far.transcript[far.session] = [{ id: "msg_remote", type: "user", text: "the plum orchard", time: { created: 1 } }];
    a = near.linked(message => {
      if (message.ready) aReady.resolve();
      if (message.failure) answer.reject(new Error(message.failure));
      if (message.frame) b!.send({ frame: message.frame });
      if (message.replyTo === "search") answer.resolve(message.routeAnswer);
    });
    b = far.linked(message => {
      if (message.ready) bReady.resolve();
      if (message.failure) answer.reject(new Error(message.failure));
      if (message.frame) a!.send({ frame: message.frame });
    });
    await Promise.all([aReady.promise, bReady.promise]);
    a.send({ call: { path: "kuvo/setup/search?q=plum" }, replyTo: "search" });
    const found = await answer.promise;
    expect(found.status).toBe(200);
    expect(JSON.parse(found.text)).toMatchObject({ results: [
      { session: far.session, snippet: expect.stringContaining("plum orchard"), computer: far.computer },
    ], awayComputers: [], unavailableComputers: [] });
    expect(existsSync(path.join(near.home, "search", `engine.${far.session}.json`))).toBe(false);
  } finally {
    a?.kill(); b?.kill();
    await Promise.all([a?.exited, b?.exited]);
    setup.close();
  }
}, 30_000);

test("search reports the computer it could not search because it was away", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    await awayAWhile(near, far);
    const found = await near.run({ kind: "route", path: "kuvo/setup/search?q=plum" });
    expect(found.status).toBe(200);
    expect(JSON.parse(found.text)).toEqual({ results: [], awayComputers: [far.computer], unavailableComputers: [] });
  } finally { setup.close(); }
}, 30_000);

test("merged views name a computer until its first copy arrives and distinguish an away computer", async () => {
  const setup = pair(false, false, { withLibrary: true, managed: true });
  const { near, far } = setup;
  const topics = {
    ...targets, library: "kuvo/setup/library/events", counts: "kuvo/setup/waiting/counts/events",
    notices: "kuvo/setup/notices/events", moves: "kuvo/moves/events", archive: "kuvo/settings/archived/events", bots: "kuvo/bots/events", queue: "kuvo/work-queue/events?owner=all",
  };
  const watch = Buffer.from(JSON.stringify(topics)).toString("base64url");
  const read = async (away?: string) => {
    const { status, text } = await near.run({ kind: "route", path: `kuvo/line?once=1&watch=${watch}`, away });
    expect(status).toBe(200);
    const [, ...states] = text.trim().split("\n").map(JSON.parse as (line: string) => any);
    expect(states.map(topic => topic.t).sort()).toEqual(Object.keys(topics).sort());
    for (const state of states) expect(state).not.toHaveProperty("refused");
    return Object.fromEntries(states.map(topic => [topic.t, topic.d]));
  };
  try {
    const pending = await read();
    for (const state of Object.values(pending)) expect(state.pendingComputers).toEqual([far.computer]);
    await awayAWhile(near, far);
    const away = await read();
    for (const state of Object.values(away)) expect(state.pendingComputers).toEqual([]);
    expect(pending.library.items).toEqual([]);
    expect(away.library.computers).toContainEqual(expect.objectContaining({ key: far.computer.id, state: "away" }));

    await share(far, near, far.session);
    const queues = await far.run({ kind: "publish-queue" });
    expect(await near.run({ kind: "receive", frames: queues })).toBe(true);
    const received = await read();
    for (const state of Object.values(received)) expect(state.pendingComputers).toEqual([]);
    expect(received.listing.sessions[far.session].computer).toEqual(far.computer);
    expect(received.waiting.waiting.map((row: any) => row.session)).toEqual([far.session]);
    expect(received.library.items.map((row: any) => row.computer.key)).toContain(far.computer.id);
  } finally { setup.close(); }
}, 30_000);

test("deleting an archived session from the setup page only changes its owner", async () => {
  const setup = pair(false, false, { managed: true });
  const { near, far } = setup;
  const aReady = Promise.withResolvers<void>();
  const bReady = Promise.withResolvers<void>();
  const answer = Promise.withResolvers<{ status: number; text: string }>();
  let a: ReturnType<Box["linked"]> | undefined;
  let b: ReturnType<Box["linked"]> | undefined;
  try {
    near.cases[near.session]!.time.archived = 300;
    far.cases[far.session]!.time.archived = 300;
    await share(far, near);
    const page = JSON.parse((await near.read("kuvo/settings/archived")).text);
    const rows = page.groups.flatMap((group: any) => group.rows.flatMap((row: any) => [row, ...(row.items ?? [])]));
    const archived = rows.find((row: any) => row.id === `session-${far.computer.id}:${far.session}`);
    expect(archived.computer).toEqual(far.computer);
    a = near.linked(message => {
      if (message.ready) aReady.resolve();
      if (message.failure) answer.reject(new Error(message.failure));
      if (message.frame) b!.send({ frame: message.frame });
      if (message.replyTo === "delete") answer.resolve(message.routeAnswer);
    });
    b = far.linked(message => {
      if (message.ready) bReady.resolve();
      if (message.failure) answer.reject(new Error(message.failure));
      if (message.frame) a!.send({ frame: message.frame });
    });
    await Promise.all([aReady.promise, bReady.promise]);
    a.send({ call: {
      path: `kuvo/settings/archived/${encodeURIComponent(archived.id)}/delete`,
      body: { was: archived.revision },
    }, replyTo: "delete" });
    const deleted = await answer.promise;
    expect([deleted.status, Object.keys(near.cases), Object.keys(far.cases)]).toEqual([200, [near.session], []]);
  } finally {
    a?.kill(); b?.kill();
    await Promise.all([a?.exited, b?.exited]);
    setup.close();
  }
}, 45_000);

test("a remote bot's tools are resolved on its owner before the first session copy", async () => {
  const setup = pair();
  const { near, far } = setup;
  const aReady = Promise.withResolvers<void>();
  const bReady = Promise.withResolvers<void>();
  const answer = Promise.withResolvers<{ status: number; text: string }>();
  let a: ReturnType<Box["linked"]> | undefined;
  let b: ReturnType<Box["linked"]> | undefined;
  try {
    const directory = path.join(far.home, "project");
    writeFileSync(path.join(near.home, "run", "peer-agents.json"), JSON.stringify({
      [far.computer.id]: { at: Date.now(), features, list: [
        { kind: "bot", id: "bot_far", name: "Remote bot", session: far.session },
      ] },
    }));
    a = near.linked(message => {
      if (message.ready) aReady.resolve();
      if (message.failure) answer.reject(new Error(message.failure));
      if (message.frame) b!.send({ frame: message.frame });
      if (message.replyTo === "tools") answer.resolve(message.routeAnswer);
    });
    b = far.linked(message => {
      if (message.ready) bReady.resolve();
      if (message.failure) answer.reject(new Error(message.failure));
      if (message.frame) a!.send({ frame: message.frame });
    });
    await Promise.all([aReady.promise, bReady.promise]);
    a.send({ call: { path: `kuvo/sessions/${far.session}/location?tools=1` }, replyTo: "tools" });
    const found = await answer.promise;
    expect(found.status).toBe(200);
    expect(JSON.parse(found.text)).toMatchObject({ on: far.computer, tools: {
      directory, ready: { files: true, browser: true, changes: true, terminal: true },
    } });
  } finally {
    a?.kill(); b?.kill();
    await Promise.all([a?.exited, b?.exited]);
    setup.close();
  }
}, 30_000);

test("the Dock counts a bot's own ask once when both the Sessions and Bots rails include it", async () => {
  const setup = pair();
  const { near, far } = setup;
  try {
    const folder = path.join(far.home, "bots", "bot_far");
    mkdirSync(folder, { recursive: true });
    writeFileSync(path.join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_far", name: "Remote bot" }));
    writeFileSync(path.join(folder, "session"), far.session);
    await share(far, near, far.session);
    const watch = Buffer.from(JSON.stringify({ waiting: "kuvo/setup/waiting/counts/events" })).toString("base64url");
    const answer = await near.read(`kuvo/line?once=1&watch=${watch}`);
    const [, first] = answer.text.trim().split("\n").map(JSON.parse as (line: string) => any);
    expect(first).toMatchObject({ t: "waiting", d: { sessions: 1, bots: 1, dock: 1 } });
  } finally { setup.close(); }
}, 30_000);
