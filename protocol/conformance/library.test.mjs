// Library and Artifacts conformance (rust-box.md row 5.5a):
// Both boxes (TS and Rust) answer the same routes, bytes, diffs, streams, and format refusals.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { boxBinary, boxCommand, isolatedEnv, request, scratch, until } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedPeerSetup } from "./peer-fixture.mjs";

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
  const badKind = await ask("GET", "kuvo/library?kind=video");
  assert.equal(badKind.status, 200);
  assert.deepEqual(badKind.json.items, []);
  assert.equal(badKind.json.computers[0].state, "failed");
  assert.equal((await ask("GET", "kuvo/setup/library/events?kind=video")).status, 400);

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
  const saved = path.join(box.home, "agents", "sessions", "ses_old");
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

test("library thumbnails, followed event lines, and future library format refusal", async t => {
  const { computer } = await bench(t);
  const box = await computer("StreamsAndThumbs");
  const project = path.join(realpathSync(box.home), "project");
  mkdirSync(project, { recursive: true });
  const img = path.join(project, "pixel.png");
  const bin = path.join(project, "archive.bin");
  // Minimal valid 1x1 PNG
  const png = Buffer.from(
    "89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de0000000c49444154789c63f8cfc000000301010018dd8db00000000049454e44ae426082",
    "hex",
  );
  writeFileSync(img, png);
  writeFileSync(bin, Buffer.from([0, 1, 2, 3, 4]));
  const imgHex = sha256(png);
  seedArtifacts(
    box.home,
    [{
      a: {
        id: artId(img), path: img, name: "pixel.png", kind: "image", type: "image/png",
        where: { bots: ["bot_ada"], project, sessions: ["ses_s1"], groups: [] },
        pinned: null, gone: null, updated: "2026-10-26T10:00:00.000Z",
      },
      v: {
        n: 1, at: "2026-10-26T10:00:00.000Z", by: { kind: "agent", bot: "bot_ada" },
        via: ["generate_image"], status: "added", edits: 1, size: png.length, hash: "sha256:" + imgHex, kept: true,
      },
    }],
    [[imgHex, png]],
  );
  seedLibrary(box.home, [
    { id: libId(img), path: img, name: "pixel.png", kind: "image", type: "image/png", how: "generated", bot: "bot_ada", session: "ses_s1", at: "2026-10-26T10:05:00.000Z" },
    { id: libId(bin), path: bin, name: "archive.bin", kind: "other", type: "application/octet-stream", how: "attached", session: "ses_s1", at: "2026-10-26T10:04:00.000Z" },
  ]);

  const ask = box.connect(await box.start().ready());

  // Thumbnail for PNG returns PNG bytes; non-thumbnailable returns 404
  const thumb = await ask("GET", `kuvo/library/${libId(img)}/thumbnail`);
  assert.equal(thumb.status, 200);
  assert.equal(thumb.bytes.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");
  assert.equal((await ask("GET", `kuvo/library/${libId(bin)}/thumbnail`)).status, 404);

  // Merged setup library and setup outputs over kuvo/line?once=1
  const watch = Buffer.from(JSON.stringify({
    lib: "kuvo/setup/library/events",
    outputs: "kuvo/setup/outputs/events?session=ses_s1",
  })).toString("base64url");
  const line = await ask("GET", `kuvo/line?once=1&watch=${watch}`);
  assert.equal(line.status, 200, line.text);
  const frames = line.text.trim().split("\n").map(s => JSON.parse(s));
  const libFrame = frames.find(f => f.t === "lib");
  const outFrame = frames.find(f => f.t === "outputs");
  assert.ok(libFrame, `missing lib frame in ${line.text}`);
  assert.deepEqual(libFrame.d.items.map(i => i.name), ["pixel.png", "archive.bin"]);
  assert.ok(outFrame, `missing outputs frame in ${line.text}`);
  assert.deepEqual(outFrame.d.artifacts.map(a => a.name), ["pixel.png"]);
});

test("future library log formatVersion is refused without modifying the log", async t => {
  const { computer } = await bench(t);
  const box = await computer("FutureLibFormat");
  const logFile = path.join(box.home, "library", "log.jsonl");
  mkdirSync(path.dirname(logFile), { recursive: true });
  const futureLine = '{"formatVersion":2}\n';
  writeFileSync(logFile, futureLine);
  writeFileSync(path.join(box.home, "library", "backfill.json"), JSON.stringify({ done: 1 }));
  const ask = box.connect(await box.start().ready());
  const list = await ask("GET", "kuvo/library");
  assert.equal(list.status, 200);
  assert.equal(list.json.computers[0].state, "failed");
  assert.equal(readFileSync(logFile, "utf8"), futureLine);
});

test("merged Library and Outputs views include away peers' kept copies and pending computers", { timeout: 30000 }, async t => {
  const { computer, relay } = await bench(t);
  const near = await computer("Studio");
  const livePeer = await computer("Spare");
  const { main, member, spare, save } = seedPeerSetup(near.home, relay.url, 1, 3);
  seedPeerSetup(livePeer.home, relay.url, 3, 3);
  const seen = 1_700_000_000_000;

  // Local library item for bot_ada
  const project = path.join(realpathSync(near.home), "project");
  mkdirSync(project, { recursive: true });
  const localBrief = path.join(project, "brief.pdf");
  writeFileSync(localBrief, "%PDF-1.4 local");
  seedLibrary(near.home, [
    { id: libId(localBrief), path: localBrief, name: "brief.pdf", kind: "document", type: "application/pdf", how: "shown", bot: "bot_ada", session: "ses_near", at: "2026-10-26T10:00:00.000Z" },
  ]);

  // Lab (member) is away with kept library-v1.json, outputs-v1.json, and lists-v1.json (delegated session);
  // Spare is connected on the relay with its mail held so its first copy hasn't arrived yet -> pendingComputers.
  const catalog = { agents: [], offered: { known: { engine: false, agents: false }, models: [] } };
  save("run/peer-agents.json", {
    [member.key]: { at: seen, heard: seen, list: [], kind: member.kind, features: ["library", "line.merged-library", "artifacts", "line.merged-outputs"] },
  });
  save("run/peer-reach.json", { [member.key]: seen });
  save("run/peer-reach-checked.json", { [member.key]: Date.now() });
  save(`run/peers/${member.key}/lists-v1.json`, {
    at: seen, seen,
    sessions: [{ id: "ses_far", facts: { kind: "chat", directory: "/scratch", title: "Delegated", created: 10, updated: 20, parent: "ses_near" }, actions: ["open", "manage"] }],
    projects: [], queues: [], waiting: [], catalog,
  });
  save(`run/peers/${member.key}/library-v1.json`, {
    at: seen, seen,
    items: [{
      id: "lib_" + "a".repeat(24), name: "brief.pdf", kind: "document", type: "application/pdf",
      how: "shown", path: "/far/brief.pdf", bot: "bot_ada", session: "ses_far",
      at: Date.parse("2026-10-26T10:05:00.000Z") / 1000, there: true, thumbnail: false,
    }],
    sources: { bots: [{ id: "bot_ada", name: "Ada" }], you: false },
    botNames: { bot_ada: "Ada" },
  });
  save(`run/peers/${member.key}/outputs-v1.json`, {
    at: seen, seen,
    artifacts: [{
      id: "art_" + "b".repeat(24), path: "/far/delegated.txt", name: "delegated.txt", kind: "document",
      type: "text/plain", gone: null, updated: Date.parse("2026-10-26T10:06:00.000Z") / 1000, pinned: false,
      latest: { n: 1, hash: "sha256:" + "c".repeat(64) }, sessions: ["ses_far"],
    }],
  });

  const from = Buffer.from(spare.address, "base64url");
  const to = Buffer.from(main.address, "base64url");
  const release = relay.intercept(from, to, () => true);
  t.after(release);
  await livePeer.start().ready();
  await until(() => relay.has(from), "Spare did not connect to the relay");

  const ask = near.connect(await near.start().ready());
  const watch = Buffer.from(JSON.stringify({
    lib: "kuvo/setup/library/events?source=bot_ada&kind=document&q=brief",
    outputs: "kuvo/setup/outputs/events?session=ses_near",
  })).toString("base64url");
  const line = await ask("GET", `kuvo/line?once=1&watch=${watch}`);
  assert.equal(line.status, 200, line.text);
  const frames = line.text.trim().split("\n").map(s => JSON.parse(s));

  const libFrame = frames.find(f => f.t === "lib");
  assert.ok(libFrame, `missing lib frame in ${line.text}`);
  assert.deepEqual(
    libFrame.d.items.map(i => ({ name: i.name, there: i.there, computer: i.computer.name })),
    [
      { name: "brief.pdf", there: false, computer: member.name },
      { name: "brief.pdf", there: true, computer: main.name },
    ],
  );
  const labComp = libFrame.d.computers.find(c => c.key === member.key);
  assert.equal(labComp.state, "away");
  assert.equal(typeof labComp.seen, "number");
  assert.ok(
    libFrame.d.pendingComputers.some(c => c.id === spare.key && c.name === spare.name),
    `expected ${spare.name} in pendingComputers: ${JSON.stringify(libFrame.d.pendingComputers)}`,
  );

  const outFrame = frames.find(f => f.t === "outputs");
  assert.ok(outFrame, `missing outputs frame in ${line.text}`);
  assert.equal(outFrame.d.artifacts.length, 1);
  assert.equal(outFrame.d.artifacts[0].name, "delegated.txt");
  assert.equal(outFrame.d.artifacts[0].computer.id, member.key);
  assert.equal(outFrame.d.artifacts[0].away, true);
  assert.equal(typeof outFrame.d.artifacts[0].lastSeen, "number");
});

test("an output change doesn't republish an unchanged Library to another computer", {timeout:30000, skip:boxBinary().typescript}, async t => {
  const {computer,relay}=await bench(t);
  const sender=await computer("Studio"), receiver=await computer("Lab");
  const {main,member}=seedPeerSetup(sender.home,relay.url,1,2);
  seedPeerSetup(receiver.home,relay.url,2,2);
  const file=path.join(realpathSync(sender.home),"notes.txt");
  writeFileSync(file,"notes");
  const hash=sha256("notes"), id=artId(file), at="2026-10-26T10:00:00.000Z";
  const loose=path.join(realpathSync(sender.home),"loose.txt");
  writeFileSync(loose,"untracked");
  seedLibrary(sender.home,[{id:libId(file),path:file,name:"notes.txt",kind:"document",type:"text/plain",how:"shown",session:"ses_notes",at},
    {id:libId(loose),path:loose,name:"loose.txt",kind:"document",type:"text/plain",how:"shown",session:"ses_notes",at}]);
  seedArtifacts(sender.home,[{a:{id,path:file,name:"notes.txt",kind:"document",type:"text/plain",where:{bots:[],sessions:["ses_notes"],groups:[]},pinned:null,gone:null,updated:at},v:{n:1,at,by:{kind:"agent",session:"ses_notes"},via:["write"],status:"added",edits:1,size:5,hash:"sha256:"+hash,kept:true}}],[[hash,Buffer.from("notes")]]);
  await receiver.start().ready();
  const ask=sender.connect(await sender.start().ready());
  await until(()=>relay.has(Buffer.from(main.address,"base64url")) && relay.has(Buffer.from(member.address,"base64url")),"Both computers never connected.");
  const copy=name=>{
    try {return JSON.parse(readFileSync(path.join(receiver.home,"run/peers",main.key,name),"utf8"));} catch {return null;}
  };
  await until(()=>copy("library-v1.json")?.items?.length===2 && copy("outputs-v1.json")?.artifacts?.length===1,"The initial copies never arrived.");
  const seen=copy("library-v1.json").seen;
  const pinned=await ask("POST",`kuvo/artifacts/${id}/pin`,{pinned:true});
  assert.equal(pinned.status,200,pinned.text);
  await until(()=>copy("outputs-v1.json")?.artifacts?.[0]?.pinned===true,"The pinned output was never sent.");
  assert.equal(copy("library-v1.json").seen,seen,"An unchanged Library was sent again.");
  rmSync(loose);
  await until(()=>copy("library-v1.json")?.items?.find(item=>item.id===libId(loose))?.there===false,"The disappeared file was never reflected in the peer Library.");
});

test("marketplace, programs and mcp catalog routes match across boxes", async t => {
  const { computer } = await bench(t);
  const box = await computer("MarketBox");
  mkdirSync(path.join(box.home, "box"), { recursive: true });
  mkdirSync(path.join(box.home, "bin"), { recursive: true });
  writeFileSync(path.join(box.home, "box", "opencode.json"), "{}\n");
  writeFileSync(path.join(box.home, "bin", "mcp-catalog.json"), JSON.stringify({
    servers: [
      { id: "context7", title: "Context7", summary: "Docs", access: "open", settings: { type: "remote", url: "https://c7.example/mcp" } },
      { id: "linear", title: "Linear", summary: "Issues", access: "signIn", settings: { type: "remote", url: "https://linear.example/mcp" } },
      { id: "later", title: "Future", summary: "Unknown access", access: "pairing" },
    ],
  }));

  const ready = await box.start().ready();
  const ask = box.connect(ready);
  const appAsk = (method, route, body) => request(ready, method, route, body, { "x-kuvo-from": "app" });

  await t.test("kuvo/programs reports missing, relative/variable unknown, and bad empty name", async () => {
    assert.equal((await ask("GET", "kuvo/programs?name=")).status, 400);
    const missing = await ask("GET", "kuvo/programs?name=kuvo-definitely-missing-bin-xyz");
    assert.equal(missing.status, 200);
    assert.equal(missing.json.path, null);
    assert.equal(missing.json.unknown, undefined);
    const unk = await ask("GET", "kuvo/programs?name=.%2Fserver");
    assert.equal(unk.status, 200);
    assert.deepEqual(unk.json, { name: "./server", path: null, unknown: true });
  });

  await t.test("kuvo/mcp/catalog lists known access words and adds a server with codemode false", async () => {
    const cat = await ask("GET", "kuvo/mcp/catalog");
    assert.equal(cat.status, 200);
    assert.deepEqual(cat.json.servers.map(s => [s.id, s.access, s.added]), [
      ["context7", "open", false],
      ["linear", "signIn", false],
    ]);
    const added = await ask("POST", "kuvo/mcp/catalog/context7", {});
    assert.equal(added.status, 200);
    assert.equal(added.json.mcp.servers.context7.codemode, false);
    assert.equal((await ask("POST", "kuvo/mcp/catalog/later", {})).status, 404);
  });

  await t.test("kuvo/marketplace serves bundled catalog, PNG logo, custom app projection, and skill lifecycle", async () => {
    const listed = await appAsk("GET", "kuvo/marketplace");
    assert.equal(listed.status, 200);
    const [apps, agents, skills] = listed.json.areas;
    assert.deepEqual([apps.count, agents.count, skills.count], [10, 6, 8]);
    assert.ok(apps.items.some(a => a.id === "github" && a.logo === "kuvo/marketplace/apps/github/logo"));
    assert.ok(!apps.items.some(a => a.id === "slack"));

    const logo = await appAsk("GET", "kuvo/marketplace/apps/github/logo");
    assert.equal(logo.status, 200);
    assert.equal(logo.bytes.subarray(0, 8).toString("hex"), "89504e470d0a1a0a");

    // Add custom open app -> projected to opencode.json -> remove -> cleaned from opencode.json
    const cust = await appAsk("POST", "kuvo/marketplace/apps/custom", {
      title: "Docs MCP", url: "https://docs.example.com/mcp", auth: "open",
    });
    assert.equal(cust.status, 200);
    assert.equal(cust.json.id, "docs-mcp");
    const cfg1 = JSON.parse(readFileSync(path.join(box.home, "box", "opencode.json"), "utf8"));
    assert.equal(cfg1.mcp.servers["marketplace-docs-mcp"].url, "https://docs.example.com/mcp");
    assert.equal((await appAsk("DELETE", "kuvo/marketplace/apps/docs-mcp")).status, 200);
    const cfg2 = JSON.parse(readFileSync(path.join(box.home, "box", "opencode.json"), "utf8"));
    assert.equal(cfg2.mcp?.servers?.["marketplace-docs-mcp"], undefined);

    // Create -> toggle enabled -> edit -> delete -> restore skill
    assert.equal((await appAsk("POST", "kuvo/marketplace/skills/create", {
      name: "release-notes", description: "Write release notes", instructions: "Summarize commits.",
    })).status, 200);
    assert.equal((await appAsk("POST", "kuvo/marketplace/skills/release-notes/enabled", { on: false })).status, 200);
    const det = await appAsk("GET", "kuvo/marketplace/skills/release-notes");
    assert.equal(det.status, 200);
    assert.equal((await appAsk("PATCH", "kuvo/marketplace/skills/release-notes", {
      was: det.json.revision, description: "Write crisp release notes", instructions: "Group by area.",
    })).status, 200);
    const det2 = await appAsk("GET", "kuvo/marketplace/skills/release-notes");
    assert.equal((await appAsk("DELETE", "kuvo/marketplace/skills/release-notes", { was: det2.json.revision })).status, 200);
    const afterDel = await appAsk("GET", "kuvo/marketplace");
    const delItem = afterDel.json.areas[2].here.deleted.items.find(i => i.title === "release-notes");
    assert.ok(delItem, "deleted skill appears in Recently Deleted");
    assert.equal((await appAsk("POST", "kuvo/marketplace/skills/restore", { id: delItem.id })).status, 200);
    assert.equal((await appAsk("GET", "kuvo/marketplace/skills/release-notes")).json.installed, true);

    // Other Places add & remove
    assert.equal((await appAsk("POST", "kuvo/marketplace/skills/places", { place: "/tmp/extra-skills" })).status, 200);
    const withPlace = await appAsk("GET", "kuvo/marketplace");
    assert.deepEqual(withPlace.json.areas[2].here.places.items.map(i => i.title), ["/tmp/extra-skills"]);
    assert.equal((await appAsk("DELETE", "kuvo/marketplace/skills/places", { index: 0 })).status, 200);
  });
});
