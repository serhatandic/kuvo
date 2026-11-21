import { createHash } from "node:crypto";

export const KEY = /^[A-Za-z0-9_-]{43}$/;
export const SETUP_ID = /^set_[A-Za-z0-9_-]{22}$/;

export function recordKey(...parts: string[]): string {
  return createHash("sha256").update(JSON.stringify(parts)).digest("base64url");
}
