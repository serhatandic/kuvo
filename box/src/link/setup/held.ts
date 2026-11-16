// Which of this Mac's machines hold it back from leading its setup by their word (PROTOCOL.md,
// "Your setup"): `leadsNow` waits on them, and `kuvo/setup` and the card name them.

import { IS_MAC } from "../home";
import { peersOf } from "../peers/known";
import { goneOf } from "../work-queue/numbers";
import { toldShared } from "../work-queue/told";
import { followedSetup } from "./record";
import type { LinkState } from "../state";

/** The machines whose word holds this Mac back from leading its setup (`kuvo/setup`'s `heldBy`):
 * to start leading, each that hasn't said it knows setups (an older box, which says no `shared` in
 * how it numbers); once it leads, none. A machine more than one Mac set up is yours all the same,
 * so an older one saying it's shared holds nothing back. One that never told this Mac how it
 * numbers, or that numbering counts as gone (not heard for days), holds nothing back either, so one
 * stale machine never keeps a Mac out of a setup. */
export function heldBy(link: LinkState | null | undefined, now = Date.now()): string[] {
  if (!IS_MAC || !link || followedSetup(link)?.role === "primary") return [];
  const gone = goneOf(link, now);
  return peersOf(link).filter(peer => !gone.has(peer.key) && toldShared(peer.key) === undefined).map(peer => peer.key);
}
