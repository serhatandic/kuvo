import "./safety"
import { afterEach, beforeEach, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, truncateSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { engineHandler } from "../src/link/server/router";
import { serveBox } from "../src/link/server/serve";
import { FILE_LIMIT } from "../src/link/areas/file";

let root: string;
beforeEach(() => { root = mkdtempSync(join(tmpdir(), "kuvo-file-info-")); });
afterEach(() => rmSync(root, { recursive: true, force: true }));

const route = (file: string, resource = "info") =>
  `kuvo/file${resource ? `/${resource}` : ""}?uri=${encodeURIComponent(pathToFileURL(file).href)}`;

const handle = engineHandler();
async function ask(path: string, method = "GET") {
  let status = 0;
  const chunks: Buffer[] = [];
  for await (const part of handle({ method, path, headers: {}, body: Buffer.alloc(0), fromApp: true }, undefined)) {
    if ("head" in part) status = part.head.status;
    else chunks.push(part.body);
  }
  return { status, body: JSON.parse(Buffer.concat(chunks).toString()) };
}

test("file info reports each kind by extension and measures bytes", async () => {
  for (const [name, kind, mime] of [
    ["a #ş.PNG", "picture", "image/png"],
    ["a.pdf", "pdf", "application/pdf"],
    ["a.json", "text", "application/json"],
    ["a.constructor", "other", "application/octet-stream"],
  ]) {
    const file = join(root, name);
    writeFileSync(file, "ş");
    expect(await ask(route(file))).toEqual({ status: 200, body: { exists: true, name, size: 2, kind, mime } });
  }
  const big = join(root, "big.pdf");
  writeFileSync(big, "");
  truncateSync(big, FILE_LIMIT + 1);
  expect((await ask(route(big))).body.size).toBe(FILE_LIMIT + 1);
  expect((await ask(route(big, ""))).status).toBe(413);
});

test("missing and non-file paths give a normal info reply and 404 for bytes", async () => {
  mkdirSync(join(root, "folder"));
  for (const name of ["gone.pdf", "folder"]) {
    const file = join(root, name);
    expect(await ask(route(file))).toEqual({ status: 200, body: { exists: false, name, size: null, kind: null, mime: null } });
    expect((await ask(route(file, ""))).status).toBe(404);
  }
  expect((await ask("kuvo/file/info?uri=https://example.com")).status).toBe(400);
  expect(await ask(route(root), "POST")).toEqual({ status: 405, body: {} });
});

test("the folder and text routes use paths from the box, including literal URI characters", async () => {
  const folder = join(root, "..é");
  const name = "a%20 #ş\\.txt";
  const path = join(folder, name);
  mkdirSync(folder);
  writeFileSync(path, "the document");
  const rootList = await ask(`kuvo/folders?path=${encodeURIComponent(root)}&files=1&hidden=1`);
  expect(rootList.body.folders).toEqual([{ name: "..é", path: folder }]);
  const listed = await ask(`kuvo/folders?path=${encodeURIComponent(folder)}&files=1`);
  expect(listed.body.files).toEqual([name]);
  expect(listed.body.fileEntries).toEqual([{ name, path }]);
  const path64 = Buffer.from(path).toString("base64url");
  const server = serveBox("password", handle);
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Expected a TCP address");
    const url = `http://127.0.0.1:${address.port}/kuvo/file/text?path64=${path64}`;
    const reply = await fetch(url, { headers: { authorization: "Basic " + btoa("opencode:password") } });
    expect(reply.status).toBe(200);
    expect(await reply.json()).toEqual({ text: true, encoding: "utf-8", content: "the document" });
  } finally {
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
  }
  expect((await ask("kuvo/file/text?path64=cmVsYXRpdmU")).status).toBe(400);
});

