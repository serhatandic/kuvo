// What a session's events say of it from moment to moment, which the session list takes in as they
// come and never reads a session again for (PROTOCOL.md, "What a bot is doing now"): whether its
// model is thinking, and whether it's blocked. Debounced, so a bot's face doesn't flicker: thinking
// comes on after a stretch of reasoning and goes off a moment after it stops; a retry blocks a
// session only once it has gone on a while. The engine's and the coding agents' bridge's events are
// the same family (`session.reasoning.started`, `session.tool.called`, …).

import { errorOf, scopeOf } from "../work/unanswering";
import type { Blocked, Stopped } from "./rule";

/** How long a model reasons before it's thinking, and how long thinking lasts after it stops (ms). */
export const THINKING_AFTER = 2000;
export const THINKING_STOPS_AFTER = 1500;
/** How long a session retries (no step begun since) before it's blocked (ms). */
export const RETRY_BLOCKS_AFTER = 10_000;

/** What ends a stretch of reasoning: its block ends, the model writes words, or calls a tool. */
const REASONING_STOPS = new Set(["session.reasoning.ended", "session.text.started", "session.tool.input.started", "session.tool.called"]);
const RUN_ENDS = new Set(["session.execution.succeeded", "session.execution.failed", "session.execution.interrupted"]);
/** Every event a session's moments go by. */
export const MOMENT_EVENTS = new Set([...REASONING_STOPS, ...RUN_ENDS, "session.reasoning.started", "session.execution.started", "session.retry.scheduled", "session.step.started", "session.deleted"]);

/** What blocked it, from an event's `error`: a limit as the box tells one anywhere (an account's or
 * a model's, `work/unanswering.ts`: the engine's `provider.quota` or `provider.rate-limit`, the
 * coding agents' `limit`, …), else an error (a sign-in that ended among them). */
export const blockedBy = (error: unknown): Blocked => {
  const said = errorOf(error);
  return said && said.type !== "provider.auth" && scopeOf(said) ? "limit" : "error";
};

type Timer = ReturnType<typeof setTimeout>;

/** The moments of the box's sessions (on the engine and the bridge), told by `changed` as a fact
 * changes, a moment after for those debounced. */
export class Moments {
  /** Those thinking (as listed), each reasoning now or not long since. */
  readonly thinking = new Set<string>();
  /** Those that stopped on an error or a limit and haven't run since. */
  readonly stopped = new Map<string, Stopped>();
  /** Those that have been retrying a while. */
  readonly retrying = new Map<string, Blocked>();
  private reasoning = new Map<string, Timer | null>();
  private stopping = new Map<string, Timer>();
  private retries = new Map<string, { cause: Blocked; timer: Timer }>();

  constructor(private readonly changed: () => void) {}

  /** `type` heard for `id`: whether it was one of the events moments go by. */
  hear(type: string, id: string, data: { error?: unknown } = {}): boolean {
    if (!MOMENT_EVENTS.has(type)) return false;
    if (type === "session.reasoning.started") this.reasons(id);
    else if (REASONING_STOPS.has(type)) this.stopsReasoning(id);
    if (type === "session.execution.started" || RUN_ENDS.has(type) || type === "session.deleted") this.endThinking(id);
    if (type === "session.execution.started" || type === "session.deleted") this.unstop(id);
    if (type === "session.execution.failed") this.stop(id, blockedBy(data.error));
    if (type === "session.retry.scheduled") this.retryHeard(id, blockedBy(data.error));
    if (type === "session.step.started" || type === "session.execution.started" || RUN_ENDS.has(type) || type === "session.deleted") this.retried(id);
    return true;
  }

  /** Those kept as stopped from before (the list as it was written): stopped still, till they run,
   * at the turn they stopped. */
  seed(stopped: Iterable<[string, Stopped]>): void {
    for (const [id, stop] of stopped) this.stopped.set(id, stop);
  }

  /** Everything let go, its timers too. */
  clear(): void {
    for (const timer of [...this.reasoning.values(), ...this.stopping.values(), ...[...this.retries.values()].map(each => each.timer)]) if (timer) clearTimeout(timer);
    this.reasoning.clear();
    this.stopping.clear();
    this.retries.clear();
    this.thinking.clear();
    this.stopped.clear();
    this.retrying.clear();
  }

  private later(ms: number, then: () => void): Timer {
    const timer = setTimeout(then, ms);
    timer.unref?.();
    return timer;
  }

  private reasons(id: string): void {
    // (Thinking still, if it was: it hadn't stopped long enough to go off.)
    const off = this.stopping.get(id);
    if (off) { clearTimeout(off); this.stopping.delete(id); }
    if (this.reasoning.has(id)) return;
    if (this.thinking.has(id)) { this.reasoning.set(id, null); return; }
    this.reasoning.set(id, this.later(THINKING_AFTER, () => {
      this.reasoning.set(id, null);
      this.thinking.add(id);
      this.changed();
    }));
  }

  private stopsReasoning(id: string): void {
    if (!this.reasoning.has(id)) return;
    const on = this.reasoning.get(id);
    this.reasoning.delete(id);
    // (Too short a stretch: it never was thinking.)
    if (on) { clearTimeout(on); return; }
    if (!this.thinking.has(id) || this.stopping.has(id)) return;
    this.stopping.set(id, this.later(THINKING_STOPS_AFTER, () => {
      this.stopping.delete(id);
      if (this.thinking.delete(id)) this.changed();
    }));
  }

  private endThinking(id: string): void {
    const on = this.reasoning.get(id);
    if (on) clearTimeout(on);
    this.reasoning.delete(id);
    const off = this.stopping.get(id);
    if (off) clearTimeout(off);
    this.stopping.delete(id);
    if (this.thinking.delete(id)) this.changed();
  }

  private stop(id: string, by: Blocked): void {
    this.stopped.set(id, { by });
    this.changed();
  }

  private unstop(id: string): void {
    if (this.stopped.delete(id)) this.changed();
  }

  private retryHeard(id: string, cause: Blocked): void {
    const under = this.retries.get(id);
    // (Retrying still: from when it began, with what it retries now.)
    if (under) {
      under.cause = cause;
      if (this.retrying.has(id) && this.retrying.get(id) !== cause) { this.retrying.set(id, cause); this.changed(); }
      return;
    }
    const retry = { cause, timer: this.later(RETRY_BLOCKS_AFTER, () => {
      this.retrying.set(id, retry.cause);
      this.changed();
    }) };
    this.retries.set(id, retry);
  }

  private retried(id: string): void {
    const under = this.retries.get(id);
    if (!under) return;
    clearTimeout(under.timer);
    this.retries.delete(id);
    if (this.retrying.delete(id)) this.changed();
  }
}
