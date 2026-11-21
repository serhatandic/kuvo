import "./safety";
import { afterAll, beforeAll, expect, spyOn, test } from "bun:test";
import fs from "node:fs";
import { K, ENGINE_FILE, LINK_FILE, RUN } from "../src/link/home";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { newKey, addressOf, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, signingKeyOf } from "../src/link/documents/format";
import { newSetupID } from "../src/link/setup/record";
import { readLink, writeLink } from "../src/link/state";
import { engineHandler } from "../src/link/server/router";
import { allItems, changeQueues, MOST_NOTE, newItemID, YOURS } from "../src/link/work-queue/store";
import { handUpkeep } from "../src/link/work-queue/hand";
import { handItem } from "../src/link/work-queue/ops";
import { packSessionNotes, placeSessionNotes, forgetSessionNotes } from "../src/link/ownership/session-notes";
import { editQueue } from "../src/link/work-queue/offline";
import { recordKey } from "../src/link/ownership/store";
import { mutationKey } from "../src/link/ownership/mutation";
import { sendOfflineEdits, takeOfflineEdit } from "../src/link/ownership/edits";
import { ask } from "./route-answer";

const handle = engineHandler();
const session = "ses_OrdinaryNotes";
const sessionPath = (note: string) => `kuvo/sessions/${session}/notes/${note}`;
const queuePath = (id: string, queue = YOURS) => `kuvo/data/edits?kind=queueItems&id=${id}&queue=${queue}`;
const initial = "Start.";
const keyPair = () => {
  const key = newKey(), sign = newSigningSeed();
  return { key: key.toString("base64url"), sign, peer: {
    key: publicOf(key).toString("base64url"), sign: signingKeyOf(sign),
    address: addressOf(key).toString("base64url"), kind: "mac" as const, name: "Fixture",
  } };
};
const local = keyPair(), remote = keyPair();
const setup = newSetupID();
let previousLink: ReturnType<typeof readLink>;
let previousEngine: Buffer | undefined;
let engineWrites = 0;
const engine = Bun.serve({
  hostname: "127.0.0.1", port: 0, fetch(request) {
    if (request.method !== "GET") engineWrites++;
    return new URL(request.url).pathname === `/api/session/${session}`
      ? Response.json({ data: { id: session, metadata: { approval: "local", kuvo: { ownServers: ["kept"] } }, permissions: ["kept"] } })
      : Response.json({}, { status: 404 });
  },
});
function assign(computer?: string) {
  writeLink({
    key: local.key, sign: local.sign,
    setup: { id: setup, epoch: 1, primary: local.peer.key, sign: local.peer.sign, role: "primary", managed: true, checkpointed: true },
    sharedSetup: setup, shared: {
      membershipAndAccess: { sequence: 1, value: { boxes: [local.peer, remote.peer], left: [] } },
      ownershipAssignments: { sequence: 1, value: computer ? { [`session computer:${session}`]: { computer } } : {} },
    },
  });
}
beforeAll(() => {
  previousLink = readLink();
  previousEngine = fs.existsSync(ENGINE_FILE) ? fs.readFileSync(ENGINE_FILE) : undefined;
  upgradeKuvoFolder(K);
  fs.mkdirSync(RUN, { recursive: true });
  fs.writeFileSync(ENGINE_FILE, JSON.stringify({ port: engine.port!, password: "disposable-fixture" }));
  assign();
});
afterAll(() => {
  engine.stop(true);
  forgetSessionNotes([session]);
  if (previousLink) writeLink(previousLink);
  else fs.rmSync(LINK_FILE, { force: true });
  if (previousEngine) fs.writeFileSync(ENGINE_FILE, previousEngine);
  else fs.rmSync(ENGINE_FILE, { force: true });
});

const createSession = (note: string, text = initial) => ask(handle, "PATCH", sessionPath(note), {
  operation: `create-${note}`, create: true, baseRevision: null, fields: { text },
});
function createQueue(text = initial) {
  const id = newItemID(), by = { user: true } as const, at = Date.now();
  changeQueues(book => book.queue(YOURS).items.push({
    id, owner: YOURS, title: "Fixture", status: "todo", body: "", order: 0, labels: [],
    notes: [{ text, at, by }], files: [], links: [], sessions: [], created: { at, by }, updated: { at, by },
  }));
  return id;
}

