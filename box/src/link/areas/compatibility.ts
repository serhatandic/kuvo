import required from "../../../../protocol/gate-required.txt" with { type: "text" };
import type { Compatibility } from "../../shared/routes/box";
import { offlineEditsReady, ownershipReady } from "../ownership/store";

const REQUIRED = Array.from(new Set(required.split("\n").map(line => line.trim()).filter(line => line && !line.startsWith("#")))).sort();
const BASELINE = required.split("\n").find(line => line.startsWith("# Baseline: "))!.slice("# Baseline: ".length).trim().split(/\s+/);

/** Compatibility is for display. Progress toward a release does not affect which routes may run. */
export function computerCompatibility(features: readonly string[]): Compatibility {
  const has = new Set(features);
  const missing = REQUIRED.filter(name => !has.has(name));
  if (!missing.length) return { state: "open" };
  return { state: BASELINE.some(name => !has.has(name)) ? "held" : "behind", missing };
}

export function editFeatures<T extends string>(features: readonly T[]): T[] {
  const ownership = ownershipReady();
  const edits = offlineEditsReady();
  return features.filter(feature => (!["data.offline-edits", "data.session-notes"].includes(feature) || edits) && (!["data.mutation-identity", "typed.draft", "typed.submission-recovery"].includes(feature) || ownership));
}