test("byte downloads honor the announced size when a file changes", async () => {
  const file = join(root, "changing.txt");
  const path = route(file, "") + "&max=3";
  const download = () => handle({ method: "GET", path, headers: {}, body: Buffer.alloc(0), fromApp: true }, undefined);
  writeFileSync(file, "old");
  const growing = download();
  expect((await growing.next()).value).toMatchObject({ head: { status: 200, headers: { "content-length": "3" } } });
  writeFileSync(file, "new longer content");
  const chunks: Buffer[] = [];
  for await (const part of growing) if ("body" in part) chunks.push(part.body);
  expect(Buffer.concat(chunks).toString()).toBe("new");

  writeFileSync(file, "old");
  const shrinking = download();
  expect((await shrinking.next()).value).toMatchObject({ head: { status: 200, headers: { "content-length": "3" } } });
  truncateSync(file, 0);
  await expect(shrinking.next()).rejects.toThrow("The file changed while it was being read.");
});

test("the file info route is served by the other computer's box route command", () => {
  const remoteHome = join(root, "remote");
  mkdirSync(remoteHome);
  const file = join(remoteHome, "remote.txt");
  writeFileSync(file, "from the remote box");
  const link = fileURLToPath(new URL("../dist/kuvo-link.js", import.meta.url));
  const result = spawnSync(process.execPath, [link, "box", "route"], {
    input: JSON.stringify({ method: "GET", path: route(file) }),
    env: { ...process.env, HOME: remoteHome, KUVO_HOME: join(remoteHome, ".kuvo-code") },
    cwd: remoteHome,
    encoding: "utf8",
  });
  expect(result.status).toBe(0);
  const answer = JSON.parse(result.stdout);
  expect(answer.status).toBe(200);
  expect(JSON.parse(answer.body)).toEqual({ exists: true, name: "remote.txt", size: 19, kind: "text", mime: "text/plain" });
});

test("remote downloads can opt into the source's execute bits with the bytes", async () => {
  const file = join(root, "launch");
  const textFile = join(root, "report.txt");
  writeFileSync(file, "echo extensionless", { mode: 0o700 });
  writeFileSync(textFile, "normal text", { mode: 0o600 });
  const link = fileURLToPath(new URL("../dist/kuvo-link.js", import.meta.url));
  for (const [source, executable, body] of [[file, "1", "echo extensionless"], [textFile, "0", "normal text"]]) {
    const result = spawnSync(process.execPath, [link, "box", "route"], {
      input: JSON.stringify({ method: "GET", path: route(source, "") + "&withMode=1" }),
      env: { ...process.env, HOME: root, KUVO_HOME: join(root, ".kuvo-code") },
      encoding: "utf8",
    });
    expect(result.status).toBe(0);
    const answer = JSON.parse(result.stdout);
    expect(answer.status).toBe(200);
    expect(answer.type).toBe(`application/vnd.kuvo.file; executable=${executable}`);
    expect(Buffer.from(answer.bodyBase64, "base64").toString()).toBe(body);
  }
});

test("file metadata and bytes both require the box password", async () => {
  const file = join(root, "private.txt");
  writeFileSync(file, "private");
  const server = serveBox("password", handle);
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    if (!address || typeof address === "string") throw new Error("Expected a TCP address");
    for (const resource of ["", "info"]) {
      const url = `http://127.0.0.1:${address.port}/${route(file, resource)}`;
      expect((await fetch(url)).status).toBe(401);
      expect((await fetch(url, { headers: { authorization: "Basic " + btoa("opencode:wrong") } })).status).toBe(401);
      const answer = await fetch(url, { headers: { authorization: "Basic " + btoa("opencode:password") } });
      expect(answer.status).toBe(200);
      await answer.arrayBuffer();
    }
  } finally {
    server.closeAllConnections();
    await new Promise<void>(resolve => server.close(() => resolve()));
  }
});

test.skipIf(process.getuid?.() === 0)("file metadata and bytes both check read permission", async () => {
  const file = join(root, "unreadable.txt");
  writeFileSync(file, "private");
  chmodSync(file, 0);
  expect((await ask(route(file))).status).toBe(403);
  expect((await ask(route(file, ""))).status).toBe(403);
});
