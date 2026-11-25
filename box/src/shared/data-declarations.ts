// Policy for durable and transient Box data (#848, rules 28–50 and 55).
// Family names refer to "Stored formats" in docs/architecture/stored-formats.md.
// An inventory family can contain several kinds with different policies. The owner is the
// authority for a record, regardless of where a replica, journal or imported copy lives.
import { OWNER, YOURS } from "./work-queue-owner";

export type DataOwner = "submission issuer" | "session computer" | "group host" | "bot home" | "primary" | "local computer" | "client" | "backup source" | "backup receiver";

export const BACKUP_ARCHIVE_VERSION = 1;

/** The authority is determined by the queue key. Unknown owner kinds have no policy yet. */
function workQueueOwner(owner: string): DataOwner | undefined {
  if (!OWNER.test(owner)) return undefined;
  if (owner === YOURS) return "local computer";
  return owner.startsWith("grp_") ? "group host" : "session computer";
}

export type DataConflict =
  | "fields"             // Merge independent fields. Ask the user about divergent edits to the same field.
  | "keep both notes"    // Merge independent passages. Keep both rewrites of the same passage.
  | "owner order"        // The owner sequences operations and deduplicates retries by identity.
  | "native format"      // The underlying editor, filesystem or program controls merging.
  | "regenerate";       // Recompute from the owner or source.

// Deletions defeat stale edits for all kinds. Backup policy is for normal encrypted backups.
// Vaults have a separate, optional encrypted export. External project files can be opted in.
export type RestorePolicy = "portable" | "machine-local" | "needs fresh approval";

export type BackupSnapshot = "bytes" | "engine database" | "sqlite" | "agent transcript";
type BackupStorage = readonly (string | {
  pattern: string;
  restore?: RestorePolicy;
  snapshot?: BackupSnapshot;
  inventory?: "session payloads" | "queue payloads";
})[];
type BackupVia = "accepted setup" | "queue items" | "owner message receipts" | "mutation evidence";

export type DataDeclaration = {
  readonly shared?: { readonly seed?: "member copies"; readonly publish?: true; readonly localSave?: "usage"; readonly checkpointOnly?: true; readonly localEffects?: readonly string[] };
  readonly owner: DataOwner | typeof workQueueOwner;
  readonly conflict: DataConflict;
  readonly backup: boolean;
  readonly restore: RestorePolicy;
  /** Kuvo-owned locations, relative to logical roots (which may live beside the box on a Mac).
   * A component may use *, and a trailing slash includes the tree. */
  readonly storage?: BackupStorage;
  readonly backupVia?: BackupVia;
  readonly backupExclusion?: string;
  /** Independent of the Box folder version. Writers use these locations and format versions. */
  readonly externalStorage?: {
    readonly root: string;
    readonly formatVersion: number;
    readonly archiveSuffix?: string;
    readonly journal?: string;
    readonly partial?: string;
  };
  readonly families: readonly [string, ...string[]];
} & (
  | { readonly backup: false }
  | { readonly backup: true; readonly storage: readonly [BackupStorage[number], ...BackupStorage[number][]] }
  | { readonly backup: true; readonly backupVia: BackupVia }
  | { readonly backup: true; readonly backupExclusion: string }
);

