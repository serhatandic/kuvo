// Kuvo Link's relay. Settings come from the environment:
//   PORT, HOST                    where to listen (8790, 0.0.0.0)
//   DATA_DIR                      where push registrations and setup leases are kept (./data)
//   LEASE_RELAY_HOST_IPS          comma-separated addresses boxes on this host connect from
//   TRUST_PROXY=1                 behind a proxy on this host: count connections by X-Forwarded-For
//   APNS_KEY_ID, APNS_TEAM_ID     the APNs key's ID and the Apple Developer team
//   APNS_KEY or APNS_KEY_FILE     the key itself (.p8 contents) or its path
// Without an APNs key it relays as usual and answers pushes with "no phone".
import { readFileSync } from 'node:fs';
import { apnsSender } from './apns.ts';
import { createRelay } from './relay.ts';
import { SetupLeases } from './leases.ts';
import { Registrations } from './registrations.ts';

const env = process.env;
const pem = env.APNS_KEY ?? (env.APNS_KEY_FILE ? readFileSync(env.APNS_KEY_FILE, 'utf8') : undefined);
const push = pem && env.APNS_KEY_ID && env.APNS_TEAM_ID ? apnsSender({ keyID: env.APNS_KEY_ID, teamID: env.APNS_TEAM_ID, pem }) : undefined;

const data = env.DATA_DIR ?? './data';
const registrations = new Registrations(data);
const leases = new SetupLeases(data);
const leaseRelayHostIPs = env.LEASE_RELAY_HOST_IPS?.split(',').map(ip => ip.trim()).filter(Boolean);
const relay = createRelay({ registrations, leases, leaseRelayHostIPs, push, trustProxy: env.TRUST_PROXY === '1', log: line => console.log(line) });
const port = Number(env.PORT ?? 8790);
relay.server.listen(port, env.HOST ?? '0.0.0.0', () => {
  console.log(`Kuvo Link relay on ${port}${push ? ', with pushes' : ', without pushes (no APNs key)'}`);
});

// Counts only: who talks to whom stays out of the logs. With how the hour's pushes went (sent, no
// phone registered there, not one of its senders, refused by Apple, and Apple's reasons).
setInterval(() => {
  const pushes = relay.pushCounts();
  const reasons = Object.entries(pushes.reasons).map(([reason, count]) => `${reason} ${count}`).join(', ');
  console.log(`${relay.connections()} connected, ${registrations.size} phones registered for pushes; pushes this hour: ${pushes.sent} sent, ${pushes.noPhone} no phone, ${pushes.notAllowed} not allowed, ${pushes.refused} refused${reasons ? ` (${reasons})` : ''}`);
}, 60 * 60_000).unref();

for (const signal of ['SIGTERM', 'SIGINT'] as const) {
  process.on(signal, async () => {
    registrations.flush();
    await relay.close();
    process.exit(0);
  });
}
