// The bridge bench controls relay time and connectivity without touching a deployed relay.
export function relayLeaseStandIn() {
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
      const { action, computer, sign, term, high, roster, alwaysOn, bootstrap } = JSON.parse(Buffer.from(data, "base64url"));
      calls.push({ action, computer, term });
      if (offline.has(computer)) return response.writeHead(503).end();
      const at = now();
      const takeoverAt = () => Math.max(graceUntil, (lease?.expires ?? 0) + (alwaysOn ? 0 : 60_000));
      if (!lease && action === "claim" && bootstrap ||
          lease && action === "claim" && term === lease.term && at >= takeoverAt()) {
        lease = { holder: computer, sign, term: (lease?.term ?? 0) + 1, expires: at + 60_000, high: Math.max(high, lease?.high ?? 0) };
      } else if (action === "renew" && lease?.holder === computer && term === lease.term && roster?.[computer] === sign &&
          (at < lease.expires || at < graceUntil)) {
        lease = { ...lease, expires: at + 60_000, high: Math.max(high, lease.high) };
      }
      response.writeHead(200, { "content-type": "application/json" }).end(JSON.stringify(lease
        ? { lease, remaining: Math.max(0, lease.expires - at), startsIn: 0, takeoverIn: Math.max(0, takeoverAt() - at), relayHost: true }
        : { lease: null }));
    },
  };
}
