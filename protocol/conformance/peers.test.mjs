// Restart with a kept peer session and no connection to its owner.
import assert from "node:assert/strict";
import { test } from "node:test";
import { until } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { seedAwaySession, seedPeerSetup } from "./peer-fixture.mjs";

test("an away computer's last session keeps its owner and offers no actions", { timeout: 20000 }, async t => {
  const { computer, relay } = await bench(t);
  const near = await computer("Studio");
  const { peer, seen } = seedAwaySession(near.home, relay.url);
  const request = near.connect(await near.start().ready());
  const found = await request("GET", "kuvo/sessions/ses_far/location");
  assert.equal(found.status, 200, found.text);
  assert.deepEqual(found.json, { on: { ...peer, away: true, lastSeen: seen, actions: [] } });
  const waiting = await request("GET", "kuvo/setup/waiting/counts");
  assert.equal(waiting.status, 200, waiting.text);
  assert.equal(waiting.json.dock, 0);
  assert.deepEqual(waiting.json.pendingComputers, []);
});

test("two enrolled computers find each other through the relay", { timeout: 30000 }, async t => {
  const { computer, relay } = await bench(t);
  const studio = await computer("Studio");
  const lab = await computer("Lab");
  const { main, member } = seedPeerSetup(studio.home, relay.url);
  seedPeerSetup(lab.home, relay.url, 2);
  const [near, far] = await Promise.all([studio.start().ready(), lab.start().ready()]);
  await Promise.all([
    [studio.connect(near), main, member],
    [lab.connect(far), member, main],
  ].map(async ([request, own, peer]) => {
    const computers = await until(async () => {
      const answer = await request("GET", "kuvo/computers");
      assert.equal(answer.status, 200, answer.text);
      return answer.json.computers.find(row => row.key === peer.key)?.reach === "connected" && answer.json.computers;
    }, `${own.name} did not hear from ${peer.name}`, 20000);
    assert.deepEqual(computers.map(({ name, key, reach, via, address, working }) => ({ name, key, reach, via, address, working })), [
      { name: own.name, key: own.key, reach: "connected", via: "here", address: own.address, working: 0 },
      { name: peer.name, key: peer.key, reach: "connected", via: "relay", address: peer.address, working: 0 },
    ]);
  }));
});
