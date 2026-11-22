import "./safety"
import { expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { stageRestore } from "../src/link/backup/stage";
import { storageInventory } from "../src/link/backup/locations";
import { KUVO_FOLDER_LAYOUT, upgradeKuvoFolder } from "../src/shared/folder-version";
import { forwardDecisionKey, forwardOriginKey, checkForwardEvidence, packForwardEvidence, placeForwardEvidence } from "../src/link/ownership/forward-evidence";
import { removeRecord } from "../src/link/ownership/store";
import { K } from "../src/link/home";
import { pendingForward, runForwardDecision } from "../src/link/work-queue/forward";
import type { ForwardEvidence } from "../src/link/ownership/forward-evidence";

const setup = "set_" + "a".repeat(22);
const owner = "b".repeat(43);
const replacement = "c".repeat(43);
const submitter = { kind: "paired", key: "d".repeat(43), pairing: "test" } as const;
const name = (key: string) => `home/run/owned-data/mutations/${key}.json`;

test("capture and restore distinguish old shadows, primary decisions and opaque legacy evidence", () => {
  const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-forward-evidence-"));
  try {
    const home = path.join(scratch, "home");
    const directory = path.join(home, "run", "owned-data");
    const write = (family: string, key: string, fields: object) => {
      const dir = path.join(directory, family);
      fs.mkdirSync(dir, { recursive: true });
      const file = path.join(dir, key + ".json");
      fs.writeFileSync(file, JSON.stringify(fields));
      return file;
    };
    const origin = forwardOriginKey(setup, "listed-rename");
    const digest = "x".repeat(43);
    const originFields = { setup, owner, operation: "listed-rename", submitter, area: "computers", record: replacement,
      kind: "forwardedOperationOrigins" } satisfies ForwardEvidence;
    write("mutations", origin, { digest });
    write("forward-origins", origin, { formatVersion: 1, digest });
    const legacy = forwardDecisionKey(setup, "legacy");
    write("mutations", legacy, { digest });
    const decision = forwardDecisionKey(setup, "accepted-rename");
    const fields = { ...originFields, kind: "forwardedPrimaryDecisions", operation: "accepted-rename", ref: { kind: "setupNames", id: replacement } };
    write("mutations", decision, { formatVersion: 1, digest, forward: fields, answer: { status: 200, value: { name: "Lab" } }, at: 123 });
    const inventory = () => [...storageInventory({ home })];
    expect(inventory().map(({ name, kind }) => [name, kind]).sort()).toEqual([
      [name(decision), "forwardedPrimaryDecisions"], [name(legacy), "legacyMutationEvidence"],
    ].sort());
    write("mutations", origin, { digest, forward: originFields });
    const prior = inventory();
    expect(prior.map(entry => entry.name)).not.toContain(name(origin));
    const stage = stageRestore(path.join(scratch, "staged"), KUVO_FOLDER_LAYOUT, prior.map(entry => ({ name: entry.name, file: entry.file! })));
    expect([...stage.entries.keys()].sort()).toEqual([name(decision), name(legacy)].sort());
    const future = write("mutations", decision, { formatVersion: 90, digest, forward: fields });
    const bytes = fs.readFileSync(future, "utf8");
    expect(() => stageRestore(path.join(scratch, "future"), KUVO_FOLDER_LAYOUT, inventory().map(entry => ({ name: entry.name, file: entry.file! }))))
      .toThrow("format version 90");
    expect(fs.readFileSync(future, "utf8")).toBe(bytes);
  } finally {
    fs.rmSync(scratch, { recursive: true, force: true });
  }
});

test("a moved queue decision answers at a new owner without admitting another change", async () => {
  upgradeKuvoFolder(K);
  const grant = { setup, submitter, operation: "moved-queue-patch" };
  const item = "wqi_" + "a".repeat(20);
  const queue = "ses_forwardReceiptFixture";
  const ref = { kind: "queueItems" as const, id: item, queue };
  const intent = { method: "PATCH" as const, item, body: { title: "First" } };
  const key = forwardDecisionKey(setup, grant.operation);
  const scope = { sessions: [queue], items: new Set([item]) };
  let effects = 0;
  const effect = async () => ({ status: 200, value: { effects: ++effects } });
  try {
    const accepted = await runForwardDecision(intent, grant, owner, ref, () => null, effect);
    const payload = packForwardEvidence(setup, scope);
    expect(payload?.[key]?.forward).toMatchObject({ owner, setup, ref, kind: "forwardedQueueDecisions", submitter });
    expect(packForwardEvidence(setup, { sessions: [], items: new Set() })).toBeUndefined();
    removeRecord("mutations", key);
    expect(() => checkForwardEvidence(payload, "set_" + "z".repeat(22), scope)).toThrow("another record");
    const carried = checkForwardEvidence(payload, setup, scope);
    placeForwardEvidence(carried);
    placeForwardEvidence(carried);
    expect(await runForwardDecision(intent, grant, replacement, ref, () => null, effect)).toEqual(accepted);
    expect((await runForwardDecision({ ...intent, body: { title: "Changed" } }, grant, replacement, ref, () => null, effect)).status).toBe(409);
    expect(effects).toBe(1);
    const future = { digest: "x".repeat(43), formatVersion: 90 };
    const file = path.join(K, "run", "owned-data", "mutations", key + ".json");
    fs.writeFileSync(file, JSON.stringify(future));
    await expect(runForwardDecision(intent, grant, replacement, ref, () => null, effect)).rejects.toThrow("format version 90");
    expect(JSON.parse(fs.readFileSync(file, "utf8"))).toEqual(future);
    expect(effects).toBe(1);
  } finally {
    removeRecord("mutations", key);
  }
});

test("future and ambiguous forwarded spools stay pending for removal", () => {
  const act = { id: "wqa_" + "a".repeat(20), at: 1, item: "thing", method: "PATCH" };
  expect(pendingForward({ act }, 2)).toBe(true);
  expect(pendingForward({ act: { ...act, formatVersion: 90 } }, 999_000)).toBe(true);
  expect(pendingForward({ act, scopedAct: null }, 999_000)).toBe(true);
  expect(pendingForward({ scopedAct: act, grant: { formatVersion: 90 } }, 999_000)).toBe(true);
});

test("a scoped draft's legacy replay fence follows the session while the outcome stays with its original owner", async () => {
  upgradeKuvoFolder(K);
  const { draftKey } = await import("../src/link/ownership/forward-evidence");
  const { runMutation } = await import("../src/link/ownership/mutation");
  const { readRecord } = await import("../src/link/ownership/store");
  const { submissionKey } = await import("../src/link/ownership/submission-address");
  const { pack, unpack } = await import("../src/link/moves/bundle");
  const { bundleOf } = await import("../src/link/moves/records");
  const move = "mov_draftFenceFixture";
  const session = "ses_draftFenceFixture";
  const id = "msg_moveddraft";
  const submission = { setup, session, id: `${setup}.${owner}.${id}`, origin: owner, submittedThrough: replacement, submitter };
  const key = submissionKey(setup, session, submission.id);
  const legacy = draftKey(setup, session, id);
  const scope = { sessions: [session], items: new Set<string>() };
  let effects = 0;
  const perform = async () => ({ status: 200, value: { effects: ++effects, message: id } });
  try {
    expect((await runMutation(key, { id }, perform, undefined, undefined, submission, legacy)).status).toBe(200);
    const payload = packForwardEvidence(setup, scope);
    expect(Object.keys(payload ?? {})).toEqual([legacy]);
    expect(readRecord("submissions", key)).toMatchObject({ submission, answer: { status: 200 } });
    removeRecord("mutations", legacy);
    await pack(move, { kind: "chat", thing: session, name: "Fixture", exports: [{ info: { id: session }, messages: [] }], wasPaused: false, forwardEvidence: payload });
    const { contents } = await unpack(move);
    placeForwardEvidence(checkForwardEvidence(contents.forwardEvidence, setup, scope));
    expect((await runMutation(legacy, { id }, perform)).status).toBe(409);
    expect(effects).toBe(1);
  } finally {
    removeRecord("mutations", legacy);
    removeRecord("submissions", key);
    fs.rmSync(bundleOf(move), { recursive: true, force: true });
  }
});
