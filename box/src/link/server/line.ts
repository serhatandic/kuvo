// The one line (docs/architecture/message-delivery.md, rule 2; PROTOCOL.md, "The line"): a
// client opens one stream to this box, names what it follows (its topics, each a route the box
// already follows), and the box pushes every change on it, numbered, with a heartbeat. A dropped
// stream is resumed from its last number: what it missed is sent again, or, where that's gone, each
// topic starts over from what it is now (`fresh`). The line lives in the process that answered it;
// its topics keep running a while after its stream drops, so a reconnect misses nothing.
// Platform: the same on macOS and Linux.

import { randomBytes } from "node:crypto";
import { isObject, parsed } from "../../shared/json";
import type { AnswerPiece, AnyPiece, Asker, LinkRequest, RouteContext } from "./context";

export const LINE_ROUTE = "kuvo/line";
export const LINE_WATCH_ROUTE = "kuvo/line/watch";

/** How often a quiet line says it's there (`beat`), how long a line's topics keep running with no
 * stream attached (`kept`), and how much of what was sent it keeps to send again (`bytes`). A test
 * sets its own. An event stream that says nothing at all (not even its own heartbeat) for
 * `eventsQuiet` is opened again, saying a gap: one that stalled never freezes a conversation. */
export const lineTiming = { beat: 5_000, kept: 120_000, bytes: 4 * 1024 * 1024, eventsQuiet: 60_000 };
/** How long `once` waits for every topic's first line. */
const ONCE_MS = 10_000;
/** Before a topic that ended (its source dropped, the engine wasn't ready) is opened again. */
const AGAIN_MS = [250, 1_000, 2_000, 5_000];

/** What runs a topic's route: the box's own dispatcher, as the line's asker. */
export type Dispatch = (request: LinkRequest, signal: AbortSignal) => AsyncIterable<AnyPiece> | Iterable<AnyPiece>;

interface Sent { n: number; text: string }

interface Topic {
  route: string;
  stop: AbortController;
}

interface Line {
  id: string;
  owner: string;
  dispatch: Dispatch;
  base: LinkRequest;
  n: number;
  sent: Sent[];
  bytes: number;
  topics: Map<string, Topic>;
  /** The stream attached now, told each line as it's sent. */
  listener: ((text: string) => void) | null;
  expiry: ReturnType<typeof setTimeout> | undefined;
}

const lines = new Map<string, Line>();

/** For tests: every line ended. */
export function endLines(): void {
  for (const line of [...lines.values()]) end(line);
}

/** For tests: how many lines this process keeps. */
export function lineCount(): number {
  return lines.size;
}

function end(line: Line) {
  clearTimeout(line.expiry);
  for (const topic of line.topics.values()) topic.stop.abort();
  line.topics.clear();
  line.listener = null;
  if (lines.get(line.id) === line) lines.delete(line.id);
}

/** Who a line belongs to: only they may resume it or change its topics. */
function ownerOf(request: LinkRequest, phone: Asker | undefined): string {
  if (phone?.key) return `key:${phone.key.toString("hex")}:${phone.pairing ?? ""}`;
  return ["fromApp", "fromMac", "served", "internal", "addressedSsh", "residentSsh", "desktop"].filter(flag => (request as unknown as Record<string, unknown>)[flag]).join(",") || "local";
}

/** The topics a client names: `{name: route}`, given as base64url JSON in a query, or as JSON. */
export function readWatch(given: unknown): Map<string, string> | null {
  let value = given;
  if (typeof given === "string") {
    try {
      value = JSON.parse(Buffer.from(given, "base64url").toString("utf8"));
    } catch {
      return null;
    }
  }
  if (!isObject(value)) return null;
  const topics = new Map<string, string>();
  for (const [name, route] of Object.entries(value)) {
    // A topic is one of the box's own follows: a GET it already answers, never the line itself.
    if (typeof route !== "string" || !route || route.startsWith("/") || route.split("?")[0].startsWith(LINE_ROUTE) || name.length > 200) return null;
    topics.set(name, route);
  }
  return topics;
}

function emit(line: Line, value: Record<string, unknown>) {
  const n = ++line.n;
  const text = JSON.stringify({ ...value, n });
  line.sent.push({ n, text });
  line.bytes += text.length;
  while (line.bytes > lineTiming.bytes && line.sent.length > 1) line.bytes -= line.sent.shift()!.text.length;
  line.listener?.(text);
}

/** Whether `route` streams events (server-sent: a conversation's), not states (ndjson lines). */
function isEvents(route: string, contentType: string | undefined): boolean {
  if (contentType?.includes("event-stream")) return true;
  const path = route.split("?")[0];
  return path === "api/event" || path === "kuvo/agents/api/event";
}

