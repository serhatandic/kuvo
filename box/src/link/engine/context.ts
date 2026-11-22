// The native engine's session response has lifetime usage. Use its latest measured step instead.
import { stepContext, tokenCount, type ContextSize } from "../../shared/context";

import type { EngineSession } from "./sessions";

export async function currentEngineContext(url: URL, headers: Record<string, string>, signal: AbortSignal | undefined, session: EngineSession | undefined): Promise<ContextSize | undefined> {
  if (!session?.id || session.revert) return;
  const deadline = AbortSignal.timeout(1500);
  const read = async (route: string) => {
    const response = await fetch(new URL(route, url), { headers, signal: signal ? AbortSignal.any([signal, deadline]) : deadline });
    if (response.ok) return ((await response.json()) as { data?: unknown } | null)?.data;
    await response.body?.cancel();
    return undefined;
  };
  try {
    const messages = await read(`${url.pathname}/message?limit=8&order=desc`);
    if (!Array.isArray(messages)) return;
    for (const message of messages) {
      if (message?.type === "compaction" && message.status !== "failed") break;
      if (message?.type !== "assistant" || message.error) continue;
      const used = stepContext(message.tokens);
      if (used === undefined) continue;
      const ref = message.model ?? session.model;
      if (!ref) return { used };
      const models = await read("/api/model").catch(() => undefined);
      const window = Array.isArray(models) ? models.find(model => model.providerID === ref.providerID && (model.id === ref.id || model.aliases?.includes(ref.id)))?.limit?.context : undefined;
      return { used, ...(tokenCount(window) && window > 0 ? { window } : {}) };
    }
  } catch {}
  return undefined;
}
