import fs from "node:fs";
import path from "node:path";
import { locked, textOf, writeDurablePrivate, writePrivate } from "../files";
import { isObject, parsed } from "../../shared/json";
import { RUN } from "../home";
import { appendUpdateEvent } from "./journal";

const file = path.join(RUN, "fleet-transfer.json");
export const STUCK_AFTER = 5 * 60_000;
export type TransferStep = "requesting manifest" | "receiving pack" | "offering pack" | "offer accepted";
interface TransferAttempt {
  version: string;
  sha256?: string;
  peer: string;
  step: TransferStep;
  at: number;
  failures: number;
  failuresByPeer?: Record<string, number>;
  reason?: string;
  lastFailedAt?: number;
  retryAt?: number;
}

export function transferAttempt(): TransferAttempt | null {
  const value = parsed(textOf(file));
  return isObject(value) && typeof value.at === "number" ? value as unknown as TransferAttempt : null;
}

export function checkpointTransfer(version: string, peer: string, step: TransferStep, at: number, reason?: string, sha256?: string): TransferAttempt {
  const previous = transferAttempt();
  const same = previous?.version === version && previous.sha256 === sha256;
  const failuresByPeer = same ? { ...previous.failuresByPeer } : {};
  const failures = (failuresByPeer[peer] ?? (same && previous.peer === peer ? previous.failures : 0)) + (reason ? 1 : 0);
  failuresByPeer[peer] = failures;
  const retryAt = reason ? at + Math.min(60 * 60_000, 30_000 * 2 ** Math.min(failures - 1, 7)) : undefined;
  const lastFailedAt = reason ? at : same ? previous.lastFailedAt : undefined;
  const attempt = { version, sha256, peer, step, at, failures, failuresByPeer, lastFailedAt, ...(reason ? { reason, retryAt } : {}) };
  try { writePrivate(file, JSON.stringify(attempt)); } catch (error) {
    console.error(`Kuvo updates: Couldn't record an update attempt: ${(error as Error).message}`);
  }
  if (reason || !same || previous.peer !== peer || previous.step !== step || previous.reason)
    appendUpdateEvent(version, step, reason ?? `Release transfer with ${peer}.`, at);
  return attempt;
}

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
