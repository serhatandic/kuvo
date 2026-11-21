import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { textOf, writeDurablePrivate } from "../files";
import { K, RUN } from "../home";
import { newerVersion, switchFailure } from "./update";
import { activeBin, previousBin, versionBin } from "./versions";

export interface ReleaseRef { version: string; sha256: string }
export interface CannotUpdate extends ReleaseRef { reason: string; fixable: boolean }

export const sameRelease = (a?: ReleaseRef | null, b?: ReleaseRef | null): boolean =>
  !!a && !!b && a.version === b.version && a.sha256 === b.sha256;

export function releaseRef(value: unknown): ReleaseRef | undefined {
  if (!isObject(value) || typeof value.version !== "string" || !/^\d+\.\d+\.\d+(?:-beta\.\d+)?$/.test(value.version)
    || typeof value.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(value.sha256)) return;
  return { version: value.version, sha256: value.sha256 };
}

export function cannotFact(value: unknown): CannotUpdate | undefined {
  const release = releaseRef(value);
  if (!release || !isObject(value) || typeof value.reason !== "string" || typeof value.fixable !== "boolean") return;
  return { ...release, reason: value.reason.slice(0, 300), fixable: value.fixable };
}

export const CANNOT_FILE = path.join(RUN, "fleet-cannot.json");
export const READY_FILE = path.join(RUN, "fleet-ready.json");
export const readyFact = (): ReleaseRef | undefined => {
  const staged = releaseRef(parsed(textOf(READY_FILE)));
  if (!staged) return;
  const target = versionBin(staged.version, staged.sha256);
  if (activeBin(K) !== target && fs.existsSync(path.join(K, target, ".installed.json"))) return staged;
  return undefined;
};

export function recordReady(release: ReleaseRef): void {
  writeDurablePrivate(READY_FILE, JSON.stringify(release));
}

export function recordCannot(release: ReleaseRef, reason: string, fixable: boolean): void {
  writeDurablePrivate(CANNOT_FILE, JSON.stringify({ ...release, reason, fixable }));
}

export const localCannot = (): CannotUpdate | undefined => cannotFact(parsed(textOf(CANNOT_FILE)));

export function cannotFor(release: ReleaseRef): CannotUpdate | undefined {
  const failure = switchFailure();
  if (failure?.version === release.version && release.sha256.startsWith(failure.sha12)) {
    return { ...release, reason: failure.message, fixable: false };
  }
  const cannot = localCannot();
  if (sameRelease(cannot, release)) return cannot;
  return undefined;
}

export function retryCannot(release: ReleaseRef): void {
  if (sameRelease(localCannot(), release)) fs.rmSync(CANNOT_FILE, { force: true });
}

/** During the start guard the selected bin is provisional. Announce the last working version. */
export function runningVersion(selected: string | null): string | null {
  const previous = textOf(previousBin(K))?.trim();
  if (!previous || previous === activeBin(K)) return selected;
  const version = textOf(path.join(K, previous, "box-version"))?.trim();
  return newerVersion(version, null) ? version! : null;
}
