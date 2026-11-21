import type { LinkState } from "../state";

export const INSTALLER_RECOVERY_ROUTE = "kuvo/setup/installer-recovery";

export interface MergeTarget {
  source: string;
  computer: string;
  resume?: string;
}

export interface InstallerInvite {
  setup: string;
  epoch: number;
  expires: number;
  credentials?: string[];
  credentialSelectionDigest?: string;
  credentialSeed?: string;
  credentialsCompleted?: true;
  cancelled?: true;
  activated?: true;
  merge?: MergeTarget;
  actor?: { key: string; pairing: string };
  claim?: { key: string; sign: string; address: string; until: number; provisional?: true; recoveryBound?: true };
}

export interface InstallerClaim {
  setup: string;
  invite: string;
  until: number;
  recovery?: { key: string; secret: string };
  adopted?: true;
  cancelPending?: true;
  activationPending?: true;
  portableActivation?: true;
  credentialsPending?: true;
  credentialsAckPending?: true;
  credentialsMissing?: number;
}
export type InstallerCancellation = Pick<InstallerClaim, "setup" | "invite"> & { key: string; secret: string };

export function installerInvitesLive(link: LinkState | null | undefined, now = Date.now()): boolean {
  return Object.values(link?.installerInvites ?? {}).some(invite => (invite.claim?.until ?? invite.expires) > now);
}

/** Joining and issuing new authority from this box must not overlap. */
export function activeInstallerClaim(link: LinkState | null | undefined): boolean {
  return Boolean(link?.macRotation || link?.installerClaim && !link.installerClaim.adopted && link.setup?.role !== "member"
    && link.installerClaim.until > Date.now());
}
