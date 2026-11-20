// A compatibility refusal is carried in the authenticated handshake or local HTTP response.
import { readPeerAgents } from "../peers/directory";
import { releaseChoice } from "./choice";
import { boxVersion, needsRelease, newerVersion, sameVersion, updateOnTrial, updateState } from "./update";

export function clientUpdateRequired(app: unknown, protocol: unknown, desktop: boolean) {
  const choice = releaseChoice();
  if (!choice?.clientProtocol) return null;
  const compatible = desktop ? typeof app === "string" && !newerVersion(choice.version, app)
    : typeof protocol === "number" && Number.isSafeInteger(protocol) && protocol >= choice.clientProtocol;
  if (compatible) return null;
  return {
    reason: "client-update-required" as const,
    title: "Update Kuvo Code",
    message: `Update Kuvo Code on this device to connect to your setup on ${choice.version}.`,
    target: choice.version,
  };
}


// Keep the routes a running turn can need while fencing new work on an older computer.
const reachable = new Set([
  "kuvo/info", "kuvo/computers", "kuvo/computers/events", "kuvo/update", "kuvo/update/retry", "kuvo/update/dismiss",
  "kuvo/box/app", "kuvo/box/update", "kuvo/box/proof", "kuvo/box/at-work", "kuvo/devices/pair", "kuvo/agent-tools",
]);
export function computerUpdateRequired(route: string, method = "GET") {
  const choice = releaseChoice();
  const trial = updateOnTrial();
  const current = trial ? updateState().trial?.from ?? boxVersion() : boxVersion();
  if (!choice?.clientProtocol || (!needsRelease(choice, current) && !(trial && sameVersion(choice.version, current))) || reachable.has(route)
    || /^kuvo\/(?:asks|(?:sandbox|vault|work)\/asks|sessions\/[^/]+\/(?:asks|decisions|stop))(?:\/|$)/.test(route)
    || /^(?:kuvo\/agents\/)?api\/(?:permission|question)(?:\/|$)/.test(route)
    || /^(?:kuvo\/agents\/)?api\/session\/[^/]+\/(?:permission|question|abort)(?:\/|$)/.test(route)
    || (method === "GET" && /^(?:(?:kuvo\/agents\/)?api\/(?:event|session\/[^/]+\/(?:message|status))|kuvo\/sessions\/[^/]+\/(?:chat|events))(?:\/|$)/.test(route))) return null;
  return { reason: "computer-update-required", message: `This computer needs to update to ${choice.version} before it can be used.`, current, target: choice.version };
}

// A pre-fleet peer does not apply the owner's admission check when it receives relay mail.
export function legacyPeerUpdateRequired(peer: string) {
  const choice = releaseChoice();
  if (!choice?.clientProtocol) return null;
  const facts = readPeerAgents()[peer];
  if (!facts || facts.features?.includes("update.fleet")) return null;
  const computer = facts.computer;
  const current = computer?.kuvo ?? computer?.version ?? null;
  if (current && !newerVersion(choice.version, current)
    && !(sameVersion(choice.version, current) && computer?.installedSha256 !== choice.sha256)) return null;
  return { reason: "computer-update-required", message: `This computer needs to update to ${choice.version} before it can be used.`, current, target: choice.version };
}
