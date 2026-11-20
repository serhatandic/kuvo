import { ledSetup } from "./record";
import { macDoc, macsOf } from "../peers/known";
import { macSign, type LinkState } from "../state";

export interface DeviceRevocation {
  key: string;
  pairing: string;
}

// A setup-less reader can identify its sole SSH enroller, but not a setup ID.
export function deviceRevocationScope(link: LinkState): string | undefined {
  if (link.setup) return link.setup.id;
  if (link.macs && link.macs.length > 1) return undefined;
  const mac = link.mac ?? link.macs?.[0];
  return mac ? link.macSigns?.[mac] ?? mac : undefined;
}

export function archiveDeviceRevocations(link: LinkState, scope = deviceRevocationScope(link) ?? "unattributed"): void {
  const setup = link.setup;
  if (setup && scope === setup.id) retirePhoneIssuer(link, setup.sign, scope);
  if (!link.removed?.length) return;
  rememberDeviceRevocations(link, scope, link.removed);
  delete link.removed;
}

export function rememberDeviceRevocations(link: LinkState, scope: string, incoming: DeviceRevocation[]): void {
  link.formerDeviceRevocations ??= {};
  const retained = [...link.formerDeviceRevocations[scope] ?? [], ...incoming];
  link.formerDeviceRevocations[scope] = [...new Map(retained.map(entry => [JSON.stringify([entry.key, entry.pairing]), entry])).values()];
}

export function retirePhoneIssuer(link: LinkState, origin: string, scope: string): void {
  const issuers = link.formerPhoneIssuers ??= {};
  issuers[origin] = [...new Set([...issuers[origin] ?? [], scope])];
}

export function phoneDocumentSetup(link: LinkState, origin: string): string | undefined {
  const mac = !link.setup?.managed && macsOf(link).find(key => macSign(link, key) === origin);
  const offered = mac ? ledSetup(macDoc(link, mac, "peers")) : undefined;
  return (offered || link.setup)?.id;
}

export function takesPhoneScope(link: LinkState, origin: string, given: unknown, scope: string | undefined): boolean {
  if (given !== undefined) return given === scope;
  // An untagged list from a signer known to have led a different setup
  // cannot establish which setup issued the device grant.
  return !(link.setup?.sign === origin && link.setup.id !== scope)
    && !link.formerPhoneIssuers?.[origin]?.some(id => id !== scope);
}

export function historicallyRevoked(link: LinkState, key: string, pairing: string, origin?: string | null): boolean {
  const history = link.formerDeviceRevocations;
  if (!history) return false;
  const scope = origin && phoneDocumentSetup(link, origin) || deviceRevocationScope(link);
  // Old readers could leave a list without any provenance. Keep those exact pairing
  // denials locally. Never publish them as revocations issued by the new setup.
  const enroller = origin ?? (link.setup?.role === "member" ? link.setup.sign : undefined);
  return [scope, enroller, "unattributed"].some(each => each && history[each]?.some(entry => entry.key === key && entry.pairing === pairing));
}
