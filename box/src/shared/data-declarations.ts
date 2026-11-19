// Policy for durable and transient Box data (#848, rules 28–50 and 55).
// Family names refer to "Stored formats" in .claude/notes/848-U-versions-inventory.md.
// An inventory family can contain several kinds with different policies. The owner is the
// authority for a record, regardless of where a replica, journal or imported copy lives.
import { OWNER, YOURS } from "./work-queue-owner";

export type DataOwner = "session computer" | "group host" | "bot home" | "primary" | "local computer" | "client";

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

export interface DataDeclaration {
  readonly shared?: { readonly publish?: true; readonly localSave?: "usage" | "release"; readonly checkpointOnly?: true; readonly localEffects?: readonly string[] };
  readonly owner: DataOwner | typeof workQueueOwner;
  readonly conflict: DataConflict;
  readonly backup: boolean;
  readonly restore: RestorePolicy;
  /** Kuvo-owned locations, relative to logical roots (which may live beside the box on a Mac).
   * A component may use *, and a trailing slash includes the tree. */
  readonly storage?: readonly (string | { pattern: string; restore: RestorePolicy })[];
  readonly families: readonly [string, ...string[]];
}

export const DATA_DECLARATIONS = {
  // The primary owns membership on every platform. SSH destinations stay local.
  setupLeadership: { owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents"] },
  setupReplica: { owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents"] },
  setupSaveOutbox: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Link state"] },
  setupNames: { shared: { publish: true, checkpointOnly: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", families: ["Link state", "Signed documents"] },
  setupStandbyLease: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Link state"] },
  setupStandbySettings: { owner: "local computer", conflict: "fields", backup: true, restore: "needs fresh approval", storage: ["home/setup-takeover.json"], families: ["Link state"] },
  membershipAndAccess: { shared: { publish: true, checkpointOnly: true, localEffects: ["untold", "untoldMembership", "pendingMachineCleanup"] }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Link state", "Signed documents", "Phone trust and sync"] },
  setupSecurityAndReleaseChoice: { shared: { publish: true, localSave: "release" }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Local fences and audit", "Installed release"] },
  computerIdentityAndTrust: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Link state", "Phone trust and sync", "SSH trust and agent sockets"] },
  clientIdentity: { owner: "client", conflict: "owner order", backup: false, restore: "machine-local", families: ["Desktop settings"] },
  setupCard: { owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", families: ["Setup discovery and welcome"] },
  welcomeProgress: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Setup discovery and welcome"] },
  boxMacLinkSettings: { owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", families: ["Mac link and remembered machines"] },
  clientDisplayAndConnections: { owner: "client", conflict: "fields", backup: true, restore: "machine-local", families: ["Desktop settings", "Mac link and remembered machines", "Legacy Mac source"] },
  peerDirectoryAndReach: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Peer directory and reach"] },
  installAndSeedProgress: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Add, reinstall and seed", "Repair and update state", "Installed release"] },

  conversations: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["engine/opencode*.db", "engine/storage/session/", "engine/storage/message/", "engine/storage/part/", "agents/sessions/*/session.json", "agents/sessions/*/agent.json", "agents/sessions/*/messages.json"], families: ["Engine database", "Conversation metadata and queue", "Coding agent native storage", "Legacy Mac source"] },
  conversationSettings: { owner: "session computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/run/session-settings.json", "home/run/named.json", "home/run/archived.json"], families: ["Engine database", "Conversation metadata and queue"] },
  sessionQueue: { owner: "session computer", conflict: "owner order", backup: true, restore: "needs fresh approval", storage: ["home/run/queues.json", "home/run/waiting/"], families: ["Conversation metadata and queue"] },
  sessionFiles: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["attachments/", "macAttachments/"], families: ["Managed attachment files", "Artifacts"] },
  sessionNotes: { owner: "session computer", conflict: "keep both notes", backup: true, restore: "portable", storage: ["home/run/work/notes/"], families: ["Conversation metadata and queue", "Pending work and work status"] },
  pendingSessionWork: { owner: "session computer", conflict: "owner order", backup: true, restore: "needs fresh approval", storage: ["home/run/work/awaited/", "home/run/work/wakes.json", "home/run/work/background.json", "home/run/work/asks.json", "home/run/work/oversight.json", "home/run/work/homes.json", "home/run/work/stopped.json", "home/run/work/unanswering.json"], families: ["Pending work and work status", "Local fences and audit"] },
  sessionGoals: { owner: "session computer", conflict: "fields", backup: true, restore: "needs fresh approval", storage: ["home/run/work/goals.json"], families: ["Pending work and work status"] },
  sessionSchedules: { owner: "session computer", conflict: "fields", backup: true, restore: "portable", storage: ["wakes/*/session", "wakes/*/directory", "wakes/*/via", "wakes/*/zone", "wakes/*/paused", "wakes/*/wakes/", "wakes/*/watches/", "wakes/*/summaries/"], families: ["Wake-ups and watches"] },
  offlineEditOutbox: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Offline edit proposals and recovery"] },
  offlineEditSnapshots: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Offline edit proposals and recovery"] },
  queueDepartureFences: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/owned-data/edit-departures/*.json"], families: ["Work Queue items and history", "Offline edit proposals and recovery"] },
  queueDeletionFences: { owner: workQueueOwner, conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/owned-data/edit-deletions/*.json"], families: ["Work Queue items and history", "Offline edit proposals and recovery"] },
  ownerReadSnapshots: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Peer directory and reach"] },
  ownerMessageOutbox: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Letters, receipts and calls"] },
  ownerGroupMessageReceipts: { owner: "group host", conflict: "owner order", backup: true, restore: "portable", families: ["Group chats and bridge delivery", "Letters, receipts and calls"] },
  ownerMessageReceipts: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/owned-data/accepted/*.json", "home/run/owned-data/mutations/*.json"], families: ["Letters, receipts and calls"] },
  sessionLettersAndDelivery: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/letters-seen.json", "home/run/typed-sent.json", "home/run/work/sent.json", "home/run/work/told.json", { pattern: "home/run/outbox/sending/*.json", restore: "needs fresh approval" }], families: ["Letters, receipts and calls", "Legacy Mac source"] },
  searchIndex: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Search database"] },

  botSettings: { owner: "bot home", conflict: "fields", backup: true, restore: "portable", storage: ["bots/*/bot.json", "bots/*/settings.json", "bots/*/job.json", "bots/*/kit.json", "bots/deleted.json"], families: ["Bot metadata and edits", "Bot text and ownership markers", "Legacy Mac source"] },
  botMemory: { owner: "bot home", conflict: "owner order", backup: true, restore: "portable", storage: ["bots/*/MEMORY.md", "bots/*/session", "bots/*/directory", "bots/*/via", "bots/*/zone"], families: ["Bot text and ownership markers"] },
  botNotes: { owner: "bot home", conflict: "keep both notes", backup: true, restore: "portable", storage: ["bots/*/notes.md"], families: ["Bot text and ownership markers"] },
  botSchedules: { owner: "bot home", conflict: "fields", backup: true, restore: "portable", storage: ["bots/*/paused", "bots/*/wakes/", "bots/*/watches/", "bots/*/summaries/"], families: ["Bot metadata and edits", "Wake-ups and watches"] },
  groupTranscript: { owner: "group host", conflict: "owner order", backup: true, restore: "portable", storage: ["groups/*/messages.json"], families: ["Group chats and bridge delivery", "Legacy Mac source"] },
  groupSettings: { owner: "group host", conflict: "fields", backup: true, restore: "portable", storage: ["groups/*/group.json", "groups/settings.json"], families: ["Group chats and bridge delivery"] },
  privateGroupSeats: { owner: "bot home", conflict: "owner order", backup: true, restore: "portable", families: ["Group chats and bridge delivery", "Coding agent native storage"] },

  queueItems: { owner: workQueueOwner, conflict: "fields", backup: true, restore: "portable", storage: ["home/run/work-queue/queues/*.json"], families: ["Work Queue items and history"] },
  queueNotes: { owner: workQueueOwner, conflict: "keep both notes", backup: true, restore: "portable", families: ["Work Queue items and history"] },
  queueFiles: { owner: workQueueOwner, conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/work-queue/files/"], families: ["Managed attachment files"] },
  queueHistoryAndDelivery: { owner: workQueueOwner, conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/work-queue/acted.json", "home/run/work-queue/seen.json", "home/run/work-queue/queues-carried.json", "home/run/work-queue/handed.json"], families: ["Work Queue items and history", "Work Queue labels, delivery and receipts"] },
  queueNumberAllocation: { shared: {}, owner: "primary", conflict: "owner order", backup: true, restore: "portable", families: ["Work Queue ledger"] },
  queueNumberUse: { owner: "local computer", conflict: "owner order", backup: true, restore: "needs fresh approval", storage: ["home/run/work-queue/numbers.json"], families: ["Work Queue ledger"] },
  queueLabels: { shared: { publish: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["home/run/work-queue/labels.json"], families: ["Work Queue labels, delivery and receipts", "Work Queue categories/status migration"] },
  queueMigration: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/work-queue/model.json", "home/run/work-queue/categories.json"], families: ["Work Queue categories/status migration"] },

  ownershipAssignments: { shared: { publish: true, checkpointOnly: true }, owner: "primary", conflict: "owner order", backup: true, restore: "needs fresh approval", families: ["Signed documents", "Moves and handoffs"] },
  transferReceipts: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/run/moves/decisions/*.json", "home/run/moves/decision-hints/*.json", "home/run/moves/ended.json", "home/run/moves/moved.json"], families: ["Moves and handoffs", "Rehome journal and staged trees"] },
  stagedTransferCopies: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Moves and handoffs", "Rehome journal and staged trees"] },
  projectPathsAndWorktrees: { owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", storage: ["home/box/projects.json", "home/box/worktrees.json", "home/box/folders.json", "home/box/found-folders.json"], families: ["Projects and worktrees", "Rehome journal and staged trees"] },
  externalProjectFiles: { owner: "local computer", conflict: "native format", backup: false, restore: "machine-local", families: ["Projects and worktrees", "Bot text and ownership markers"] },
  gitPreferences: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/box/git.json"], families: ["Git preferences and backups"] },
  gitBackupRefs: { owner: "local computer", conflict: "native format", backup: false, restore: "machine-local", families: ["Git preferences and backups"] },
  artifactStorageSettings: { owner: "local computer", conflict: "fields", backup: true, restore: "machine-local", storage: ["artifacts/settings.json"], families: ["Artifacts"] },
  managedArtifacts: { owner: "session computer", conflict: "owner order", backup: true, restore: "portable", storage: ["artifacts/log/*.jsonl", "artifacts/copies/"], families: ["Artifacts"] },

  setupPersonalities: { shared: { publish: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["bots/personalities.json"], families: ["Personalities", "Signed documents"] },
  setupTimeZone: { shared: { publish: true }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["home/time-zone.json"], families: ["Time zone"] },
  setupUsageDisplay: { shared: { publish: true, localSave: "usage" }, owner: "primary", conflict: "fields", backup: true, restore: "portable", storage: ["agents/usage/settings.json"], families: ["Usage settings"] },
  localSettingsAndPermissions: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/box/settings.json", "home/box/settings-flows.json", "home/box/work.json", "home/box/auto.json", "home/box/approval.json", "home/box/commands.json", { pattern: "home/box/permissions.json", restore: "needs fresh approval" }, { pattern: "home/box/permissions-later.json", restore: "needs fresh approval" }], families: ["Computer settings and local tools", "Local fences and audit"] },
  modelsAndReusableTools: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/box/models.json", "home/box/model-names.json", "home/box/kuvo-models.json"], families: ["Computer settings and local tools", "Engine/plugin/MCP configuration"] },
  engineAndIntegrationSettings: { owner: "local computer", conflict: "fields", backup: true, restore: "needs fresh approval", families: ["Engine/plugin/MCP configuration"] },
  installedProgramsAndCatalogs: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Engine/plugin/MCP configuration", "Installed release"] },
  localSignInsAndKeys: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Sign-ins and integration credentials", "Coding agent native storage", "Engine/plugin/MCP configuration", "Add, reinstall and seed"] },
  vaultKeys: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Vault key wrapper"] },
  vaultItemsGrantsAndPause: { owner: "local computer", conflict: "fields", backup: false, restore: "machine-local", families: ["Vault integrity records", "Vault values, activity and pause"] },
  vaultSecretValues: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Vault values, activity and pause"] },
  vaultActivity: { owner: "local computer", conflict: "owner order", backup: false, restore: "machine-local", families: ["Vault values, activity and pause"] },

  providerLimitReadings: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Plan limits and usage readings"] },
  usageRecords: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["agents/usage/insights.sqlite", "agents/usage/usage.json"], families: ["Plan limits and usage readings", "Insights SQLite"] },
  usageCaches: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Plan limits and usage readings", "Cursor fallback cache"] },
  browserProfile: { owner: "local computer", conflict: "native format", backup: false, restore: "machine-local", families: ["Browser profile"] },
  browserSettings: { owner: "local computer", conflict: "fields", backup: true, restore: "portable", storage: ["home/browser/settings.json"], families: ["Browser history and settings"] },
  browserHistory: { owner: "local computer", conflict: "owner order", backup: true, restore: "portable", storage: ["home/browser/history.json", "home/browser/visits.jsonl", "home/browser/visits.jsonl.folding-*"], families: ["Browser history and settings"] },
  browserIconCache: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Browser history and settings"] },
  runtimeRecords: { owner: "local computer", conflict: "regenerate", backup: false, restore: "machine-local", families: ["Runtime rendezvous", "Runtime markers and locks", "Pending work and work status"] },
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
