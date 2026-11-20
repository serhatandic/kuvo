import { pause } from "./wait";

// The events in a `text/event-stream` body, parsed.
export async function* serverEvents<Event>(body: AsyncIterable<Uint8Array>): AsyncGenerator<Event> {
  const decoder = new TextDecoder();
  let buffer = "";
  let data: string[] = [];
  for await (const chunk of body) {
    buffer += decoder.decode(chunk, { stream: true });
    let end: number;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end).replace(/\r$/, "");
      buffer = buffer.slice(end + 1);
      if (line === "") {
        if (data.length) {
          try { yield JSON.parse(data.join("\n")); } catch {}
        }
        data = [];
      } else if (line.startsWith("data:")) {
        data.push(line.slice(5).replace(/^ /, ""));
      }
    }
  }
}


/** Follow a local service across restarts, bounding a connection that stops sending heartbeats. */
export async function followEvents<Event>(
  connect: (signal: AbortSignal) => Promise<Response>,
  heardEvent: (event: Event) => void,
  signal?: AbortSignal,
  connected?: (signal: AbortSignal) => void,
): Promise<void> {
  while (!signal?.aborted) {
    const connection = new AbortController();
    const listening = signal ? AbortSignal.any([signal, connection.signal]) : connection.signal;
    let late: ReturnType<typeof setTimeout> | undefined;
    const heard = () => {
      clearTimeout(late);
      late = setTimeout(() => connection.abort(), 45_000);
      late.unref?.();
    };
    try {
      heard();
      const response = await connect(listening);
      if (!response.ok || !response.body) await response.body?.cancel();
      else {
        connected?.(listening);
        for await (const event of serverEvents<Event>(response.body)) {
          heard();
          heardEvent(event);
        }
      }
    } catch {
      // The source can be starting or changing versions.
    } finally {
      connection.abort();
      clearTimeout(late);
    }
    await pause(1000, signal);
  }
}
