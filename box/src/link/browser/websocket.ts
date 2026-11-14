// Minimal RFC 6455 transport. Authentication belongs to the upgrade, never the HTTP handler.
import { ChunkQueue } from "./buffers";
import crypto from "node:crypto";
import type http from "node:http";
import type { Duplex } from "node:stream";
import { canonicalPath } from "../server/paths";
import { browserView } from "./view";
import type { SocketEvents, SocketHandle } from "../relay/host";

export function viewUpgrade(server: http.Server, password: string, connect: (events: SocketEvents) => SocketHandle = browserView): void {
  server.on("upgrade", (request, socket: Duplex, head) => {
    socket.on("error", () => socket.destroy());
    const expected = Buffer.from("Basic " + Buffer.from(`opencode:${password}`).toString("base64"));
    const given = Buffer.from(request.headers.authorization ?? "");
    const refuse = (status: number) => socket.end(`HTTP/1.1 ${status} Refused\r\nConnection: close\r\n\r\n`, () => socket.destroy());
    if (given.length !== expected.length || !crypto.timingSafeEqual(given, expected)) { refuse(401); return; }
    if (request.method !== "GET" || canonicalPath(request.url ?? "") !== "kuvo/browser/view") { refuse(403); return; }
    const key = request.headers["sec-websocket-key"];
    if (request.headers["sec-websocket-version"] !== "13" || typeof key !== "string" || !/^[A-Za-z0-9+/]{22}==$/.test(key)
      || request.headers.upgrade?.toLowerCase() !== "websocket" || !/\bupgrade\b/i.test(String(request.headers.connection))) { refuse(400); return; }
    const pending = new ChunkQueue();
    let fragment: Buffer[] = [], fragmentSize = 0, fragmenting = false, opened = false;
    let handle: SocketHandle | undefined;
    const finish = () => { handle?.close(); socket.destroy(); };
    const write = (opcode: number, data: Buffer) => {
      if (!opened || socket.destroyed) return false;
      if (opcode === 2 && socket.writableLength > 256000) return false;
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
        if ((a & 112) || !(b & 128) || ![0, 1, 8, 9, 10].includes(op)) return finish();
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
        if (op === 8) { write(8, body); handle?.close(); socket.end(); return; }
        if (op === 9) { write(10, body); continue; }
        if (op === 10) continue;
        if ((op === 0) !== fragmenting) return finish();
        if (fragment.length >= 4096) return finish();
        fragment.push(body); fragmentSize += length; fragmenting = !fin;
        if (fin) {
          const whole = Buffer.concat(fragment);
          try { new TextDecoder("utf-8", { fatal: true }).decode(whole); } catch { return finish(); }
          handle?.send(whole);
          fragment = []; fragmentSize = 0;
        }
      }
    };
    socket.on("error", finish);
    socket.on("close", () => handle?.close());
    handle = connect({
      open() {
        const accept = crypto.createHash("sha1").update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest("base64");
        socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
        opened = true; if (head.length) read(head);
      },
      text: data => { write(1, data); },
      binary: data => {
        if (write(2, data)) return;
        // This transport dropped the picture, so it returns that picture's keeper credit.
        const length = data.readUInt32BE(0);
        const meta = JSON.parse(data.subarray(4, 4 + length).toString());
        handle?.send(Buffer.from(JSON.stringify({ method: "Kuvo.frameAck", params: { frameId: meta.params.frameId } }) + "\n"));
      },
      end() { if (!opened) refuse(502); else socket.end(); },
    });
    socket.on("data", read);
  });
}
