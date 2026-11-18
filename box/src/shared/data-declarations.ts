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
export interface DataDeclaration {
  readonly owner: DataOwner | typeof workQueueOwner;
  readonly conflict: DataConflict;
  readonly backup: boolean;
  readonly families: readonly [string, ...string[]];
}

export const DATA_DECLARATIONS = {
  // The primary owns membership on every platform. SSH destinations stay local.
  setupLeadership: { owner: "primary", conflict: "owner order", backup: true, families: ["Link state", "Signed documents"] },
  setupStandbyLease: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Link state"] },
  setupStandbySettings: { owner: "local computer", conflict: "fields", backup: true, families: ["Link state"] },
  membershipAndAccess: { owner: "primary", conflict: "owner order", backup: true, families: ["Link state", "Signed documents", "Phone trust and sync"] },
  setupSecurityAndReleaseChoice: { owner: "primary", conflict: "owner order", backup: true, families: ["Local fences and audit", "Installed release"] },
  computerIdentityAndTrust: { owner: "local computer", conflict: "owner order", backup: false, families: ["Link state", "Phone trust and sync", "SSH trust and agent sockets"] },
  clientIdentity: { owner: "client", conflict: "owner order", backup: false, families: ["Desktop settings"] },
  setupCard: { owner: "local computer", conflict: "fields", backup: true, families: ["Setup discovery and welcome"] },
  welcomeProgress: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Setup discovery and welcome"] },
  boxMacLinkSettings: { owner: "local computer", conflict: "fields", backup: true, families: ["Mac link and remembered machines"] },
  clientDisplayAndConnections: { owner: "client", conflict: "fields", backup: true, families: ["Desktop settings", "Mac link and remembered machines", "Legacy Mac source"] },
  peerDirectoryAndReach: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Peer directory and reach"] },
  installAndSeedProgress: { owner: "local computer", conflict: "owner order", backup: false, families: ["Add, reinstall and seed", "Repair and update state", "Installed release"] },

  conversations: { owner: "session computer", conflict: "owner order", backup: true, families: ["Engine database", "Conversation metadata and queue", "Coding agent native storage", "Legacy Mac source"] },
  conversationSettings: { owner: "session computer", conflict: "fields", backup: true, families: ["Engine database", "Conversation metadata and queue"] },
  sessionQueue: { owner: "session computer", conflict: "owner order", backup: true, families: ["Conversation metadata and queue"] },
  sessionFiles: { owner: "session computer", conflict: "owner order", backup: true, families: ["Managed attachment files", "Artifacts"] },
  sessionNotes: { owner: "session computer", conflict: "keep both notes", backup: true, families: ["Conversation metadata and queue", "Pending work and work status"] },
  pendingSessionWork: { owner: "session computer", conflict: "owner order", backup: true, families: ["Pending work and work status", "Local fences and audit"] },
  sessionGoals: { owner: "session computer", conflict: "fields", backup: true, families: ["Pending work and work status"] },
  sessionSchedules: { owner: "session computer", conflict: "fields", backup: true, families: ["Wake-ups and watches"] },
  sessionLettersAndDelivery: { owner: "session computer", conflict: "owner order", backup: true, families: ["Letters, receipts and calls", "Legacy Mac source"] },
  searchIndex: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Search database"] },

  botSettings: { owner: "bot home", conflict: "fields", backup: true, families: ["Bot metadata and edits", "Bot text and ownership markers", "Legacy Mac source"] },
  botMemory: { owner: "bot home", conflict: "owner order", backup: true, families: ["Bot text and ownership markers"] },
  botNotes: { owner: "bot home", conflict: "keep both notes", backup: true, families: ["Bot text and ownership markers"] },
  botSchedules: { owner: "bot home", conflict: "fields", backup: true, families: ["Bot metadata and edits", "Wake-ups and watches"] },
  groupTranscript: { owner: "group host", conflict: "owner order", backup: true, families: ["Group chats and bridge delivery", "Legacy Mac source"] },
  groupSettings: { owner: "group host", conflict: "fields", backup: true, families: ["Group chats and bridge delivery"] },
  privateGroupSeats: { owner: "bot home", conflict: "owner order", backup: true, families: ["Group chats and bridge delivery", "Coding agent native storage"] },

  queueItems: { owner: workQueueOwner, conflict: "fields", backup: true, families: ["Work Queue items and history"] },
  queueNotes: { owner: workQueueOwner, conflict: "keep both notes", backup: true, families: ["Work Queue items and history"] },
  queueFiles: { owner: workQueueOwner, conflict: "owner order", backup: true, families: ["Managed attachment files"] },
  queueHistoryAndDelivery: { owner: workQueueOwner, conflict: "owner order", backup: true, families: ["Work Queue items and history", "Work Queue labels, delivery and receipts"] },
  queueNumberAllocation: { owner: "primary", conflict: "owner order", backup: true, families: ["Work Queue ledger"] },
  queueNumberUse: { owner: "local computer", conflict: "owner order", backup: true, families: ["Work Queue ledger"] },
  queueLabels: { owner: "primary", conflict: "fields", backup: true, families: ["Work Queue labels, delivery and receipts", "Work Queue categories/status migration"] },
  queueMigration: { owner: "local computer", conflict: "owner order", backup: true, families: ["Work Queue categories/status migration"] },

  ownershipAssignments: { owner: "primary", conflict: "owner order", backup: true, families: ["Signed documents", "Moves and handoffs"] },
  transferReceipts: { owner: "local computer", conflict: "owner order", backup: true, families: ["Moves and handoffs", "Rehome journal and staged trees"] },
  stagedTransferCopies: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Moves and handoffs", "Rehome journal and staged trees"] },
  projectPathsAndWorktrees: { owner: "local computer", conflict: "fields", backup: true, families: ["Projects and worktrees", "Rehome journal and staged trees"] },
  externalProjectFiles: { owner: "local computer", conflict: "native format", backup: false, families: ["Projects and worktrees", "Bot text and ownership markers"] },
  gitPreferences: { owner: "local computer", conflict: "fields", backup: true, families: ["Git preferences and backups"] },
  gitBackupRefs: { owner: "local computer", conflict: "native format", backup: false, families: ["Git preferences and backups"] },
  artifactStorageSettings: { owner: "local computer", conflict: "fields", backup: true, families: ["Artifacts"] },
  managedArtifacts: { owner: "session computer", conflict: "owner order", backup: true, families: ["Artifacts"] },

  setupPersonalities: { owner: "primary", conflict: "fields", backup: true, families: ["Personalities", "Signed documents"] },
  setupTimeZone: { owner: "primary", conflict: "fields", backup: true, families: ["Time zone"] },
  setupUsageDisplay: { owner: "primary", conflict: "fields", backup: true, families: ["Usage settings"] },
  localSettingsAndPermissions: { owner: "local computer", conflict: "fields", backup: true, families: ["Computer settings and local tools", "Local fences and audit"] },
  modelsAndReusableTools: { owner: "local computer", conflict: "fields", backup: true, families: ["Computer settings and local tools", "Engine/plugin/MCP configuration"] },
  engineAndIntegrationSettings: { owner: "local computer", conflict: "fields", backup: true, families: ["Engine/plugin/MCP configuration"] },
  installedProgramsAndCatalogs: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Engine/plugin/MCP configuration", "Installed release"] },
  localSignInsAndKeys: { owner: "local computer", conflict: "owner order", backup: false, families: ["Sign-ins and integration credentials", "Coding agent native storage", "Engine/plugin/MCP configuration", "Add, reinstall and seed"] },
  vaultKeys: { owner: "local computer", conflict: "owner order", backup: false, families: ["Vault key wrapper"] },
  vaultItemsGrantsAndPause: { owner: "local computer", conflict: "fields", backup: false, families: ["Vault integrity records", "Vault values, activity and pause"] },
  vaultSecretValues: { owner: "local computer", conflict: "owner order", backup: false, families: ["Vault values, activity and pause"] },
  vaultActivity: { owner: "local computer", conflict: "owner order", backup: false, families: ["Vault values, activity and pause"] },

  providerLimitReadings: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Plan limits and usage readings"] },
  usageRecords: { owner: "local computer", conflict: "owner order", backup: true, families: ["Plan limits and usage readings", "Insights SQLite"] },
  usageCaches: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Plan limits and usage readings", "Cursor fallback cache"] },
  browserProfile: { owner: "local computer", conflict: "native format", backup: false, families: ["Browser profile"] },
  browserSettings: { owner: "local computer", conflict: "fields", backup: true, families: ["Browser history and settings"] },
  browserHistory: { owner: "local computer", conflict: "owner order", backup: true, families: ["Browser history and settings"] },
  browserIconCache: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Browser history and settings"] },
  runtimeRecords: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Runtime rendezvous", "Runtime markers and locks", "Pending work and work status"] },
  computerLogsAndCaches: { owner: "local computer", conflict: "regenerate", backup: false, families: ["Logs and disposable caches", "Local fences and audit", "Coding agent native storage", "Work Queue labels, delivery and receipts"] },
  clientLogs: { owner: "client", conflict: "regenerate", backup: false, families: ["Desktop settings", "Logs and disposable caches"] },
} as const satisfies Record<string, DataDeclaration>;