async function patchQueue(id: string, body: unknown) {
  return (await ask(handle, "PATCH", `kuvo/work-queue/items/${id}`, body)).json;
}

async function afterRestart(reads: string[]) {
  const script = `
    import { engineHandler } from "./box/src/link/server/router";
    import { ask } from "./box/test/route-answer";
    const handler = engineHandler();
    console.log(JSON.stringify(await Promise.all(${JSON.stringify(reads)}.map(path => ask(handler, "GET", path)))));
  `;
  const child = Bun.spawn([process.execPath, "-e", script], {
    cwd: new URL("../..", import.meta.url).pathname, env: { ...process.env, KUVO_HOME: K }, stdout: "pipe", stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([child.stdout.text(), child.stderr.text(), child.exited]);
  if (code !== 0) throw new Error(stderr);
  return JSON.parse(stdout);
}

for (const reverse of [false, true]) {
  for (const identical of [false, true]) test(`session and legacy queue note append, reverse=${reverse}, identical=${identical}`, async () => {
    const note = `append-${reverse}-${identical}`;
    const { json: created } = await createSession(note);
    expect(created.status).toBe("committed");
    const id = createQueue();
    const { json: queue } = await ask(handle, "GET", queuePath(id));
    const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
    expect(field).toMatch(/^note:[\w-]+$/);
    const wordings = ["Start. Alpha.", identical ? "Start. Alpha." : "Start. Beta."];
    const clock = spyOn(Date, "now");
    try {
      for (const [order, index] of (reverse ? [1, 0] : [0, 1]).entries()) {
        clock.mockReturnValue(Date.UTC(2025, 0, 2 - order));
        const edit = { operation: `${note}-session-${index}`, baseRevision: created.revision, fields: { text: wordings[index] } };
        const saved = await ask(handle, "PATCH", sessionPath(note), edit);
        expect(saved.json.status).toBe("committed");
        expect(await ask(handle, "PATCH", sessionPath(note), edit)).toEqual(saved);
        const queueEdit = { operation: `${note}-queue-${index}`, baseRevision: queue.revision, fields: { [field]: wordings[index] }, owner: YOURS };
        const queued = await patchQueue(id, queueEdit);
        expect(queued.status).toBe("committed");
        expect(await patchQueue(id, queueEdit)).toEqual(queued);
      }
    } finally { clock.mockRestore(); }
    const expected = identical ? "Start. Alpha. Alpha." : "Start. Alpha. Beta.";
    const [sessionView, queueView] = await afterRestart([sessionPath(note), queuePath(id)]);
    expect(sessionView.json).toMatchObject({ fields: { text: expected }, choices: [] });
    expect(queueView.json).toMatchObject({ fields: { [field]: expected }, choices: [] });
  });
}

test("line breaks and adjacent anchors preserve three independently identified insertions", async () => {
  const cases = [
    { base: "Start.\n", texts: ["Start.\n\nAlpha.", "Start.\n\nBeta.", "Start.\n\nGamma."], accepted: "Start.\n\nAlpha.\nBeta.\nGamma." },
    { base: "Start. End.", texts: ["Start. A. End.", "Start. A End.", "Start.\nB End."], accepted: "Start.\nB A A. End." },
  ];
  for (const [caseIndex, sample] of cases.entries()) {
    for (const [orderIndex, order] of [[0, 1, 2], [0, 2, 1], [2, 1, 0]].entries()) {
      const note = `anchors-${caseIndex}-${orderIndex}`;
      const { json: created } = await createSession(note, sample.base);
      const id = createQueue(sample.base);
      const { json: queue } = await ask(handle, "GET", queuePath(id));
      const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
      for (const index of order) {
        const text = sample.texts[index]!;
        expect((await ask(handle, "PATCH", sessionPath(note), { operation: `${note}-s-${index}`,
          baseRevision: created.revision, fields: { text } })).json.status).toBe("committed");
        expect((await patchQueue(id, { operation: `${note}-q-${index}`, baseRevision: queue.revision,
          fields: { [field]: text }, owner: YOURS })).status).toBe("committed");
      }
      expect((await ask(handle, "GET", sessionPath(note))).json.fields.text).toBe(sample.accepted);
      expect((await ask(handle, "GET", queuePath(id))).json.fields[field]).toBe(sample.accepted);
    }
  }
});

test("a disjoint edit against a newer base preserves the original insertion anchors and identities", async () => {
  const base = "Heading red. Start.";
  for (const [index, [first, last, suffix]] of [
    [" Alpha", " Alpha", " Alpha Alpha"],
    [" Alpha.", " Zulu.", " Alpha. Zulu."],
    [" Zulu.", " Alpha.", " Alpha. Zulu."],
  ].entries()) {
    const note = `interleaved-${index}`;
    const { json: created } = await createSession(note, base);
    const id = createQueue(base);
    const { json: queue } = await ask(handle, "GET", queuePath(id));
    const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
    let sr = created.revision, qr = queue.revision;
    for (const [step, text] of [base + first, "Heading blue. Start." + first, base + last].entries()) {
      const s = await ask(handle, "PATCH", sessionPath(note), { operation: `${note}-s-${step}`,
        baseRevision: step === 1 ? sr : created.revision, fields: { text } });
      const q = await patchQueue(id, { operation: `${note}-q-${step}`, baseRevision: step === 1 ? qr : queue.revision,
        fields: { [field]: text }, owner: YOURS });
      expect(s.json.status).toBe("committed");
      expect(q.status).toBe("committed");
      sr = s.json.revision;
      qr = q.revision;
    }
    expect((await ask(handle, "GET", sessionPath(note))).json.fields.text).toBe("Heading blue. Start." + suffix);
    expect((await ask(handle, "GET", queuePath(id))).json.fields[field]).toBe("Heading blue. Start." + suffix);
  }
});

test("both note routes keep disjoint rewrites and retain overlapping alternatives after restart", async () => {
  for (const reverse of [false, true]) {
    const original = "Alpha: red. Beta: green. Gamma: white.";
    const note = `rewrite-${reverse}`;
    const { json: created } = await createSession(note, original);
    const id = createQueue(original);
    const { json: queue } = await ask(handle, "GET", queuePath(id));
    const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
    const left = "Alpha: blue. Beta: green. Gamma: black.";
    const right = "Alpha: yellow. Beta: orange. Gamma: white.";
    const wordings = reverse ? [right, left] : [left, right];
    for (const [index, text] of wordings.entries()) {
      const expected = index ? "choice" : "committed";
      expect((await ask(handle, "PATCH", sessionPath(note), { operation: `${note}-s-${index}`,
        baseRevision: created.revision, fields: { text } })).json.status).toBe(expected);
      expect((await patchQueue(id, { operation: `${note}-q-${index}`, baseRevision: queue.revision,
        fields: { [field]: text }, owner: YOURS })).status).toBe(expected);
    }
    const variants = ["Alpha: blue. Beta: orange. Gamma: black.", "Alpha: yellow. Beta: orange. Gamma: black."];
    const [accepted, proposed] = reverse ? variants.toReversed() : variants;
    const [s, q] = await afterRestart([sessionPath(note), queuePath(id)]);
    expect(s.json).toMatchObject({ fields: { text: accepted }, choices: [{ field: "text", base: original, accepted, proposed }] });
    expect(q.json).toMatchObject({ fields: { [field]: accepted }, choices: [{ field, base: original, accepted, proposed }] });
    const resolve = (view: any) => ({ baseRevision: view.revision, resolve: { id: view.choices[0].id, use: "proposed" } });
    expect((await ask(handle, "PATCH", sessionPath(note), { ...resolve(s.json), operation: `${note}-s-pick` })).json.status).toBe("committed");
    expect((await patchQueue(id, { ...resolve(q.json), operation: `${note}-q-pick`, owner: YOURS })).status).toBe("committed");
    expect((await ask(handle, "GET", sessionPath(note))).json.fields.text).toBe(proposed!);
    expect((await ask(handle, "GET", queuePath(id))).json.fields[field]).toBe(proposed!);
  }
});

test("oversized alternatives stay recoverable without committing an unselectable choice", async () => {
  const note = "large-alternative";
  const original = "Keep. End.";
  const accepted = "X. End. ".padEnd(MOST_NOTE, "a");
  const { json: created } = await createSession(note, original);
  const id = createQueue(original);
  const { json: queue } = await ask(handle, "GET", queuePath(id));
  const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
  const edit = (operation: string, baseRevision: string, field: string, value: string) => ({ operation, baseRevision, fields: { [field]: value } });
  const drafts = "Longer. End.";
  expect((await ask(handle, "PATCH", sessionPath(note), edit("large-s", created.revision, "text", accepted))).json.status).toBe("committed");
  expect((await patchQueue(id, { ...edit("large-q", queue.revision, field, accepted), owner: YOURS })).status).toBe("committed");
  const s = await ask(handle, "PATCH", sessionPath(note), edit("large-s-late", created.revision, "text", drafts));
  const q = await patchQueue(id, { ...edit("large-q-late", queue.revision, field, drafts), owner: YOURS });
  expect(s.json.status).toBe("interrupted");
  expect(q.status).toBe("interrupted");
  expect((await ask(handle, "GET", sessionPath(note))).json).toMatchObject({ fields: { text: accepted }, choices: [] });
  expect((await ask(handle, "GET", queuePath(id))).json).toMatchObject({ fields: { [field]: accepted }, choices: [] });
});

test("large note histories retire the base and keep rejected text on both real routes", async () => {
  const note = "retired";
  const { json: created } = await createSession(note);
  const id = createQueue();
  const { json: queue } = await ask(handle, "GET", queuePath(id));
  const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
  let sr = created.revision, qr = queue.revision;
  for (let index = 0; index < 15; index++) {
    const text = String(index).padEnd(MOST_NOTE, "x");
    const saved = await ask(handle, "PATCH", sessionPath(note), { operation: `retired-s-${index}`, baseRevision: sr, fields: { text } });
    const queued = await patchQueue(id, { operation: `retired-q-${index}`, baseRevision: qr, fields: { [field]: text }, owner: YOURS });
    expect(saved.json.status).toBe("committed");
    expect(queued.status).toBe("committed");
    sr = saved.json.revision;
    qr = queued.revision;
  }
  const s = await ask(handle, "PATCH", sessionPath(note), { operation: "retired-s-late", baseRevision: created.revision, fields: { text: "Draft" } });
  const q = await patchQueue(id, { operation: "retired-q-late", baseRevision: queue.revision, fields: { [field]: "Draft" }, owner: YOURS });
  for (const receipt of [s.json, q]) expect(receipt.status).toBe("refresh");
  const [savedS, savedQ] = await afterRestart([s.json, q].map(receipt => `kuvo/data/edits/${receipt.operation}`));
  expect(savedS.json.recovery).toEqual({ text: "Draft" });
  expect(savedQ.json.recovery).toEqual({ [field]: "Draft" });
});

test("deletion fences late edits on both note routes and preserves a recoverable draft", async () => {
  const note = "deleted";
  const { json: created } = await createSession(note);
  const id = createQueue();
  const { json: queue } = await ask(handle, "GET", queuePath(id));
  const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
  expect((await ask(handle, "DELETE", sessionPath(note), { operation: "delete-note", baseRevision: created.revision })).json.status).toBe("committed");
  const ref = { kind: "queueItems", id, queue: YOURS };
  expect((await ask(handle, "POST", "kuvo/data/edits", { operation: "delete-item", ref, baseRevision: queue.revision, delete: true })).json.status).toBe("committed");
  const s = await ask(handle, "PATCH", sessionPath(note), { operation: "deleted-note-late", baseRevision: created.revision, fields: { text: "Draft" } });
  const q = await patchQueue(id, { operation: "deleted-item-late", baseRevision: queue.revision, fields: { [field]: "Draft" }, owner: YOURS });
  expect(s.json.status).toBe("deleted");
  expect(q.status).toBe("deleted");
  const [sView, qView, sReceipt, qReceipt] = await afterRestart([sessionPath(note), queuePath(id), ...[s.json, q].map(receipt => `kuvo/data/edits/${receipt.operation}`)]);
  expect(sView.json).toMatchObject({ deleted: true, fields: {} });
  expect(qView.json).toMatchObject({ deleted: true });
  expect(sReceipt.json.recovery).toEqual({ text: "Draft" });
  expect(qReceipt.json.recovery).toEqual({ [field]: "Draft" });
  expect((await createSession(note, "Reused")).status).toBe(409);
  expect((await ask(handle, "GET", `kuvo/sessions/${session}/notes`)).json.fields.ids).not.toContain(note);
});

test("session notes follow ownership, queue notes follow handoff, and a revoked grant cannot edit either", async () => {
  const note = "handoff";
  const { json: created } = await createSession(note);
  const id = createQueue();
  const { json: queue } = await ask(handle, "GET", queuePath(id));
  const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
  const sessionRef = { kind: "sessionNotes" as const, id: session, note };
  const queueRef = { kind: "queueItems" as const, id, queue: session };
  const packed = JSON.parse(JSON.stringify(packSessionNotes([session])));
  assign(remote.peer.key);
  const rejected: unknown[] = [];
  const places = { targets: [{ port: engine.port!, password: "disposable-fixture" }], bridge: false };
  const deliver = (nonce: string, edit: object, allowed = true) => takeOfflineEdit(remote.peer, {
    setup, nonce, submitter: allowed ? { kind: "computer" } : { kind: "ssh", key: "r".repeat(43) },
    edit: { ...edit, operation: mutationKey(setup, nonce) },
  }, value => rejected.push(value.offlineEditReceipt), places, null, true);
  const draft = { ref: sessionRef, baseRevision: created.revision, fields: { text: "Start. After handoff." } };
  try {
    await deliver("held-note", draft);
    expect(rejected).toEqual([]);
    forgetSessionNotes([session]);
    placeSessionNotes(packed, [session], "fixture-move");
    await deliver("held-note", draft);
    expect(rejected).toEqual([]);
  } finally { assign(local.peer.key); }
  await deliver("revoked-note", draft, false);
  expect(rejected.at(-1)).toMatchObject({ receipt: { status: "interrupted" } });
  await deliver("held-note", draft);
  expect(rejected.at(-1)).toMatchObject({ receipt: { status: "committed" } });
  const prior = rejected.at(-1);
  await deliver("held-note", draft, false);
  expect(rejected.at(-1)).toEqual(prior);
  changeQueues(book => handItem(book, book.find({ id })!, session, { user: true }, Date.now()));
  handUpkeep(readLink());
  for (let attempt = 0; attempt < 50 && allItems().find(item => item.id === id)?.owner !== session; attempt++) await Bun.sleep(10);
  expect(allItems().find(item => item.id === id)?.owner).toBe(session);
  const queueDraft = { ref: queueRef, baseRevision: queue.revision, fields: { [field]: "Start. Moved." } };
  await deliver("revoked-queue", queueDraft, false);
  expect(rejected.at(-1)).toMatchObject({ receipt: { status: "interrupted" } });
  await deliver("moved-queue", queueDraft);
  expect(rejected.at(-1)).toMatchObject({ receipt: { status: "committed" } });
  const [s, q] = await afterRestart([sessionPath(note), queuePath(id, session)]);
  expect(s.json.fields.text).toBe("Start. After handoff.");
  expect(q.json.fields[field]).toBe("Start. Moved.");

  expect((await ask(handle, "PATCH", sessionPath(note), { operation: "privilege", baseRevision: s.json.revision,
    fields: { permissions: ["allow"], metadata: {} } })).json.status).toBe("interrupted");
  expect((await ask(handle, "GET", sessionPath(note), undefined, { fromApp: false })).status).toBe(403);
  expect(engineWrites).toBe(0);
});

test("legacy and current listers replay the same queue-note edit across an upgrade", async () => {
  for (const oldOwner of [false, true]) {
    const id = createQueue();
    const { json: queue } = await ask(handle, "GET", queuePath(id));
    const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
    const nonce = `mixed-version-${oldOwner}`;
    const legacy = recordKey(setup, remote.peer.key, nonce);
    const current = mutationKey(setup, nonce);
    const edit = { ref: { kind: "queueItems" as const, id, queue: YOURS }, baseRevision: queue.revision,
      fields: { [field]: "Start. Once." } };
    const replies: unknown[] = [];
    const places = { targets: [], bridge: false };
    const deliver = (sender: typeof local.peer, operation: string) => takeOfflineEdit(sender, {
      setup, nonce, submitter: { kind: "computer" }, edit: { ...edit, operation },
    }, value => replies.push(value.offlineEditReceipt), places, null, true);
    if (oldOwner) {
      // A pre-upgrade receipt has only the legacy key on disk.
      expect(editQueue({ ...edit, operation: legacy }, () => true)).toMatchObject({ status: "committed" });
    } else {
      await deliver(remote.peer, legacy);
      expect(replies.at(-1)).toMatchObject({ receipt: { operation: legacy, status: "committed" } });
      // An older owner can still replay its original operation after a queue-only handoff.
      expect(editQueue({ ...edit, operation: legacy }, () => true, () => false)).toMatchObject({ operation: legacy, status: "committed" });
    }
    await deliver(local.peer, current);
    expect(replies.at(-1)).toMatchObject({ receipt: { operation: current, status: "committed" } });
    await deliver(remote.peer, legacy);
    expect(replies.at(-1)).toMatchObject({ receipt: { operation: legacy, status: "committed" } });
    expect((await ask(handle, "GET", queuePath(id))).json.fields[field]).toBe("Start. Once.");
  }
});

test("two historical sender-scoped receipts sharing a nonce still replay their own decisions", async () => {
  const id = createQueue();
  const { json: queue } = await ask(handle, "GET", queuePath(id));
  const field = Object.keys(queue.fields).find(key => key.startsWith("note:"))!;
  const nonce = "old-nonce-shared";
  const current = mutationKey(setup, nonce);
  const old = [local.peer, remote.peer].map((peer, index) => ({
    peer,
    edit: {
      ref: { kind: "queueItems" as const, id, queue: YOURS }, baseRevision: queue.revision,
      operation: recordKey(setup, peer.key, nonce), fields: { [field]: index ? "Start. Second." : "Start. First." },
    },
  }));
  const decisions = old.map(({ edit }) => {
    const decision = editQueue(edit, () => true);
    if (!decision) throw new Error("Failed to prepare a legacy receipt.");
    return decision;
  });
  const replies: any[] = [];
  const send = (who: typeof local.peer, edit: typeof old[number]["edit"]) => takeOfflineEdit(who, {
    setup, nonce, submitter: { kind: "computer" }, edit,
  }, value => replies.push(value.offlineEditReceipt), { targets: [], bridge: false }, null, true);
  await send(local.peer, { ...old[0]!.edit, operation: current });
  expect(replies.at(-1).receipt).toEqual({ ...decisions[0], operation: current });
  for (const [index, { peer, edit }] of old.entries()) {
    await send(peer, edit);
    expect(replies.at(-1).receipt).toEqual(decisions[index]);
  }
  expect((await ask(handle, "GET", queuePath(id))).json.fields[field]).toBe("Start. First. Second.");
});

test("a pending note retries when the wall clock moves backward", async () => {
  const note = "offline-clock";
  const { json: created } = await createSession(note);
  assign(local.peer.key);
  const proposal = { operation: "offline-clock-edit", baseRevision: created.revision, fields: { text: "Start. Recovered." } };
  const engineFile = fs.readFileSync(ENGINE_FILE);
  fs.rmSync(ENGINE_FILE);
  let pending;
  try {
    pending = await ask(handle, "PATCH", sessionPath(note), proposal);
    expect(pending.status).toBe(202);
    sendOfflineEdits(null, readLink(), Date.now() + 3_600_000, () => ({ targets: [], bridge: false }));
    await Bun.sleep(0);
  } finally { fs.writeFileSync(ENGINE_FILE, engineFile); }
  sendOfflineEdits(null, readLink(), Date.now(), () => ({ targets: [{ port: engine.port!, password: "disposable-fixture" }], bridge: false }));
  const receiptPath = `kuvo/data/edits/${pending.json.operation}`;
  for (let attempts = 0; attempts < 50; attempts++) {
    const result = await ask(handle, "GET", receiptPath);
    if (result.json.receipt.status === "committed") {
      expect((await ask(handle, "GET", sessionPath(note))).json.fields.text).toBe("Start. Recovered.");
      return;
    }
    await Bun.sleep(10);
  }
  throw new Error("The pending note did not retry after clock reversal.");
});