const wait = (ms: number, signal: AbortSignal) => new Promise<void>(done => {
  if (signal.aborted) return done();
  const timer = setTimeout(done, ms);
  signal.addEventListener("abort", () => { clearTimeout(timer); done(); }, { once: true });
});

/** Runs one topic until it's dropped: each line its route says is sent on the line; one that ends
 * is opened again (a conversation's events say there may be a gap), one the box turns down is said,
 * and ends unless it's only not ready yet (5xx). */
async function runTopic(line: Line, name: string, topic: Topic) {
  const { signal } = topic.stop;
  let attempts = 0;
  let refusedLast = "";
  let last: string | undefined;
  let tries = 0;
  while (!signal.aborted) {
    const attempt = attempts++;
    let status = 0;
    let events = false;
    let refusal = "";
    let rest = "";
    let data: string[] = [];
    const take = (text: string) => {
      if (signal.aborted) return;
      if (events) {
        // Server-sent: an event is its `data:` lines, ended by a blank one.
        if (text === "") {
          const body = data.join("\n");
          data = [];
          if (!body) return;
          const value = parsed(Buffer.from(body));
          if (value !== undefined) emit(line, { t: name, d: value });
        } else if (text.startsWith("data:")) {
          data.push(text.slice(text[5] === " " ? 6 : 5));
        }
        return;
      }
      if (!text.trim()) return;
      // A state that didn't change since the last one sent isn't sent again (after a reopen).
      if (text === last) return;
      const value = parsed(Buffer.from(text));
      if (value === undefined) return;
      last = text;
      emit(line, { t: name, d: value });
    };
    // This opening's own end: the topic's, or its events gone quiet.
    const opening = new AbortController();
    const ended = () => opening.abort();
    signal.addEventListener("abort", ended, { once: true });
    let quiet: ReturnType<typeof setTimeout> | undefined;
    const heard = () => {
      clearTimeout(quiet);
      if (events) quiet = setTimeout(() => opening.abort(), lineTiming.eventsQuiet);
    };
    try {
      const request: LinkRequest = { ...line.base, method: "GET", path: topic.route, body: undefined };
      for await (const piece of line.dispatch(request, opening.signal) as AsyncIterable<AnyPiece>) {
        if (signal.aborted || opening.signal.aborted) break;
        if (piece.head) {
          status = piece.head.status;
          const headers = piece.head.headers ?? {};
          const type = Object.entries(headers).find(([key]) => key.toLowerCase() === "content-type")?.[1];
          events = isEvents(topic.route, typeof type === "string" ? type : undefined);
          heard();
          if (status >= 200 && status < 300) {
            tries = 0;
            refusedLast = "";
            // Events from before this opening may be missing (an earlier one dropped, or wasn't
            // ready): the client reads its conversation again.
            if (events && attempt > 0) emit(line, { t: name, gap: true });
          }
        }
        if (piece.body) {
          if (status && (status < 200 || status >= 300)) {
            refusal += piece.body.toString("utf8");
            continue;
          }
          heard();
          rest += piece.body.toString("utf8");
          let at: number;
          while ((at = rest.indexOf("\n")) >= 0) {
            take(rest.slice(0, at).replace(/\r$/, ""));
            rest = rest.slice(at + 1);
          }
        }
      }
      if (rest) take(rest.replace(/\r$/, ""));
      if (events) take("");
    } catch {
      // Cut off (the engine went halfway, or it went quiet): opened again, as one that ended.
    } finally {
      clearTimeout(quiet);
      signal.removeEventListener("abort", ended);
    }
    if (signal.aborted) return;
    if (status && (status < 200 || status >= 300)) {
      const said = parsed(Buffer.from(refusal));
      const message = isObject(said) && typeof said.message === "string" ? said.message : refusal.slice(0, 500);
      const refused = { status, message, ...(isObject(said) ? { said } : {}) };
      // (Said once while it goes on being so, not at each try.)
      if (JSON.stringify(refused) !== refusedLast) emit(line, { t: name, refused });
      refusedLast = JSON.stringify(refused);
      // Turned down: it ends. Not ready yet: tried again.
      if (status < 500) {
        line.topics.delete(name);
        return;
      }
    }
    await wait(AGAIN_MS[Math.min(tries++, AGAIN_MS.length - 1)], signal);
  }
}

/** Changes a line's topics to `watch`: new ones start, ones gone stop, the rest go on. */
function setTopics(line: Line, watch: Map<string, string>) {
  for (const [name, topic] of line.topics) {
    if (watch.get(name) !== topic.route) {
      topic.stop.abort();
      line.topics.delete(name);
    }
  }
  for (const [name, route] of watch) {
    if (line.topics.has(name)) continue;
    const topic = { route, stop: new AbortController() };
    line.topics.set(name, topic);
    void runTopic(line, name, topic);
  }
}

