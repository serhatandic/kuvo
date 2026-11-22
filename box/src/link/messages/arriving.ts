// Stop takes the messages still on their way with it (PROTOCOL.md, "The queue"), so no client waits
// for its own sends before it stops: what the box is sending a session as Stop comes reaches the
// engine first, and is held with the rest; what a client names as still on its way
// (`POST kuvo/sessions/<id>/stop {items}`) is held as it comes, rather than set going.

import { changeQueueLater, queueOf } from "../engine/queue";

/** How long a message Stop named as on its way is held when it comes (ms): longer, and it's a send
 * of its own. */
export const ARRIVING_FOR = 120_000;
/** How long Stop waits, at most, for what the box is sending to reach the engine (ms; a test sets
 * its own, for a send slower than that). */
export const stopWaits = { within: 10_000 };

// Each session's sends under way here. (What Stop named is kept with its queue, `run/queues.json`,
// so a box that starts again meanwhile still holds it.)
const sending = new Map<string, Set<Promise<void>>>();

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
export async function sendsSettled(session: string, within = stopWaits.within): Promise<void> {
  const sends = [...(sending.get(session) ?? [])];
  if (!sends.length) return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  await Promise.race([Promise.all(sends), new Promise<void>(resolve => { timer = setTimeout(resolve, within); })]);
  clearTimeout(timer);
}

/** Messages `items` of `session` held as they come, for `ARRIVING_FOR` from `now`. */
export async function holdArriving(session: string, items: readonly string[], now = Date.now()): Promise<void> {
  if (!items.length) return;
  await changeQueueLater(session, state => ({ ...state, arriving: { ...state.arriving, ...Object.fromEntries(items.map(id => [id, now + ARRIVING_FOR])) } }));
}

/** What Stop named for `session` forgotten: it went on since (Resume, Send Now), so what comes
 * now goes as it was sent. */
export async function forgetArriving(session: string): Promise<void> {
  if (queueOf(session).arriving) await changeQueueLater(session, ({ arriving: _, ...state }) => state);
}

/** Whether message `id` to `session`, coming now, is one Stop named: it's held then, once. */
export async function heldAsItComes(session: string, id: string | undefined, now = Date.now()): Promise<boolean> {
  if (!id || queueOf(session).arriving?.[id] === undefined) return false;
  let until = 0;
  await changeQueueLater(session, state => {
    const { [id]: named, ...rest } = state.arriving ?? {};
    until = named ?? 0;
    const { arriving: _, ...without } = state;
    return Object.keys(rest).length ? { ...without, arriving: rest } : without;
  });
  return until > now;
}
