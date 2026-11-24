// A real resident reads Work Queue files in an isolated home with a local relay.
import assert from "node:assert/strict";
import { verify } from "node:crypto";
import { existsSync, readFileSync, renameSync, rmSync, symlinkSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, localRelay, openLine, prepareHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

async function withQueue(run) {
  const root = scratch();
  const home = path.join(root, "box");
  const queue = path.join(home, "run", "work-queue");
  const relay = await localRelay();
  const binary = boxBinary();
  let box;
  try {
    prepareHome(home, binary);
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    await run(queue, async () => {
      box = startBox({ binary, home, env: isolatedEnv(root), relay, mac: true });
      const resident = await box.ready();
      const ask = (route, method = "GET", body, headers) => request(resident, method, route, body, headers);
      ask.events = query => openLine(resident, query, "kuvo/work-queue/events");
      return ask;
    });
  } finally {
    await box?.stop();
    await relay.close();
    rmSync(root, { recursive: true, force: true });
  }
}

test("old categories become statuses and labels and the original bytes are kept", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const file = path.join(queue, "queues", "usr_you.json");
    const parked = "wqi_abcdefghijklmnopqrst";
    const waiting = "wqi_ABCDEFGHIJKLMNOPQRST";
    const original = JSON.stringify({ owner: "usr_you", items: [
      { id: parked, title: "Old plan", category: "parked", number: 9, futureField: { keep: true } },
      { id: waiting, title: "Your decision", category: "cat_approval", number: 8 },
    ] });
    write(file, original);
    write(path.join(queue, "categories.json"), JSON.stringify({ custom: [
      { id: "cat_approval", name: "Waiting on you" },
    ] }));
    const ask = await start();
    const answer = await ask("kuvo/work-queue?owner=all&glance=1&counts=1");
    assert.equal(answer.status, 200, answer.text);
    assert.deepEqual(answer.json.items.map(({ id, status, labels }) => ({ id, status, labels })), [
      { id: waiting, status: "needs-you", labels: [] },
      { id: parked, status: "backlog", labels: ["lbl_parked"] },
    ]);
    assert.deepEqual(answer.json.labels, [{ id: "lbl_parked", name: "Parked", color: "graphite" }]);
    assert.equal(answer.json.counts.open, 2);
    assert.equal(answer.json.counts.labels.lbl_parked, 1);
    assert.equal(readFileSync(path.join(queue, "before-statuses", "queues", "usr_you.json"), "utf8"), original);
    const kept = JSON.parse(readFileSync(file, "utf8"));
    assert.equal(kept.formatVersion, 1);
    assert.deepEqual(kept.items[0].futureField, { keep: true });
    assert.equal(kept.items[0].category, undefined);
    assert.equal(existsSync(path.join(queue, "categories.json")), false);
    assert.equal(JSON.parse(readFileSync(path.join(queue, "model.json"), "utf8")).model, 2);
  }));

for (const [name, relative, original, message] of [
  ["owner file", ["queues", "usr_you.json"], '{"formatVersion":99,"items":[]}', /Work Queue owner file format version 99 is unsupported/],
  ["number ledger", ["numbers.json"], '{"formatVersion":2,"leases":[[41,42]],"next":43}', /Work Queue number ledger format version 2 is unsupported/],
]) {
  test(`a future ${name} is preserved when startup refuses it`, { timeout: 30000 }, async () => {
    const root = scratch();
    const home = path.join(root, "box");
    const queue = path.join(home, "run", "work-queue");
    const file = path.join(queue, ...relative);
    const binary = boxBinary();
    const relay = await localRelay();
    let box;
    try {
      prepareHome(home, binary);
      write(file, original);
      write(path.join(home, "folder-version.json"), '{"version":23}\n');
      box = boxCommand(binary, home, isolatedEnv(root), relay,
        ["serve", ...(binary.typescript ? ["--resident"] : [])]);
      assert.notEqual((await box.exited).code, 0);
      assert.match(box.log(), message);
      assert.equal(readFileSync(file, "utf8"), original);
      assert.equal(existsSync(path.join(queue, "before-statuses")), false);
    } finally {
      await box?.stop();
      await relay.close();
      rmSync(root, { recursive: true, force: true });
    }
  });
}

