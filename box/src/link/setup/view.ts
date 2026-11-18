import { acceptedMembership, namedRoster } from "./membership";
// Who's in your setup, as this box says it (`kuvo/setup`, PROTOCOL.md "Your setup"): facts only,
// the same on every box of one setup, from the document its main computer signs.

import { isObject } from "../../shared/json";
import { primaryPeersBody } from "../documents/work";
import { MOST_PEERS, ownKey } from "../peers/known";
import { isWelcomed } from "./welcomed";
import { followedSetup, setupDoc, startsFromOthers } from "./record";
import type { SETUP } from "../../shared/routes/box";
import type { Infer } from "../../shared/shape";
import type { LinkState } from "../state";

export type SetupView = Infer<typeof SETUP>;
type Computer = SetupView["computers"][number];

/** This box's setup as a client reads it: its id and epoch, its main computer, this box's role, each
 * computer in it (its key, name, kind, role, and whether Kuvo Code's app runs it), a main computer
 * another box named at the same epoch, the #181 switch, and whether you've had the first run with
 * this box (`welcomed`). In no
 * setup, nulls and no computers. */
export function setupView(link: LinkState | null | undefined): SetupView {
  const setup = followedSetup(link);
  const own = link ? ownKey(link) : null;
  const switched = startsFromOthers(link);
  // This box's own, in a setup or not.
  const welcomed = isWelcomed();
  if (!setup || !own) return { setup: null, epoch: null, primary: null, you: { key: own, role: null }, computers: [], startsFromOthers: switched, heldBy: [], welcomed };
  const membership = acceptedMembership(link);
  const boxes: unknown[] = membership ? namedRoster(link!, membership.boxes) : setup.role === "member" ? (Array.isArray(setupDoc(link)?.body.boxes) ? setupDoc(link)!.body.boxes as unknown[] : [])
    : primaryPeersBody(link!).boxes;
  const computers: Computer[] = [];
  for (const box of boxes.slice(0, MOST_PEERS + 1)) {
    if (!isObject(box) || typeof box.key !== "string" || (box.kind !== "mac" && box.kind !== "machine") || computers.some(each => each.key === box.key)) continue;
    computers.push({ key: box.key, name: String(box.name ?? "").slice(0, 100), kind: box.kind, role: box.key === setup.primary ? "primary" : "member", app: box.kind === "mac" });
  }
  if (setup.managed) computers.sort((a, b) => Number(b.role === "primary") - Number(a.role === "primary"));
  const main = computers.find(each => each.key === setup.primary);
  return {
    setup: setup.id,
    epoch: setup.epoch,
    primary: { key: setup.primary, name: main?.name ?? "", kind: main?.kind ?? "mac" },
    you: { key: own, role: setup.role },
    ...(setup.standby ? { standby: setup.standby.key } : {}),
    computers,
    ...(setup.conflict ? { conflict: setup.conflict } : {}),
    startsFromOthers: switched,
    heldBy: [],
    welcomed,
  };
}
