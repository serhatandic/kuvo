// A real resident reads Work Queue files in an isolated home with a local relay.
import assert from "node:assert/strict";
import { verify } from "node:crypto";
import { existsSync, readFileSync, renameSync, rmSync, symlinkSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, boxCommand, isolatedEnv, localRelay, openLine, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";

async function withQueue(run) {
  const root = scratch();
  const home = path.join(root, "box");
  const queue = path.join(home, "run", "work-queue");
  const relay = await localRelay();
  const binary = boxBinary();
  let box;
  try {
    privateHome(home);
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
      privateHome(home);
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

test("reference previews use UTF-16 offsets and skip Markdown and outside references", { timeout: 30000 }, () =>
  withQueue(async (_queue, start) => {
    const ask = await start();
    const app = { "x-kuvo-from": "app" };
    for (const [text, numbers] of [
      ["🖼 See #821 and (#82). Ranked #1, #2 in search, PR #3, #4seller, gh:acme/project-#5, `#6`, [#7](https://example.com), https://example.com/#8.\n\n```\n#92\n", [821, 82]],
      ["🖼 #9000", [9000]],
      ["🧭 See #71, `#72` and [#73](https://example.com).\n\n    #74\n\nIssue #75, #1 result, gh:acme/project#76, https://example.com/#77 and (#78).", [71, 78]],
      ["~~~ markdown\n#41\n~~~~\n\n`` See #42 ` followed by #43 ``\n\n[See #44](https://example.test/path(a(b))) [word #45][target]\n\n[target]: https://example.test\n\n🖼 עברית #46 ثم #47", [46, 47]],
    ]) {
      const reply = await ask("kuvo/work-queue/refs", "POST", { text }, app);
      assert.equal(reply.status, 200, reply.text);
      assert.deepEqual(reply.json.mentions, numbers.map(number => {
        const start = text.search(new RegExp(`#${number}(?![0-9])`));
        return { start, end: start + 1 + String(number).length, number };
      }));
    }
  }));

test("editing a local item changes status and keeps its links and history", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const second = { id: "wqi_abcdefghijklmnopqrst", number: 71 };
    const stamp = { at: 1760000000000, by: { user: true } };
    write(path.join(queue, "model.json"), '{"formatVersion":1,"model":2}');
    write(path.join(queue, "queues", "usr_you.json"), JSON.stringify({
      formatVersion: 1, owner: "usr_you", items: [
        { ...second, title: "Second", status: "todo", created: stamp, updated: stamp },
      ],
    }));
    const ask = await start();
    const app = { "x-kuvo-from": "app" };
    const items = "kuvo/work-queue/items";
    const item = async title => {
      const reply = await ask(items, "POST", { title, operation: `queue-edit-${title}` }, app);
      assert.equal(reply.status, 200, reply.text);
      return reply.json.item;
    };
    const first = await item("First");
    const source = `${items}/${first.id}`;
    const target = `${items}/${second.id}`;
    const body = `Follow #${second.number}.`;
    const changed = await ask(source, "PATCH", { body, status: "in-progress", top: true, due: "2026-12-31", priority: "high" }, app);
    assert.equal(changed.status, 200, changed.text);
    assert.equal(changed.json.item.status, "in-progress");
    assert.equal(changed.json.item.priority, "high");
    assert.equal(changed.json.item.due, "2026-12-31");
    const linked = await ask(source, "PATCH", { itemRelation: { item: second.id, kind: "blocks" } }, app);
    assert.equal(linked.status, 200, linked.text);
    assert.equal(linked.json.itemRelationApplied, true);
    const detail = await ask(source);
    assert.equal(detail.status, 200, detail.text);
    assert.equal(detail.json.item.body, body);
    assert.deepEqual(detail.json.item.bodyMentions, [{
      number: second.number, start: 7, end: 8 + String(second.number).length, item: second.id,
    }]);
    assert.deepEqual(detail.json.item.events.map(({ kind, from, to, top }) => ({ kind, from, to, top })), [
      { kind: "moved", from: "todo", to: "in-progress", top: true },
    ]);
    const edited = await ask(source, "PATCH", { body: `${body} Check #${second.number} again.` }, app);
    assert.equal(edited.status, 200, edited.text);
    assert.deepEqual((await ask(source)).json.item.bodyMentions.map(({ item }) => item), [second.id, second.id]);
    const inverse = await ask(target);
    assert.equal(inverse.status, 200, inverse.text);
    assert.deepEqual(inverse.json.item.itemRelations.map(({ id, kind }) => ({ id, kind })), [
      { id: first.id, kind: "blocked-by" }, { id: first.id, kind: "mentioned-in" },
    ]);
    const unlinked = await ask(source, "PATCH", { itemRelation: { item: second.id, kind: null } }, app);
    assert.equal(unlinked.status, 200, unlinked.text);
    assert.deepEqual((await ask(target)).json.item.itemRelations.map(({ kind }) => kind), ["mentioned-in"]);
  }));

test("removing an inline picture preserves the current text and saved attachments", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const id = "wqi_abcdefghijklmnopqrst";
    const anchor = "0123456789abcdef01234567";
    const url = `kuvo-attachment:file/${anchor}`;
    const image = `![left.png](${url})`;
    const next = "![right.png](kuvo-attachment:legacy/2)";
    const latest = `🎨 The rest of the description stays. ${image} / ${next}\n\n\`${image}\``;
    const stamp = { at: 1760000000000, by: { user: true } };
    write(path.join(queue, "model.json"), '{"formatVersion":1,"model":2}');
    write(path.join(queue, "queues", "usr_you.json"), JSON.stringify({
      formatVersion: 1, owner: "usr_you", items: [{
        id, title: "Pictures", status: "todo", body: latest, created: stamp, updated: stamp,
        files: [{ name: "left.png", type: "image/png", size: 1, anchor }, { name: "right.png", type: "image/png", size: 1 }],
      }],
    }));
    write(path.join(queue, "files", id, "left.png"), "a");
    write(path.join(queue, "files", id, "right.png"), "b");
    const ask = await start();
    const route = `kuvo/work-queue/items/${id}`;
    const remove = { removeInlineImages: [{ url, name: "left.png" }] };
    for (let times = 0; times < 2; times++) {
      const result = await ask(route, "PATCH", remove, { "x-kuvo-from": "app" });
      assert.equal(result.status, 200, result.text);
      assert.equal(result.json.inlineImagesApplied, true);
    }
    const detail = await ask(route);
    assert.equal(detail.status, 200, detail.text);
    assert.equal(detail.json.item.body, latest.replace(image, ""));
    assert.deepEqual(detail.json.item.files.map(({ name }) => name), ["left.png", "right.png"]);
    assert.equal((await ask(`${route}/files/right.png`)).text, "b");
  }));

