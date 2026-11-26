// The app's terminals (`api/pty`, and `api/pty/:id/connect` as a socket, PROTOCOL.md "Sockets"),
// as the app reaches them on a computer's own server. The reference box passes them to its engine,
// the pinned OpenCode; the Rust box runs them itself, with no OpenCode at all. Both must give the
// app the same: the routes' answers, the socket's frames and its close codes.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const version = "2.2.6";
const binary = boxBinary();
// (The reference's terminals are its engine's: it needs the pinned OpenCode.)
const windows = process.platform === "win32";
const skip = (binary.typescript && windows && "The reference doesn't run on Windows.")
  || (binary.typescript && !pinned && "Set KUVO_CONFORMANCE_OPENCODE to the pinned OpenCode (used in place).");
const basic = password => "Basic " + Buffer.from(`opencode:${password}`).toString("base64");
const units = text => text.length;

/** What arrives, in order, each taken once it has come. */
class Arrivals {
  items = [];
  waiting = [];
  add(item) {
    const wait = this.waiting.shift();
    if (wait) wait(item);
    else this.items.push(item);
  }
  next(timeout = 10000) {
    if (this.items.length) return Promise.resolve(this.items.shift());
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("Nothing more came.")), timeout);
      this.waiting.push(item => { clearTimeout(timer); resolve(item); });
    });
  }
}

