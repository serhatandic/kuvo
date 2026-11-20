import type { LinkState } from "../state";

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
  claim?: { key: string; sign: string; address: string; until: number; provisional?: true };
}

export interface InstallerClaim { setup: string; invite: string; until: number; credentialsPending?: true; credentialsAckPending?: true; credentialsMissing?: number; cancelPending?: true; activationPending?: true }

export function installerInvitesLive(link: LinkState | null | undefined, now = Date.now()): boolean {
  return Object.values(link?.installerInvites ?? {}).some(invite => (invite.claim?.until ?? invite.expires) > now);
}

/** Joining and issuing new authority from this box must not overlap. */
export function activeInstallerClaim(link: LinkState | null | undefined): boolean {
  return Boolean(link?.macRotation || link?.installerClaim && link.setup?.role !== "member"
    && link.installerClaim.until > Date.now());
}
