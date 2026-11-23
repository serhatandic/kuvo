import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { link, until } from "./setup-handover-fixture.js";

const waitForWipe = async (box, signal, timeout = 12_000) => {
  try {
    await until("box erased", () => !fs.existsSync(box.home), AbortSignal.any([signal, AbortSignal.timeout(timeout)]));
  } catch (error) {
    const log = path.join(box.run, "uninstall.log");
    throw new Error(`${error.message} on ${box.name}: journal=${JSON.stringify(box.read().localWipe)}, log=${fs.existsSync(log) ? fs.readFileSync(log, "utf8") : "(none)"}`, { cause: error });
  }
};
const removalOf = (box, peer) => box.read().shared?.membershipAndAccess?.value.removals?.[peer.key];

function seedLocalWork(box, { botWork = true } = {}) {
  const bot = path.join(box.home, "bots", "bot_retained", "notes.md");
  const session = path.join(box.home, "agents", "sessions", "ses_retained", "messages.json");
  const files = botWork ? [bot, session] : [session];
  for (const file of files) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, "local work");
  }
  return files;
}

function separateExportRoot(t, source) {
  if (process.platform === "darwin") {
    const root = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-export-"));
    const image = path.join(root, "archive.dmg");
    const mount = path.join(root, "mounted");
    let attached = false;
    const diskImage = args => {
      const ran = spawnSync("/usr/bin/hdiutil", args, { encoding: "utf8", timeout: 20_000 });
      assert.equal(ran.status, 0, ran.error?.message || ran.stderr || String(ran.signal));
    };
    t.after(() => {
      if (attached) diskImage(["detach", mount]);
      fs.rmSync(root, { recursive: true, force: true });
    });
    fs.mkdirSync(mount);
    // APFS avoids hdiutil's deprecated HFS+ creation path.
    diskImage(["create", "-size", "64m", "-fs", "APFS", "-volname", "Kuvo Export Fixture", image]);
    diskImage(["attach", "-nobrowse", "-noautoopen", "-owners", "off", "-mountpoint", mount, image]);
    attached = true;
    assert.notEqual(fs.statSync(mount).dev, fs.statSync(source.home).dev);
    return fs.realpathSync(mount);
  }
  const root = fs.mkdtempSync("/dev/shm/kuvo-removal-fixture-");
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  assert.notEqual(fs.statSync(root).dev, fs.statSync(source.home).dev);
  return root;
}

const inlineFiles = payloads => Object.entries(payloads).map(([name, bytes]) => ({
  name, uri: `data:application/octet-stream;base64,${bytes.toString("base64")}`,
}));

async function uploadSessionFiles(box, id, payloads) {
  const sent = await box.route("POST", "kuvo/sessions/ses_owned/send", {
    id, words: "Open the attached files", files: inlineFiles(payloads),
  });
  assert.equal(sent.status, 200, JSON.stringify(sent.value));
  const folder = "attachments/ses_owned";
  const uploads = fs.readdirSync(path.join(box.home, folder), { withFileTypes: true });
  return uploads.filter(upload => upload.isDirectory()).flatMap(upload => {
    const relative = `${folder}/${upload.name}`;
    const names = fs.readdirSync(path.join(box.home, relative));
    return names.filter(name => Object.hasOwn(payloads, name)).map(name => `${relative}/${name}`);
  });
}

async function uploadQueueFiles(box, payloads, owner = "ses_owned") {
  const added = await box.route("POST", "kuvo/work-queue/items", {
    owner, operation: "add-backup-fixture-attachments", title: "Keep these files", files: inlineFiles(payloads),
  }, true);
  assert.equal(added.status, 200, JSON.stringify(added.value));
  const { id } = added.value.item;
  const read = await box.route("GET", `kuvo/work-queue/items/${id}`);
  assert.equal(read.status, 200, JSON.stringify(read.value));
  const { files } = read.value.item;
  assert.deepEqual(files.map(({ name, size }) => [name, size]), Object.entries(payloads).map(([name, bytes]) => [name, bytes.length]));
  return { id, paths: files.map(({ name }) => `run/work-queue/files/${id}/${name}`) };
}

const operationID = () => crypto.randomBytes(16).toString("base64url");
const wipeRoute = box => `kuvo/computers/${box.key}/unlink-and-wipe`;
function preparation(box, role, operation) {
  const dir = path.join(box.run, "wipe-preparation");
  if (!fs.existsSync(dir)) return undefined;
  for (const name of fs.readdirSync(dir)) {
    if (!name.startsWith(role + "-") || !name.endsWith(".json")) continue;
    const state = JSON.parse(fs.readFileSync(path.join(dir, name), "utf8"));
    if (state.operation === operation) return state;
  }
  return undefined;
}
async function waitForPreparedRemoval(primary, source, operation, signal) {
  await until("prepared removal committed", () => {
    const failure = preparation(primary, "intent", operation)?.error ?? preparation(source, "source", operation)?.error;
    assert.equal(failure, undefined);
    const removal = removalOf(primary, source);
    return removal?.operation === operation && removal.generation;
  }, signal);
}

function removalMessage(packet, recipient, sender) {
  const envelope = link.readFrame(packet.payload);
  const prefix = Buffer.from("kuvo-removal-v1\0");
  if (envelope?.kind !== link.Frame.mail || !envelope.body.subarray(0, prefix.length).equals(prefix)) return;
  const body = envelope.body.subarray(prefix.length);
  const key = Buffer.from(sender.key, "base64url");
  if (!body.subarray(0, key.length).equals(key)) return;
  return JSON.parse(link.openOneWay(body.subarray(key.length), Buffer.from(recipient.identity.key, "base64url"), key).payload);
}

export {
  inlineFiles, operationID, preparation, removalMessage, removalOf, seedLocalWork,
  separateExportRoot, uploadQueueFiles, uploadSessionFiles, waitForPreparedRemoval, waitForWipe,
  wipeRoute,
};