describe("Terminals", { skip }, () => {
  let root, home, work, other, box, listener, worker, engine, relay;

  before(async () => {
    root = fs.realpathSync(scratch());
    home = path.join(root, "box");
    work = path.join(root, "work");
    other = path.join(root, "other");
    privateHome(home);
    for (const folder of [work, other]) fs.mkdirSync(folder, { recursive: true });
    write(path.join(home, "folder-version.json"), '{"version":23}\n');
    // A shell with no profile to ask questions: the same one for both boxes and OpenCode.
    const env = { ...isolatedEnv(root), SHELL: "/bin/bash", BASH_SILENCE_DEPRECATION_WARNING: "1", KUVO_SANDBOX: "off" };
    if (binary.typescript) {
      const program = path.join(home, "engine", version, "opencode");
      fs.mkdirSync(path.dirname(program), { recursive: true });
      fs.symlinkSync(fs.realpathSync(pinned), program);
      const reserve = net.createServer();
      await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
      const port = reserve.address().port;
      await new Promise(resolve => reserve.close(resolve));
      const password = "scratch-engine";
      const dirs = Object.fromEntries(["data", "config", "state", "cache"].map(sub => [`XDG_${sub.toUpperCase()}_HOME`, path.join(home, sub)]));
      for (const folder of [...Object.values(dirs), path.join(home, "workspace")]) fs.mkdirSync(folder, { recursive: true });
      worker = launch(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"],
        { ...env, ...dirs, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password }, path.join(home, "workspace"));
      engine = { port, password };
      await until(async () => (await request(engine, "GET", "api/pty").catch(() => null))?.status === 200, "The reference's engine", 30000);
      write(path.join(home, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version }));
    }
    relay = await localRelay();
    box = startBox({ binary, home, env, relay });
    listener = await box.ready();
  });

  after(async () => {
    await box?.stop();
    await worker?.stop?.();
    await relay?.close();
    if (root) fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
  });

  const inFolder = folder => ({ "x-opencode-directory": encodeURIComponent(folder) });
  const call = (method, route, body, folder = work) => request(listener, method, route, body, inFolder(folder), 15000);

  /** A terminal's socket, as the app opens one: with serve's password, in the folder. */
  function connect(id, query = "", password = listener.password) {
    const socket = new WebSocket(`ws://127.0.0.1:${listener.port}/api/pty/${id}/connect${query}`,
      { headers: { authorization: basic(password), "x-kuvo-route-version": "1", ...inFolder(work) } });
    socket.binaryType = "arraybuffer";
    const got = new Arrivals();
    let printed = "";
    socket.onmessage = event => {
      if (typeof event.data === "string") printed += event.data;
      got.add(typeof event.data === "string" ? event.data : Buffer.from(event.data));
    };
    const opened = new Promise((resolve, reject) => {
      socket.onopen = () => resolve();
      socket.onerror = () => reject(new Error("The terminal's socket didn't open."));
    });
    const ended = new Promise(resolve => { socket.onclose = event => resolve({ code: event.code, reason: event.reason }); });
    /** Waits until what it printed holds `text`. */
    const printedSoFar = () => printed;
    const shows = async text => until(async () => printed.includes(text), () => `"${text}" in the terminal, which shows ${JSON.stringify(printed.slice(-400))}`, 15000);
    return { socket, got, opened, ended, shows, printed: printedSoFar };
  }

  /** The note a socket sends after what it printed so far: where its output had got to. */
  const cursorOf = message => {
    assert.ok(Buffer.isBuffer(message) && message[0] === 0, `a note, not ${JSON.stringify(String(message)).slice(0, 80)}`);
    return JSON.parse(message.subarray(1).toString("utf8")).cursor;
  };

  /** What a socket sends before its note, and the note's cursor. */
  async function untilNote(terminal) {
    let text = "";
    for (;;) {
      const message = await terminal.got.next();
      if (typeof message !== "string") return { text, cursor: cursorOf(message) };
      text += message;
    }
  }

  // An upgrade asked by hand, for the status it's refused with.
  async function refusal(route, headers) {
    const socket = net.createConnection(listener.port, "127.0.0.1");
    socket.write(`GET /${route} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n${headers.map(header => `${header}\r\n`).join("")}\r\n`);
    // (Its status: the words after it are each server's own, and it may keep the connection.)
    let said = "";
    for await (const chunk of socket) {
      said += chunk;
      if (said.includes("\r\n")) break;
    }
    socket.destroy();
    return said.split("\r\n")[0].split(" ").slice(0, 2).join(" ");
  }

  let id, shell;

  test("A terminal is the computer's shell in the folder asked for, listed for that folder only", async () => {
    const made = await call("POST", "api/pty", { cwd: work });
    assert.equal(made.status, 200, made.text);
    const terminal = made.json.data;
    id = terminal.id;
    assert.match(id, /^pty_[0-9a-f]{12}[0-9A-Za-z]{14}$/);
    // (On Windows the computer's shell is Git Bash's.)
    shell = terminal.command;
    if (windows) assert.match(shell, /[\\/]bash\.exe$/i);
    assert.deepEqual({ ...terminal, pid: typeof terminal.pid }, {
      id, title: `Terminal ${id.slice(-4)}`, command: windows ? shell : "/bin/bash", args: ["-l"], cwd: work, status: "running", pid: "number",
    });
    assert.deepEqual(made.json.location, { directory: work });
    const listed = await call("GET", "api/pty");
    assert.deepEqual(listed.json.data.map(each => each.id), [id]);
    assert.deepEqual((await call("GET", "api/pty", undefined, other)).json.data, []);
    const one = await call("GET", `api/pty/${id}`);
    assert.equal(one.status, 200);
    assert.deepEqual(one.json.data, terminal);
  });

  test("What's typed reaches it, as text or binary, at the size it's drawn at, and what it prints comes back", async () => {
    const resized = await call("PUT", `api/pty/${id}`, { size: { rows: 30, cols: 100 } });
    assert.equal(resized.status, 200, resized.text);
    assert.equal(resized.json.data.id, id);
    const terminal = connect(id, "?cursor=0");
    await terminal.opened;
    await untilNote(terminal);
    // (What it prints as text, a character split across reads and all: é✓ from octal escapes.)
    terminal.socket.send("printf 'size:%s \\303\\251\\342\\234\\223 %s\\n' \"$(stty size)\" \"$(pwd -W 2>/dev/null || pwd)\"\r");
    await terminal.shows(`size:30 100 é✓ ${work.replaceAll("\\", "/")}`);
    terminal.socket.send(new TextEncoder().encode("echo binary-$((40+2))\r"));
    await terminal.shows("binary-42");
    // Kuvo's own settings and secrets aren't in its shell; a terminal's TERM is.
    terminal.socket.send("echo \"term:$TERM secret:${OPENCODE_SERVER_PASSWORD}${KUVO_AGENTS_PASSWORD}:end\"\r");
    await terminal.shows("term:xterm-256color secret:");
    // (The reference's shell is its engine's, with the engine's password.)
    if (!binary.typescript) await terminal.shows("term:xterm-256color secret::end");
    // The client closes its socket: the code it closed with comes back, and the shell runs on.
    terminal.socket.close(4001, "panel closed");
    assert.deepEqual(await terminal.ended, { code: 4001, reason: "panel closed" });
    assert.equal((await call("GET", `api/pty/${id}`)).json.data.status, "running");
  });

  test("A socket opened again takes up where its cursor says, and its note says where the output got to", async () => {
    const all = connect(id, "?cursor=0");
    await all.opened;
    const { text, cursor } = await untilNote(all);
    assert.ok(text.includes("binary-42"));
    assert.equal(units(text), cursor);
    const latest = connect(id, "?cursor=-1");
    await latest.opened;
    assert.deepEqual(await untilNote(latest), { text: "", cursor });
    const tail = connect(id, `?cursor=${cursor - 5}`);
    await tail.opened;
    assert.deepEqual(await untilNote(tail), { text: text.slice(-5), cursor });
    const past = connect(id, `?cursor=${cursor + 100}`);
    await past.opened;
    assert.deepEqual(await untilNote(past), { text: "", cursor });
    for (const terminal of [all, latest, tail, past]) terminal.socket.close(1000);
  });

  test("Its shell exiting closes its sockets; it's kept, with its code, until removed", async () => {
    const terminal = connect(id, "?cursor=-1");
    await terminal.opened;
    await untilNote(terminal);
    terminal.socket.send("exit 3\r");
    assert.deepEqual(await terminal.ended, { code: 1000, reason: "" });
    await until(async () => (await call("GET", `api/pty/${id}`)).json.data.status === "exited", "the terminal exited");
    const ended = (await call("GET", `api/pty/${id}`)).json.data;
    assert.equal(ended.exitCode, 3);
    assert.deepEqual((await call("GET", "api/pty")).json.data, [ended]);
    const late = connect(id, "?cursor=0");
    await late.opened;
    assert.deepEqual(await late.ended, { code: 4404, reason: "session exited" });
    const removed = await call("DELETE", `api/pty/${id}`);
    assert.equal(removed.status, 204);
    const again = await call("DELETE", `api/pty/${id}`);
    assert.equal(again.status, 404);
    assert.deepEqual(again.json, { _tag: "PtyNotFoundError", ptyID: id, message: `PTY session not found: ${id}` });
    assert.equal((await call("GET", `api/pty/${id}`)).status, 404);
    assert.deepEqual((await call("GET", "api/pty")).json.data, []);
  });

  test("A terminal removed while its socket is open closes it", async () => {
    const made = (await call("POST", "api/pty", {})).json.data;
    assert.equal(made.cwd, work);
    const terminal = connect(made.id);
    await terminal.opened;
    assert.equal((await call("DELETE", `api/pty/${made.id}`)).status, 204);
    assert.deepEqual(await terminal.ended, { code: 1000, reason: "" });
    // Nothing runs for it once it's removed.
    const alive = pid => { try { process.kill(pid, 0); return true; } catch { return false; } };
    await until(async () => !alive(made.pid), "its shell stopped", 5000);
  });

  test("A long burst is kept as OpenCode keeps it: its last 2 MiB, sent again in 64 KiB pieces", async () => {
    const made = await call("POST", "api/pty", { command: windows ? shell : "/bin/sh", args: ["-c", "head -c 3000000 /dev/zero | tr '\\0' a; sleep 30"], title: "burst" });
    const terminal = made.json.data;
    assert.deepEqual([terminal.title, terminal.args], ["burst", ["-c", "head -c 3000000 /dev/zero | tr '\\0' a; sleep 30", "-l"]]);
    let pieces;
    await until(async () => {
      const socket = connect(terminal.id, "?cursor=0");
      await socket.opened;
      pieces = [];
      for (;;) {
        const message = await socket.got.next();
        if (typeof message !== "string") { pieces.push(cursorOf(message)); break; }
        pieces.push(units(message));
      }
      socket.socket.close(1000);
      // (Windows' console repaints what's printed as a screen, so its count is its own.)
      return windows ? pieces.at(-1) > 2_200_000 : pieces.at(-1) === 3000000;
    }, "the whole burst", 20000);
    assert.deepEqual(pieces.slice(0, -1), Array(32).fill(65536));
    if (!windows) assert.equal(pieces.at(-1), 3000000);
    const renamed = await call("PUT", `api/pty/${terminal.id}`, { title: "renamed" });
    assert.equal(renamed.json.data.title, "renamed");
    assert.equal((await call("PUT", `api/pty/${terminal.id}`, { size: { rows: 0, cols: 0 } })).status, 400);
    assert.equal((await call("PUT", "api/pty/nope", { size: { rows: 3, cols: 3 } })).status, 400);
    assert.equal((await call("DELETE", `api/pty/${terminal.id}`)).status, 204);
  });

  test("A terminal's socket is refused without serve's password, only for a terminal and a known route version", async () => {
    const made = (await call("POST", "api/pty", {})).json.data;
    const upgrade = `api/pty/${made.id}/connect`;
    assert.equal(await refusal(upgrade, [`Authorization: ${basic("wrong")}`]), "HTTP/1.1 401");
    assert.equal(await refusal(upgrade, []), "HTTP/1.1 401");
    if (engine) assert.equal(await refusal(upgrade, [`Authorization: ${basic(engine.password)}`]), "HTTP/1.1 401");
    const serve = `Authorization: ${basic(listener.password)}`;
    assert.equal(await refusal(`api/pty/${made.id}`, [serve]), "HTTP/1.1 403");
    assert.equal(await refusal("api/event", [serve]), "HTTP/1.1 403");
    assert.equal(await refusal(upgrade, [serve, "X-Kuvo-Route-Version: 2"]), "HTTP/1.1 400");
    // A terminal that isn't there: its socket doesn't open, and the client is told.
    assert.equal(await refusal("api/pty/pty_gone/connect", [serve]), "HTTP/1.1 502");
    assert.equal((await call("DELETE", `api/pty/${made.id}`)).status, 204);
  });
});
