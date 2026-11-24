// Library and Artifacts conformance (rust-box.md row 5.5a):
// Both boxes (TS and Rust) answer the same routes, bytes, diffs, streams, and format refusals.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { boxBinary, boxCommand, isolatedEnv, scratch } from "./adapter.mjs";
import { bench } from "./bench.mjs";

const sha256 = data => createHash("sha256").update(data).digest("hex");
const artId = filePath => "art_" + sha256(filePath).slice(0, 24);
const libId = filePath => "lib_" + sha256(filePath).slice(0, 24);

function seedArtifacts(home, entries, copies = []) {
  mkdirSync(path.join(home, "artifacts", "log"), { recursive: true });
  mkdirSync(path.join(home, "artifacts", "copies"), { recursive: true });
  writeFileSync(path.join(home, "artifacts", "key"), Buffer.alloc(32, 0x24));
  for (const [hex, bytes] of copies) {
    const dir = path.join(home, "artifacts", "copies", hex.slice(0, 2));
    mkdirSync(dir, { recursive: true });
    writeFileSync(path.join(dir, hex), bytes);
  }
  const lines = entries.map(e => JSON.stringify({ ...e, formatVersion: 1 })).join("\n") + "\n";
  writeFileSync(path.join(home, "artifacts", "log", "2026-10.jsonl"), lines);
}

function seedLibrary(home, items) {
  mkdirSync(path.join(home, "library"), { recursive: true });
  const lines = items.map(i => JSON.stringify({ ...i, formatVersion: 1 })).join("\n") + "\n";
  writeFileSync(path.join(home, "library", "log.jsonl"), lines);
  writeFileSync(path.join(home, "library", "backfill.json"), JSON.stringify({ done: 1 }));
}

