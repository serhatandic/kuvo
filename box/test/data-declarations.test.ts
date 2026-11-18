import "./safety";
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { DATA_DECLARATIONS } from "../src/shared/data-declarations";

// Required policy kinds from #848. This checklist is independent of the declarations and the
// storage inventory because several kinds can share a family.
const REQUIRED_KINDS = [
  "membershipAndAccess",
  "setupLeadership",
  "setupReplica",
  "setupSaveOutbox",
  "setupNames",
  "setupStandbyLease",
  "setupStandbySettings",
  "setupSecurityAndReleaseChoice",
  "computerIdentityAndTrust",
  "clientIdentity",
  "setupCard",
  "welcomeProgress",
  "boxMacLinkSettings",
  "clientDisplayAndConnections",
  "peerDirectoryAndReach",
  "installAndSeedProgress",

  "conversations",
  "conversationSettings",
  "sessionQueue",
  "sessionFiles",
  "sessionNotes",
  "pendingSessionWork",
  "sessionGoals",
  "sessionSchedules",
  "sessionLettersAndDelivery",
  "searchIndex",

  "botSettings",
  "botMemory",
  "botNotes",
  "botSchedules",
  "groupTranscript",
  "groupSettings",
  "privateGroupSeats",

  "queueItems",
  "queueNotes",
  "queueFiles",
  "queueHistoryAndDelivery",
  "queueNumberAllocation",
  "queueNumberUse",
  "queueLabels",
  "queueMigration",

  "ownershipAssignments",
  "transferReceipts",
  "stagedTransferCopies",
  "projectPathsAndWorktrees",
  "externalProjectFiles",
  "gitPreferences",
  "gitBackupRefs",
  "artifactStorageSettings",
  "managedArtifacts",

  "setupPersonalities",
  "setupTimeZone",
  "setupUsageDisplay",
  "localSettingsAndPermissions",
  "modelsAndReusableTools",
  "engineAndIntegrationSettings",
  "installedProgramsAndCatalogs",
  "localSignInsAndKeys",
  "vaultKeys",
  "vaultItemsGrantsAndPause",
  "vaultSecretValues",
  "vaultActivity",

  "providerLimitReadings",
  "usageRecords",
  "usageCaches",
  "browserProfile",
  "browserSettings",
  "browserHistory",
  "browserIconCache",
  "runtimeRecords",
  "computerLogsAndCaches",
  "clientLogs",
];

test("every required data kind has its own declaration", () => {
  expect(Object.keys(DATA_DECLARATIONS).sort()).toEqual([...REQUIRED_KINDS].sort());
});

test("every stored format in the version inventory has a data declaration", () => {
  const inventory = readFileSync(new URL("../../.claude/notes/848-U-versions-inventory.md", import.meta.url), "utf8");
  const stored = inventory.split("## Stored formats\n")[1]?.split("\n## ")[0] ?? "";
  const families = Array.from(stored.matchAll(/^\| ([^|]+) \|/gm), row => row[1]!)
    .filter(name => name !== "Family" && !/^:?-+:?$/.test(name));
  expect(families.length).toBeGreaterThan(0);
  expect(new Set(families).size).toBe(families.length);

  const declared: string[] = Object.values(DATA_DECLARATIONS).flatMap(data => [...data.families]);
  expect([...new Set(declared)].sort()).toEqual([...families].sort());
});

test("Work Queue item policies use the authority for the queue's owner", () => {
  for (const kind of ["queueItems", "queueNotes", "queueFiles", "queueHistoryAndDelivery"] as const) {
    const { owner } = DATA_DECLARATIONS[kind];
    expect(owner).toBeFunction();
    expect(owner("ses_Example")).toBe("session computer");
    expect(owner("grp_example")).toBe("group host");
    expect(owner("usr_you")).toBe("local computer");
    expect(owner("future_example")).toBeUndefined();
  }
});
