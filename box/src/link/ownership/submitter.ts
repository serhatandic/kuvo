import { isObject } from "../../shared/json";
import { KEY } from "./record-address";

export type Submitter =
  | { kind: "computer" }
  | { kind: "paired"; key: string; pairing: string }
  | { kind: "ssh"; key: string };

export function readSubmitter(value: unknown): Submitter | undefined {
  if (!isObject(value)) return;
  if (value.kind === "computer") return { kind: value.kind };
  if (typeof value.key !== "string" || !KEY.test(value.key)) return;
  if (value.kind === "ssh") return { kind: value.kind, key: value.key };
  if (value.kind === "paired" && typeof value.pairing === "string" && /^[A-Za-z0-9_-]{0,32}$/.test(value.pairing))
    return { kind: value.kind, key: value.key, pairing: value.pairing };
  return undefined;
}
