// The engine and the coding agents' bridge beside it, as this box reaches them: on 127.0.0.1, at
// what their records say, with their passwords.

import fs from "node:fs";
import { processRuns as runs } from "../../shared/platform";
import type { Fields } from "../../shared/json";
import { bridgeRecordFormat, readRendezvous, type BridgeRecord, type EngineRecord, type Target } from "../../shared/records";
import { BRIDGE_FILE, ENGINE_FILE, REAL_BOX } from "../home";
import { checkRouteReply, ROUTE_VERSION, ROUTE_VERSION_HEADER } from "../../shared/route-version";
import { sleep } from "../../shared/wait";

export const readEngineFile = (): EngineRecord => JSON.parse(fs.readFileSync(ENGINE_FILE, "utf8"));
// The coding agents' bridge beside the engine, when it runs: its port and password.
export const readBridgeFile = (): BridgeRecord => readRendezvous(bridgeRecordFormat, BRIDGE_FILE);

/** The coding agents' bridge beside the engine, while it runs; null otherwise. */
export function bridgeTarget(): Target | null {
  try {
    const bridge = readBridgeFile();
    return bridge?.port && bridge?.password && runs(bridge.pid, "kuvo-agents") ? bridge : null;
  } catch {
    return null;
  }
}

/** Where a letter's agent can be here (`targets`): the engine, and the coding agents' bridge while
 * it runs; and whether both can be asked now (`bridge`), as only then is one neither knows gone. One
 * the engine doesn't know while the bridge isn't running may be the bridge's, and one the bridge
 * doesn't know while the engine's starting may be the engine's: kept to try again. */
export interface PeerPlaces {
  targets: Target[];
  bridge: boolean;
}

export function peerPlaces(): PeerPlaces {
  const targets: Target[] = [];
  let engine = false;
  try {
    targets.push(readEngineFile());
    engine = true;
  } catch {}
  const bridge = bridgeTarget();
  if (bridge) targets.push(bridge);
  return { targets, bridge: engine && bridge !== null };
}

// Where a letter's agent can be here: the engine, and the coding agents' bridge while it runs.
export const peerTargets = (): Target[] => peerPlaces().targets;

/** An answer of the engine's (or the coding agents' bridge's) as their APIs always give one, with
 * what was asked for under `data`, before that's checked: whatever came there (unknown), or null
 * for an empty answer or none. */
export type UncheckedAnswer = Fields<"data"> | null;

/** An answer of the engine's (or the coding agents' bridge's) as their APIs always give one: what
 * was asked for under `data`, in `T`, the shape they document for it. (As they answer it: a type
 * that trusts them, for what's read without a check.) */
export interface EngineAnswer<T> {
  data: T;
}

// Asks the engine for `route` (a path under its API), as JSON.
export function engineGet(engine: Target, route: string, folder?: string): Promise<unknown> {
  return fetch(`http://127.0.0.1:${engine.port}/${route}`, { headers: { [ROUTE_VERSION_HEADER]: ROUTE_VERSION, authorization: `Basic ${Buffer.from(`opencode:${engine.password}`).toString("base64")}`, ...(folder ? { "x-opencode-directory": encodeURIComponent(folder) } : {}) }, signal: AbortSignal.timeout(10_000) })
    .then(async response => {
      await checkRouteReply(response);
      if (!response.ok) throw new Error(`${response.status}`);
      return response.json();
    });
}

/**
 * Once `engine` has set up its plugins: an engine that opened a folder a moment ago, or is reading
 * it again, lists its integrations, agents and skills at once with what it has so far (2.2.7 does,
 * without what its plugins give, and then says `integration.updated` and `agent.updated`), while
 * its providers wait for them; so they're asked for first. It throws when the engine doesn't answer.
 */
export async function pluginsSetUp(engine: Target, folder?: string, signal?: AbortSignal): Promise<void> {
  await call(engine, "GET", "/api/provider", undefined, signal ?? AbortSignal.timeout(10_000), folder);
}

/** `engine`'s list at `route` (`api/integration`, a path under its API) once its plugins are set
 * up; null when it doesn't answer, or lists nothing yet: not read, never "none". */
export async function engineList(engine: Target, route: string, folder?: string): Promise<unknown[] | null> {
  try {
    await pluginsSetUp(engine, folder);
    const listed = (await engineGet(engine, route, folder) as UncheckedAnswer)?.data;
    return Array.isArray(listed) && listed.length ? listed : null;
  } catch {
    return null;
  }
}

