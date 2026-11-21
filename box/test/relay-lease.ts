import path from "node:path";
import { RUN } from "../src/link/home";
import { leaseClock, rememberRelayLease } from "../src/link/setup/lease";
import type { LinkState } from "../src/link/state";

export const leaseFile = path.join(RUN, "setup-relay-lease.json");

export function grantRelayLease(link: LinkState) {
  const setup = link.setup!;
  const since = leaseClock();
  const duration = 60_000;
  rememberRelayLease({
    setup: setup.id, holder: setup.primary, sign: setup.sign, term: setup.epoch,
    high: setup.numberCeiling ?? 0, relayHost: false, since, validAfter: since, duration, expires: since.wall + duration,
  });
}
