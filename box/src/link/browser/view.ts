// The same keeper stream for serve and paired relay clients. No engine or Chrome port.
import net from "node:net";
import path from "node:path";
import { RUN, socketKey } from "../home";
import type { SocketEvents, SocketHandle } from "../relay/host";

const COMMANDS = new Set(["Kuvo.view", "Kuvo.frameAck", "Kuvo.pages", "Kuvo.input", "Kuvo.go", "Kuvo.upload", "Kuvo.hold", "Kuvo.claim"]);
let windowAt = 0, messages = 0;
/** Shared by every relay viewer on this box; the relay counts messages, not bytes. */
export function frameBudget(count: number, now = Date.now(), ceiling = 100): boolean {
  if (now - windowAt >= 1000) { windowAt = now; messages = 0; }
  if (messages + count > ceiling) return false;
  messages += count;
  return true;
}
export function browserView(events: SocketEvents, relay = false, run = RUN, key = socketKey): SocketHandle {
  const socket = net.createConnection(path.join(run, "browser.sock"));
  let buffer = "", input = "", opened = false, ended = false;
  const facts = new Map<string, Buffer>();
  let notice = 0;
  const flush = () => {
    for (const [name, bytes] of facts) {
      if (!frameBudget(Math.ceil(bytes.length / 59000))) break;
      facts.delete(name); events.text(bytes);
    }
    if (facts.size < 32) socket.resume();
  };
  const timer = relay ? setInterval(flush, 50) : undefined;
  timer?.unref();
  const end = (error?: Error) => { clearInterval(timer); if (ended) return; ended = true; socket.destroy(); events.end(error); };
  socket.on("connect", () => socket.write(JSON.stringify({ method: "Kuvo.key", params: { key: key() } }) + "\n"));
  socket.on("error", end);
  socket.on("close", () => end());
  socket.setEncoding("utf8");
  socket.on("data", (chunk: string) => {
    buffer += chunk;
    if (buffer.length > 80 * 1024 * 1024) return end(new Error("Browser message too large."));
    let at: number;
    while ((at = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, at); buffer = buffer.slice(at + 1);
      let message: any;
      try { message = JSON.parse(line); } catch { return end(new Error("Invalid browser message.")); }
      if (!opened) {
        if (message.method !== "Kuvo.hello") return end(new Error("Browser refused connection."));
        opened = true; events.open();
        socket.write(JSON.stringify({ method: "Kuvo.viewing" }) + "\n");
      }
      if (message.method === "Kuvo.frame") {
        const { data, ...meta } = message.params;
        const jpeg = Buffer.from(data, "base64");
        const header = Buffer.from(JSON.stringify({ method: "Kuvo.frame", params: meta }));
        const length = Buffer.alloc(4); length.writeUInt32BE(header.length);
        const frame = Buffer.concat([length, header, jpeg]);
        // Binary fits one relay message; oversize pictures are dropped and credited locally.
        if ((relay && (frame.length > 60000 || !frameBudget(1, Date.now(), 80)))) {
          socket.write(JSON.stringify({ method: frame.length > 60000 ? "Kuvo.frameDropped" : "Kuvo.frameAck", params: { frameId: meta.frameId, ...(frame.length > 60000 ? { reason: "oversize" } : {}) } }) + "\n");
          continue;
        }
        events.binary(frame);
      } else {
        if (message.id === undefined && !String(message.method).startsWith("Kuvo.")) continue;
        const bytes = Buffer.from(line + "\n");
        if (!relay) events.text(bytes);
        else {
          const state = ["Kuvo.pages", "Kuvo.status", "Kuvo.viewport", "Kuvo.pointer", "Kuvo.cursor"].includes(message.method);
          const name = state ? message.method + ":" + (message.params?.targetId ?? "") : "notice:" + ++notice;
          facts.set(name, bytes);
          if (facts.size >= 64) socket.pause();
          flush();
        }
      }
    }
  });
  return {
    send(data) {
      if (!opened || ended) return;
      input += data.toString("utf8");
      if (input.length > 72 * 1024 * 1024) return end(new Error("Browser request too large."));
      let at: number;
      while ((at = input.indexOf("\n")) >= 0) {
        const line = input.slice(0, at); input = input.slice(at + 1);
        let message: any;
        try { message = JSON.parse(line); } catch { return end(new Error("Invalid browser request.")); }
        if (!message || typeof message !== "object" || !COMMANDS.has(message.method)) return end(new Error("Unknown view command."));
        socket.write(JSON.stringify(message) + "\n");
      }
    },
    close() { clearInterval(timer); ended = true; socket.destroy(); },
  };
}