test("existing item details, history, saved mentions and links reopen together", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const target = "wqi_abcdefghijklmnopqrst";
    const origin = "wqi_ABCDEFGHIJKLMNOPQRST";
    const stamp = { at: 1760000000000, by: { user: true } };
    const mention = { start: 4, end: 6, number: 7, item: target };
    const alias = { setup: "set_abcdefghijklmnopqrstuv", name: "Earlier setup", number: 12 };
    write(path.join(queue, "model.json"), '{"formatVersion":1,"model":2}');
    write(path.join(queue, "queues", "usr_you.json"), JSON.stringify({
      formatVersion: 1, owner: "usr_you", items: [
        {
          id: origin, title: "Release plan", body: "See #7", number: 8, status: "todo",
          created: stamp, updated: stamp, numberAliases: [alias],
          events: [{ ...stamp, kind: "moved", from: "backlog", to: "todo", fromLabel: "lbl_parked" }],
          notes: [{ ...stamp, id: "note-1", text: "See #7" }],
          relations: [{ item: target, kind: "blocks" }],
          mentionIndex: { body: [mention], notes: [[mention]], targets: [target] },
        },
        { id: target, title: "Fix startup", number: 7, status: "backlog", created: stamp, updated: stamp },
      ],
    }));
    const ask = await start();
    const detail = await ask("kuvo/work-queue/items/%238");
    assert.equal(detail.status, 200, detail.text);
    assert.equal(detail.json.item.id, origin);
    assert.deepEqual(detail.json.item.bodyMentions, [mention]);
    assert.deepEqual(detail.json.item.notes, [{ ...stamp, id: "note-1", text: "See #7", mentions: [mention] }]);
    assert.deepEqual(detail.json.item.itemRelations, [
      { id: target, title: "Fix startup", number: 7, kind: "blocks" },
      { id: target, title: "Fix startup", number: 7, kind: "mentions" },
    ]);
    assert.equal(detail.json.item.events[0].fromLabel, "lbl_parked");
    assert.equal(detail.json.item.can.attach, true);
    assert.equal(detail.json.item.canLinkItems, true);
    const backlink = await ask("kuvo/work-queue/items/7");
    assert.equal(backlink.status, 200, backlink.text);
    assert.deepEqual(backlink.json.item.itemRelations.map(link => link.kind), ["blocked-by", "mentioned-in"]);
    const refs = await ask("kuvo/work-queue/refs?refs=12,%237,set_abcdefghijklmnopqrstuv:12,13");
    assert.equal(refs.status, 200, refs.text);
    assert.equal(refs.json.refs["#7"].id, target);
    assert.equal(refs.json.refs["set_abcdefghijklmnopqrstuv:12"].id, origin);
    assert.equal(refs.json.refs["13"], null);
    assert.deepEqual(refs.json.aliases["12"][0].numberAliases, [alias]);
    const list = await ask("kuvo/work-queue?owner=all&glance=1&counts=1&q=release+see&sources=none");
    assert.equal(list.status, 200, list.text);
    assert.deepEqual(list.json.items.map(item => item.id), [origin]);
    assert.equal(list.json.counts.open, 1);
  }));

test("a glance uses a saved GitHub closure while keeping the item's own status on disk", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const file = path.join(queue, "queues", "usr_you.json");
    const source = { kind: "github", ref: "gh:sample/project#14" };
    const original = JSON.stringify({ formatVersion: 1, owner: "usr_you", items: [
      { id: "wqi_abcdefghijklmnopqrst", title: "Old title", status: "waiting", source },
    ] });
    write(path.join(queue, "model.json"), '{"formatVersion":1,"model":2}');
    write(file, original);
    write(path.join(queue, "seen.json"), JSON.stringify({
      formatVersion: 1,
      reads: {
        "github.com/sample/project#14": {
          at: 1760000000000,
          value: {
            kind: "github", id: "issue-example", number: 14, title: "Closed upstream",
            url: "https://github.com/sample/project/issues/14", state: "not_planned", closedAt: 1759900000000,
          },
        },
      },
    }));
    const ask = await start();
    const answer = await ask("kuvo/work-queue?owner=all&glance=1&counts=1");
    assert.equal(answer.status, 200, answer.text);
    const [item] = answer.json.items;
    const { title, status, keptIn, doneAt, doneAs, closesOn, struck, numberPending } = item;
    assert.deepEqual({ title, status, keptIn, doneAt, doneAs, closesOn, struck }, {
      title: "Closed upstream", status: "done", keptIn: "waiting", doneAt: 1759900000000,
      doneAs: "Closed on GitHub", closesOn: "github", struck: true,
    });
    assert.equal(numberPending, undefined);
    assert.equal(item.source.name, "project#14");
    assert.equal(item.can.set, false);
    assert.equal(answer.json.counts.open, 0);
    assert.equal(readFileSync(file, "utf8"), original);
  }));

