// The same keeper stream for serve and paired relay clients. No engine or Chrome port.
import { browserControlFormat } from "./keeper-client";
import { encodeBrowserFrame } from "./media";
import { LineBuffer } from "./buffers";
import { execFile } from "node:child_process";
import net from "node:net";
import path from "node:path";
import { K, OWN_RUNTIME, RUN, socketKey } from "../home";
import type { SocketEvents, SocketHandle } from "../relay/host";

const COMMANDS = new Set(["Kuvo.view", "Kuvo.frameAck", "Kuvo.frameDropped", "Kuvo.pages", "Kuvo.input", "Kuvo.go", "Kuvo.upload", "Kuvo.hold", "Kuvo.claim", "Kuvo.dialogAnswer"]);
const START_FAILED = "The browser couldn’t start.";
let starting: Promise<void> | undefined;

/** Concurrent views share a start. Use the same startup path as the browser tools and SSH view. */
function startKeeper(): Promise<void> {
  return starting ??= new Promise<void>((resolve, reject) => {
    const script = path.join(K, "bin", "kuvo-browser.js");
    execFile(process.execPath, [...OWN_RUNTIME, script, "start"], {
      cwd: K, env: { ...process.env, KUVO_HOME: K, BUN_BE_BUN: "1" },
    }, (error, stdout) => {
      if (error) return reject(new Error(START_FAILED));
      try {
        const problem = JSON.parse(stdout);
        if (problem === null) resolve();
        else reject(new Error(problem.message || START_FAILED));
      } catch {
        reject(new Error(START_FAILED));
      }
    });
  }).finally(() => { starting = undefined; });
}

let windowAt = 0, messages = 0;
/** Shared by every relay viewer on this box; the relay counts messages, not bytes. */
export function frameBudget(count: number, now = Date.now(), ceiling = 100): boolean {
  if (now - windowAt >= 1000) { windowAt = now; messages = 0; }
  if (messages + count > ceiling) return false;
  messages += count;
  return true;
}
export function browserView(events: SocketEvents, relay = false, run = RUN, key = socketKey, options: { unsent?: () => number; budget?: (count: number, ceiling?: number) => boolean } = {}): SocketHandle {
  const budget = options.budget ?? ((count: number, ceiling = 100) => frameBudget(count, Date.now(), ceiling));
  let socket: net.Socket | undefined;
  let opened = false, greeted = false, ended = false, requestedStart = false;
  const incoming = new LineBuffer(80 * 1024 * 1024), input = new LineBuffer(72 * 1024 * 1024);
  const facts = new Map<string, Buffer>();
  let notice = 0;
  const flush = () => {
    for (const [name, bytes] of facts) {
      if (!budget(Math.ceil(bytes.length / 59000))) break;
      facts.delete(name); events.text(bytes);
    }
    if (facts.size < 32) socket?.resume();
  };
  const timer = relay ? setInterval(flush, 50) : undefined;
  timer?.unref();
  const open = () => {
    if (opened) return;
    opened = true;
    events.open();
  };
  const close = () => { clearInterval(timer); ended = true; socket?.destroy(); };
  const end = (error?: Error) => {
    if (ended) return;
    close();
    events.end(error);
  };
  const failedToStart = (error: Error) => {
    if (ended) return;
    events.text(Buffer.from(JSON.stringify(browserControlFormat.write({
      method: "Kuvo.error", params: { kind: "failed", message: error.message },
    })) + "\n"));
    close();
    events.end(error, { code: 1011, reason: START_FAILED });
  };
  const wake = () => {
    requestedStart = true;
    socket = undefined;
    // A cold start (or installing Chrome) can outlast the client's upgrade deadline.
    open();
    void startKeeper().then(() => { if (!ended) dial(); }, failedToStart);
  };
  const received = (client: net.Socket, chunk: Buffer) => {
    try { incoming.feed(chunk, line => {
      if (ended) return;
      let message: any;
      message = browserControlFormat.read(JSON.parse(line));
      if (!greeted) {
        if (message.method === "Kuvo.error" && requestedStart) {
          return failedToStart(new Error(message.params?.message || START_FAILED));
        }
        if (message.method !== "Kuvo.hello") return end(new Error("Browser refused connection."));
        greeted = true;
        open();
      }
      if (message.method === "Kuvo.frame") {
        const { data, ...meta } = message.params;
        const jpeg = Buffer.from(data, "base64");
        const frame = encodeBrowserFrame(meta, jpeg);
        // Binary fits one relay message; oversize pictures are dropped and credited locally.
        const reason = !relay ? null : frame.length > 60000 ? "oversize" : (options.unsent?.() ?? 0) > 0 ? "unsent" : !budget(1, 80) ? "budget" : null;
        if (reason) {
          client.write(JSON.stringify({ method: "Kuvo.frameDropped", params: { frameId: meta.frameId, reason } }) + "\n");
          return;
        }
        events.binary(frame);
      } else {
        if (message.id === undefined && !String(message.method).startsWith("Kuvo.")) return;
        const bytes = Buffer.from(line + "\n");
        if (!relay) events.text(bytes);
        else {
          const state = ["Kuvo.pages", "Kuvo.status", "Kuvo.viewport", "Kuvo.pointer", "Kuvo.cursor"].includes(message.method);
          const name = state ? message.method + ":" + (message.params?.targetId ?? "") : "notice:" + ++notice;
          facts.set(name, bytes);
          if (facts.size >= 64) client.pause();
          flush();
        }
      }
    }); } catch (error) { end(error as Error); }
  };
  function dial() {
    const client = socket = net.createConnection(path.join(run, "browser.sock"));
    client.on("connect", () => {
      try {
        client.write(JSON.stringify(browserControlFormat.write({ method: "Kuvo.key", params: { key: key() } })) + "\n");
      } catch (error) { end(error as Error); }
    });
    client.on("error", (error: NodeJS.ErrnoException) => {
      if (ended) return;
      if (!requestedStart && !greeted && (error.code === "ENOENT" || error.code === "ECONNREFUSED")) wake();
      else if (requestedStart && !greeted) failedToStart(error);
      else end(error);
    });
    client.on("close", () => {
      if (ended || socket !== client) return;
      if (greeted) end();
      else if (!requestedStart) wake();
      else failedToStart(new Error(START_FAILED));
    });
    client.on("data", chunk => received(client, chunk));
  }
  dial();
  return {
    send(data) {
      if (!greeted || ended) return;
      try { input.feed(data, line => {
        if (ended) return;
        let message: any;
        message = browserControlFormat.read(JSON.parse(line));
        if (!message || typeof message !== "object" || !COMMANDS.has(message.method)) return end(new Error("Unknown view command."));
        socket?.write(line + "\n");
      }); } catch (error) { end(error as Error); }
    },
    close,
  };
}
