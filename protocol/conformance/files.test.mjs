// Files and folders (rust-box.md row 6.3a): a file's info, text and bytes, the same from both boxes.
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, readFileSync, realpathSync, rmSync, truncateSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { scratch } from "./adapter.mjs";
import { bench } from "./bench.mjs";

const FILE_LIMIT = 25 * 1024 * 1024;
const at = (file, resource = "info") =>
  `kuvo/file${resource ? `/${resource}` : ""}?uri=${encodeURIComponent(pathToFileURL(file).href)}`;
const text64 = file => `kuvo/file/text?path64=${Buffer.from(file).toString("base64url")}`;

test("a file's info, text and bytes", async t => {
  const root = scratch();
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const { computer } = await bench(t);
  const box = await computer("Files");
  const ask = box.connect(await box.start().ready());

  await t.test("info by kind and size", async () => {
    for (const [name, kind, mime] of [
      ["a #ş.PNG", "picture", "image/png"], ["a.pdf", "pdf", "application/pdf"],
      ["a.json", "text", "application/json"], ["a.constructor", "other", "application/octet-stream"],
      [".bashrc", "other", "application/octet-stream"], ["a%20 #ş\\.txt", "text", "text/plain"],
    ]) {
      const file = path.join(root, name);
      writeFileSync(file, "ş");
      const reply = await ask("GET", at(file));
      assert.deepEqual([reply.status, reply.json], [200, { exists: true, name, size: 2, kind, mime }], name);
    }
    const big = path.join(root, "big.pdf");
    writeFileSync(big, "");
    truncateSync(big, FILE_LIMIT + 1);
    assert.equal((await ask("GET", at(big))).json.size, FILE_LIMIT + 1);
    const tooBig = await ask("GET", at(big, ""));
    assert.deepEqual([tooBig.status, tooBig.json], [413, { message: "That file is too big to open here." }]);
  });

  await t.test("missing files, folders and other addresses", async () => {
    mkdirSync(path.join(root, "folder"), { recursive: true });
    writeFileSync(path.join(root, "plain"), "x");
    for (const [file, name] of [[path.join(root, "gone.pdf"), "gone.pdf"], [path.join(root, "folder"), "folder"], [path.join(root, "plain") + "/", "plain"]]) {
      const info = await ask("GET", at(file));
      assert.deepEqual([info.status, info.json], [200, { exists: false, name, size: null, kind: null, mime: null }], file);
      for (const resource of ["", "text"]) {
        const missing = await ask("GET", at(file, resource));
        assert.deepEqual([missing.status, missing.json], [404, { message: "That file isn’t there anymore.", reason: "fileNotFound" }]);
      }
    }
    for (const route of ["kuvo/file/info?uri=https://example.com", "kuvo/file/info", "kuvo/file?uri=", "kuvo/file/info?uri=file://host/x",
      "kuvo/file/info?uri=file:///a%252Fb", "kuvo/file/text?uri=x&path64=L3RtcA", `kuvo/file/info?path64=${Buffer.from("/tmp").toString("base64url")}`]) {
      const refused = await ask("GET", route);
      assert.deepEqual([refused.status, refused.json], [400, { message: "That isn’t a file:// address." }], route);
    }
    for (const route of ["kuvo/file/text?path64=cmVsYXRpdmU", "kuvo/file/text?path64=", "kuvo/file/text?path64=L3RtcA=", "kuvo/file/text?path64=_w"]) {
      const refused = await ask("GET", route);
      assert.deepEqual([refused.status, refused.json], [400, { message: "Name the file by its full path." }], route);
    }
    const post = await ask("POST", at(root));
    assert.deepEqual([post.status, post.json], [405, {}]);
  });

  await t.test("text: UTF-8 only, within what's asked", async () => {
    const folder = path.join(root, "..é");
    mkdirSync(folder, { recursive: true });
    const doc = path.join(folder, "doc.md");
    writeFileSync(doc, "﻿the document");
    for (const route of [at(doc, "text"), text64(doc), text64(path.join(folder, ".", "..", "..é", "doc.md") + "/")]) {
      const reply = await ask("GET", route);
      assert.deepEqual([reply.status, reply.json], [200, { text: true, encoding: "utf-8", content: "the document" }], route);
    }
    for (const bytes of [Buffer.from([0x61, 0, 0x62]), Buffer.from([0xff, 0xfe, 0x61, 0])]) {
      writeFileSync(path.join(root, "binary.dat"), bytes);
      assert.deepEqual((await ask("GET", at(path.join(root, "binary.dat"), "text"))).json, { text: false });
    }
    for (const [max, status] of [["3", 413], ["15", 200], ["abc", 200], ["", 413], ["0x10", 200], ["1e1", 413], ["-1", 413]]) {
      const reply = await ask("GET", at(doc, "text") + `&max=${encodeURIComponent(max)}`);
      assert.equal(reply.status, status, `max=${max}`);
      if (status === 413) assert.deepEqual(reply.json, { message: "That file is too big to show here." });
    }
    const empty = path.join(root, "empty.txt");
    writeFileSync(empty, "");
    assert.deepEqual((await ask("GET", at(empty, "text") + "&max=0")).json, { text: true, encoding: "utf-8", content: "" });
  });

  await t.test("bytes with their type, length and, asked, mode", async () => {
    const launch = path.join(root, "launch");
    const report = path.join(root, "report.txt");
    writeFileSync(launch, "echo extensionless", { mode: 0o700 });
    writeFileSync(report, "normal text", { mode: 0o600 });
    const plain = await ask("GET", at(report, ""));
    assert.deepEqual([plain.status, plain.type, plain.text], [200, "text/plain", "normal text"]);
    for (const [file, executable, body] of [[launch, "1", "echo extensionless"], [report, "0", "normal text"]]) {
      const reply = await ask("GET", at(file, "") + "&withMode=1");
      assert.deepEqual([reply.status, reply.type, reply.text], [200, `application/vnd.kuvo.file; executable=${executable}`, body]);
    }
    const capped = await ask("GET", at(report, "") + "&max=3");
    assert.deepEqual([capped.status, capped.json], [413, { message: "That file is too big to show here." }]);
    const empty = path.join(root, "nothing.bin");
    writeFileSync(empty, "");
    const none = await ask("GET", at(empty, ""));
    assert.deepEqual([none.status, none.type, none.bytes.length], [200, "application/octet-stream", 0]);
  });

  await t.test("each name a client encodes comes back as the file it names", async () => {
    const vectors = JSON.parse(readFileSync(new URL("../../box/test/fixtures/file-addresses/addresses.json", import.meta.url), "utf8"));
    assert.ok(vectors.addresses.length > 0);
    const folder = path.join(realpathSync(root), "addresses");
    mkdirSync(folder);
    assert.match(folder, /^[A-Za-z0-9/._-]+$/);
    for (const { name, encoded } of vectors.addresses) {
      writeFileSync(path.join(folder, name), `the file ${name}`);
      const reply = await ask("GET", `kuvo/file?uri=file://${folder}/${encoded}`);
      assert.deepEqual({ name, status: reply.status, body: reply.text }, { name, status: 200, body: `the file ${name}` });
    }
  });

  await t.test("unreadable files are refused", { skip: process.getuid?.() === 0 }, async () => {
    const file = path.join(root, "unreadable.txt");
    writeFileSync(file, "private");
    chmodSync(file, 0);
    for (const resource of ["info", "", "text"]) {
      const reply = await ask("GET", at(file, resource));
      assert.deepEqual([reply.status, reply.json], [403, { message: "That file can’t be opened here." }], resource);
    }
  });
});
