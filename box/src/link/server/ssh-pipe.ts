// A private bridge from the SSH command's stdio to the resident dispatcher.
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { once } from "node:events";
import { privateFolder } from "../../shared/files";
import { isObject } from "../../shared/json";
import { ROUTE_VERSION } from "../../shared/route-version";
import { ChunkQueue } from "../browser/buffers";
import { localToolRequest } from "./tool-call";
import { APP_FOLDER, isSocketKey, RUN, socketKey } from "../home";
import type { LinkHandler } from "./context";
import { answerSshRoute, sshLine, writeSshBytes, type SshCommand } from "./ssh-route";

const folder = path.join(RUN, "route");
const socketPath = path.join(folder, "ssh");
const packetSize = 64 * 1024;
const accepted = 1;

export function selectSshPipe(): void {
  privateFolder(folder);
}

/** Selection outlives the listener so a restart can't redirect writes into a transient process. */
export function sshPipeSelected(): boolean {
  if (APP_FOLDER) return false;
  try { fs.lstatSync(folder); return true; }
  catch (error) { return (error as NodeJS.ErrnoException).code !== "ENOENT"; }
}

function socketIsPrivate(): boolean {
  const stat = fs.lstatSync(socketPath);
  return stat.isSocket() && !(stat.mode & 0o077) && (process.getuid === undefined || stat.uid === process.getuid());
}

// Zero marks stdio EOF. A socket disconnect still cancels a request after stdin EOF,
// and tells the CLI an answer was cut short if the server never sent stdout EOF.
async function* packets(socket: net.Socket): AsyncGenerator<Buffer> {
  const pending = new ChunkQueue();
  for await (const bytes of socket.iterator({ destroyOnReturn: false })) {
    pending.append(bytes);
    while (pending.length >= 4) {
      const size = pending.peek(4).readUInt32BE();
      if (size > packetSize) throw new Error("Invalid SSH pipe packet.");
      if (pending.length < 4 + size) break;
      pending.drop(4);
      if (size === 0) {
        // Keep detecting a real disconnect after the framed stdin EOF.
        setImmediate(() => socket.resume());
        return;
      }
      yield pending.read(size);
    }
  }
  throw new Error("SSH pipe disconnected.");
}

function packet(socket: net.Socket, bytes: Buffer): Promise<void> {
  const framed = Buffer.allocUnsafe(4 + bytes.length);
  framed.writeUInt32BE(bytes.length);
  bytes.copy(framed, 4);
  return writeSshBytes(socket, framed);
}

async function sendPackets(socket: net.Socket, bytes: Buffer): Promise<void> {
  for (let start = 0; start < bytes.length; start += packetSize) {
    await packet(socket, bytes.subarray(start, start + packetSize));
  }
}

async function clearStaleSocket(): Promise<void> {
  try {
    if (!socketIsPrivate()) throw new Error("Kuvo's SSH socket isn't private.");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return;
    throw error;
  }
  const found = net.createConnection(socketPath);
  try {
    await once(found, "connect", { signal: AbortSignal.timeout(2000) });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ECONNREFUSED") throw error;
    fs.unlinkSync(socketPath);
    return;
  } finally { found.destroy(); }
  throw new Error("Kuvo's SSH socket already has an owner.");
}

export async function serveSshPipe(handle: LinkHandler, addressedHandle: LinkHandler = handle): Promise<void> {
  socketKey();
  await clearStaleSocket();
  const server = net.createServer(socket => {
    const disconnected = new AbortController();
    socket.on("end", () => disconnected.abort());
    socket.on("close", () => disconnected.abort());
    socket.on("error", () => disconnected.abort());
    const input = packets(socket);
    const write = (bytes: Buffer) => sendPackets(socket, bytes);
    const deadline = setTimeout(() => socket.destroy(), 10_000).unref();
    void (async () => {
      const first = await input.next();
      const command: unknown = first.done ? null : JSON.parse(first.value.toString("utf8"));
      if (!isObject(command) || !isSocketKey(command.key) || typeof command.addressed !== "boolean" ||
          typeof command.internal !== "boolean" || (command.internalPath !== undefined && typeof command.internalPath !== "string") ||
          (command.addressed && (command.internal || command.internalPath !== undefined))) return void socket.destroy();
      clearTimeout(deadline);
      await write(Buffer.from([accepted]));
      await answerSshRoute({ addressed: command.addressed, internal: command.internal,
        internalPath: command.internalPath as string | undefined }, input, write, disconnected.signal,
        command.addressed ? addressedHandle : handle);
      await packet(socket, Buffer.alloc(0));
      socket.end();
    })().catch(() => socket.destroy()).finally(() => clearTimeout(deadline));
  });
  const listening = once(server, "listening");
  const old = process.umask(0o177);
  try { server.listen(socketPath); }
  finally { process.umask(old); }
  await listening;
}

