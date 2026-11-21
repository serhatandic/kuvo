import fs from "node:fs";
import path from "node:path";
import { locked, textOf, writeDurablePrivate } from "../files";
import { isObject, parsed } from "../../shared/json";
import { RUN } from "../home";

const retryFile = path.join(RUN, "fleet-retry.json");
export interface RetryRequest { version: string; sha256: string }
export function requestedRetry(): RetryRequest | null {
  const value = parsed(textOf(retryFile));
  return isObject(value) && typeof value.version === "string" && typeof value.sha256 === "string"
    ? { version: value.version, sha256: value.sha256 } : null;
}

export function askForRetry(release: RetryRequest): boolean {
  return locked(`${retryFile}.lock`, () => {
    if (requestedRetry()?.sha256 === release.sha256) return false;
    writeDurablePrivate(retryFile, JSON.stringify(release));
    return true;
  }, { required: true });
}

export function claimRetry(): RetryRequest | null {
  if (!requestedRetry()) return null;
  return locked(`${retryFile}.lock`, () => {
    const requested = requestedRetry();
    if (requested) fs.rmSync(retryFile, { force: true });
    return requested;
  }, { required: true });
}
