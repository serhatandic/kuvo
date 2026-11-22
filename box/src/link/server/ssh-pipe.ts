// A private bridge from the SSH command's stdio to the resident dispatcher.
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { once } from "node:events";
import { privateFolder } from "../../shared/files";
import { isObject } from "../../shared/json";
import { ROUTE_VERSION } from "../../shared/route-version";
import { ChunkQueue } from "../browser/buffers";
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

export async function pipeSshRoute(command: SshCommand): Promise<void> {
  let key: string;
  try {
    if (!socketIsPrivate()) return unavailable();
    key = socketKey({ create: false });
  } catch { return unavailable(); }

  const socket = net.createConnection(socketPath);
  const abort = () => socket.destroy();
  process.once("SIGTERM", abort);
  process.stdout.once("error", abort);
  socket.on("error", () => {}); // The iterator or pending write reports the error to the exchange.
  const timeout = setTimeout(abort, 5000).unref();
  let admitted = false;
  try {
    const response = packets(socket);
    await packet(socket, Buffer.from(JSON.stringify({ ...command, key })));
    const first = await response.next();
    if (first.done || first.value.length !== 1 || first.value[0] !== accepted) return await unavailable();
    admitted = true;
    clearTimeout(timeout);
    const send = async () => {
      for await (const bytes of process.stdin) await sendPackets(socket, bytes);
      await packet(socket, Buffer.alloc(0));
    };
    void send().catch(error => {
      if (!["EPIPE", "ERR_STREAM_DESTROYED", "ERR_STREAM_WRITE_AFTER_END"].includes((error as NodeJS.ErrnoException).code ?? "")) abort();
    });
    for await (const bytes of response) await writeSshBytes(process.stdout, bytes);
  } catch {
    // From the moment the server accepts, there is no safe fallback or retry.
    if (!admitted) await unavailable();
    else process.exitCode = 1;
  } finally {
    clearTimeout(timeout);
    socket.destroy();
    process.stdin.destroy();
    process.off("SIGTERM", abort);
    process.stdout.off("error", abort);
  }
}