test("artifacts routes: scopes, versions, bytes, prose/code diffs, pin, restore, storage and budget", async t => {
  const { computer } = await bench(t);
  const box = await computer("Artifacts");
  const project = path.join(realpathSync(box.home), "project");
  const bots = path.join(realpathSync(box.home), "bots");
  mkdirSync(project, { recursive: true });
  mkdirSync(path.join(bots, "bot_ada", "workspace"), { recursive: true });
  writeFileSync(path.join(bots, "bot_ada", "bot.json"), JSON.stringify({ name: "Ada", look: { color: "mint", shape: "gem", face: "calm" } }));

  const doc = path.join(project, "notes.md");
  const v1Text = "The user likes tea in the morning.\n";
  const v2Text = "The user likes coffee in the morning.\n";
  writeFileSync(doc, v2Text);
  const h1 = sha256(v1Text);
  const h2 = sha256(v2Text);
  const id = artId(doc);

  const header = {
    id, path: doc, name: "notes.md", kind: "document", type: "text/markdown",
    where: { bots: ["bot_ada"], project, sessions: ["ses_code"], groups: [] },
    pinned: null, gone: null, updated: "2026-10-26T10:01:00.000Z",
  };
  const v1 = {
    n: 1, at: "2026-10-26T10:00:00.000Z", by: { kind: "agent", session: "ses_code", bot: "bot_ada", agent: "engine" },
    turn: { session: "ses_code", message: "m1" }, via: ["write"], status: "added", edits: 1,
    lines: { added: 1, removed: 0 }, size: Buffer.byteLength(v1Text), hash: "sha256:" + h1, kept: true,
  };
  const v2 = {
    n: 2, at: "2026-10-26T10:01:00.000Z", by: { kind: "agent", session: "ses_code", bot: "bot_ada", agent: "engine" },
    turn: { session: "ses_code", message: "m2" }, via: ["edit"], status: "modified", edits: 1,
    lines: { added: 1, removed: 1 }, size: Buffer.byteLength(v2Text), hash: "sha256:" + h2, kept: true,
  };

  // Also seed a PNG binary artifact
  const pic = path.join(project, "shot.png");
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00, 0xc3, 0x28]);
  writeFileSync(pic, png);
  const hp = sha256(png);
  const picId = artId(pic);
  const picHeader = {
    id: picId, path: pic, name: "shot.png", kind: "image", type: "image/png",
    where: { bots: [], project, sessions: ["ses_code"], groups: [] },
    pinned: null, gone: null, updated: "2026-10-26T10:02:00.000Z",
  };
  const vp = {
    n: 1, at: "2026-10-26T10:02:00.000Z", by: { kind: "agent", session: "ses_code", agent: "engine" },
    turn: { session: "ses_code", message: "m3" }, via: ["generate_image"], status: "added", edits: 1,
    size: png.length, hash: "sha256:" + hp, kept: true,
  };

  seedArtifacts(
    box.home,
    [{ a: header, v: v1 }, { a: header, v: v2 }, { a: picHeader, v: vp }],
    [[h1, Buffer.from(v1Text)], [h2, Buffer.from(v2Text)], [hp, png]],
  );

  const ask = box.connect(await box.start().ready());

  await t.test("scope validation and list ordering", async () => {
    assert.equal((await ask("GET", "kuvo/artifacts")).status, 400);
    assert.equal((await ask("GET", "kuvo/artifacts?bot=bot_ada&group=grp_team")).status, 400);
    assert.equal((await ask("GET", "kuvo/artifacts?bot=bot_ada&kind=poem")).status, 400);
    assert.equal((await ask("POST", "kuvo/artifacts?bot=bot_ada", {})).status, 405);
    assert.equal((await ask("GET", "kuvo/artifacts/art_000000000000000000000000")).status, 404);

    const listed = await ask("GET", "kuvo/artifacts?session=ses_code");
    assert.equal(listed.status, 200, listed.text);
    assert.deepEqual(listed.json.artifacts.map(a => a.name), ["shot.png", "notes.md"]);
    assert.equal(listed.json.next, null);
    assert.equal(listed.json.artifacts[1].restorable, true);
  });

  await t.test("detail, version bytes, prose changes and image pair changes", async () => {
    const detail = await ask("GET", `kuvo/artifacts/${id}`);
    assert.equal(detail.status, 200);
    assert.equal(detail.json.versions.length, 2);

    const b1 = await ask("GET", `kuvo/artifacts/${id}/versions/1`);
    assert.equal(b1.status, 200);
    assert.equal(b1.text, v1Text);

    const bp = await ask("GET", `kuvo/artifacts/${picId}/versions/1`);
    assert.equal(bp.status, 200);
    assert.equal(bp.type, "image/png");
    assert.deepEqual(bp.bytes, png);

    const diff = await ask("GET", `kuvo/artifacts/${id}/versions/2/changes`);
    assert.equal(diff.status, 200);
    assert.equal(diff.json.style, "words");
    assert.equal(diff.json.from, 1);
    assert.equal(diff.json.to, 2);
    assert.deepEqual(diff.json.lines[0].runs, [
      { t: " ", text: "The user likes " },
      { t: "-", text: "tea" },
      { t: "+", text: "coffee" },
      { t: " ", text: " in the morning." },
    ]);

    const pair = await ask("GET", `kuvo/artifacts/${picId}/versions/1/changes`);
    assert.equal(pair.status, 200);
    assert.deepEqual(pair.json, { style: "pair", from: null, to: 1 });
  });

  await t.test("timeline, pin, restore, storage and budget confirmation", async () => {
    const tl = await ask("GET", "kuvo/artifacts/timeline?session=ses_code");
    assert.equal(tl.status, 200);
    assert.deepEqual(tl.json.entries.map(e => [e.artifact.name, e.version.n]), [
      ["shot.png", 1], ["notes.md", 2], ["notes.md", 1],
    ]);

    const pinned = await ask("POST", `kuvo/artifacts/${id}/pin`, { pinned: true, note: "Keep" });
    assert.equal(pinned.status, 200);
    assert.equal(pinned.json.pinned.by, "you");
    assert.equal(pinned.json.pinned.note, "Keep");

    const reordered = await ask("GET", "kuvo/artifacts?session=ses_code");
    assert.deepEqual(reordered.json.artifacts.map(a => a.name), ["notes.md", "shot.png"]);
    await ask("POST", `kuvo/artifacts/${id}/pin`, { pinned: false });

    const restored = await ask("POST", `kuvo/artifacts/${id}/restore`, { version: 1 });
    assert.equal(restored.status, 200, restored.text);
    assert.equal(readFileSync(doc, "utf8"), v1Text);

    const storage = await ask("GET", "kuvo/artifacts/storage");
    assert.equal(storage.status, 200);
    assert.ok(storage.json.older > 0);

    // Lowering budget below older without confirmed returns 400
    assert.equal((await ask("POST", "kuvo/artifacts/storage", { budget: 0 })).status, 400);
    // Stale was returns 409
    assert.equal((await ask("POST", "kuvo/artifacts/storage", { budget: 0, was: 999999, confirmed: true })).status, 409);

    const cleared = await ask("POST", "kuvo/artifacts/storage/clear", {});
    assert.equal(cleared.status, 200);
    assert.ok(cleared.json.cleared >= 1);
    assert.equal(cleared.json.older, 0);
  });

  await t.test("setup outputs lists session artifacts", async () => {
    assert.equal((await ask("GET", "kuvo/setup/outputs")).status, 400);
    const out = await ask("GET", "kuvo/setup/outputs?session=ses_code");
    assert.equal(out.status, 200, out.text);
    assert.deepEqual(out.json.artifacts.map(a => a.name).sort(), ["notes.md", "shot.png"]);
    assert.deepEqual(out.json.pendingComputers, []);
  });
});

