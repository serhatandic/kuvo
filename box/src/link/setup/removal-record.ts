import type { MoveKind } from "../../shared/routes/moves";
import { isObject } from "../../shared/json";
import { cleanPeer, fitsPeer, KEY, type Peer } from "../peers/known";
import { recordKey } from "../ownership/store";
import type { LinkState } from "../state";
import { acceptedMembership } from "./membership";

export const REMOVAL_FEATURE = "computers.removal";
export const PREPARATION_FEATURE = "computers.wipe-preparation";
export const EXPORT_INVENTORY_VERSION = 1;
export const REMOVAL_EXPORT_REQUIRED = "Relay backups require setup access. Choose a separate disk or SSH export to finish wiping this computer.";
export const REMOVAL_LAYOUT_UNSUPPORTED = "This computer’s data layout cannot be wiped from here. Cancel the wipe to leave its data in place.";
export { OPERATION_ID as REMOVAL_ID } from "./shared";
import { OPERATION_ID } from "./shared";

export interface WipePreparationClaim {
  operation: string;
  choiceHash: string;
}

export interface ExportReference {
  source: string;
  id: string;
  at: number;
}

export const MOVE_INVENTORY_VERSION = 1;
export const REMOVAL_OPERATIONS_FEATURE = "computers.removal-operations";

export interface RemovalPlan {
  choiceHash: string;
  moveTo?: string;
  archive?: true;
}

export type SelectedMove = { kind: MoveKind; id: string };
export const MAX_SELECTED_MOVES = 512;

export function selectionOf(value: unknown): SelectedMove[] | undefined {
  if (!Array.isArray(value) || value.length > MAX_SELECTED_MOVES) return;
  const selected: SelectedMove[] = [];
  const seen = new Set<string>();
  for (const item of value) {
    if (!isObject(item) || typeof item.id !== "string" || !/^[A-Za-z0-9_-]{1,84}$/.test(item.id) ||
      (item.kind !== "chat" && item.kind !== "bot" && item.kind !== "group") || seen.has(item.id)) return;
    selected.push({ id: item.id, kind: item.kind });
    seen.add(item.id);
  }
  return selected;
}

export interface MemberRemoval {
  peer: Peer;
  operation: string;
  wipe: boolean;
  local?: true;
  requiresPreparation?: true;
  export?: ExportReference;
  delivered?: true;
  plan?: RemovalPlan;
  selection?: SelectedMove[];
  generation?: string;
  inventoryVersion?: number;
  moveInventoryVersion?: number;
  withdrawnGeneration?: string;
  error?: string;
  needsExport?: true;
  retry?: string;
  unresolved?: string[];
  resolution?: "cancelled" | "lost";
  superseded?: Record<string, RemovalPlan>;
}

export interface LocalWipe {
  setup: string;
  operation: string;
  primary?: Peer;
  source?: string;
  prepared?: true;
  backup?: ExportReference;
  hostKeysCleared?: true;
  generation?: string;
  requiresPreparation?: true;
}

export const generationOf = (value: unknown): value is string => typeof value === "string" && /^[\w-]{43}$/.test(value);

function planOf(value: unknown): RemovalPlan | undefined {
  if (!isObject(value) || !generationOf(value.choiceHash) ||
    (value.moveTo !== undefined && (typeof value.moveTo !== "string" || !KEY.test(value.moveTo)))) return;
  return { choiceHash: value.choiceHash, ...(typeof value.moveTo === "string" ? { moveTo: value.moveTo } : {}),
    ...(value.archive === true ? { archive: true } : {}) };
}

