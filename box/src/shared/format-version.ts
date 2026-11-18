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

/** Family stamps describe a record's encoding. Decoders may adapt legacy bytes in
 * memory. The folder-version runner owns durable changes spanning local files. */
export function objectFormat(
  name: string,
  version: number,
  decode: (fields: Record<string, unknown>, from: number) => Record<string, unknown> = fields => fields,
) {
  return {
    version,
    read(value: unknown): Record<string, unknown> {
      if (!isObject(value)) throw new Error(`${name} isn't a JSON object.`);
      const { formatVersion: from, ...fields } = value;
      return decode(fields, readableVersion(name, from, version));
    },
    write<T extends object>(fields: T) {
      return { ...fields, formatVersion: version };
    },
  };
}

function readableVersion(name: string, from: unknown, current: number) {
  const version = formatVersion(name, from, 0, current);
  if (version > current) throw new UnsupportedFormatVersion(name, version, current);
  return version;
}

/** Version 1 keeps the legacy object fields in place for neighbouring writers. */
export const additiveObjectFormat = (name: string) => objectFormat(name, 1);

/** For legacy lists whose readers already filter out objects without the list's item fields. */
export function additiveListFormat(name: string) {
  const version = 1;
  return {
    read(value: unknown): unknown[] {
      if (!Array.isArray(value)) throw new Error(`${name} isn't a JSON array.`);
      const marker = value[0];
      if (!isObject(marker) || !Object.hasOwn(marker, "formatVersion")) return value;
      readableVersion(name, marker.formatVersion, version);
      return value.slice(1);
    },
    write<T>(items: readonly T[]) {
      return [{ formatVersion: version }, ...items];
    },
  };
}
