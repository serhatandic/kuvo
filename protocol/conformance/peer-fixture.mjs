// Fixed throwaway identities and a signed layout-23 setup, usable by either resident.
import { createHash, createPrivateKey, createPublicKey, sign } from "node:crypto";
import path from "node:path";
import { relayAddress, write } from "./adapter.mjs";

function identity(seed, prefix) {
  const key = createPrivateKey({ key: Buffer.concat([Buffer.from(prefix, "hex"), seed]), format: "der", type: "pkcs8" });
  const pub = createPublicKey(key).export({ format: "der", type: "spki" }).subarray(-32).toString("base64url");
  return { key, pub };
}

export function seedPeerSetup(home, relay, own = 1, count = 2, retired, additional = {}) {
  const bytes = (value, size = 32) => Buffer.alloc(size, value);
  const computers = Array.from({ length: count }, (_, index) => index + 1).map(number => {
    const seed = bytes(number);
    const secret = bytes(number + 50);
    const noise = identity(seed, "302e020100300506032b656e04220420");
    const signer = identity(seed, "302e020100300506032b657004220420");
    const row = { key: noise.pub, address: relayAddress(secret).toString("base64url"),
      name: ["Studio", "Lab", "Spare"][number - 1], kind: "machine", sign: signer.pub };
    const signed = (kind, body) => {
      const document = Buffer.from(JSON.stringify({ v: 1, kind, origin: signer.pub, n: 1, body }));
      const signature = sign(null, Buffer.concat([Buffer.from("kuvo-link doc v1\0"), document]), signer.key);
      return { d: document.toString("base64url"), s: signature.toString("base64url") };
    };
    return { row, seed: seed.toString("base64url"), secret: secret.toString("base64url"), signed };
  });
  const [main, member] = computers;
  const here = computers[own - 1];
  const id = "set_" + bytes(1, 16).toString("base64url");
  const setup = { id, epoch: 1, primary: main.row.key };
  const shared = { ...additional, membershipAndAccess: { sequence: 1, value: {
    boxes: computers.map(({ row }) => row), left: retired ? [retired] : [], moved: [], devices: { phones: [], removed: [] }, removalRevisions: {},
  } } };
  const checkpoint = main.signed("setup-checkpoint", { id, shared, archives: [] });
  const hash = createHash("sha256").update(checkpoint.d).digest("base64url");
  const save = (file, value) => write(path.join(home, file), JSON.stringify(value));
  save("folder-version.json", { version: 23 });
  save("run/setup-checkpoints/" + hash + ".json", checkpoint);
  save("run/link.json", {
    formatVersion: 1, key: here.seed, sign: here.seed, name: here.row.name, secret: here.secret, phones: [], relay,
    setup: { ...setup, managed: true, checkpointed: true, role: own === 1 ? "primary" : "member", sign: main.row.sign },
    sharedSetup: id, shared, leadership: { 1: main.signed("leadership", { setup, relayTerm: 1, checkpoint: hash }) },
  });
  return { main: main.row, member: member.row, spare: computers[2]?.row, save };
}

export function seedAwaySession(home, relay) {
  const { member, save } = seedPeerSetup(home, relay);
  const seen = 1_700_000_000_000;
  const catalog = { agents: [], offered: { known: { engine: false, agents: false }, models: [] } };
  const facts = { kind: "chat", directory: "/scratch", title: "Peer session", created: 10, updated: 20 };
  save("run/peer-agents.json", { [member.key]: { at: seen, heard: seen, list: [], kind: member.kind } });
  save("run/peer-reach.json", { [member.key]: seen });
  save("run/peer-reach-checked.json", { [member.key]: Date.now() });
  save("run/peers/" + member.key + "/lists-v1.json", {
    at: seen, seen, sessions: [{ id: "ses_far", facts, actions: ["open", "manage"] }], projects: [], queues: [], waiting: [], catalog,
  });
  return { peer: { id: member.key, name: member.name }, seen };
}
