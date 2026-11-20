// The release decision uses the same accepted setup save as other main-computer choices.
import { isObject, parsed } from "../../shared/json";
import { localSetupValue } from "../../shared/setup-local";
import { RUN } from "../home";
import { readPeerAgents } from "../peers/directory";
import { peersOf, type Peer } from "../peers/known";
import { readLink } from "../state";

export const RELEASE_CHOICE = "setupSecurityAndReleaseChoice";
export const releaseSetup = () => localSetupValue(RUN, RELEASE_CHOICE);
export type ReaderLayouts = Readonly<Record<string, number>>;
export const readerLayout = (value: unknown): number =>
  typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : 0;

export function releaseChoice() {
  const value = releaseSetup()?.value;
  if (!isObject(value) || typeof value.version !== "string" || !/^\d+\.\d+\.\d+(?:-beta\.\d+)?$/.test(value.version)
    || typeof value.sha256 !== "string" || !/^[a-f0-9]{64}$/.test(value.sha256) || typeof value.signed !== "string") return null;
  const manifest = acceptedManifest(value);
  const protocol = isObject(manifest) ? manifest.clientProtocol : null;
  const releasedAt = isObject(manifest) && typeof manifest.released === "string" ? Date.parse(manifest.released) || 0 : 0;
  return { version: value.version, sha256: value.sha256, signed: value.signed, releasedAt,
    clientProtocol: typeof protocol === "number" && Number.isSafeInteger(protocol) ? protocol : 0 };
}

// Pre-J primaries kept the signed envelope without projecting its new fields.
function acceptedManifest(value: Record<string, unknown>) {
  const envelope = typeof value.signed === "string" ? parsed(Buffer.from(value.signed, "base64")) : null;
  return isObject(envelope) && typeof envelope.manifest === "string" ? parsed(Buffer.from(envelope.manifest, "base64")) : null;
}

export function releaseLayout(value: unknown): number {
  if (!isObject(value)) return 0;
  const manifest = acceptedManifest(value);
  return Math.max(readerLayout(value.layout), isObject(manifest) ? readerLayout(manifest.layout) : 0);
}

/** A member's authenticated reader announcement establishes its ceiling. The immediately
 * preceding release has a known layout and predates that field. */
export function layoutHold(layout: number, link = readLink(), members: readonly Pick<Peer, "key" | "name">[] = peersOf(link), readers?: ReaderLayouts): string | null {
  if (layout === 0) return null;
  const facts = readPeerAgents();
  const behind = members.filter(peer => {
    const carried = readerLayout(readers?.[peer.key]);
    if (carried) return carried < layout;
    const said = facts[peer.key]?.computer;
    return (said?.layout ?? (said?.kuvo === "0.8.0-beta.35" ? 18 : 0)) < layout;
  });
  if (!behind.length) return null;
  return `Waiting for ${behind.map(peer => peer.name).join(", ")} to support Kuvo folder layout ${layout}.`;
}
