import { isObject } from "./json";

export class UnsupportedFormatVersion extends Error {
  constructor(format: string, found: unknown, current: number) {
    const version = typeof found === "number" && Number.isSafeInteger(found) ? String(found) : "invalid";
    super(`${format} format version ${version} is unsupported (this build writes ${current}).`);
  }
}

/** Explicit versions are nonnegative integers. Only an absent field denotes the legacy shape. */
function formatVersion(format: string, value: unknown, legacy: number, current: number): number {
  if (value === undefined) return legacy;
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
    throw new UnsupportedFormatVersion(format, value, current);
  }
  return value;
}

type Upgrade<T> = (value: T) => T;

/** Step i takes version i to i+1. The caller commits the result as one write, under the
 * format's writer lock if it has one. Steps on live data must work on a copy. */
export function upgradeFormat<T>(format: string, from: unknown, value: T, steps: readonly Upgrade<T>[]): T {
  const version = formatVersion(format, from, 0, steps.length);
  if (version > steps.length) throw new UnsupportedFormatVersion(format, version, steps.length);
  return steps.slice(version).reduce((record, step) => step(record), value);
}

/** An additive object format. The stamp travels with the fields older writers already know.
 * Reading upgrades in memory so polling readers never race another process's write. */
export function objectFormat(name: string, steps: readonly Upgrade<Record<string, unknown>>[]) {
  return {
    version: steps.length,
    read(value: unknown): Record<string, unknown> {
      if (!isObject(value)) throw new Error(`${name} isn't a JSON object.`);
      const { formatVersion: from, ...fields } = value;
      return upgradeFormat(name, from, fields, steps);
    },
    write<T extends object>(fields: T) {
      return { ...fields, formatVersion: steps.length };
    },
  };
}

/** Version 1 keeps the legacy object fields in place for neighbouring writers. */
export const additiveObjectFormat = (name: string) => objectFormat(name, [fields => fields]);

/** For legacy lists whose readers already filter out objects without the list's item fields. */
export function additiveListFormat(name: string) {
  const steps: readonly Upgrade<unknown[]>[] = [items => items];
  return {
    read(value: unknown): unknown[] {
      if (!Array.isArray(value)) throw new Error(`${name} isn't a JSON array.`);
      const marker = value[0];
      return isObject(marker) && Object.hasOwn(marker, "formatVersion")
        ? upgradeFormat(name, marker.formatVersion, value.slice(1), steps)
        : upgradeFormat(name, undefined, value, steps);
    },
    write<T>(items: readonly T[]) {
      return [{ formatVersion: steps.length }, ...items];
    },
  };
}