function query(path: string): URLSearchParams {
  const at = path.indexOf("?");
  return new URLSearchParams(at < 0 ? "" : path.slice(at + 1));
}

/** `GET kuvo/line` and `POST kuvo/line/watch`. */
export async function* lineRoute(context: RouteContext, dispatch: Dispatch): AsyncGenerator<AnswerPiece> {
  const { request, phone, json, signal } = context;
  const method = request.method.toUpperCase();
  const owner = ownerOf(request, phone);
  if (context.route === LINE_WATCH_ROUTE) {
    if (method !== "POST") return yield* json(405, {});
    const body = parsed(request.body);
    const id = isObject(body) && typeof body.line === "string" ? body.line : "";
    const watch = isObject(body) ? readWatch(body.watch) : null;
    if (!watch) return yield* json(400, { message: "Name what this line follows." });
    const line = lines.get(id);
    // (A line this box doesn't know: the client opens it again, with its topics.)
    if (!line || line.owner !== owner) return yield* json(404, { fresh: true, message: "Open the line again." });
    setTopics(line, watch);
    return yield* json(200, { line: line.id });
  }
  if (method !== "GET") return yield* json(405, {});
  const asked = query(request.path);
  const watch = readWatch(asked.get("watch") ?? "e30");
  if (!watch) return yield* json(400, { message: "Name what this line follows." });
  const after = Number(asked.get("after") ?? "0");
  const once = asked.get("once") === "1";
  const base: LinkRequest = { ...request, path: "", body: undefined };

  let line = once ? undefined : lines.get(asked.get("line") ?? "");
  // Resumed only by whoever opened it, from a number it sent, whose next is still kept.
  const resumable = line !== undefined && line.owner === owner && Number.isInteger(after) && after >= 0 && after <= line.n &&
    (after === line.n || (line.sent.length > 0 && line.sent[0].n <= after + 1));
  const fresh = !resumable;
  if (fresh) {
    line = { id: randomBytes(12).toString("base64url"), owner, dispatch, base, n: 0, sent: [], bytes: 0, topics: new Map(), listener: null, expiry: undefined };
    lines.set(line.id, line);
  }
  const it = line!;
  clearTimeout(it.expiry);
  it.dispatch = dispatch;
  it.base = base;

  const queue: string[] = [];
  let wake: (() => void) | null = null;
  const listener = (text: string) => { queue.push(text); wake?.(); };
  // What it missed, and what comes next, read in the same turn: nothing between them is lost.
  const from = fresh ? 0 : after;
  for (const sent of it.sent) if (sent.n > from) queue.push(sent.text);
  it.listener = listener;
  setTopics(it, watch);

  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson", "cache-control": "no-store" } } };
  yield { body: Buffer.from(JSON.stringify({ hello: { line: it.id, fresh, beat: lineTiming.beat } }) + "\n") };

  if (once) {
    // Each topic's state now, then it ends (a box that can't keep a stream: asked again later).
    const until = Date.now() + ONCE_MS;
    const pending = new Set(watch.keys());
    try {
      while (pending.size && Date.now() < until && !signal?.aborted) {
        while (queue.length) {
          const text = queue.shift()!;
          const said = JSON.parse(text) as { t?: string };
          if (said.t) pending.delete(said.t);
          yield { body: Buffer.from(text + "\n") };
        }
        if (!pending.size) break;
        await new Promise<void>(done => {
          const timer = setTimeout(done, 50);
          wake = () => { clearTimeout(timer); done(); };
        });
        wake = null;
      }
    } finally {
      end(it);
    }
    return;
  }

  const woken = () => wake?.();
  signal?.addEventListener("abort", woken, { once: true });
  let beat = Date.now();
  try {
    while (!signal?.aborted && it.listener === listener) {
      if (queue.length) {
        const texts = queue.splice(0);
        beat = Date.now();
        yield { body: Buffer.from(texts.join("\n") + "\n") };
        continue;
      }
      if (Date.now() - beat >= lineTiming.beat) {
        beat = Date.now();
        yield { body: Buffer.from(JSON.stringify({ beat: it.n }) + "\n") };
        continue;
      }
      await new Promise<void>(done => {
        const timer = setTimeout(done, Math.max(1, lineTiming.beat - (Date.now() - beat)));
        wake = () => { clearTimeout(timer); done(); };
      });
      wake = null;
    }
  } finally {
    signal?.removeEventListener("abort", woken);
    // Its stream gone (not taken over by a newer one): its topics go on a while, for a resume.
    if (it.listener === listener) {
      it.listener = null;
      it.expiry = setTimeout(() => end(it), lineTiming.kept);
      it.expiry.unref?.();
    }
  }
}
