import { additiveObjectFormat } from "../../shared/format-version";

export const setupInputFormat = additiveObjectFormat("SSH setup input");
export const settingsSeedFormat = additiveObjectFormat("Box settings seed");
export const credentialSeedFormat = additiveObjectFormat("Box seed");

/** The deployed guarded reservation token is the first recoverable format. Older
 * opaque reservations still hold the account and require manual recovery. */
export const RESERVATION_PREFIX = "guarded-";
export const UNSUPPORTED_RESERVATION = "SSH setup reservation format is unsupported. Update Kuvo and retry.";
export function isSetupReservation(token: string): boolean {
  return /^guarded-[0-9a-f]{48}$/.test(token);
}
