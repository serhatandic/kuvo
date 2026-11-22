// The engine's session response has lifetime usage. Use its latest measured step instead.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { isObject } from "../../shared/json";
import { contextCapacity, stepContext, type ContextSize } from "../../shared/context";
import { locked, writePrivate } from "../files";
import { K } from "../home";
import type { EngineSession } from "./sessions";

const file = join(K, "cache", "engine-context-windows.json");
const cached = (): Record<string, unknown> => {
  try {
    const value: unknown = JSON.parse(readFileSync(file, "utf8"));
    if (isObject(value)) return value;
  } catch {}
  return {};
};

function rememberWindow(ref: NonNullable<EngineSession["model"]>, live: unknown): number | undefined {
  const key = JSON.stringify([ref.providerID, ref.id]);
  const previous = cached()[key];
  if (!contextCapacity(live)) return contextCapacity(previous) ? previous : undefined;
  if (previous !== live) {
    try {
      locked(file + ".lock", () => {
        const windows = cached();
        windows[key] = live;
        writePrivate(file, JSON.stringify(windows));
      }, { required: true, wait: 20 });
    } catch {}
  }
  return live;
}

export async function currentEngineContext(url: URL, headers: Record<string, string>, signal: AbortSignal | undefined, session: EngineSession | undefined): Promise<{ context?: ContextSize; contextWindow?: number }> {
  if (!session?.id) return {};
  const deadline = AbortSignal.timeout(1500);
  const read = async (route: string) => {
    const response = await fetch(new URL(route, url), { headers, signal: signal ? AbortSignal.any([signal, deadline]) : deadline });
    if (response.ok) return ((await response.json()) as { data?: unknown } | null)?.data;
    await response.body?.cancel();
    return undefined;
  };
  const [messages, models] = await Promise.all([
    session.revert ? undefined : read(`${url.pathname}/message?limit=8&order=desc`).catch(() => undefined),
    read("/api/model").catch(() => undefined),
  ]);
  let ref = session.model;
  let used: number | undefined;
  if (Array.isArray(messages)) for (const message of messages) {
    if (message?.type === "compaction" && message.status !== "failed") break;
    if (message?.type !== "assistant" || message.error) continue;
    used = stepContext(message.tokens);
    if (used === undefined) continue;
    ref = message.model ?? ref;
    break;
  }
  const live = ref && Array.isArray(models) ? models.find(model => model?.providerID === ref.providerID && (model.id === ref.id || (Array.isArray(model.aliases) && model.aliases.includes(ref.id))))?.limit?.context : undefined;
  const window = ref ? rememberWindow(ref, live) : undefined;
  return {
    ...(window === undefined ? {} : { contextWindow: window }),
    ...(used === undefined ? {} : { context: { used, window } }),
  };
}
