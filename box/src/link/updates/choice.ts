// Compatibility with followers through beta.40. Fleet decisions use peer announcements.
import { isObject } from "../../shared/json";
import { localSetupValue } from "../../shared/setup-local";
import { RUN } from "../home";

export const RELEASE_CHOICE = "setupSecurityAndReleaseChoice";
export const releaseSetup = () => localSetupValue(RUN, RELEASE_CHOICE);
export type ReaderLayouts = Readonly<Record<string, number>>;
export const readerLayout = (value: unknown): number =>
  typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : 0;

export function releaseChoice() {
  const value = releaseSetup()?.value;
  if (!isObject(value) || typeof value.version !== "string" || !/^\d+\.\d+\.\d+(?:-beta\.\d+)?$/.test(value.version)
    || typeof value.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(value.sha256) || typeof value.signed !== "string") return null;
  return { version: value.version, sha256: value.sha256, signed: value.signed };
}
