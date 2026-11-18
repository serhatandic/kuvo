// Stage a copy of the primary's access state. Apply it to its live roster only
// after the standby receipt and carry the accepted snapshot into a takeover.
import { isObject } from "../../shared/json";
import { bodyHash } from "../documents/format";
import { primaryPeersBody } from "../documents/work";
import { ownPhones } from "../phones";
import { updateLink, type LinkState } from "../state";
import { requiresStandby } from "./record";
import { pendingShared, stageShared } from "./shared";

const accessFields = ["macPeers", "macMoved", "peerNames", "left", "removedHere", "removalRevisions",
  "phones", "removed", "vouchBlocked", "untold", "untoldMembership", "pendingMachineCleanup",
] as const satisfies readonly (keyof LinkState)[];

export function updateAccess(change: (link: LinkState | null) => LinkState | null | undefined | void, { localOnly = false } = {}) {
  let sequence: number | null = null;
  let busy = false;
  const changed = updateLink(link => {
    if (localOnly || !link?.setup || !requiresStandby(link.setup) || link.setup.role !== "primary") return change(link);
    if (!link.setup.standby) {
      busy = true;
      return null;
    }
    if (pendingShared(link, "membershipAndAccess")) {
      busy = true;
      return null;
    }
    const draft = change(structuredClone(link));
    if (!draft) return null;
    const fields = Object.fromEntries(accessFields.map(key => [key, draft[key] ?? null]));
    const pairing = Object.fromEntries((["pairing", "appPairing"] as const).flatMap(slot =>
      link[slot] && !draft[slot] ? [[slot, bodyHash(link[slot].token)]] : []));
    const { boxes, left, moved } = primaryPeersBody(draft);
    sequence = stageShared(link, "membershipAndAccess", {
      epoch: link.setup.epoch, fields, pairing, boxes, left, moved,
      devices: ownPhones(draft), vouchBlocked: draft.vouchBlocked ?? [],
    });
    busy = sequence === null;
    return busy ? null : link;
  }, { durable: true });
  const save = changed?.setup && sequence !== null ? { id: changed.setup.id, epoch: changed.setup.epoch, sequence } : null;
  return { changed, save, busy };
}

export function applyAccess(link: LinkState, value: unknown): void {
  if (!isObject(value) || !isObject(value.fields)) return;
  const fields = value.fields;
  for (const key of accessFields) {
    if (fields[key] == null) delete link[key];
    else Object.assign(link, { [key]: fields[key] });
  }
  for (const slot of ["pairing", "appPairing"] as const) {
    if (isObject(value.pairing) && link[slot] && bodyHash(link[slot].token) === value.pairing[slot]) delete link[slot];
  }
}