test("an interrupted migration uses the original copy and keeps changes made since", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const own = "wqi_abcdefghijklmnopqrst";
    const session = "wqi_ABCDEFGHIJKLMNOPQRST";
    const before = path.join(queue, "before-statuses", "queues");
    const first = JSON.stringify({ owner: "usr_you", items: [
      { id: own, title: "Original description", category: "parked", order: 1 },
    ] });
    const second = JSON.stringify({ owner: "ses_test", items: [
      { id: session, title: "Other item", category: "parked", order: 2 },
    ] });
    write(path.join(before, "usr_you.json"), first);
    write(path.join(before, "ses_test.json"), second);
    write(path.join(queue, "queues", "usr_you.json"), JSON.stringify({
      formatVersion: 1, owner: "usr_you", items: [
        { id: own, title: "Edited after the first write", status: "backlog", order: 0, labels: ["lbl_parked"] },
      ],
    }));
    write(path.join(queue, "queues", "ses_test.json"), second);
    const ask = await start();
    const answer = await ask("kuvo/work-queue?owner=all&glance=1");
    assert.equal(answer.status, 200, answer.text);
    assert.deepEqual(answer.json.items.map(({ id, title, order, status }) => ({ id, title, order, status })), [
      { id: own, title: "Edited after the first write", order: 0, status: "backlog" },
      { id: session, title: "Other item", order: 1, status: "backlog" },
    ]);
    assert.equal(readFileSync(path.join(before, "usr_you.json"), "utf8"), first);
    assert.equal(readFileSync(path.join(before, "ses_test.json"), "utf8"), second);
  }));

test("an attachment downloads only the regular file saved with its item", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const id = "wqi_abcdefghijklmnopqrst";
    const folder = path.join(queue, "files", id);
    const file = path.join(folder, "release notes.txt");
    const bytes = Buffer.from("The saved attachment.\n");
    write(file, bytes);
    write(path.join(queue, "model.json"), '{"formatVersion":1,"model":2}');
    write(path.join(queue, "queues", "usr_you.json"), JSON.stringify({
      formatVersion: 1, owner: "usr_you", items: [
        { id, title: "Release", status: "todo", number: 1, files: [
          { name: "release notes.txt", size: bytes.length, type: "text/plain", at: 1760000000000 },
        ] },
      ],
    }));
    const ask = await start();
    const route = "kuvo/work-queue/items/1/files/release%20notes.txt";
    const answer = await ask(route);
    assert.equal(answer.status, 200, answer.text);
    assert.equal(answer.type, "text/plain");
    assert.deepEqual(answer.bytes, bytes);
    const absent = await ask("kuvo/work-queue/items/1/files/unlisted");
    assert.equal(absent.status, 404, absent.text);
    assert.deepEqual(absent.json, { message: "There's no such file." });
    if (process.platform !== "win32") {
      const replacement = path.join(queue, "replacement.txt");
      renameSync(file, replacement);
      symlinkSync(replacement, file);
      const refused = await ask(route);
      assert.equal(refused.status, 404, refused.text);
      assert.deepEqual(refused.json, { message: "There's no such file." });
      rmSync(file);
      renameSync(replacement, file);
    }
    const outside = path.join(queue, "replacement-dir");
    renameSync(folder, outside);
    symlinkSync(outside, folder, process.platform === "win32" ? "junction" : "dir");
    const refusedFolder = await ask(route);
    assert.equal(refusedFolder.status, 404, refusedFolder.text);
  }));

