// The release decision uses the same accepted setup save as other main-computer choices.
import { isObject, parsed } from "../../shared/json";
import { localSetupValue } from "../../shared/setup-local";
import { RUN } from "../home";
import { readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { followedSetup } from "../setup/record";
import { readLink } from "../state";

export const RELEASE_CHOICE = "setupSecurityAndReleaseChoice";
export const releaseSetup = () => localSetupValue(RUN, RELEASE_CHOICE);

export function releaseChoice() {
  const value = releaseSetup()?.value;
  if (!isObject(value) || typeof value.version !== "string" || !/^\d+\.\d+\.\d+(?:-beta\.\d+)?$/.test(value.version)
    || typeof value.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(value.sha256) || typeof value.signed !== "string") return null;
  // A pre-J primary can accept a new signed manifest without projecting its new fields.
  const envelope = parsed(Buffer.from(value.signed, "base64"));
  const manifest = isObject(envelope) && typeof envelope.manifest === "string" ? parsed(Buffer.from(envelope.manifest, "base64")) : null;
  const protocol = isObject(manifest) ? manifest.clientProtocol : null;
  return { version: value.version, sha256: value.sha256, signed: value.signed,
    clientProtocol: typeof protocol === "number" && Number.isSafeInteger(protocol) ? protocol : 0 };
}

/** A member's authenticated reader announcement establishes its ceiling. The immediately
 * preceding release has a known layout and predates that field. */
export function layoutHold(layout: number, link = readLink()): string | null {
  if (!followedSetup(link) || !releaseSetup()) return null;
  const facts = readPeerAgents();
  const behind = peersOf(link).filter(peer => {
    const said = facts[peer.key]?.computer;
    return (said?.layout ?? (said?.kuvo === "0.8.0-beta.35" ? 18 : 0)) < layout;
  });
  if (!behind.length) return null;
  return `Waiting for ${behind.map(peer => peer.name).join(", ")} to support Kuvo folder layout ${layout}.`;
}