test("client pictures keep unique names, anchors and bytes", { timeout: 30000 }, () =>
  withQueue(async (queue, start) => {
    const ask = await start();
    const app = { "x-kuvo-from": "app" };
    const draft = "![Attachment](kuvo-attachment:draft/0/Photo.png)";
    const picture = (name, bytes) => ({ name, uri: `data:image/png;base64,${Buffer.from(bytes).toString("base64")}` });
    const added = await ask("kuvo/work-queue/items", "POST", {
      operation: "queue-attach-pictures", title: "Pictures", body: draft,
      files: [picture("Photo.png", "first")],
    }, app);
    assert.equal(added.status, 200, added.text);
    const id = added.json.item.id;
    const route = `kuvo/work-queue/items/${id}`;
    const before = (await ask(route)).json.item;
    assert.match(before.files[0].anchor, /^[0-9a-f]{24}$/);
    assert.equal(before.body, `![Photo.png](kuvo-attachment:file/${before.files[0].anchor})`);
    write(path.join(queue, "files", id, "photo 2.png"), "occupied");
    const saved = await ask(route, "PATCH", {
      body: `${before.body}\n${draft}`, files: [picture("photo.png", "second")],
    }, app);
    assert.equal(saved.status, 200, saved.text);
    const after = (await ask(route)).json.item;
    assert.deepEqual(after.files.map(({ name }) => name), ["Photo.png", "photo 3.png"]);
    assert.equal(after.body, `${before.body}\n![photo 3.png](kuvo-attachment:file/${after.files[1].anchor})`);
    assert.equal((await ask(`${route}/files/Photo.png`)).text, "first");
    assert.equal((await ask(`${route}/files/photo%203.png`)).text, "second");
    assert.equal(readFileSync(path.join(queue, "files", id, "photo 2.png"), "utf8"), "occupied");
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
test("work queue views merge live and deleted rows by their owning computer", { timeout: 120000 }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab");
  const main = studio.connect(await studio.start().ready());
  const setup = await main("GET", "kuvo/setup");
  await until(() => leases.leaseFor(setup.json.setup), "The main did not obtain its number lease.");
  const app = { "x-kuvo-from": "app" };
  const enrollment = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(enrollment.status, 201, enrollment.text);
  const joined = await lab.join(enrollment.json.code);
  assert.equal(joined.code, 0, joined.log);
  const member = lab.connect(await lab.start().ready());
  const items = "kuvo/work-queue/items";
  const added = await member("POST", items, {
    operation: "told-from-lab", title: "Lab prototype", body: "The keeper's full description.",
    files: [{ name: "proof.txt", uri: "data:text/plain;base64,Ynl0ZXM=" }],
  }, app);
  assert.equal(added.status, 200, added.text);
  const id = added.json.item.id;
  const route = "kuvo/work-queue?owner=all&glance=1&counts=1";
  const seen = await until(async () => {
    const answer = await main("GET", route);
    assert.equal(answer.status, 200, answer.text);
    const item = answer.json.items.find(item => item.id === id);
    return item && { view: answer.json, item };
  }, "Studio did not receive the Lab queue.", 30000);
  assert.equal(seen.item.computer.name, "Lab");
  assert.equal(seen.view.counts.computers[seen.item.computer.key], 1);
  const ref = await main("GET", `kuvo/work-queue/refs?refs=${id}`);
  assert.equal(ref.status, 200, ref.text);
  assert.equal(ref.json.refs[id].computer.name, "Lab");
  const copy = JSON.parse(readFileSync(path.join(studio.home, "run", "work-queue", "peers", `${seen.item.computer.key}.json`), "utf8"));
  assert.equal(copy.formatVersion, 1);
  assert.equal(copy.items.find(item => item.id === id).title, "Lab prototype");
  const detail = await main("GET", `${items}/${id}`, undefined, app);
  assert.equal(detail.status, 200, detail.text);
  assert.equal(detail.json.item.told, false);
  assert.equal(detail.json.item.body, "The keeper's full description.");
  const file = await main("GET", `${items}/${id}/files/proof.txt`, undefined, app);
  assert.equal(file.status, 200, file.text);
  assert.equal(file.text, "bytes");
  const edited = await main("PATCH", `${items}/${id}`, { priority: "high" }, app);
  assert.equal(edited.status, 200, edited.text);
  assert.equal(edited.json.item.priority, "high");
  const deleted = await main("DELETE", `${items}/${id}`, undefined, app);
  assert.equal(deleted.status, 200, deleted.text);
  await until(async () => {
    const answer = await main("GET", route);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.removed.some(item => item.id === id && item.computer.key === seen.item.computer.key)
      && !answer.json.items.some(item => item.id === id);
  }, "Studio did not receive the Lab deletion.", 30000);
  const restored = await main("POST", `${items}/${id}/restore`, {}, app);
  assert.equal(restored.status, 200, restored.text);
  await until(async () => {
    const answer = await main("GET", route);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.items.some(item => item.id === id && item.computer.key === seen.item.computer.key)
      && !answer.json.removed.some(item => item.id === id);
  }, "Studio did not receive the Lab restore.", 30000);
  const onKeeper = await member("GET", `${items}/${id}`);
  assert.equal(onKeeper.status, 200, onKeeper.text);
  assert.equal(onKeeper.json.item.priority, "high");
  assert.deepEqual(onKeeper.json.item.events.slice(-2).map(event => event.kind), ["deleted", "restored"]);
});

test("a Markdown backlog keeps its numbers, headings and previous imports", { timeout: 40000 }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio");
  const queue = path.join(studio.home, "run", "work-queue");
  const route = studio.connect(await studio.start().ready());
  const setup = await route("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const ask = (url, method = "GET", body, headers) => route(method, url, body, headers);
  const source = path.join(queue, "..", "..", "backlog.md");
  write(source, [
    "# Done last week",
    "- #62 [x] Sent the release. Add this paragraph.",
    "# Plans",
    "- #60 Confirm the revised tests. Details stay in the item.",
    "- #61 / #63 Gather the remaining questions",
    "- #60 A second line with #64",
    "LATER: #61",
    "NEXT: #62",
    "",
  ].join("\n"));
  const app = { "x-kuvo-from": "app" };
  const creation = await ask("kuvo/work-queue/items", "POST", { operation: "import-test-first", title: "An existing item" }, app);
  assert.equal(creation.status, 200, creation.text);
  const id = creation.json.item.id;
  const used = await until(async () => (await ask("kuvo/work-queue/items/" + id)).json.item.number,
    "The first item did not spend a number from the lease.", 12000);
  const proposal = { owner: "usr_you", files: [source] };
  const first = await ask("kuvo/work-queue/import", "POST", proposal, { "x-kuvo-from": "app" });
  assert.equal(first.status, 200, first.text);
  assert.deepEqual(first.json, {
    added: 3,
    byStatus: { todo: 1, backlog: 1, done: 1 },
    waiting: 0,
    skipped: ["#63 (named with #61 on one line)", "#64 (named, but no line starts with it)"],
    said: "Imported 3 items with their numbers: 1 Todo, 1 Backlog, 1 Done. Left out (2): #63 (named with #61 on one line); #64 (named, but no line starts with it).",
  });
  const item = await ask("kuvo/work-queue/items/60");
  assert.equal(item.status, 200, item.text);
  assert.equal(item.json.item.number, 60);
  assert.equal(item.json.item.title, "Confirm the revised tests.");
  assert.equal(item.json.item.body, "#60 Confirm the revised tests. Details stay in the item.\n\n#60 A second line with #64");
  const retried = await ask("kuvo/work-queue/import", "POST", proposal, { "x-kuvo-from": "app" });
  assert.equal(retried.status, 200, retried.text);
  assert.equal(retried.json.added, 0);
  assert.match(retried.json.said, /#60–62 \(already in the Work Queue here\)/);
  const file = JSON.parse(readFileSync(path.join(queue, "numbers.json"), "utf8"));
  assert.deepEqual(file.used, [[used, used], [60, 62]]);
});

test("a member waits for the main to grant the numbers of its imported items", { timeout: 90000 }, async t => {
  const { computer, leases } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab");
  const main = studio.connect(await studio.start().ready());
  const setup = await main("GET", "kuvo/setup");
  assert.equal(setup.status, 200, setup.text);
  await until(() => leases.leaseFor(setup.json.setup), "The main did not claim its relay lease.");
  const invite = await main("POST", "kuvo/computers/add/code", {}, { "x-kuvo-from": "mac" });
  assert.equal(invite.status, 201, invite.text);
  const joined = await lab.join(invite.json.code);
  assert.equal(joined.code, 0, joined.log);
  const member = lab.connect(await lab.start().ready());
  const source = path.join(lab.home, "backlog.md");
  write(source, "- #240 Follow up on the member's backlog.\n");
  const answer = await member("POST", "kuvo/work-queue/import", { owner: "usr_you", files: source }, { "x-kuvo-from": "app" });
  assert.equal(answer.status, 200, answer.text);
  assert.equal(answer.json.waiting, 1);
  assert.match(answer.json.said, /1 waits for the main computer to grant its number/);
  const result = await until(async () => {
    const item = await member("GET", "kuvo/work-queue/items/240");
    return item.status === 200 && item.json.item;
  }, "The member did not receive its imported number.", 45000, 150);
  assert.equal(result.title, "Follow up on the member's backlog.");
  assert.equal(result.number, 240);
  const ledger = JSON.parse(readFileSync(path.join(studio.home, "run", "work-queue", "numbers.json"), "utf8"));
  const self = await member("GET", "kuvo/setup");
  assert.deepEqual(ledger.granted[self.json.you.key], [[240, 240]]);
});

test("a member saves an offline Work Queue edit and merges it when the keeper returns", { timeout: 150000 }, async t => {
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
  const app = { "x-kuvo-from": "app" };
  const baseBody = "Check the build.\n\nSend the note.";
  const created = await main("POST", "kuvo/work-queue/items", {
    operation: "offline-start", title: "Release plans", body: baseBody,
  }, app);
  assert.equal(created.status, 200, created.text);
  const id = created.json.item.id;
  const ref = { kind: "queueItems", queue: "usr_you", id, computer: setup.json.you.key };
  const address = "kuvo/data/edits?" + new URLSearchParams(ref);
  await until(async () => {
    const list = await member("GET", "kuvo/work-queue?owner=all&glance=1", undefined, app);
    return list.json?.items.some(item => item.id === id);
  }, "The member did not observe the item.", 40000, 100);
  const revision = await member("GET", address, undefined, app);
  assert.equal(revision.status, 200, revision.text);
  assert.equal(revision.json.stale, false);
  assert.equal(revision.json.fields.body, baseBody);
  const local = await main("PATCH", "kuvo/work-queue/items/" + id, {
    title: "Release plans for next week", body: "Check the release build.\n\nSend the note.",
  }, app);
  assert.equal(local.status, 200, local.text);
  await primary.stop();
  const draft = { ref, operation: "offline-from-lab", baseRevision: revision.json.revision, fields: {
    body: "Check the build.\n\nSend the note to the team.", priority: "high",
  }};
  const pending = await member("POST", "kuvo/data/edits", draft, app);
  assert.equal(pending.status, 202, pending.text);
  assert.equal(pending.json.status, "pending");
  const operation = pending.json.operation;
  const entries = await member("GET", "kuvo/data/edits/outbox", undefined, app);
  assert.equal(entries.status, 200, entries.text);
  assert.ok(entries.json.entries.some(row => row.receipt.operation === operation));
  const resumed = studio.connect(await studio.start().ready());
  const receipt = await until(async () => {
    const answer = await member("GET", "kuvo/data/edits/" + operation, undefined, app);
    assert.equal(answer.status, 200, answer.text);
    return answer.json.receipt.status !== "pending" && answer.json.receipt;
  }, "The edit was not replayed to the keeper.", 45000, 150);
  assert.equal(receipt.status, "committed");
  const changed = await resumed("GET", "kuvo/work-queue/items/" + id);
  assert.equal(changed.status, 200, changed.text);
  assert.equal(changed.json.item.title, "Release plans for next week");
  assert.equal(changed.json.item.body, "Check the release build.\n\nSend the note to the team.");
  assert.equal(changed.json.item.priority, "high");
});

test("ordinary revision-aware queue routes and the outbox replay the same decisions", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const studio = await computer("Studio");
  const ask = studio.connect(await studio.start().ready());
  const app = { "x-kuvo-from": "app" };
  const id = "wqi_BBBBBBBBBBBBBBBBBBBB";
  const ref = { kind: "queueItems", id, queue: "usr_you" };
  const address = "kuvo/work-queue/items/" + id;
  const submit = edit => ask("POST", "kuvo/data/edits", { ref, ...edit }, app);
  const create = { operation: "first-offline-replay", baseRevision: null, create: true, fields: { title: "Original" } };
  const made = await ask("POST", "kuvo/work-queue/items", { id, owner: ref.queue, baseRevision: null, fields: create.fields },
    { ...app, "x-kuvo-operation": create.operation });
  assert.equal(made.status, 200, made.text);
  assert.equal(made.json.status, "committed");
  assert.deepEqual((await submit(create)).json, made.json);
  const edit = { operation: "second-offline-replay", baseRevision: made.json.revision, fields: { title: "First name" } };
  const changed = await ask("PATCH", address, { owner: ref.queue, ...edit }, app);
  assert.equal(changed.status, 200, changed.text);
  assert.equal(changed.json.status, "committed");
  assert.deepEqual((await submit(edit)).json, changed.json);
  const conflict = await submit({ operation: "choice-for-title", baseRevision: made.json.revision, fields: { title: "Second name" } });
  assert.equal(conflict.status, 200, conflict.text);
  assert.equal(conflict.json.status, "choice");
  assert.equal(conflict.json.choices.length, 1);
  const removed = await ask("DELETE", address, { owner: ref.queue, operation: "delete-with-revision", baseRevision: conflict.json.revision }, app);
  assert.equal(removed.status, 200, removed.text);
  assert.equal(removed.json.status, "committed");
  const failed = await submit({ operation: "try-deleted-item", baseRevision: changed.json.revision, fields: { body: "Saved draft" } });
  assert.equal(failed.status, 409, failed.text);
  assert.equal(failed.json.status, "deleted");
  const outbox = await ask("GET", "kuvo/data/edits/" + failed.json.operation, undefined, app);
  assert.equal(outbox.status, 200, outbox.text);
  assert.deepEqual(outbox.json.recovery, { body: "Saved draft" });
  const restore = await ask("POST", address + "/restore", {}, app);
  assert.equal(restore.status, 200, restore.text);
  const stale = await submit({ operation: "try-before-deletion", baseRevision: changed.json.revision, fields: { body: "Stale draft" } });
  assert.equal(stale.status, 409, stale.text);
  assert.equal(stale.json.status, "refresh");
});
