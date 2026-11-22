// Minimal RFC 6455 transport for the box's own server (`serve`): the browser's live view
// (`kuvo/browser/view`), and a terminal's socket on the engine (`api/pty/<id>/connect`), opened as
// the relay opens it (`engineConnect`). Authentication belongs to the upgrade, never the HTTP handler.
import { readBrowserFrame } from "./media";
import { ROUTE_VERSION, ROUTE_VERSION_HEADER, routeVersionError } from "../../shared/route-version";
import { ChunkQueue } from "./buffers";
import crypto from "node:crypto";
import type http from "node:http";
import type { Duplex } from "node:stream";
import { canonicalPath, socketAllowed } from "../server/paths";
import { browserView } from "./view";
import { NO_VIEW_ON_MAC } from "./routes";
import { IS_MAC } from "../home";
import { sendableClose, type SocketClose, type SocketEvents, type SocketHandle } from "../relay/host";
import type { LinkRequest } from "../server/context";

/** What opens a socket on the engine here, for a client on this server: `engineConnect`, with no phone. */
export type EngineSockets = (request: LinkRequest, phone: undefined, events: SocketEvents) => SocketHandle;

// (`engine`: where the other sockets a phone may open through the relay go, a terminal's; without
// it, only the view opens.)
export function viewUpgrade(server: http.Server, password: string, connect: (events: SocketEvents) => SocketHandle = browserView, here = !IS_MAC, engine?: EngineSockets): void {
  server.on("upgrade", (request, socket: Duplex, head) => {
    socket.on("error", () => socket.destroy());
    const expected = Buffer.from("Basic " + Buffer.from(`opencode:${password}`).toString("base64"));
    const given = Buffer.from(request.headers.authorization ?? "");
    const refuse = (status: number) => socket.end(`HTTP/1.1 ${status} Refused\r\nConnection: close\r\n\r\n`, () => socket.destroy());
    if (given.length !== expected.length || !crypto.timingSafeEqual(given, expected)) { refuse(401); return; }
    const sent = canonicalPath(request.url ?? "");
    const view = sent === "kuvo/browser/view";
    // (A terminal's, on the engine: the other socket a phone may open through the relay.)
    const terminal = !view && sent !== null && !!engine && socketAllowed(sent);
    if (request.method !== "GET" || !(view || terminal)) { refuse(403); return; }
    // (A Mac's box has no browser of its own: why, in words.)
    if (view && !here) {
      const words = JSON.stringify({ message: NO_VIEW_ON_MAC });
      socket.end(`HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: ${Buffer.byteLength(words)}\r\nConnection: close\r\n\r\n${words}`, () => socket.destroy());
      return;
    }
    if (routeVersionError(request.headers[ROUTE_VERSION_HEADER])) { refuse(400); return; }
    const key = request.headers["sec-websocket-key"];
    if (request.headers["sec-websocket-version"] !== "13" || typeof key !== "string" || !/^[A-Za-z0-9+/]{22}==$/.test(key)
      || request.headers.upgrade?.toLowerCase() !== "websocket" || !/\bupgrade\b/i.test(String(request.headers.connection))) { refuse(400); return; }
    const pending = new ChunkQueue();
    let fragment: Buffer[] = [], fragmentSize = 0, fragmenting = false, opened = false, binary = false;
    let handle: SocketHandle | undefined;
    const finish = () => { handle?.close(); socket.destroy(); };
    const write = (opcode: number, data: Buffer) => {
      if (!opened || socket.destroyed || socket.writableEnded) return false;
      // (The view's pictures are dropped while its viewer is behind; the engine's binary never is.)
      if (opcode === 2 && view) { if (socket.writableLength > 256000) return false; }
      else if ((opcode === 1 || opcode === 2) && socket.writableLength + data.length > 4 * 1024 * 1024) { finish(); return false; }
      const header = Buffer.alloc(data.length < 126 ? 2 : data.length <= 65535 ? 4 : 10);
      header[0] = 0x80 | opcode;
      if (header.length === 2) header[1] = data.length;
      else if (header.length === 4) { header[1] = 126; header.writeUInt16BE(data.length, 2); }
      else { header[1] = 127; header.writeBigUInt64BE(BigInt(data.length), 2); }
      socket.write(Buffer.concat([header, data]));
      return true;
    };
    const read = (data: Buffer) => {
      pending.append(data);
      while (pending.length >= 2) {
        const head = pending.peek(Math.min(14, pending.length));
        const a = head[0]!, b = head[1]!, op = a & 15, fin = !!(a & 128);
        // (The view takes only text; the engine, binary too.)
        if ((a & 112) || !(b & 128) || (![0, 1, 8, 9, 10].includes(op) && !(op === 2 && terminal))) return finish();
        let length = b & 127, offset = 2;
        if (length === 126) { if (pending.length < 4) return; length = head.readUInt16BE(2); offset = 4; }
        else if (length === 127) {
          if (pending.length < 10) return;
          const n = head.readBigUInt64BE(2);
          if (n > 72n * 1024n * 1024n) return finish();
          length = Number(n); offset = 10;
        }
        if (length + fragmentSize > 72 * 1024 * 1024 || (op >= 8 && (!fin || length > 125))) return finish();
        if (pending.length < offset + 4 + length) return;
        const mask = head.subarray(offset, offset + 4);
        pending.drop(offset + 4);
        const body = pending.read(length);
        for (let i = 0; i < body.length; i++) body[i] = body[i]! ^ mask[i % 4]!;
        // (Its close code and reason go on to the other side.)
        if (op === 8) { write(8, body); handle?.close(body.length >= 2 ? { code: body.readUInt16BE(0), reason: body.subarray(2).toString("utf8") } : undefined); socket.end(); return; }
        if (op === 9) { write(10, body); continue; }
        if (op === 10) continue;
        if ((op === 0) !== fragmenting) return finish();
        if (fragment.length >= 4096) return finish();
        if (!fragmenting) binary = op === 2;
        fragment.push(body); fragmentSize += length; fragmenting = !fin;
        if (fin) {
          const whole = Buffer.concat(fragment);
          // A binary message (only a terminal's) to the engine as binary; text must be UTF-8.
          if (binary) handle?.binary?.(whole);
          else {
            try { new TextDecoder("utf-8", { fatal: true }).decode(whole); } catch { return finish(); }
            handle?.send(whole);
          }
          fragment = []; fragmentSize = 0;
        }
      }
    };
    // How the other side closed it, in a close frame: its code and reason, or none where it gave
    // none (1005); one cut off (1006, 1015) is cut off here too.
    const closing = (closed: SocketClose) => {
      if (closed.code === 1005) write(8, Buffer.alloc(0));
      if (!sendableClose(closed.code)) return;
      const code = Buffer.alloc(2);
      code.writeUInt16BE(closed.code);
      write(8, Buffer.concat([code, Buffer.from(closed.reason).subarray(0, 123)]));
    };
    socket.on("error", finish);
    socket.on("close", () => handle?.close());
    const events: SocketEvents = {
      open() {
        const accept = crypto.createHash("sha1").update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
        socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n${ROUTE_VERSION_HEADER}: ${ROUTE_VERSION}\r\n\r\n`);
        opened = true; if (head.length) read(head);
      },
      text: data => { write(1, data); },
      binary: view ? data => {
        let meta: ReturnType<typeof readBrowserFrame>;
        try { meta = readBrowserFrame(data); } catch { finish(); return; }
        if (write(2, data)) return;
        // This transport dropped the picture, so it returns that picture's keeper credit.
        handle?.send(Buffer.from(JSON.stringify({ method: "Kuvo.frameDropped", params: { frameId: meta.frameId, reason: "backpressure" } }) + "\n"));
      } : data => { write(2, data); },
      end(_error, closed) {
        if (!opened) refuse(502);
        else { if (closed) closing(closed); socket.end(); }
      },
    };
    handle = view ? connect(events) : engine!({ method: "GET", path: sent!, headers: request.headers, body: Buffer.alloc(0), served: true }, undefined, events);
    socket.on("data", read);
  });
}