/** `target`'s answer to `method route` (with `body` as JSON), as JSON, or null for an empty one; it throws for a status that isn't ok. `patience`: how long it may take. */
export async function asked(target: Target, method: string, route: string, body?: unknown, patience = 30_000): Promise<unknown> {
  const response = await fetch(`http://127.0.0.1:${target.port}/${route}`, {
    method,
    headers: { [ROUTE_VERSION_HEADER]: ROUTE_VERSION, authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`, ...(body ? { "content-type": "application/json" } : {}) },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(patience),
  });
  await checkRouteReply(response);
  if (!response.ok) throw new Error(`It answered ${response.status}.`);
  const text = await response.text();
  return text ? JSON.parse(text) : null;
}

/**
 * `target`'s answer to `method route` (a path from its root, with `body` as JSON), as its API
 * answers: what's under `data`, or the whole answer; it throws with the answer's message for a
 * status that isn't ok. `folder`: the folder it's about, as the engine reads it.
 */
export async function call(target: Target, method: string, route: string, body?: unknown, signal?: AbortSignal, folder?: string, headers?: Record<string, string>): Promise<unknown> {
  const value = await callReply(target, method, route, body, signal, folder, headers);
  return value?.data ?? value;
}

/** The full JSON reply for routes that preserve the engine or bridge envelope. */
export async function callReply(target: Target, method: string, route: string, body?: unknown, signal?: AbortSignal, folder?: string, headers?: Record<string, string>): Promise<Fields<"message" | "data" | "carry"> | null> {
  const response = await fetch(`http://127.0.0.1:${target.port}${route}`, {
    method,
    headers: {
      ...headers,
      [ROUTE_VERSION_HEADER]: ROUTE_VERSION,
      authorization: `Basic ${Buffer.from(`opencode:${target.password}`).toString("base64")}`,
      ...(body ? { "content-type": "application/json" } : {}),
      // The folder it's about, as the engine reads it (a query naming one is ignored).
      ...(folder ? { "x-opencode-directory": encodeURIComponent(folder) } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
    signal,
  });
  // Whatever it answered, as JSON ({} for none), read for its `message` (why it failed, in words,
  // as their APIs say it) and `data` where it has them.
  await checkRouteReply(response);
  const value = await response.json().catch(() => ({})) as Fields<"message" | "data" | "carry"> | null;
  if (!response.ok) throw Object.assign(new Error((typeof value?.message === "string" && value.message) || `It answered ${response.status}.`), {
    status: response.status, ...(value?.carry === true ? { carry: true } : {}),
  });
  return value;
}

// `url`'s body, at most `most` bytes, within `timeout` ms.
export async function fetched(url: string, most: number, timeout: number): Promise<Buffer> {
  const response = await fetch(url, { signal: AbortSignal.timeout(timeout), redirect: "follow" });
  if (!response.ok) throw Object.assign(new Error(`It answered ${response.status}.`), { status: response.status });
  const data = Buffer.from(await response.arrayBuffer());
  if (data.length > most) throw new Error("It's larger than a release can be.");
  return data;
}

/** How long a download that broke off waits before it's tried again, each time (a test's folder
 * may say shorter: `KUVO_TEST_FETCH_WAIT`, in ms). */
const FETCH_WAITS = [5_000, 20_000];
const fetchWaits = (): number[] => {
  const test = Number(process.env.KUVO_TEST_FETCH_WAIT);
  return !REAL_BOX && test >= 0 && process.env.KUVO_TEST_FETCH_WAIT ? FETCH_WAITS.map(() => test) : FETCH_WAITS;
};

/** `fetched`, tried again a moment later where it broke off on the way (the connection dropped
 * or timed out, the server failed or asked to wait): a release's files and engine are large, and a
 * Wi-Fi hiccup halfway shouldn't fail an update that a tap on Try Again then does. What the server
 * answered for good (it isn't there, it's refused, it's too large) isn't asked again. */
export async function fetchedPatiently(url: string, most: number, timeout: number, waits: number[] = fetchWaits()): Promise<Buffer> {
  for (let tried = 0; ; tried++) {
    try {
      return await fetched(url, most, timeout);
    } catch (error) {
      const status = (error as { status?: unknown } | null)?.status;
      const lasting = typeof status === "number" ? status < 500 && status !== 408 && status !== 429 : /larger than a release/.test(String((error as Error | undefined)?.message));
      if (lasting || tried >= waits.length) throw error;
      await sleep(waits[tried]!);
    }
  }
}
