import "./safety";
import { expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { configApplyAct, netChanges } from "../src/link/areas/apply-from";
import { APPLY_FOLDER, prepareApply } from "../src/link/areas/apply-journal";
import { CONFIG_FILE, changeSettings, readSettings } from "../src/link/areas/config";
import { CATALOG_FILE } from "../src/link/areas/mcp-catalog";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, signingKeyOf } from "../src/link/documents/format";
import { takeAgents } from "../src/link/peers/directory";
import { openMail, type Peer } from "../src/link/peers/known";
import { readFrame } from "../src/link/relay/frames";
import { engineHandler } from "../src/link/server/router";
import { newSetupID } from "../src/link/setup/record";
import { writeLink, type LinkState } from "../src/link/state";
import { takeWorkQueueMail } from "../src/link/work-queue/across";
import { forwardsSoon, sendForwards, type Act } from "../src/link/work-queue/forward";

function computer(name: string) {
  const secret = newKey();
  const sign = newSigningSeed();
  const peer: Peer = {
    key: publicOf(secret).toString("base64url"), address: addressOf(secret).toString("base64url"),
    sign: signingKeyOf(sign), name, kind: "machine",
  };
  return { peer, key: secret.toString("base64url"), sign };
}

function fixture() {
  const source = computer("Source"), destination = computer("Destination");
  const setup = newSetupID(), epoch = 1;
  const common: Pick<LinkState, "shared" | "sharedSetup"> = {
    sharedSetup: setup, shared: { membershipAndAccess: { sequence: 1, value: { boxes: [source.peer, destination.peer] } } },
  };
  const sourceLink: LinkState = { ...common, key: source.key, sign: source.sign, setup: {
    id: setup, epoch, primary: source.peer.key, sign: source.peer.sign!, role: "primary", checkpointed: true,
  } };
  const destinationLink: LinkState = { ...common, key: destination.key, sign: destination.sign,
    setup: { ...sourceLink.setup!, role: "member" },
  };
  return { source, destination, sourceLink, destinationLink, setup, epoch };
}

const handler = engineHandler(() => { throw new Error("no engine"); }, { name: "Destination" }, () => { throw new Error("no agents"); });
async function ask(method: string, route: string, body?: unknown) {
  let status = 0, value: any;
  for await (const part of handler({ method, path: route, headers: {}, served: true, fromApp: true,
    body: body === undefined ? undefined : Buffer.from(JSON.stringify(body)) }, undefined, AbortSignal.timeout(1800))) {
    if ("head" in part) status = part.head.status;
    else value = JSON.parse(part.body.toString());
  }
  return { status, value };
}

function receive(bytes: Buffer, link: LinkState) {
  const mail = readFrame(bytes.subarray(17));
  const opened = mail && openMail(mail.body, link);
  if (!opened) throw new Error("Unopenable peer mail");
  return opened;
}

test("source receipts net in write order and refuse gaps and overlapping paths", () => {
  const model = ["model"];
  expect(netChanges([[{ path: model, before: "old", after: "one" }], [{ path: model, before: "one", after: "two" }]]))
    .toEqual([{ path: model, before: "old", after: "two" }]);
  expect(netChanges([[{ path: model, before: "old", after: "one" }], [{ path: model, before: "gap", after: "two" }]])).toBeUndefined();
  expect(netChanges([[{ path: ["mcp"], after: {} }], [{ path: ["mcp", "servers"], after: {} }]])).toBeUndefined();
  expect(netChanges([[{ path: model, before: "old", after: "one" }], [{ path: model, before: "one", after: "old" }]])).toEqual([]);
});

