// Files and folders (rust-box.md row 6.3a): a file's info, text and bytes, the same from both boxes.
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, readFileSync, realpathSync, rmSync, truncateSync, writeFileSync } from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { test } from "node:test";
import { pathToFileURL } from "node:url";
import { scratch, until } from "./adapter.mjs";
import { sessionOf } from "./engine.mjs";
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

  const make = (top, names) => {
    for (const name of names) {
      mkdirSync(path.dirname(path.join(top, name)), { recursive: true });
      writeFileSync(path.join(top, name), `${name}\n`);
    }
    return top;
  };
  const git = (folder, ...args) => {
    const ran = spawnSync("git", ["-c", "init.defaultBranch=main", ...args], { cwd: folder, encoding: "utf8", env: { ...process.env, GIT_CONFIG_GLOBAL: "/dev/null", GIT_CONFIG_NOSYSTEM: "1" } });
    assert.equal(ran.status, 0, ran.stderr);
  };
  const repository = (name, names) => {
    const top = make(path.join(realpathSync(root), name), names);
    git(top, "init", "-q");
    writeFileSync(path.join(top, ".gitignore"), "node_modules/\n");
    return top;
  };
  const find = (directory, q, more = "") => ask("GET", `kuvo/files/find?directory=${encodeURIComponent(directory)}&q=${encodeURIComponent(q)}${more}`);

  await t.test("a folder's folders and, for a Files panel, its files, in Finder's order", async () => {
    const top = make(path.join(realpathSync(root), "listing"), ["file29.txt", "File3.txt", ".hidden", ".DS_Store", "b/x", "A/x", ".git/HEAD", ".hg/x", "repo/.git/HEAD", "a b/x", "10/x", "9/x"]);
    const plain = await ask("GET", `kuvo/folders?path=${encodeURIComponent(top)}`);
    assert.equal(plain.status, 200, plain.text);
    assert.equal(plain.json.path, top);
    assert.equal(plain.json.parent, path.dirname(top));
    assert.equal(typeof plain.json.home, "string");
    assert.deepEqual(plain.json.folders, [
      { name: "9", path: path.join(top, "9") }, { name: "10", path: path.join(top, "10") }, { name: "A", path: path.join(top, "A") },
      { name: "a b", path: path.join(top, "a b") }, { name: "b", path: path.join(top, "b") }, { name: "repo", path: path.join(top, "repo"), repository: true },
    ]);
    assert.equal(plain.json.files, undefined);
    const panel = await ask("GET", `kuvo/folders?path=${encodeURIComponent(top)}&files=1&hidden=1`);
    assert.deepEqual(panel.json.folders.map(each => each.name), ["9", "10", "A", "a b", "b", "repo"]);
    assert.deepEqual(panel.json.files, [".hidden", "File3.txt", "file29.txt"]);
    assert.deepEqual(panel.json.fileEntries, panel.json.files.map(name => ({ name, path: path.join(top, name) })));
    const hidden = await ask("GET", `kuvo/folders?path=${encodeURIComponent(top)}&hidden=1`);
    assert.deepEqual(hidden.json.folders.map(each => each.name), [".git", ".hg", "9", "10", "A", "a b", "b", "repo"]);
    const top2 = await ask("GET", "kuvo/folders?path=/");
    assert.equal(top2.json.parent, null);
    // (The server refuses a `..` in a query; a doubled or trailing separator is resolved.)
    const dotted = await ask("GET", `kuvo/folders?path=${encodeURIComponent(top + "//b/./")}`);
    assert.equal(dotted.json.path, path.join(top, "b"));
    for (const [route, status, json] of [
      [`kuvo/folders?path=relative`, 400, { message: "Send a folder's full path, or one that starts with ~/." }],
      [`kuvo/folders?path=${encodeURIComponent(path.join(top, "gone"))}`, 404, { message: `There's no folder ${path.join(top, "gone")} here.`, reason: "No such folder" }],
      [`kuvo/folders?path=${encodeURIComponent(path.join(top, "file29.txt"))}`, 404, { message: `${path.join(top, "file29.txt")} isn't a folder.`, reason: "Not a folder" }],
    ]) {
      const reply = await ask("GET", route);
      assert.deepEqual([reply.status, reply.json], [status, json], route);
    }
    const home = await ask("GET", "kuvo/folders?path=~");
    assert.equal(home.json.path, home.json.home);
  });

  await t.test("a new folder where asked", async () => {
    const top = path.join(realpathSync(root), "making");
    mkdirSync(top);
    const made = await ask("POST", "kuvo/folders", { path: path.join(top, "x", "..", "New") });
    assert.deepEqual([made.status, made.json], [200, { path: path.join(top, "New") }]);
    for (const [body, status, json] of [
      [{ path: path.join(top, "New") }, 409, { message: "There's already something called New there." }],
      [{ path: path.join(top, "gone", "New") }, 404, { message: `There's no folder ${path.join(top, "gone")} here.` }],
      [{ path: "relative" }, 400, { message: "Send the new folder's full path, or one that starts with ~/." }],
      [{}, 400, { message: "Send the new folder's full path, or one that starts with ~/." }],
    ]) {
      const reply = await ask("POST", "kuvo/folders", body);
      assert.deepEqual([reply.status, reply.json], [status, json], JSON.stringify(body));
    }
  });

  await t.test("every file under a folder: git's list, else a walk", async () => {
    const top = repository("tree-repo", ["src/a.ts", "node_modules/x.js", "b.md"]);
    const listed = await ask("GET", `kuvo/folders/tree?path=${encodeURIComponent(top)}&dated=1`);
    assert.equal(listed.status, 200, listed.text);
    assert.deepEqual([listed.json.path, listed.json.git, [...listed.json.files].sort()], [top, true, [".gitignore", "b.md", "src/a.ts"]]);
    assert.equal(listed.json.modified.length, 3);
    assert.ok(listed.json.modified.every(each => Number.isInteger(each)));
    const plain = make(path.join(realpathSync(root), "tree-plain"), ["a/b.txt", "node_modules/x", "dist/y", "c.txt"]);
    const walked = await ask("GET", `kuvo/folders/tree?path=${encodeURIComponent(plain)}`);
    assert.deepEqual([walked.json.git, [...walked.json.files].sort(), walked.json.modified], [false, ["a/b.txt", "c.txt"], undefined]);
    for (const route of ["kuvo/folders/tree", "kuvo/folders/tree?path=%20", "kuvo/folders/tree?path=rel"]) {
      const reply = await ask("GET", route);
      assert.deepEqual([reply.status, reply.json], [400, { message: "Send a folder's full path, or one that starts with ~/." }], route);
    }
  });

  await t.test("finding files: names before paths, shorter first, nothing ignored", async () => {
    const top = repository("site", ["src/forms/signup-form.tsx", "src/signup.ts", "docs/signup/notes.md", "src/sign.ts", "node_modules/pkg/signup.js", "README.md"]);
    const said = await find(top, "SignUp");
    assert.equal(said.status, 200, said.text);
    assert.deepEqual(said.json, { url: null, files: [
      { path: path.join(top, "src/signup.ts"), name: "signup.ts", folder: "src" },
      { path: path.join(top, "src/forms/signup-form.tsx"), name: "signup-form.tsx", folder: "src/forms" },
      { path: path.join(top, "docs/signup/notes.md"), name: "notes.md", folder: "docs/signup" },
    ] });
    assert.deepEqual((await find(top, "rdme")).json.files, [{ path: path.join(top, "README.md"), name: "README.md", folder: "" }]);
    assert.deepEqual((await find(top, "sgnp")).json.files.map(each => each.name), ["signup.ts", "signup-form.tsx", "notes.md"]);
    for (const [limit, count] of [["2", 2], ["0", 4], ["2.5", 4], ["x", 4], ["300", 4]]) {
      assert.equal((await find(top, "s", `&limit=${limit}`)).json.files.length, count, limit);
    }
    assert.deepEqual(await find(top, "   ").then(reply => [reply.status, reply.json]), [200, { url: null, files: [] }]);
    const plain = make(path.join(realpathSync(root), "find-plain"), ["notes/todo.txt", "node_modules/x/todo.js", "build/todo.o", "todo.txt"]);
    assert.deepEqual((await find(plain, "todo")).json.files.map(each => each.path), [path.join(plain, "todo.txt"), path.join(plain, "notes/todo.txt")]);
    assert.equal((await ask("GET", "kuvo/files/find?q=signup")).status, 400);
    const gone = await find(path.join(root, "nowhere"), "signup");
    assert.deepEqual([gone.status, gone.json], [404, { message: `There's no folder ${path.join(root, "nowhere")} here.`, reason: "No such folder" }]);
    assert.equal((await find("relative/folder", "signup")).status, 400);
    // (A `?` past the first is part of the query: in what's typed, and in a folder's name.)
    const asked = repository("asked?here", ["src/signup.ts"]);
    const typed = await find(asked, "https://example.com/a?b=1");
    assert.deepEqual([typed.status, typed.json], [200, { url: "https://example.com/a?b=1", files: [] }]);
    assert.equal((await find(asked, "signup")).json.files[0]?.path, path.join(asked, "src/signup.ts"));
    assert.equal((await ask("GET", `kuvo/folders?path=${encodeURIComponent(asked)}`)).json.path, asked);
  });

  await t.test("what's typed reads as a web address only where it is one", async () => {
    const top = repository("addresses", ["src/signup.ts"]);
    const file = (await find(top, "signup.ts")).json;
    assert.deepEqual(file, { url: null, files: [{ path: path.join(top, "src/signup.ts"), name: "signup.ts", folder: "src" }] });
    for (const [typed, url] of [
      ["https://example.com/a?b=1", "https://example.com/a?b=1"], ["HTTP://Example.com", "HTTP://Example.com"],
      ["example.com", "https://example.com"], ["kuvo.run", "https://kuvo.run"],
      ["docs.example.dev:8080/guide", "https://docs.example.dev:8080/guide"], ["github.com/owner/repo", "https://github.com/owner/repo"],
      ["localhost", "http://localhost"], ["localhost:5173/app", "http://localhost:5173/app"], ["127.0.0.1:8080", "http://127.0.0.1:8080"],
      ["192.168.1.20/admin", "http://192.168.1.20/admin"], ["[::1]:3000", "http://[::1]:3000"], ["foo.bar", null], ["foo.bar/x", "https://foo.bar/x"],
      ["signup.ts", null], ["notes.md", null], ["install.sh", null], ["main.go:12", null], ["index.html#top", null], ["src/main.rs", null],
      ["package.json", null], ["", null], ["two words.com", null], ["https://", null], ["ftp://example.com", null],
      ["mailto:me@example.com", null], ["me@example.com", null], ["1.2.3", null], ["999.1.1.1", null], ["localhost:70000", null],
      ["Example.COM", "https://Example.COM"], [" example.com ", "https://example.com"], ["a.b-.com", null], ["[::g]:80", null],
    ]) {
      assert.equal((await find(top, typed)).json.url, url, typed);
    }
  });
});

