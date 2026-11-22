// A terminal on a box's own server (`serve`, what this Mac's app talks to): `api/pty/<id>/connect`,
// opened as a WebSocket with serve's password, goes to the engine's own socket as it does through
// the relay, and each side's messages and close pass to the other as they came (PROTOCOL.md,
// "Sockets"; feature `pty.socket`). The real server; the engine is a stand-in, a Bun WebSocket
// server as the engine's own is.
import "./safety"
import { afterEach, expect, test } from "bun:test"
import net from "node:net"
import type { AddressInfo } from "node:net"
import type { ServerWebSocket } from "bun"
import { serveBox } from "../src/link/server/serve"

const basic = (password: string) => "Basic " + Buffer.from(`opencode:${password}`).toString("base64")
const NOTE = Buffer.concat([Buffer.from([0]), Buffer.from('{"cursor":0}')])

/** What arrives, in order, each taken once it has come. */
class Arrivals<T> {
  private items: T[] = []
  private waiting: ((item: T) => void)[] = []
  add(item: T) {
    const wait = this.waiting.shift()
    if (wait) wait(item)
    else this.items.push(item)
  }
  next(): Promise<T> {
    return this.items.length ? Promise.resolve(this.items.shift()!) : new Promise(resolve => this.waiting.push(resolve))
  }
}

let stops: (() => unknown)[] = []
afterEach(async () => {
  for (const stop of stops.reverse()) await stop()
  stops = []
})

// The engine's terminal `pty_1`, with the engine's password: first its note of where its output
// starts, as the engine sends it, then whatever the test has it send.
function standInEngine() {
  const asked: { path: string; authorization: string | null; directory: string | null }[] = []
  const heard = new Arrivals<string | Buffer>()
  const closed = new Arrivals<{ code: number; reason: string }>()
  const sockets = new Arrivals<ServerWebSocket<unknown>>()
  const engine = Bun.serve({
    port: 0,
    hostname: "127.0.0.1",
    fetch(request, server) {
      const url = new URL(request.url)
      asked.push({ path: url.pathname + url.search, authorization: request.headers.get("authorization"), directory: request.headers.get("x-opencode-directory") })
      if (request.headers.get("authorization") !== basic("engine-secret")) return new Response(null, { status: 401 })
      if (url.pathname !== "/api/pty/pty_1/connect") return new Response("{}", { status: 404 })
      return server.upgrade(request) ? undefined : new Response(null, { status: 400 })
    },
    websocket: {
      open(socket) {
        socket.send(NOTE)
        sockets.add(socket)
      },
      message(_socket, message) { heard.add(message) },
      close(_socket, code, reason) { closed.add({ code, reason }) },
    },
  })
  stops.push(() => engine.stop(true))
  return { asked, heard, closed, sockets, port: engine.port! }
}

// The box's own server, its password `serve-secret`, the engine found at `port`. (No route is asked
// over HTTP here.)
async function serving(port: number): Promise<number> {
  const server = serveBox("serve-secret", async function* () {}, () => ({ port, password: "engine-secret" }))
  await new Promise<void>(resolve => server.listen(0, "127.0.0.1", resolve))
  stops.push(() => new Promise<void>(resolve => {
    server.closeAllConnections()
    server.close(() => resolve())
  }))
  return (server.address() as AddressInfo).port
}

// A client's terminal socket, as a client opens one: Bun's WebSocket, with serve's password.
function terminalAt(port: number, path: string, headers: Record<string, string> = {}) {
  const socket = new WebSocket(`ws://127.0.0.1:${port}/${path}`, { headers: { authorization: basic("serve-secret"), ...headers } } as any)
  socket.binaryType = "arraybuffer"
  const got = new Arrivals<string | Buffer>()
  socket.onmessage = event => got.add(typeof event.data === "string" ? event.data : Buffer.from(event.data as ArrayBuffer))
  const opened = new Promise<void>((resolve, reject) => {
    socket.onopen = () => resolve()
    socket.onerror = () => reject(new Error("The terminal's socket didn't open."))
  })
  const ended = new Promise<{ code: number; reason: string }>(resolve => { socket.onclose = event => resolve({ code: event.code, reason: event.reason }) })
  stops.push(() => socket.close())
  return { socket, got, opened, ended }
}

