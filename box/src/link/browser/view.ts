// The same keeper stream for serve and paired relay clients. No engine or Chrome port.
import { LineBuffer } from "./buffers";
import net from "node:net";
import path from "node:path";
import { RUN, socketKey } from "../home";
import type { SocketEvents, SocketHandle } from "../relay/host";

const COMMANDS = new Set(["Kuvo.view", "Kuvo.frameAck", "Kuvo.frameDropped", "Kuvo.pages", "Kuvo.input", "Kuvo.go", "Kuvo.upload", "Kuvo.hold", "Kuvo.claim"]);
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
  const socket = net.createConnection(path.join(run, "browser.sock"));
  let opened = false, ended = false;
  const incoming = new LineBuffer(80 * 1024 * 1024), input = new LineBuffer(72 * 1024 * 1024);
  const facts = new Map<string, Buffer>();
  let notice = 0;
  const flush = () => {
    for (const [name, bytes] of facts) {
      if (!budget(Math.ceil(bytes.length / 59000))) break;
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
  socket.on("data", (chunk: Buffer) => {
    try { incoming.feed(chunk, line => {
      if (ended) return;
      let message: any;
      try { message = JSON.parse(line); } catch { return end(new Error("Invalid browser message.")); }
      if (!opened) {
        if (message.method !== "Kuvo.hello") return end(new Error("Browser refused connection."));
        opened = true; events.open();

      }
      if (message.method === "Kuvo.frame") {
        const { data, ...meta } = message.params;
        const jpeg = Buffer.from(data, "base64");
        const header = Buffer.from(JSON.stringify({ method: "Kuvo.frame", params: meta }));
        const length = Buffer.alloc(4); length.writeUInt32BE(header.length);
        const frame = Buffer.concat([length, header, jpeg]);
        // Binary fits one relay message; oversize pictures are dropped and credited locally.
        const reason = !relay ? null : frame.length > 60000 ? "oversize" : (options.unsent?.() ?? 0) > 0 ? "unsent" : !budget(1, 80) ? "budget" : null;
        if (reason) {
          socket.write(JSON.stringify({ method: "Kuvo.frameDropped", params: { frameId: meta.frameId, reason } }) + "\n");
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
          if (facts.size >= 64) socket.pause();
          flush();
        }
      }
    }); } catch (error) { end(error as Error); }
  });
  return {
    send(data) {
      if (!opened || ended) return;
      try { input.feed(data, line => {
        if (ended) return;
        let message: any;
        try { message = JSON.parse(line); } catch { return end(new Error("Invalid browser request.")); }
        if (!message || typeof message !== "object" || !COMMANDS.has(message.method)) return end(new Error("Unknown view command."));
        socket.write(line + "\n");
      }); } catch (error) { end(error as Error); }
    },
    close() { clearInterval(timer); ended = true; socket.destroy(); },
  };
}
