import { expect, test } from "bun:test";
import {
  enrollmentPredecessor, readComputerHistory, rememberEnrollment, requiresReplacementConfirmation, type ComputerHistory,
} from "../src/link/setup/computer-history";

test("an OS-image identity proposes a live replacement without authorizing it", () => {
  const key = "a".repeat(43);
  const digest = "a".repeat(64);
  const history: ComputerHistory = { setup: "setup", slots: [], retiredSigns: [] };
  rememberEnrollment(history, key, digest, undefined, "os-id");
  const saved = readComputerHistory(JSON.parse(JSON.stringify(history)), new Set([key]), new Set());
  expect(saved?.slots).toEqual([{ key, fingerprint: digest, fingerprintSource: "os-id" }]);
  expect(enrollmentPredecessor(saved, digest)).toBe(key);
  expect(requiresReplacementConfirmation(saved, key, "os-id")).toBe(true);
  expect(requiresReplacementConfirmation(saved, key, undefined)).toBe(true);

  rememberEnrollment(history, key, "b".repeat(64), undefined, "dmi-uuid");
  expect(requiresReplacementConfirmation(history, key, "dmi-uuid")).toBe(false);
  expect(requiresReplacementConfirmation(history, key, "os-id")).toBe(true);
  expect(enrollmentPredecessor(history, "b".repeat(64), key)).toBe(key);
});