export const DATA_DECLARATIONS = {
  // The primary owns membership on every platform. SSH destinations stay local.
  setupLeadership: { backupVia: "accepted setup", owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents"] },
  setupReplica: { backupVia: "accepted setup", owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents"] },
  setupSaveOutbox: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Link state"] },
  setupNames: { backupVia: "accepted setup", shared: { seed: "member copies", publish: true, checkpointOnly: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", families: ["Link state", "Signed documents"] },
  setupStandbyLease: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Link state"] },
  setupStandbySettings: { owner: "local computer", conflict: "fields", backup: true, restore: "needs fresh approval", storage: ["home/setup-takeover.json"], families: ["Link state"] },
  membershipAndAccess: { backupVia: "accepted setup", shared: { publish: true, checkpointOnly: true, localEffects: ["untold", "untoldMembership", "pendingMachineCleanup", "localWipe"] }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents", "Phone trust and sync", "Computer enrollment history"] },
  setupDeparture: { backupVia: "accepted setup", shared: { publish: true, checkpointOnly: true }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents"] },
  setupMergeAuthorities: { backupVia: "accepted setup", shared: { publish: true }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents"] },
  setupMergeDenials: { backupVia: "accepted setup", shared: { publish: true }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents"] },
  setupSecurityAndReleaseChoice: { backupVia: "accepted setup", shared: { publish: true }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Local fences and audit", "Installed release"] },
  computerIdentityAndTrust: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Link state", "Phone trust and sync", "SSH trust and agent sockets"] },
  clientIdentity: { owner: "client", conflict: "owner order", backup: false, restore: "machine-local", families: ["Desktop settings"] },
  setupCard: { backupExclusion: "The replacement rebuilds discovery through enrollment.", owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", families: ["Setup discovery and welcome"] },
  welcomeProgress: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Setup discovery and welcome"] },
  boxMacLinkSettings: { backupExclusion: "The replacement creates its own Mac link.", owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", families: ["Mac link and remembered machines"] },
  clientDisplayAndConnections: { backupExclusion: "Client settings reside outside the computer backup.", owner: "client", conflict: "fields", backup: true, restore: "machine-local", families: ["Desktop settings", "Mac link and remembered machines", "Legacy Mac source"] },
  peerDirectoryAndReach: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Peer directory and reach"] },
  installAndSeedProgress: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Add, reinstall and seed", "Repair and update state", "Installed release"] },

  conversations: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: [{ pattern: "engine/opencode*.db", snapshot: "engine database" }, "engine/storage/session/", "engine/storage/message/", "engine/storage/part/", "agents/sessions/*/session.json", "agents/sessions/*/agent.json", "agents/sessions/*/messages.json", { pattern: "agents/sessions/*/messages-v1.sqlite", snapshot: "agent transcript" }], families: ["Engine database", "Conversation metadata and queue", "Coding agent native storage", "Legacy Mac source"] },
  conversationSettings: { owner: "session computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/run/session-settings.json", "home/run/named.json", "home/run/archived.json"], families: ["Engine database", "Conversation metadata and queue"] },
  sessionQueue: { owner: "session computer", conflict: "owner order", backup: true, restore: "needs fresh approval", storage: ["home/run/queues.json", "home/run/waiting/"], families: ["Conversation metadata and queue"] },
  sessionFiles: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: [{ pattern: "attachments/", inventory: "session payloads" }, { pattern: "macAttachments/", inventory: "session payloads" }], families: ["Managed attachment files", "Artifacts"] },
  sessionNotes: { owner: "session computer", conflict: "keep both notes", backup: true, restore: "portable", storage: ["home/run/work/notes/"], families: ["Conversation metadata and queue", "Pending work and work status"] },
  pendingSessionWork: { owner: "session computer", conflict: "owner order", backup: true, restore: "needs fresh approval", storage: ["home/run/work/awaited/", "home/run/work/wakes.json", "home/run/work/background.json", "home/run/work/asks.json", "home/run/work/oversight.json", "home/run/work/homes.json", "home/run/work/stopped.json", "home/run/work/unanswering.json"], families: ["Pending work and work status", "Local fences and audit"] },
  sessionSchedules: { owner: "session computer", conflict: "fields", backup: true, restore: "portable", storage: ["wakes/*/session", "wakes/*/directory", "wakes/*/via", "wakes/*/zone", "wakes/*/paused", "wakes/*/wakes/", "wakes/*/watches/", "wakes/*/summaries/"], families: ["Wake-ups and watches"] },
  computerWipePreparation: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Computer removal delivery"] },
  directoryWipeReceipts: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local",
    externalStorage: { root: "Box sibling removal receipts", journal: "last-setup-wipe or app-member-wipe", formatVersion: 1 },
    families: ["Computer removal delivery"] },
  computerRemovalDelivery: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Computer removal delivery"] },
  // The departing computer's own inventory of the work it kept at unlink: file stamps only it can match.
  releasedWork: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Retained work after unlink"] },
  diskBackupArchives: { owner: "backup source", conflict: "owner order", backup: false, restore: "needs fresh approval",
    externalStorage: { root: "kuvo-backups", archiveSuffix: ".kuvo", formatVersion: BACKUP_ARCHIVE_VERSION }, families: ["Encrypted backup archives"] },
  sshBackupArchives: { owner: "backup source", conflict: "owner order", backup: false, restore: "needs fresh approval",
    externalStorage: { root: ".kuvo-backups", archiveSuffix: ".kuvo", formatVersion: BACKUP_ARCHIVE_VERSION }, families: ["Encrypted backup archives"] },
  relayBackupArchives: { owner: "backup source", conflict: "owner order", backup: false, restore: "needs fresh approval",
    externalStorage: { root: ".kuvo-backups/relay", archiveSuffix: ".kuvo", formatVersion: BACKUP_ARCHIVE_VERSION }, families: ["Encrypted backup archives"] },
  relayBackupUploads: { owner: "backup receiver", conflict: "owner order", backup: false, restore: "machine-local",
    externalStorage: { root: ".kuvo-backups/relay", journal: ".upload.json", partial: ".upload.partial", formatVersion: 1 }, families: ["Relay backup upload reservations"] },
  setupMergeArchives: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/setup-merges/"], families: ["Local fences and audit"] },
  offlineEditOutbox: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Offline edit proposals and recovery"] },
  offlineEditSnapshots: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Offline edit proposals and recovery"] },
  queueDepartureFences: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/owned-data/edit-departures/*.json"], families: ["Work Queue items and history", "Offline edit proposals and recovery"] },
  queueDeletionFences: { owner: workQueueOwner, conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/owned-data/edit-deletions/*.json"], families: ["Work Queue items and history", "Offline edit proposals and recovery"] },
  ownerReadSnapshots: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Peer directory and reach"] },
  ownerMessageOutbox: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local",
    storage: ["home/run/owned-data/outbox/*.json", "home/run/owned-data/message-receipts/*.json"], families: ["Letters, receipts and calls"] },
  forwardedOperationOrigins: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local",
    storage: ["home/run/owned-data/forward-origins/*.json"], families: ["Letters, receipts and calls"] },
  legacyMutationEvidence: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable",
    storage: ["home/run/owned-data/mutations/*.json"], families: ["Letters, receipts and calls"] },
  forwardedPrimaryDecisions: { owner: "primary", conflict: "owner order", backup: true, restore: "portable",
    backupVia: "mutation evidence", families: ["Letters, receipts and calls"] },
  forwardedQueueDecisions: { owner: workQueueOwner, conflict: "owner order", backup: true, restore: "portable",
    backupVia: "mutation evidence", families: ["Letters, receipts and calls"] },
  forwardedSessionDecisions: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable",
    backupVia: "mutation evidence", families: ["Letters, receipts and calls"] },
  forwardedGroupDecisions: { owner: "group host", conflict: "owner order", backup: true, restore: "portable",
    backupVia: "mutation evidence", families: ["Letters, receipts and calls"] },
  forwardedComputerDecisions: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable",
    backupVia: "mutation evidence", families: ["Letters, receipts and calls"] },
  ownerGroupMessageReceipts: { backupVia: "owner message receipts", owner: "group host", conflict: "owner order", backup: true, restore: "portable", families: ["Group chats and bridge delivery", "Letters, receipts and calls"] },
  ownerMessageReceipts: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/owned-data/accepted/*.json"], families: ["Letters, receipts and calls"] },
  draftSubmissionReplayFences: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable",
    backupVia: "mutation evidence", families: ["Letters, receipts and calls"] },
  draftSubmissionEvidence: { owner: "submission issuer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/owned-data/submissions/*.json"], families: ["Letters, receipts and calls"] },
  sessionLettersAndDelivery: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/letters-seen.json", "home/run/typed-sent.json", "home/run/work/sent.json", "home/run/work/told.json", { pattern: "home/run/outbox/sending/*.json", restore: "needs fresh approval" }], families: ["Letters, receipts and calls", "Legacy Mac source"] },
  searchIndex: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Search database"] },

  botSettings: { owner: "bot home", conflict: "fields", backup: true, restore: "portable", storage: ["bots/*/bot.json", "bots/*/settings.json", "bots/*/job.json", "bots/*/kit.json", "bots/deleted.json"], families: ["Bot metadata and edits", "Bot text and ownership markers", "Legacy Mac source"] },
  botMemory: { owner: "bot home", conflict: "owner order", backup: true, restore: "portable", storage: ["bots/*/MEMORY.md", "bots/*/session", "bots/*/directory", "bots/*/via", "bots/*/zone"], families: ["Bot text and ownership markers"] },
  botNotes: { owner: "bot home", conflict: "owner order", backup: true, restore: "portable", storage: ["bots/*/notes.md"], families: ["Bot text and ownership markers"] },
  botSchedules: { owner: "bot home", conflict: "fields", backup: true, restore: "portable", storage: ["bots/*/paused", "bots/*/wakes/", "bots/*/watches/", "bots/*/summaries/"], families: ["Bot metadata and edits", "Wake-ups and watches"] },
  groupTranscript: { owner: "group host", conflict: "owner order", backup: true, restore: "portable", storage: ["groups/*/messages.json"], families: ["Group chats and bridge delivery", "Legacy Mac source"] },
  groupSettings: { owner: "group host", conflict: "fields", backup: true, restore: "portable", storage: ["groups/*/group.json", "groups/settings.json"], families: ["Group chats and bridge delivery"] },
  privateGroupSeats: { owner: "bot home", conflict: "owner order", backup: true, restore: "portable", storage: ["bots/*/groups/*.md", "groups/peer-memory.json", { pattern: "groups/peer-work.json", restore: "needs fresh approval" }, { pattern: "groups/peer-turns.json", restore: "needs fresh approval" }], families: ["Group chats and bridge delivery", "Coding agent native storage"] },

  queueItems: { owner: workQueueOwner, conflict: "fields", backup: true, restore: "portable", storage: ["home/run/work-queue/queues/*.json"], families: ["Work Queue items and history"] },
  queueNotes: { backupVia: "queue items", owner: workQueueOwner, conflict: "keep both notes", backup: true, restore: "portable", families: ["Work Queue items and history"] },
  queueFiles: { owner: workQueueOwner, conflict: "owner order", backup: true, restore: "portable", storage: [{ pattern: "home/run/work-queue/files/", inventory: "queue payloads" }], families: ["Managed attachment files"] },
  queueHistoryAndDelivery: { owner: workQueueOwner, conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/work-queue/acted.json", "home/run/work-queue/seen.json", "home/run/work-queue/queues-carried.json", "home/run/work-queue/handed.json"], families: ["Work Queue items and history", "Work Queue labels, delivery and receipts"] },
  queueNumberAllocation: { backupVia: "accepted setup", shared: {}, owner: "primary", conflict: "owner order", backup: true, restore: "portable", families: ["Work Queue ledger"] },
  queueNumberUse: { owner: "local computer", conflict: "owner order", backup: true, restore: "needs fresh approval", storage: ["home/run/work-queue/numbers.json"], families: ["Work Queue ledger"] },
  queueLabels: { shared: { seed: "member copies", publish: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["home/run/work-queue/labels.json"], families: ["Work Queue labels, delivery and receipts", "Work Queue categories/status migration"] },
  queueMigration: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/work-queue/model.json", "home/run/work-queue/categories.json"], families: ["Work Queue categories/status migration"] },

  ownershipAssignments: { backupVia: "accepted setup", shared: { publish: true, checkpointOnly: true }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Signed documents", "Moves and handoffs"] },
  ownershipRecovery: { backupVia: "accepted setup", shared: { publish: true, checkpointOnly: true }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Signed documents"] },
  transferReceipts: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/moves/decisions/*.json", "home/run/moves/decision-hints/*.json", "home/run/moves/ended.json", "home/run/moves/moved.json"], families: ["Moves and handoffs", "Rehome journal and staged trees"] },
  stagedTransferCopies: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Moves and handoffs", "Rehome journal and staged trees"] },
  projectPathsAndWorktrees: { owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", storage: ["home/box/projects.json", "home/box/worktrees.json", "home/box/folders.json", "home/box/found-folders.json"], families: ["Projects and worktrees", "Rehome journal and staged trees"] },
  externalProjectFiles: { owner: "local computer", conflict: "native format", backup: false, restore: "machine-local", families: ["Projects and worktrees", "Bot text and ownership markers"] },
  gitPreferences: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/box/git.json"], families: ["Git preferences and backups"] },
  gitBackupRefs: { owner: "local computer", conflict: "native format", backup: false, restore: "machine-local", families: ["Git preferences and backups"] },
  artifactStorageSettings: { owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", storage: ["artifacts/settings.json"], families: ["Artifacts"] },
  managedArtifacts: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["artifacts/log/*.jsonl", "artifacts/copies/"], families: ["Artifacts"] },
  libraryIndex: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["library/log.jsonl"], families: ["Artifacts"] },
  libraryThumbnails: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Artifacts"] },

  marketplaceInstallations: { shared: { publish: true }, owner: "primary", conflict: "owner order", backup: true, backupVia: "accepted setup", restore: "portable", families: ["Signed documents"] },
  setupPersonalities: { shared: { seed: "member copies", publish: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["bots/personalities.json"], families: ["Personalities", "Signed documents"] },
  setupTimeZone: { shared: { seed: "member copies", publish: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["home/time-zone.json"], families: ["Time zone"] },
  // Your starred models (the picker's Favourites). Before the setup's value exists, each box's own
  // `box/models.json` list (modelsAndReusableTools) is its copy to seed it from.
  setupStarredModels: { backupVia: "accepted setup", shared: { seed: "member copies", publish: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", families: ["Computer settings and local tools"] },
  setupUsageDisplay: { shared: { seed: "member copies", publish: true, localSave: "usage" }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["agents/usage/settings.json"], families: ["Usage settings"] },
  localSettingsAndPermissions: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/box/settings.json", "home/box/settings-flows.json", "home/box/work.json", "home/box/auto.json", "home/box/approval.json", "home/box/commands.json", { pattern: "home/box/permissions.json", restore: "needs fresh approval" }, { pattern: "home/box/permissions-later.json", restore: "needs fresh approval" }], families: ["Computer settings and local tools", "Local fences and audit"] },
  modelsAndReusableTools: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/box/models.json", "home/box/model-names.json", "home/box/kuvo-models.json"], families: ["Computer settings and local tools", "Engine/plugin/MCP configuration"] },
  engineAndIntegrationSettings: { backupExclusion: "Native settings can embed credentials and absolute paths.", owner: "local computer", conflict: "fields", backup: true, restore: "needs fresh approval", families: ["Engine/plugin/MCP configuration"] },
  installedProgramsAndCatalogs: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Engine/plugin/MCP configuration", "Installed release"] },
  localSignInsAndKeys: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Sign-ins and integration credentials", "Coding agent native storage", "Engine/plugin/MCP configuration", "Add, reinstall and seed"] },
  vaultKeys: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Vault key wrapper"] },
  vaultItemsGrantsAndPause: { owner: "local computer", conflict: "fields", backup: false, restore: "machine-local", families: ["Vault integrity records", "Vault values, activity and pause"] },
  vaultSecretValues: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Vault values, activity and pause"] },
  vaultActivity: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Vault values, activity and pause"] },

  providerLimitReadings: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Plan limits and usage readings"] },
  usageRecords: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: [{ pattern: "agents/usage/insights.sqlite", snapshot: "sqlite" }, "agents/usage/usage.json"], families: ["Plan limits and usage readings", "Insights SQLite"] },
  usageCaches: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Plan limits and usage readings", "Cursor fallback cache"] },
  browserProfile: { owner: "local computer", conflict: "native format", backup: false, restore: "machine-local", families: ["Browser profile"] },
  browserSettings: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/browser/settings.json"], families: ["Browser history and settings"] },
  browserHistory: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/browser/history.json", "home/browser/visits.jsonl", "home/browser/visits.jsonl.folding-*"], families: ["Browser history and settings"] },
  browserIconCache: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Browser history and settings"] },
  runtimeRecords: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local",
    storage: ["home/run/owned-data/receipt-index-v1/", "home/run/peers/*/lists-v1.json", "home/run/peers/*/library-v1.json", "home/run/peers/*/notices-v1.json"], families: ["Runtime rendezvous", "Runtime markers and locks", "Pending work and work status"] },
  computerLogsAndCaches: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Logs and disposable caches", "Local fences and audit", "Coding agent native storage", "Work Queue labels, delivery and receipts"] },
  clientLogs: { owner: "client", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Desktop settings", "Logs and disposable caches"] },
} as const satisfies Record<string, DataDeclaration>;

export type DataKind = keyof typeof DATA_DECLARATIONS;
type Declaration = DataKind;

export function dataKind(kind: unknown): kind is DataKind {
  return typeof kind === "string" && Object.hasOwn(DATA_DECLARATIONS, kind);
}

export function dataOwner(kind: DataKind, queue?: string): DataOwner | undefined {
  const owner = (DATA_DECLARATIONS[kind] as DataDeclaration).owner;
  return typeof owner === "string" ? owner : queue === undefined ? undefined : owner(queue);
}
export type SharedKind = {
  [K in Declaration]: typeof DATA_DECLARATIONS[K] extends { owner: "primary"; shared: object } ? K : never
}[Declaration];

export function sharedKind(kind: unknown): kind is SharedKind {
  return dataKind(kind) && DATA_DECLARATIONS[kind].owner === "primary" && "shared" in DATA_DECLARATIONS[kind];
}

export const SHARED_KINDS = Object.keys(DATA_DECLARATIONS).filter(sharedKind);

export function sharedPolicy(kind: SharedKind): NonNullable<DataDeclaration["shared"]> {
  return DATA_DECLARATIONS[kind].shared;
}
