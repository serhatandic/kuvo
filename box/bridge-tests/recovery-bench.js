import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

export function put(file, contents) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, contents);
}

export function recoveryEngine(root, owner, session) {
  const messages = [{ id: "msg_original", text: "Keep this conversation" }];
  const file = box => path.join(root, box, "engine-session.json");
  put(file(owner), JSON.stringify({ info: { id: session, title: "Recovery conversation", directory: path.join(root, owner, "chats") }, messages }));
  const imports = [], wakes = [];
  return { file, messages, imports, wakes, async handle(request, response) {
    const authorization = Buffer.from(request.headers.authorization?.slice(6) ?? "", "base64").toString();
    const box = authorization.split(":")[1];
    if (!["pc2", "pc4", "pc5"].includes(box)) return false;
    const url = new URL(request.url, "http://localhost");
    const send = (status, data) => {
      response.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify({ data }));
      return true;
    };
    const body = async () => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      return JSON.parse(Buffer.concat(chunks));
    };
    const copy = fs.existsSync(file(box)) ? JSON.parse(fs.readFileSync(file(box), "utf8")) : null;
    if (url.pathname === "/api/session/active") return send(200, {});
    if (url.pathname === "/api/session") return send(200, copy && !url.searchParams.has("parentID") ? [copy.info] : []);
    if (url.pathname === "/api/experimental/session/import" && request.method === "POST") {
      const given = await body();
      imports.push(box);
      if (copy) return send(409, null);
      put(file(box), JSON.stringify({ info: { ...given.info, directory: given.location.directory }, messages: given.messages }));
      return send(200, given.info);
    }
    if (url.pathname === `/api/experimental/session/${session}/export`) return copy ? send(200, copy) : send(404, null);
    if (url.pathname === `/api/experimental/session/${session}/instructions/entries/kuvo.moved`) return send(200, true);
    if (!url.pathname.startsWith(`/api/session/${session}`)) return false;
    if (!copy) return send(404, null);
    if (url.pathname.endsWith("/inbox") || url.pathname.endsWith("/message")) return send(200, []);
    if (url.pathname.endsWith("/prompt")) {
      wakes.push({ box, ...(await body()) });
      return send(200, {});
    }
    if (request.method === "DELETE") {
      fs.unlinkSync(file(box));
      return send(200, true);
    }
    return send(200, copy.info);
  } };
}

export function benchRelease(root, script, privateKey, key) {
  const hash = bytes => crypto.createHash("sha256").update(bytes).digest("hex");
  const version = "0.99.0";
  const engine = "1.0.0";
  const layout = Number(/^\/\/ Kuvo folder layout (\d+)/.exec(fs.readFileSync(script, "utf8"))[1]);
  const payload = path.join(root, "release-files");
  const folder = path.join(root, "release");
  const files = Object.fromEntries(Object.entries({
    "bin/box-version": version,
    "bin/kuvo-link.js": fs.readFileSync(script),
    "bin/kuvo-start": "#!/bin/sh\nexit 0\n",
  }).map(([file, bytes]) => {
    put(path.join(payload, file), bytes);
    return [file, hash(bytes)];
  }));
  put(path.join(payload, "box.json"), JSON.stringify({ version, engine, files }));
  fs.mkdirSync(folder);
  const name = `box-${version}.tar.gz`;
  const pack = path.join(folder, name);
  const packed = spawnSync("tar", ["-czf", pack, "-C", payload, "."], {
    env: { ...process.env, COPYFILE_DISABLE: "1" }, encoding: "utf8", timeout: 10_000,
  });
  assert.equal(packed.status, 0, packed.stderr);
  const data = fs.readFileSync(pack);
  // This fixture release leaves client admission unchanged during the update.
  const manifest = Buffer.from(JSON.stringify({
    kuvo: "box/1", version, engine, layout, spreads: true,
    released: new Date().toISOString(), pack: { url: `https://example.invalid/${name}`, sha256: hash(data), size: data.length },
  }));
  const signature = crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), privateKey);
  put(path.join(folder, "boxes.json"), JSON.stringify({ manifest: manifest.toString("base64"), signature: signature.toString("base64"), key }));
  return { folder, key, version, engine };
}

// Keep diagnostics outside the simulated disk. Flush each milestone before the
// next blocking operation, and report it even when node:test cancels the case.
export function durablePhase(t, root) {
  const journal = path.join(root, "bench-phases");
  let last = "starting disposable fixture";
  t.signal.addEventListener("abort", () => {
    t.diagnostic(`Last durable phase: ${last}`);
  }, { once: true });
  return phase => {
    const fd = fs.openSync(journal, "a", 0o600);
    try {
      fs.writeSync(fd, phase + "\n");
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    last = phase;
    t.diagnostic(phase);
  };
}
