// Stop takes the messages still on their way with it (PROTOCOL.md, "The queue"), so no client waits
// for its own sends before it stops: what the box is sending a session as Stop comes reaches the
// engine first, and is held with the rest; what a client names as still on its way
// (`POST kuvo/sessions/<id>/stop {items}`) is held as it comes, rather than set going.

/** How long a message Stop named as on its way is held when it comes (ms): longer, and it's a send
 * of its own. */
export const ARRIVING_FOR = 120_000;
/** How long Stop waits, at most, for what the box is sending to reach the engine (ms). */
export const SENDING_WITHIN = 10_000;

// Each session's sends under way here, and the messages to hold as they come, with when they stop being held.
const sending = new Map<string, Set<Promise<void>>>();
const arriving = new Map<string, Map<string, number>>();

/** A send to `session` begun here: under way (for a Stop to wait on) until what this returns is called. */
export function underWay(session: string): () => void {
  const sends = sending.get(session) ?? new Set();
  sending.set(session, sends);
  let finish = () => {};
  const done = new Promise<void>(resolve => { finish = resolve; });
  sends.add(done);
  return () => {
    finish();
    sends.delete(done);
    if (!sends.size && sending.get(session) === sends) sending.delete(session);
  };
}

/** Once what the box is sending `session` now has reached the engine (or failed), or after
 * `within` ms. */
export async function sendsSettled(session: string, within = SENDING_WITHIN): Promise<void> {
  const sends = [...(sending.get(session) ?? [])];
  if (!sends.length) return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([Promise.all(sends), new Promise<void>(resolve => { timer = setTimeout(resolve, within); })]);
  clearTimeout(timer);
}

/** Messages `items` of `session` held as they come, for `ARRIVING_FOR` from `now`. */
export function holdArriving(session: string, items: readonly string[], now = Date.now()): void {
  if (!items.length) return;
  const held = arriving.get(session) ?? new Map<string, number>();
  for (const [id, until] of held) if (until <= now) held.delete(id);
  for (const id of items) held.set(id, now + ARRIVING_FOR);
  arriving.set(session, held);
}

/** Whether message `id` to `session`, coming now, is one Stop named: it's held then, once. */
export function heldAsItComes(session: string, id: string | undefined, now = Date.now()): boolean {
  const held = arriving.get(session);
  const until = id ? held?.get(id) : undefined;
  if (!held || until === undefined) return false;
  held.delete(id!);
  if (!held.size) arriving.delete(session);
  return until > now;
}