test("a queue follower sees a saved change by another writer", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const file = path.join(queue, "queues", "usr_you.json");
    const id = "wqi_abcdefghijklmnopqrst";
    const record = title => JSON.stringify({ formatVersion: 1, owner: "usr_you", items: [
      { id, title, status: "todo" },
    ] });
    write(path.join(queue, "model.json"), '{"formatVersion":1,"model":2}');
    write(file, record("First title"));
    const ask = await start();
    const line = await ask.events("?owner=all&glance=1");
    assert.equal(line.status, 200);
    assert.equal(line.type, "application/x-ndjson");
    const frames = line.frames();
    try {
      assert.deepEqual((await frames.next()).value.items.map(item => item.title), ["First title"]);
      write(file + ".stage", record("Changed title"));
      renameSync(file + ".stage", file);
      assert.deepEqual((await frames.next()).value.items.map(item => item.title), ["Changed title"]);
    } finally {
      await frames.return();
      line.close();
    }
  }));

test("labels can be renamed and removed without reusing a retired ID", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const ask = await start();
    const labels = "kuvo/work-queue/labels";
    const edit = body => ask(labels, "POST", body, { "x-kuvo-from": "app" });
    const added = await edit({ add: "Rélease", color: "grey" });
    assert.equal(added.status, 200, added.text);
    const label = { id: "lbl_re-lease", name: "Rélease", color: "graphite" };
    assert.deepEqual(added.json.labels, [label]);
    assert.deepEqual(added.json.label, label);
    const changed = await edit({ change: label.id, name: "Release ready", color: "sky" });
    assert.equal(changed.status, 200, changed.text);
    const ready = { ...label, name: "Release ready", color: "sky" };
    assert.deepEqual(changed.json.labels, [ready]);
    assert.deepEqual(changed.json.label, ready);
    const removed = await edit({ remove: ready.id });
    assert.equal(removed.status, 200, removed.text);
    assert.deepEqual(removed.json.labels, []);
    assert.equal(removed.json.label, undefined);
    const again = await edit({ add: label.name });
    assert.equal(again.status, 200, again.text);
    const replacement = { ...label, id: "lbl_re-lease-2", color: "coral" };
    assert.deepEqual(again.json.labels, [replacement]);
    assert.deepEqual(again.json.label, replacement);
    const file = path.join(queue, "labels.json");
    const kept = existsSync(file) ? JSON.parse(readFileSync(file, "utf8"))
      : JSON.parse(readFileSync(path.join(queue, "..", "link.json"), "utf8")).shared.queueLabels.value;
    assert.deepEqual(kept.removed, [label.id]);
  }));

test("a label saved on a member is signed by the main and reaches both computers", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab");
  const main = studio.connect(await studio.start().ready());
  const setup = await main("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  assert.equal(setup.json.you.role, "primary");
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const app = { "x-kuvo-from": "app" };
  const labels = "kuvo/work-queue/labels";
  const first = await main("POST", labels, { add: "Ready", color: "sky" }, app);
  assert.equal(first.status, 200, first.text);
  assert.equal(first.json.status, "committed");
  const invite = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const joined = await lab.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const member = lab.connect(await lab.start().ready());
  await until(async () => {
    const answer = await member("GET", labels);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.labels.some(label => label.id === "lbl_ready");
  }, "The member did not see the main's initial label.", 20000);

  const added = await member("POST", labels, { add: "Follow up", color: "mint" }, app);
  assert.equal(added.status, 200, added.text);
  assert.equal(added.json.status, "committed");
  assert.equal(added.json.setup, setup.json.setup);
  const initial = JSON.parse(Buffer.from(first.json.proof.d, "base64url"));
  const signed = Buffer.from(added.json.proof.d, "base64url");
  const document = JSON.parse(signed);
  assert.equal(document.origin, initial.origin);
  assert.equal(document.body.kind, "queueLabels");
  const publicKey = Buffer.concat([
    Buffer.from("302a300506032b6570032100", "hex"),
    Buffer.from(initial.origin, "base64url"),
  ]);
  assert.ok(verify(null, Buffer.concat([Buffer.from("kuvo-link doc v1\0"), signed]),
    { key: publicKey, format: "der", type: "spki" }, Buffer.from(added.json.proof.s, "base64url")));
  const expected = [
    { id: "lbl_ready", name: "Ready", color: "sky" },
    { id: "lbl_follow-up", name: "Follow up", color: "mint" },
  ];
  assert.deepEqual(added.json.labels, expected);
  const stored = await main("GET", labels);
  assert.equal(stored.status, 200, stored.text);
  assert.deepEqual(stored.json.labels, expected);
  const published = await until(async () => {
    const answer = await member("GET", labels);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.labels.length === expected.length && answer.json.labels;
  }, "The member did not receive the signed label library.", 20000);
  assert.deepEqual(published, expected);
});