test("library routes: list, filter, hide, gone fallback, backfill and format refusal", async t => {
  const { computer } = await bench(t);
  const box = await computer("Library");
  const project = path.join(realpathSync(box.home), "project");
  const bots = path.join(realpathSync(box.home), "bots");
  mkdirSync(project, { recursive: true });
  mkdirSync(path.join(bots, "bot_ada", "workspace"), { recursive: true });
  writeFileSync(path.join(bots, "bot_ada", "bot.json"), JSON.stringify({ name: "Ada", look: { color: "mint", shape: "gem", face: "calm" } }));

  const pdf = path.join(project, "brief.pdf");
  const img = path.join(project, "hero.png");
  const goneImg = path.join(project, "deleted.png");
  const pngBytes = Buffer.from([0x89, 0x50, 0x4e, 0x47, 1, 2, 3, 4]);
  writeFileSync(pdf, "%PDF-1.4 brief");
  writeFileSync(img, pngBytes);
  const goneHex = sha256(pngBytes);

  // Seed artifact copy for goneImg so Library serves it even after file deletion
  seedArtifacts(
    box.home,
    [{
      a: {
        id: artId(goneImg), path: goneImg, name: "deleted.png", kind: "image", type: "image/png",
        where: { bots: ["bot_ada"], project, sessions: ["ses_1"], groups: [] },
        pinned: null, gone: null, updated: "2026-10-26T10:00:00.000Z",
      },
      v: {
        n: 1, at: "2026-10-26T10:00:00.000Z", by: { kind: "agent", bot: "bot_ada" },
        via: ["generate_image"], status: "added", edits: 1, size: pngBytes.length,
        hash: "sha256:" + goneHex, kept: true,
      },
    }],
    [[goneHex, pngBytes]],
  );

  seedLibrary(box.home, [
    { id: libId(img), path: img, name: "hero.png", kind: "image", type: "image/png", how: "generated", bot: "bot_ada", session: "ses_1", at: "2026-10-26T10:05:00.000Z" },
    { id: libId(pdf), path: pdf, name: "brief.pdf", kind: "document", type: "application/pdf", how: "shown", session: "ses_2", at: "2026-10-26T10:04:00.000Z" },
    { id: libId(goneImg), path: goneImg, name: "deleted.png", kind: "image", type: "image/png", how: "generated", bot: "bot_ada", session: "ses_1", at: "2026-10-26T10:03:00.000Z" },
  ]);

  const ask = box.connect(await box.start().ready());

  const all = await ask("GET", "kuvo/library");
  assert.equal(all.status, 200, all.text);
  assert.deepEqual(all.json.items.map(i => i.name), ["hero.png", "brief.pdf", "deleted.png"]);
  assert.equal(all.json.sources.you, true);
  assert.equal(all.json.sources.bots[0].name, "Ada");

  assert.deepEqual((await ask("GET", "kuvo/library?source=bot_ada")).json.items.map(i => i.name), ["hero.png", "deleted.png"]);
  assert.deepEqual((await ask("GET", "kuvo/library?source=you")).json.items.map(i => i.name), ["brief.pdf"]);
  assert.deepEqual((await ask("GET", "kuvo/library?kind=document")).json.items.map(i => i.name), ["brief.pdf"]);
  assert.deepEqual((await ask("GET", "kuvo/library?q=ada")).json.items.map(i => i.name), ["hero.png", "deleted.png"]);
  assert.equal((await ask("GET", "kuvo/library?kind=video")).status, 400);

  // Gone image opens from artifact copy
  const goneFile = await ask("GET", `kuvo/library/${libId(goneImg)}/file`);
  assert.equal(goneFile.status, 200);
  assert.deepEqual(goneFile.bytes, pngBytes);

  // Hide and restore
  const hidden = await ask("POST", `kuvo/library/${libId(pdf)}/hide`, { hidden: true });
  assert.equal(hidden.status, 200);
  assert.equal(typeof hidden.json.hidden, "number");
  assert.deepEqual((await ask("GET", "kuvo/library")).json.items.map(i => i.name), ["hero.png", "deleted.png"]);
  assert.deepEqual((await ask("GET", "kuvo/library?hidden=1")).json.items.map(i => i.name), ["brief.pdf"]);
  const unhidden = await ask("POST", `kuvo/library/${libId(pdf)}/hide`, { hidden: false });
  assert.equal(unhidden.status, 200);
  assert.equal(unhidden.json.hidden, undefined);
});