test("a destination previews authenticated source edits and keeps the exact result across retries", async () => {
  const { source, destination, sourceLink, destinationLink, setup, epoch } = fixture();
  fs.mkdirSync(path.dirname(CONFIG_FILE), { recursive: true });
  fs.writeFileSync(CONFIG_FILE, "{}");
  fs.mkdirSync(path.dirname(CATALOG_FILE), { recursive: true });
  fs.writeFileSync(CATALOG_FILE, JSON.stringify({ macOnly: ["mac-app"], servers: [
    { id: "popular", settings: { type: "remote", url: "https://example.test/mcp" } },
  ] }));
  writeLink(sourceLink);
  const edit = (await ask("PATCH", "kuvo/config?receipt=1", { changes: [
    { path: ["model"], value: "test/model" },
    { path: ["shell"], value: "bash" },
    { path: ["agents", "missing", "model"], value: "test/missing" },
    { path: ["mcp", "servers", "mac-app"], value: { type: "local", command: ["test"] } },
    { path: ["mcp", "servers", "custom"], value: { type: "remote", headers: { Authorization: "fixture-literal-secret" } } },
    { path: ["mcp", "servers", "with-key"], value: { headers: { Authorization: "{file:/other/KUVO_KEY_TEST}" } } },
  ] })).value;
  const catalog = (await ask("POST", "kuvo/mcp/catalog/popular?receipt=1")).value;
  const batch = await ask("POST", "kuvo/config/change-sets", { receipts: [...catalog.configChanges, ...edit.configChanges] });
  expect(batch.status).toBe(201);
  const sourceBody = { setup, epoch, source: source.peer.key, destination: destination.peer.key };
  const act = { id: "ignored", at: Date.now(), item: batch.value.changeSet, method: "GET", area: "configApply", body: sourceBody } satisfies Act;
  expect(configApplyAct(computer("Stranger").peer, act).status).toBe(503);
  expect(configApplyAct(destination.peer, { ...act, body: { ...sourceBody, epoch: epoch + 1 } }).status).toBe(503);

  const original = { shell: "fish", model: "local/model", unrelated: 42 };
  fs.writeFileSync(CONFIG_FILE, JSON.stringify(original));
  writeLink(destinationLink);
  const dry = { from: { computer: source.peer.key, changeSet: batch.value.changeSet }, dry: true };
  const apply = (body: unknown) => ask("POST", "kuvo/config/apply", body);
  expect(await apply(dry)).toEqual({ status: 409, value: { status: "unsupported", computer: source.peer.key } });
  takeAgents(source.peer, { at: Date.now(), list: [], features: ["config.apply.from"] });

  const before = forwardsSoon.send;
  let exchange = Promise.resolve();
  forwardsSoon.send = () => {
    sendForwards({ welcomed: true, send(bytes) {
      const opened = receive(bytes, sourceLink);
      exchange = exchange.then(async () => {
        writeLink(sourceLink);
        try {
          await takeWorkQueueMail(opened.sender, opened.payload.workQueue, { send(answer) {
            const opened = receive(answer, destinationLink);
            writeLink(destinationLink);
            return void takeWorkQueueMail(opened.sender, opened.payload.workQueue, null, () => ({ targets: [], bridge: false }));
          } }, () => ({ targets: [], bridge: false }));
        } finally { writeLink(destinationLink); }
      });
    } }, destinationLink, Date.now(), { ownOnly: true });
  };

  try {
    const preview = await apply(dry);
    expect(preview.status).toBe(200);
    expect(preview.value).toMatchObject({
      changes: [
        { path: ["model"], action: "set" },
        { path: ["mcp", "servers", "custom"], action: "add" },
        { path: ["mcp", "servers", "popular"], action: "add" },
      ],
      kept: [["shell"]], leftOut: [["agents", "missing"]], macOnly: ["mac-app"],
      skipped: [{ path: ["mcp", "servers", "with-key"], reason: "missingKeys", keys: ["KUVO_KEY_TEST"] }],
    });
    expect(JSON.stringify(preview.value)).not.toContain("fixture-literal-secret");
    expect(readSettings().settings).toEqual(original);

    const confirmation = { plan: preview.value.plan, approval: preview.value.approval };
    expect((await apply({ ...confirmation, approval: "incorrect" })).value.status).toBe("freshPreview");
    const [applied, replay] = await Promise.all([apply(confirmation), apply(confirmation)]);
    expect(applied).toEqual(replay);
    expect(applied.value.status).toBe("applied");
    expect(readSettings().settings).toMatchObject({ model: "test/model", unrelated: 42, shell: "fish",
      mcp: { servers: { custom: { headers: { Authorization: "fixture-literal-secret" } }, popular: { type: "remote" } } } });
    expect(await ask("GET", `kuvo/config/apply/${confirmation.plan}`)).toEqual(applied);

    const second = (await apply(dry)).value;
    expect(changeSettings([{ path: ["unrelated"], value: 43 }]).status).toBe(200);
    expect((await apply({ plan: second.plan, approval: second.approval })).value.status).toBe("freshPreview");
    const removed = { ...destinationLink, shared: { membershipAndAccess: { sequence: 2, value: { boxes: [destination.peer] } } } };
    writeLink(removed);
    expect((await apply(dry)).status).toBe(503);
    expect(await ask("GET", `kuvo/config/apply/${confirmation.plan}`)).toEqual(applied);
  } finally {
    forwardsSoon.send = before;
    await exchange;
  }
});

test("a subsequent settings writer settles a publication interrupted before its result", async () => {
  const plan = "a".repeat(32);
  fs.mkdirSync(path.dirname(CONFIG_FILE), { recursive: true });
  const staged = CONFIG_FILE + ".fixture-staged";
  fs.writeFileSync(staged, JSON.stringify({ model: "published/model" }));
  const expected = { plan, status: "applied", revision: "published" } as const;
  prepareApply(CONFIG_FILE, CONFIG_FILE, staged, expected);
  fs.renameSync(staged, CONFIG_FILE);
  expect(changeSettings([{ path: ["model"], value: "newer/model" }]).status).toBe(200);
  expect(await ask("GET", `kuvo/config/apply/${plan}`)).toEqual({ status: 200, value: expected });
  expect(readSettings().settings).toEqual({ model: "newer/model" });
  expect(fs.existsSync(path.join(APPLY_FOLDER, "pending.json"))).toBe(false);
});

test("a staging-only interruption requires another preview and leaves settings alone", async () => {
  const plan = "b".repeat(32);
  const before = readSettings().settings;
  const staged = CONFIG_FILE + ".fixture-staging-only";
  fs.writeFileSync(staged, JSON.stringify({ model: "unpublished/model" }));
  prepareApply(CONFIG_FILE, CONFIG_FILE, staged, { plan, status: "applied", revision: "unpublished" });
  expect(await ask("GET", `kuvo/config/apply/${plan}`)).toEqual({ status: 200, value: { plan, status: "freshPreview" } });
  expect(readSettings().settings).toEqual(before);
  expect(fs.existsSync(staged)).toBe(false);
});