test("a queue item keeps its creation receipt and number through delete and restore", { timeout: 30000 }, () =>
  withQueue(async (_queue, start) => {
    const ask = await start();
    const app = { "x-kuvo-from": "app" };
    const items = "kuvo/work-queue/items";
    const body = { operation: "queue-create-one", title: "First item", status: "waiting", priority: "high" };
    const first = await ask(items, "POST", body, app);
    assert.equal(first.status, 200, first.text);
    const { id, status, priority } = first.json.item;
    assert.equal(status, "waiting");
    assert.equal(priority, "high");
    const repeated = await ask(items, "POST", body, app);
    assert.equal(repeated.status, 200, repeated.text);
    assert.deepEqual(repeated.json, first.json);
    const conflict = await ask(items, "POST", { ...body, title: "A different item" }, app);
    assert.equal(conflict.status, 409, conflict.text);
    const item = items + "/" + id;
    const deleted = await ask(item, "DELETE", undefined, app);
    assert.equal(deleted.status, 200, deleted.text);
    assert.equal(deleted.json.item.can.restore, true);
    const restored = await ask(item + "/restore", "POST", {}, app);
    assert.equal(restored.status, 200, restored.text);
    assert.equal(restored.json.item.status, "waiting");
    assert.equal(restored.json.item.can.restore, false);
    assert.equal(restored.json.item.number, deleted.json.item.number);
    const detail = await ask(item);
    assert.equal(detail.status, 200, detail.text);
    assert.deepEqual(detail.json.item.events.map(event => event.kind), ["deleted", "restored"]);
  }));

test("the main grants unique Work Queue numbers across two computers", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab");
  const primary = studio.start();
  const main = studio.connect(await primary.ready());
  const setup = await main("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const invite = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const joined = await lab.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const member = lab.connect(await lab.start().ready());
  const labSetup = await member("GET", "kuvo/setup");
  assert.equal(labSetup.status, 200, labSetup.text);
  const memberKey = labSetup.json.you.key;
  const app = { "x-kuvo-from": "app" };
  const items = "kuvo/work-queue/items";
  const add = async (call, operation, title) => {
    const answer = await call("POST", items, { operation, title }, app);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.item.id;
  };
  const mainItem = await add(main, "studio-one", "Studio item");
  const first = await add(member, "lab-one", "Lab first item");
  const number = async (call, id) => until(async () => {
    const answer = await call("GET", items + "/" + id);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.item.number;
  }, "An item is still waiting for its number.", 25000);
  const [mainNumber, firstNumber] = await Promise.all([number(main, mainItem), number(member, first)]);
  const ledger = JSON.parse(readFileSync(path.join(studio.home, "run", "work-queue", "numbers.json"), "utf8"));
  await primary.stop();
  const second = await add(member, "lab-two", "Lab second item");
  const secondNumber = await number(member, second);
  assert.equal(new Set([mainNumber, firstNumber, secondNumber]).size, 3);
  assert.equal(secondNumber, firstNumber + 1);
  const grant = Object.values(ledger.given).find(block =>
    block.by === memberKey && block.from <= firstNumber && secondNumber <= block.to);
  assert.ok(grant);
  assert.ok(grant.to <= leases.leaseFor(setup.json.setup).high);
});
