// What the box's tests stand in for and start: this Mac's app (its host interface's keys, kept in
// memory as the Keychain would), a route as a client reaches it over SSH (`box route`), this Mac's
// box started as the app starts it, and where a box keeps its conversations' SSH agents.
import "./test-safety.js";
import crypto from "node:crypto";
import http from "node:http";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

export const LINK_BUNDLE = new URL("../dist/kuvo-link.js", import.meta.url);
export const script = fileURLToPath(LINK_BUNDLE);
/** Where a box keeps its conversations' SSH agents (vault/ssh.ts). */
export const sshAgentsOf = (box, home) => {
  const agents = path.join(box, "run", "ssh-agents");
  const fits = folder => Buffer.byteLength(path.join(folder, "0".repeat(33))) <= 103;
  if (process.platform !== "darwin" || fits(agents)) return agents;
  const boxID = crypto.createHash("sha256").update(path.resolve(box)).digest("hex").slice(0, 12);
  const short = path.join(home, ".kuvo-ssh", boxID);
  return fits(short) ? short : path.join("/private/tmp", `kuvo-ssh-${process.getuid()}`, boxID);
};
/** A route as a client reaches it over SSH (`box route`), in a process of its own. */
export async function boxRoute(request, env) {
  const child = spawn(process.execPath, [script, "box", "route"], { env, stdio: ["pipe", "pipe", "pipe"] });
  child.stdin.end(JSON.stringify(request));
  let out = "";
  child.stdout.setEncoding("utf8");
  for await (const chunk of child.stdout) out += chunk;
  const answer = JSON.parse(out.trim().split("\n").at(-1));
  return { status: answer.status, value: JSON.parse(answer.body || "{}") };
}

/** The stand-in for this Mac's app: its host interface's keys, kept in memory as the Keychain would.
 * `asked`: each call it took ({call, name?, prefix?, suffix?}); `turnedAway`: those without its password. */