test("future artifact index formatVersion is refused with 409 without touching the log", async t => {
  const { computer } = await bench(t);
  const box = await computer("FutureFormat");
  const logFile = path.join(box.home, "artifacts", "log", "2026-08.jsonl");
  mkdirSync(path.dirname(logFile), { recursive: true });
  const futureLine = '{"formatVersion":2}\n';
  writeFileSync(logFile, futureLine);
  const ask = box.connect(await box.start().ready());
  const reply = await ask("GET", "kuvo/artifacts?bot=bot_ada");
  assert.equal(reply.status, 409, reply.text);
  assert.equal(reply.json.code, "unsupportedFormatVersion");
  assert.equal(readFileSync(logFile, "utf8"), futureLine);
});

test("library backfill runs once from saved sessions", async t => {
  const { computer } = await bench(t);
  const box = await computer("Backfill");
  const project = path.join(realpathSync(box.home), "project");
  const saved = path.join(box.home, "sessions", "ses_old");
  mkdirSync(project, { recursive: true });
  mkdirSync(saved, { recursive: true });
  const deck = path.join(project, "deck.pdf");
  writeFileSync(deck, "%PDF-1.4");
  writeFileSync(path.join(saved, "session.json"), JSON.stringify({
    id: "ses_old", title: "Old", location: { directory: project }, time: { created: 1, updated: 2 },
  }));
  writeFileSync(path.join(saved, "messages.json"), JSON.stringify([{
    id: "msg_a", type: "assistant", time: { created: 2000, completed: 3000 },
    content: [{ type: "tool", id: "t1", name: "kuvo-browser_show_file", state: { status: "completed", input: { path: "deck.pdf" } } }],
  }]));
  const ask = box.connect(await box.start().ready());
  const listed = await ask("GET", "kuvo/library");
  assert.equal(listed.status, 200, listed.text);
  assert.deepEqual(listed.json.items.map(i => [i.name, i.how, i.session]), [["deck.pdf", "shown", "ses_old"]]);
});

test("artifact version binary bytes come back whole over route command", async t => {
  const bench_ = await bench(t);
  const box = await bench_.computer("RoutedArtifacts");
  const project = path.join(realpathSync(box.home), "project");
  mkdirSync(project, { recursive: true });
  const pic = path.join(project, "img.png");
  const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00, 0xc3, 0x28]);
  writeFileSync(pic, png);
  const hp = sha256(png);
  const id = artId(pic);
  seedArtifacts(
    box.home,
    [{
      a: {
        id, path: pic, name: "img.png", kind: "image", type: "image/png",
        where: { bots: [], project, sessions: ["ses_1"], groups: [] },
        pinned: null, gone: null, updated: "2026-10-26T10:00:00.000Z",
      },
      v: {
        n: 1, at: "2026-10-26T10:00:00.000Z", by: { kind: "agent" },
        via: ["write"], status: "added", edits: 1, size: png.length, hash: "sha256:" + hp, kept: true,
      },
    }],
    [[hp, png]],
  );
  await box.start().ready();
  const binary = boxBinary();
  const env = { ...isolatedEnv(path.dirname(box.home)), KUVO_SANDBOX: "off" };
  const command = boxCommand(binary, box.home, env, bench_.relay, binary.typescript ? ["box", "route"] : ["route"], {}, "pipe");
  let out = "";
  command.child.stdout?.on("data", chunk => { out += chunk; });
  command.child.stdin.end(JSON.stringify({ method: "GET", path: `kuvo/artifacts/${id}/versions/1` }));
  const { code } = await command.exited;
  assert.equal(code, 0, command.log());
  const answer = JSON.parse(out.trim().split("\n").at(-1));
  assert.equal(answer.status, 200);
  assert.equal(answer.type, "image/png");
  assert.deepEqual(Buffer.from(answer.bodyBase64, "base64"), png);
});