test("folders worth starting in: those worked in, newest first, then repositories", async t => {
  const { computer } = await bench(t);
  const box = await computer("Suggested");
  const outside = realpathSync(scratch());
  t.after(() => rmSync(outside, { recursive: true, force: true }));
  const folder = (at, repository = false) => {
    mkdirSync(path.join(at, repository ? ".git" : ""), { recursive: true });
    return at;
  };
  const api = folder(path.join(outside, "work", "api"), true);
  const scratchFolder = folder(path.join(outside, "scratch"));
  const gone = path.join(outside, "gone");
  box.engine.sessions.push(
    sessionOf({ id: "ses_api", directory: api, updated: 450 }),
    sessionOf({ id: "ses_scratch", directory: scratchFolder, updated: 500 }),
    sessionOf({ id: "ses_api_old", directory: api + "/", updated: 100 }),
    sessionOf({ id: "ses_gone", directory: gone, updated: 600 }),
    sessionOf({ id: "ses_archived", directory: folder(path.join(outside, "old"), true), updated: 900, archived: 900 }),
  );
  const ask = box.connect(await box.start().ready());
  const first = await ask("GET", "kuvo/folders/suggested");
  assert.equal(first.status, 200, first.text);
  const home = first.json.home;
  const projects = folder(path.join(home, "Projects", "website"), true);
  folder(path.join(home, "Projects", "plain"));
  folder(path.join(home, "Projects", ".hidden"), true);
  folder(path.join(home, "Projects", "deep", "inner"), true);
  const dotfiles = folder(path.join(home, "Developer", "dotfiles"), true);
  writeFileSync(path.join(home, "Projects", "README.md"), "not a folder");
  let reply;
  await until(async () => {
    reply = await ask("GET", "kuvo/folders/suggested");
    return reply.json?.folders?.some(each => each.seen !== null);
  }, "the sessions' folders suggested", 20000, 100);
  assert.deepEqual(reply.json.folders.slice(0, 2), [
    { path: scratchFolder, name: "scratch", seen: 500 },
    { path: api, name: "api", repository: true, seen: 450 },
  ]);
  assert.deepEqual(reply.json.folders.slice(2).map(each => [each.path, each.name, each.repository, each.seen]).sort(),
    [[dotfiles, "dotfiles", true, null], [projects, "website", true, null]]);
});