export async function standInHost(password) {
  // Importing the bundle path alone should leave the test's cleanup hooks alone.
  const { atEnd } = await import("./at-end.js");
  const host = { keys: new Map(), asked: [], turnedAway: 0, down: false, onNames: null };
  const server = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const answer = (status, value) => {
      response.writeHead(status, { "content-type": "application/json" });
      response.end(JSON.stringify(value));
    };
    // That it's the app, as the app proves it: the box's nonce signed with the password, asked
    // without it (and answered while its Keychain is locked too).
    if (request.method === "POST" && request.url === "/hello") {
      host.hellos = (host.hellos ?? 0) + 1;
      const { nonce, vault } = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      const at = host.url.replace("http://", "");
      return answer(200, {
        proof: crypto.createHmac("sha256", password).update(`kuvo-host-hello\0${nonce}\0${at}`).digest("hex"),
        // For the vault's calls, with the secret the app gave its box alone.
        ...(vault && host.vaultSecret ? { vaultProof: crypto.createHmac("sha256", host.vaultSecret).update(`kuvo-host-vault\0${nonce}\0${at}`).digest("hex") } : {}),
      });
    }
    if (request.method !== "POST" || request.headers.authorization !== `Bearer ${password}` || request.headers["content-type"] !== "application/json") {
      host.turnedAway++;
      return answer(401, {});
    }
    const given = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    const { value: _, values: __, ...asked } = given;
    host.asked.push({ call: request.url, ...asked });
    if (host.down || host.failing?.has(request.url)) return answer(500, { message: "The Keychain is locked." });
    switch (request.url) {
      case "/keys/names": {
        const names = [...host.keys.keys()].filter(name => name.startsWith(given.prefix ?? "") && name.endsWith(given.suffix ?? ""));
        // Held, as read now, until the test lets it go (`hold`): what the box asked meanwhile is seen.
        if (host.hold) {
          const hold = host.hold;
          host.hold = null;
          host.held?.();
          await hold;
        }
        answer(200, { names });
        return host.onNames?.();
      }
      case "/keys/get":
        // (`unreadable`: kept, but not given now, which the app answers as none: a Keychain prompt turned down.)
        return host.keys.has(given.name) && !host.unreadable?.has(given.name) ? answer(200, { value: host.keys.get(given.name) }) : answer(404, {});
      case "/keys/set":
        host.keys.set(given.name, given.value);
        // What happens elsewhere while a key is saved (a prompt waiting for the Keychain, say).
        host.onSet?.();
        return answer(200, {});
      case "/keys/delete":
        host.keys.delete(given.name);
        // What happens elsewhere while the box waits for its answer.
        await host.onDelete?.();
        return answer(200, {});
      // The vault's values, kept by id as the Keychain would (vault-host.test.js): only to the box,
      // with the secret the app started it with (`vaultSecret`), never to what read its folder.
      case "/vault/get":
      case "/vault/set":
      case "/vault/delete":
      case "/vault/generations":
        // Each with its own proof: an HMAC with the secret of the call, its body and when.
        if (host.vaultSecret && request.headers["x-kuvo-vault-auth"] !== crypto.createHmac("sha256", host.vaultSecret).update(`kuvo-host-vault-call\0${request.url.slice(1)}\0${crypto.createHash("sha256").update(Buffer.concat(chunks)).digest("hex")}\0${request.headers["x-kuvo-vault-at"]}`).digest("hex")) {
          host.turnedAway++;
          return answer(403, {});
        }
        break;
    }
    switch (request.url) {
      // The vault files' newest generations, as the app keeps them in the Keychain.
      case "/vault/generations":
        // (`unreadable`: its Keychain locked; `adopt`: kept by an app from before the generations were.)
        if (host.unreadable) return answer(503, { message: "This Mac's Keychain can't be read now, so its vault waits." });
        host.generations ??= {};
        if (typeof given.file === "string") {
          host.adopt = false;
          if (given.generation > (host.generations[given.file] ?? 0)) host.generations[given.file] = given.generation;
        }
        return answer(200, { generations: host.generations, ...(host.adopt ? { adopt: true } : {}) });
      case "/vault/get":
        return host.vault?.has(given.id) ? answer(200, { values: host.vault.get(given.id) }) : answer(404, {});
      case "/vault/set":
        (host.vault ??= new Map()).set(given.id, given.values);
        return answer(200, {});
      case "/vault/delete":
        host.vault?.delete(given.id);
        return answer(200, {});
      // Whether its browser is signed in to the sites the agent uses there, as the test sets it.
      case "/sites/signed-in":
        return host.sites ? answer(200, { sites: host.sites }) : answer(503, { message: "It can't tell now." });
      default:
        return answer(404, {});
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  atEnd(() => { server.closeAllConnections(); server.close(); });
  host.url = `http://127.0.0.1:${server.address().port}`;
  return host;
}

/** This Mac's box, started as the app starts it: its routes, and `stop`, which gives back all it printed. */
export async function served(env) {
  const child = spawn(process.execPath, [script, "serve"], { env: { ...env, OPENCODE_PASSWORD: "box-pw" }, stdio: ["pipe", "pipe", "pipe"] });
  let out = "", err = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", chunk => { err += chunk; });
  const exited = new Promise(resolve => child.once("exit", resolve));
  const url = await new Promise((resolve, reject) => {
    child.stdout.on("data", chunk => {
      out += chunk;
      if (out.includes("\n")) resolve(JSON.parse(out.split("\n")[0]).url);
    });
    exited.then(code => reject(new Error(`serve exited ${code}: ${err}`)));
  });
  const route = async (method, path, body) => {
    const response = await fetch(`${url}/${path}`, { method, headers: { authorization: `Basic ${Buffer.from("opencode:box-pw").toString("base64")}`, "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });
    return { status: response.status, value: await response.json().catch(() => ({})) };
  };
  const stop = async () => {
    child.stdin.end();
    await exited;
    return out + err;
  };
  return { route, stop };
}