// An upgrade asked by hand, for the status it's refused with (the line the server answers with).
async function refusal(port: number, path: string, headers: string[]): Promise<string> {
  const socket = net.createConnection(port, "127.0.0.1")
  stops.push(() => socket.destroy())
  socket.write(`GET /${path} HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n${headers.map(header => `${header}\r\n`).join("")}\r\n`)
  let said = ""
  for await (const chunk of socket) said += chunk
  return said.split("\r\n")[0]!
}

test("a terminal on the box's own server reaches the engine's with its password and passes keys and output as they came", async () => {
  const engine = standInEngine()
  const port = await serving(engine.port)
  const terminal = terminalAt(port, "api/pty/pty_1/connect?cursor=0", { "x-opencode-directory": "/home/me/project" })
  await terminal.opened
  // To the engine with its own password, never serve's, in the folder the client named.
  expect(engine.asked).toEqual([{ path: "/api/pty/pty_1/connect?cursor=0", authorization: basic("engine-secret"), directory: "/home/me/project" }])
  // Its note, binary as it sent it: a zero byte, then where its output starts.
  expect(await terminal.got.next()).toEqual(NOTE)

  // What's typed: text as text, whatever it holds, and binary as binary.
  terminal.socket.send("ls -la\r")
  terminal.socket.send("é ✓ \u001b[A")
  terminal.socket.send(new Uint8Array([0, 1, 254, 255]))
  expect(await engine.heard.next()).toBe("ls -la\r")
  expect(await engine.heard.next()).toBe("é ✓ \u001b[A")
  expect(await engine.heard.next()).toEqual(Buffer.from([0, 1, 254, 255]))

  // What the shell printed, escape sequences and all, a long burst whole, and the engine's next note.
  const shell = await engine.sockets.next()
  const burst = "a line of output\r\n".repeat(5_000)
  const note = Buffer.concat([Buffer.from([0]), Buffer.from('{"cursor":90040}')])
  shell.send("\u001b[31mred\u001b[0m \u{1F600}\r\n")
  shell.send(burst)
  shell.send(note)
  expect(await terminal.got.next()).toBe("\u001b[31mred\u001b[0m \u{1F600}\r\n")
  expect(await terminal.got.next()).toBe(burst)
  expect(await terminal.got.next()).toEqual(note)

  // The shell exits: the engine closes its socket, and the client hears its code and reason.
  shell.close(4000, "exited")
  expect(await terminal.ended).toEqual({ code: 4000, reason: "exited" })
})

test("a client closing its terminal closes the engine's socket with its code", async () => {
  const engine = standInEngine()
  const terminal = terminalAt(await serving(engine.port), "api/pty/pty_1/connect")
  await terminal.opened
  expect(await terminal.got.next()).toEqual(NOTE)
  terminal.socket.close(4001, "panel closed")
  expect(await engine.closed.next()).toEqual({ code: 4001, reason: "panel closed" })
  expect(await terminal.ended).toEqual({ code: 4001, reason: "panel closed" })
})

test("a terminal's socket is refused without serve's password, never reaching the engine; one the engine refuses fails", async () => {
  const engine = standInEngine()
  const port = await serving(engine.port)
  const upgrade = "api/pty/pty_1/connect"
  expect(await refusal(port, upgrade, [`Authorization: ${basic("wrong")}`])).toBe("HTTP/1.1 401 Refused")
  expect(await refusal(port, upgrade, [])).toBe("HTTP/1.1 401 Refused")
  // (The engine's own password, given in place of serve's, is refused as any wrong one.)
  expect(await refusal(port, upgrade, [`Authorization: ${basic("engine-secret")}`])).toBe("HTTP/1.1 401 Refused")
  // Only a terminal's socket, and only from a client that knows this box's routes.
  expect(await refusal(port, "api/pty/pty_1", [`Authorization: ${basic("serve-secret")}`])).toBe("HTTP/1.1 403 Refused")
  expect(await refusal(port, "api/event", [`Authorization: ${basic("serve-secret")}`])).toBe("HTTP/1.1 403 Refused")
  expect(await refusal(port, upgrade, [`Authorization: ${basic("serve-secret")}`, "X-Kuvo-Route-Version: 2"])).toBe("HTTP/1.1 400 Refused")
  expect(engine.asked).toEqual([])
  // A terminal the engine doesn't have: its socket doesn't open, and the client is told.
  expect(await refusal(port, "api/pty/pty_gone/connect", [`Authorization: ${basic("serve-secret")}`])).toBe("HTTP/1.1 502 Refused")
  expect(engine.asked.map(asked => asked.path)).toEqual(["/api/pty/pty_gone/connect"])
})