export function removalRecords(value: unknown, left: Set<string>, current: Set<string>): Record<string, MemberRemoval> {
  if (!isObject(value)) return {};
  return Object.fromEntries(Object.entries(value).flatMap(([key, record]) => {
    if (!isObject(record)) return [];
    const local = record.local === true && record.wipe === true && current.size === 1 && current.has(key);
    if ((!local && (!left.has(key) || current.has(key))) || !fitsPeer(record.peer)
      || record.peer.key !== key || typeof record.operation !== "string" || !OPERATION_ID.test(record.operation)
      || typeof record.wipe !== "boolean") return [];
    const point = record.export;
    const archive = record.wipe && isObject(point) && typeof point.source === "string" &&
      typeof point.id === "string" && typeof point.at === "number" && Number.isFinite(point.at)
      ? { source: point.source, id: point.id, at: point.at } : undefined;
    const { id: _destination, ...peer } = cleanPeer(record.peer);
    const plan = planOf(record.plan);
    const selection = plan?.moveTo ? selectionOf(record.selection) : undefined;
    const superseded = Object.fromEntries(Object.entries(isObject(record.superseded) ? record.superseded : {}).flatMap(([operation, choice]) => {
      const prior = planOf(choice);
      return OPERATION_ID.test(operation) && prior ? [[operation, prior]] : [];
    }));
    return [[key, { peer, operation: record.operation, wipe: record.wipe, ...(local ? { local: true } : {}),
      ...(record.wipe && (plan || record.requiresPreparation === true) ? { requiresPreparation: true } : {}),
      ...(plan ? { plan } : {}), ...(selection ? { selection } : {}), ...(generationOf(record.generation) ? { generation: record.generation } : {}),
      ...(plan?.archive && record.inventoryVersion === EXPORT_INVENTORY_VERSION ? { inventoryVersion: EXPORT_INVENTORY_VERSION } : {}),
      ...(plan?.moveTo && record.moveInventoryVersion === MOVE_INVENTORY_VERSION ? { moveInventoryVersion: MOVE_INVENTORY_VERSION } : {}),
      ...((plan?.archive || plan?.moveTo) && generationOf(record.withdrawnGeneration) ? { withdrawnGeneration: record.withdrawnGeneration } : {}),
      ...(archive ? { export: archive } : {}), ...(record.delivered === true ? { delivered: true } : {}),
      ...(record.resolution === "cancelled" || record.resolution === "lost" ? { resolution: record.resolution } : {}),
      ...(typeof record.retry === "string" && OPERATION_ID.test(record.retry) ? { retry: record.retry } : {}),
      ...(typeof record.error === "string" ? { error: record.error.slice(0, 500) } : {}),
      ...(record.wipe && !plan && record.needsExport === true ? { needsExport: true } : {}),
      ...(Array.isArray(record.unresolved) ? { unresolved: record.unresolved.filter(id => typeof id === "string").slice(0, 100) } : {}),
      ...(Object.keys(superseded).length ? { superseded } : {}),
    }]];
  }));
}

export const removalsOf = (link: LinkState | null | undefined) => acceptedMembership(link)?.removals ?? {};

export function removalNextAction({ needsExport, error, plan }: Pick<MemberRemoval, "needsExport" | "error" | "plan">) {
  if (needsExport) return "choose-export" as const;
  if (error === REMOVAL_EXPORT_REQUIRED && plan) return "revise-export" as const;
  if (error === REMOVAL_LAYOUT_UNSUPPORTED) return "cancel-wipe" as const;
  return undefined;
}

export function pendingWipes(link: LinkState | null | undefined) {
  return Object.values(removalsOf(link)).flatMap(record => {
    const { peer, operation, wipe, delivered, resolution, error, unresolved } = record;
    const nextAction = removalNextAction(record);
    return wipe && !delivered && !resolution ? [{ key: peer.key, name: peer.name, operation, ...(error ? { error } : {}),
      ...(nextAction ? { nextAction } : {}), ...(unresolved?.length ? { unresolved } : {}) }] : [];
  });
}

export const wipeOutstanding = (record: MemberRemoval | undefined) =>
  Boolean(record?.wipe && !record.delivered && !record.resolution);

export const localRemovalPending = (link: LinkState | null | undefined) => {
  const record = link?.setup?.role === "primary" ? removalsOf(link)[link.setup.primary] : undefined;
  return Boolean(record?.local && wipeOutstanding(record));
};

export function removalMoveID(operation: string, kind: MoveKind, id: string) {
  return "mov_" + Buffer.from(recordKey(operation, kind, id), "base64url").toString("hex").slice(0, 32);
}

export const removalDeliveryPending = (record: MemberRemoval) => !record.delivered && record.resolution !== "lost"
  && (!record.local || !record.resolution);
