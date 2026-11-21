// The bridge bench controls relay time and connectivity without touching a deployed relay.
// Mirrors the relay's `/setup-lease` (kuvo-link-relay, `src/leases.ts`): claim, renew, read and transfer.
// `transferStartsNow`: a hand-over's new grant starts at once, where the relay starts it when the
// old one ends (a bench that isn't about that minute skips it).
export function relayLeaseStandIn({ transferStartsNow = false } = {}) {
  let lease;
  let offset = 0;
  const now = () => Date.now() + offset;
  let graceUntil = now() + 120_000;
  const offline = new Set();
  const calls = [];
  return {
    calls,
    get lease() { return lease; },
    advance(ms) { offset += ms; },
    restart() { graceUntil = now() + 120_000; },
    disconnect(computer) {
      offline.add(computer);
      return () => offline.delete(computer);
    },
    async request(request, response) {
      if (request.url !== "/setup-lease") return response.writeHead(404).end();
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      const { data } = JSON.parse(Buffer.concat(chunks).toString());
      const { action, computer, sign, term, high, roster, alwaysOn, bootstrap, target } = JSON.parse(Buffer.from(data, "base64url"));
      calls.push({ action, computer, term });
      if (offline.has(computer)) return response.writeHead(503).end();
      const at = now();
      const takeoverAt = () => Math.max(graceUntil, (lease?.expires ?? 0) + (alwaysOn ? 0 : 60_000));
      if (!lease && action === "claim" && bootstrap ||
          lease && action === "claim" && term === lease.term && at >= takeoverAt()) {
        lease = { holder: computer, sign, term: (lease?.term ?? 0) + 1, starts: at, expires: at + 60_000, high: Math.max(high, lease?.high ?? 0), roster };
      } else if (action === "renew" && lease?.holder === computer && term === lease.term && roster?.[computer] === sign &&
          (at < lease.expires || at < graceUntil)) {
        lease = { ...lease, expires: Math.max(at, lease.starts) + 60_000, high: Math.max(high, lease.high), roster };
      } else if (action === "transfer" && lease?.holder === computer && term === lease.term && target !== computer && roster?.[target] &&
          at >= lease.starts && at < lease.expires) {
        // The departing holder may still have a cached grant, so the new one starts when it ends.
        const starts = transferStartsNow ? at : lease.expires;
        lease = { holder: target, sign: roster[target], term: lease.term + 1, starts, expires: starts + 60_000, high: Math.max(high, lease.high), roster };
      }
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(lease
        ? { lease: { holder: lease.holder, sign: lease.sign, term: lease.term, expires: lease.expires, high: lease.high },
          remaining: Math.max(0, lease.expires - at), startsIn: Math.max(0, lease.starts - at), takeoverIn: Math.max(0, takeoverAt() - at), relayHost: true }
        : { lease: null }));
    },
  };
}