async function unavailable(): Promise<void> {
  await writeSshBytes(process.stdout, sshLine({ status: 503, routeVersion: ROUTE_VERSION,
    body: JSON.stringify({ message: "Kuvo on this computer is starting. Connect again in a moment." }) }));
}

const startupWait = 80_000;

async function openPipe(command: SshCommand, signal: AbortSignal, until: number) {
  while (!signal.aborted && Date.now() < until) {
    let socket: net.Socket | undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const close = () => socket?.destroy();
    let connected = false;
    try {
      // A present socket with the wrong permissions is never used.
      if (!socketIsPrivate()) return;
      const key = socketKey({ create: false });
      socket = net.createConnection(socketPath);
      socket.on("error", () => {}); // Reported by the exchange.
      signal.addEventListener("abort", close, { once: true });
      timer = setTimeout(close, Math.min(5000, until - Date.now())).unref();
      const response = packets(socket);
      await packet(socket, Buffer.from(JSON.stringify({ ...command, key })));
      const first = await response.next();
      if (first.done || first.value.length !== 1 || first.value[0] !== accepted) throw new Error("SSH pipe did not accept the command.");
      connected = true;
      return { socket, response };
    } catch {
      // The selection survives while the socket or key is missing, stale, or starting.
    } finally {
      clearTimeout(timer);
      signal.removeEventListener("abort", close);
      if (!connected) close();
    }
    await delay(200, undefined, { signal });
  }
  return undefined;
}

export async function pipeSshRoute(command: SshCommand): Promise<void> {
  const stop = new AbortController();
  let socket: net.Socket | undefined;
  const abort = () => { stop.abort(); socket?.destroy(); process.stdin.destroy(); };
  process.once("SIGTERM", abort);
  process.stdout.once("error", abort);
  try {
    let input: Buffer | undefined;
    let replay = false;
    let until: number | undefined;
    if (command.internalPath) input = Buffer.alloc(0);
    else if (!command.addressed) {
      const chunks: Buffer[] = [];
      for await (const chunk of process.stdin) chunks.push(chunk);
      ({ input, replay } = localToolRequest(Buffer.concat(chunks)));
    }
    while (!stop.signal.aborted) {
      const opened = await openPipe(command, stop.signal, until ?? Date.now() + startupWait);
      if (!opened) {
        if (!stop.signal.aborted) await unavailable();
        return;
      }
      ({ socket } = opened);
      const active = socket;
      let pending: Buffer | undefined;
      const send = async () => {
        if (input !== undefined) await sendPackets(active, input);
        else for await (const bytes of process.stdin) await sendPackets(active, bytes);
        await packet(active, Buffer.alloc(0));
      };
      void send().catch(() => active.destroy());
      try {
        const reply: Buffer[] = [];
        for await (const bytes of opened.response) {
          if (replay) reply.push(bytes);
          else await writeSshBytes(process.stdout, bytes);
        }
        if (replay) {
          const complete = Buffer.concat(reply);
          if (complete.length) {
            const answer = JSON.parse(complete.toString("utf8"));
            const retry = answer.status === 503 && JSON.parse(answer.body || "null")?.retryToolCall === true;
            if (!retry) {
              await writeSshBytes(process.stdout, complete);
              return;
            }
            pending = complete;
          }
        } else return;
      } catch {
        if (!replay) { process.exitCode = 1; return; }
      } finally {
        active.destroy();
      }
      until ??= Date.now() + startupWait;
      if (Date.now() >= until) {
        if (pending) await writeSshBytes(process.stdout, pending);
        else await unavailable();
        return;
      }
      await delay(200, undefined, { signal: stop.signal });
    }
  } catch (error) {
    if (!stop.signal.aborted) throw error;
  } finally {
    socket?.destroy();
    process.stdin.destroy();
    process.off("SIGTERM", abort);
    process.stdout.off("error", abort);
  }
}
