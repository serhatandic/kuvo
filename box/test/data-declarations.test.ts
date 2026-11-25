import "./safety"
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { DATA_DECLARATIONS } from "../src/shared/data-declarations";

// Required policy kinds from #848. This checklist is independent of the declarations and the
// storage inventory because several kinds can share a family.
const REQUIRED_KINDS = [
  "membershipAndAccess",
  "setupLeadership",
  "setupReplica",
  "setupDeparture",
  "setupMergeAuthorities",
  "setupMergeDenials",
  "setupMergeArchives",
  "setupSaveOutbox",
  "setupNames",
  "computerRemovalDelivery",
  "directoryWipeReceipts",
  "computerWipePreparation",
  "releasedWork",
  "diskBackupArchives",
  "sshBackupArchives",
  "relayBackupArchives",
  "relayBackupUploads",
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
  "sessionSchedules",
  "sessionLettersAndDelivery",
  "offlineEditOutbox",
  "offlineEditSnapshots",
  "queueDepartureFences",
  "queueDeletionFences",
  "ownerReadSnapshots",
  "ownerMessageOutbox",
  "forwardedOperationOrigins",
  "legacyMutationEvidence",
  "forwardedPrimaryDecisions",
  "forwardedQueueDecisions",
  "forwardedSessionDecisions",
  "forwardedGroupDecisions",
  "forwardedComputerDecisions",
  "draftSubmissionEvidence",
  "draftSubmissionReplayFences",
  "ownerMessageReceipts",
  "ownerGroupMessageReceipts",
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
  "ownershipRecovery",
  "transferReceipts",
  "stagedTransferCopies",
  "projectPathsAndWorktrees",
  "externalProjectFiles",
  "gitPreferences",
  "gitBackupRefs",
  "artifactStorageSettings",
  "managedArtifacts",
  "libraryIndex",
  "libraryThumbnails",

  "marketplaceInstallations",
  "setupPersonalities",
  "setupTimeZone",
  "setupStarredModels",
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
  const inventory = readFileSync(new URL("../../docs/architecture/stored-formats.md", import.meta.url), "utf8");
  const stored = inventory.split("## Stored formats\n")[1]?.split("\n## ")[0] ?? "";
  const families = Array.from(stored.matchAll(/^\| ([^|]+) \|/gm), row => row[1]!)
    .filter(name => name !== "Family" && !/^:?-+:?$/.test(name));
  expect(families.length).toBeGreaterThan(0);
  expect(new Set(families).size).toBe(families.length);

  const declared: string[] = Object.values(DATA_DECLARATIONS).flatMap(data => [...data.families]);
  expect([...new Set(declared)].sort()).toEqual([...families].sort());
});

test("backup writers outside the Box folder have independent ownership and format declarations", () => {
  const writers = Object.entries(DATA_DECLARATIONS).filter(([, data]) => "externalStorage" in data);
  expect(writers.map(([kind]) => kind).sort()).toEqual([
    "directoryWipeReceipts", "diskBackupArchives", "relayBackupArchives", "relayBackupUploads", "sshBackupArchives",
  ]);
  for (const kind of ["diskBackupArchives", "relayBackupArchives", "sshBackupArchives"] as const) {
    expect(DATA_DECLARATIONS[kind]).toMatchObject({
      owner: "backup source", conflict: "owner order", backup: false, restore: "needs fresh approval",
      externalStorage: { archiveSuffix: ".kuvo", formatVersion: 1 },
    });
  }
  expect(DATA_DECLARATIONS.relayBackupUploads).toMatchObject({
    owner: "backup receiver", conflict: "owner order", backup: false, restore: "machine-local",
    externalStorage: { root: DATA_DECLARATIONS.relayBackupArchives.externalStorage.root,
      journal: ".upload.json", partial: ".upload.partial", formatVersion: 1 },
  });
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
