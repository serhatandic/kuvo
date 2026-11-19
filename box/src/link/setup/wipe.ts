import crypto from "node:crypto";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { removalLayoutReady, removalOperationsLayoutReady } from "../../shared/folder-version";
import { exclusiveWhile, writeDurablePrivate } from "../../shared/files";
import { withCredentialCopyLock } from "../add/seed";
import { APP_FOLDER, homeFolder, K, OWN_RUNTIME, RUN, SCRIPT } from "../home";
import { LinkHost } from "../relay/host";
import { connectOnce } from "../relay/connection";
import { mailFor, openRemovalMail, ownKey, peersOf } from "../peers/known";
import { readLink, updateLink, type LinkState } from "../state";
import { clearDesktopKeychain, logCleanup, uninstall } from "../uninstall";
import { beforeMembershipWipe, invalidatedPreparation, verifySourceGeneration } from "./before-wipe";
import { signDoc } from "../documents/format";
import { followedSetup } from "./record";
import { REMOVAL_ID, type LocalWipe } from "./removal-record";
import { newKey, publicOf } from "../crypto/keys";

export function wipeSupportedHere(): boolean {
  // An app-owned box can have data beside this folder. Its own app must arrange that wipe.
  return !APP_FOLDER && path.resolve(K) === path.resolve(homeFolder(), ".kuvo-code")
    && fs.lstatSync(K, { throwIfNoEntry: false })?.isDirectory() === true;
}

function confirmedWipe(link: LinkState | null): LocalWipe | undefined {
  const wipe = link?.localWipe;
  const setup = followedSetup(link);
  if (!link || !wipe || !setup || wipe.setup !== setup.id || !REMOVAL_ID.test(wipe.operation)) return;
  if (wipe.primary) {
    if (setup.role === "member" && setup.primary === wipe.primary.key && setup.sign === wipe.primary.sign) return wipe;
  } else if (setup.role === "primary" && setup.primary === ownKey(link) && !peersOf(link).length && !link.leadershipPending
    && link.sharedOutcomes?.[wipe.operation]?.status === "committed") return wipe;
  return undefined;
}

let working = false;
let retryAfter = 0;

/** Run apart from the server that answered the removal and the link that received the instruction. */
export function scheduleWipe(): void {
  if (working || Date.now() < retryAfter || !SCRIPT || !confirmedWipe(readLink()) || !wipeSupportedHere()) return;
  working = true;
  setTimeout(() => {
    try {
      const child = spawn(process.execPath, [...OWN_RUNTIME, SCRIPT, "wipe-member"], {
        cwd: path.dirname(K), detached: true, stdio: "ignore", env: { ...process.env, KUVO_HOME: K, BUN_BE_BUN: "1" },
      });
      const ended = () => { working = false; retryAfter = Date.now() + 30_000; };
      child.once("exit", ended);
      child.once("error", ended);
      child.unref();
    } catch (error) {
      working = false;
      retryAfter = Date.now() + 30_000;
      logCleanup(error);
    }
  }, 400);
}

// Store only a sealed acknowledgment and a fresh reply identity outside the wipe boundary.
const receiptFolder = () => path.join(path.dirname(K), ".kuvo-code-removal-receipts");
type Completion = { setup: string; operation: string; primary: string; relay: string; reply: string; secret: string; done: string; proof?: ReturnType<typeof signDoc>; deleted?: true };

function completionOf(link: LinkState, wipe: LocalWipe): Completion | undefined {
  const peer = wipe.primary;
  if (!peer || !link.relay || !link.key) return;
  const reply = newKey();
  const done = mailFor(peer, { done: wipe.operation, setup: wipe.setup, generation: wipe.generation, receipt: publicOf(reply).toString("base64url") }, link, true);
  if (!done) throw new Error("Could not save the wipe completion receipt.");
  return { setup: wipe.setup, operation: wipe.operation, primary: peer.key, relay: link.relay,
    ...(link.sign ? { proof: signDoc(link.sign, "setup-shared", 1, { removal: "completed", setup: wipe.setup,
      operation: wipe.operation, source: ownKey(link), generation: wipe.generation }) } : {}),
    reply: reply.toString("base64url"), secret: crypto.randomBytes(32).toString("base64url"), done: done.toString("base64url") };
}

function keepCompletion(value: Completion): string {
  const directory = receiptFolder();
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const info = fs.lstatSync(directory);
  if (!info.isDirectory() || (info.mode & 0o077) !== 0 || (process.getuid && process.getuid() !== info.uid)) {
    throw new Error("The wipe receipt folder must be private to this account.");
  }
  const file = path.join(directory, value.operation + ".json");
  writeDurablePrivate(file, JSON.stringify(value), path.dirname(K));
  return file;
}

async function reportCompletion(value: Completion, file?: string): Promise<boolean> {
  let acknowledged = false;
  const host = new LinkHost({
    key: Buffer.from(value.reply, "base64url"),
    hello: { v: 1, kind: "machine", name: "", app: "", engine: null },
    authorize: () => false, handle: async function* () {}, send: () => {},
    mail: (_from, body) => {
      const reply = openRemovalMail(body, { key: value.reply });
      if (reply?.key === value.primary && reply.message.setup === value.setup && reply.message.recorded === value.operation) {
        acknowledged = true;
        host.disconnect?.();
      }
    },
  });
  const send = () => host.send(Buffer.from(value.done, "base64url"));
  const repeat = setInterval(send, 500);
  const timeout = setTimeout(() => host.disconnect?.(), 8_000);
  try { await connectOnce(value.relay, Buffer.from(value.secret, "base64url"), host, send); }
  finally { clearInterval(repeat); clearTimeout(timeout); }
  if (acknowledged && file) {
    fs.rmSync(file, { force: true });
    fs.rmSync(file.slice(0, -5) + ".ack", { force: true });
  }
  return acknowledged;
}

export async function wipeCommand(): Promise<number> {
  if (!removalLayoutReady(K) || !wipeSupportedHere() || !confirmedWipe(readLink())) return 1;
  try {
    return await exclusiveWhile(path.resolve(K) + "-wipe.lock", () => withCredentialCopyLock(async () => {
      const link = readLink();
      const wipe = confirmedWipe(link);
      if (!link || !wipe) return 1;
      if (!wipe.prepared) {
        const { backupBefore } = await import("../backup/engine");
        await backupBefore("wipe");
        const ready = updateLink(current => {
          if (!current || confirmedWipe(current)?.operation !== wipe.operation) return null;
          current.localWipe = { ...wipe, prepared: true };
          return current;
        }, { durable: true });
        if (!ready) return 1;
      }
      const own = ownKey(link);
      const invalidate = (error: unknown): never => {
        if (own && wipe.generation) {
          invalidatedPreparation(wipe, own, error instanceof Error ? error.message.slice(0, 500) : undefined);
          updateLink(current => {
            if (!current || current.localWipe?.operation !== wipe.operation) return null;
            delete current.localWipe;
            return current;
          }, { durable: true });
        }
        throw error;
      };
      const check = () => {
        try { if (own) verifySourceGeneration(wipe, own); }
        catch (error) { invalidate(error); }
      };
      if (own) await beforeMembershipWipe(wipe, own).catch(invalidate);
      const stopped = uninstall();
      if (!stopped.stopped || stopped.startsAgain.length) throw new Error(stopped.message);
      const completion = completionOf(link, wipe);
      const file = completion && removalOperationsLayoutReady(K) ? keepCompletion(completion) : undefined;
      check();
      if (!clearDesktopKeychain()) throw new Error(stopped.message);
      check();
      fs.rmSync(K, { recursive: true, force: true });
      if (completion && file) {
        const value = completion;
        value.deleted = true;
        if (value.proof) writeDurablePrivate(file.slice(0, -5) + ".ack", JSON.stringify(value.proof), path.dirname(K));
        writeDurablePrivate(file, JSON.stringify(value), path.dirname(K));
        for (let attempt = 0; attempt < 15 && fs.existsSync(file); attempt++) {
          if (await reportCompletion(value, file)) break;
          await new Promise(resolve => setTimeout(resolve, 2_000));
        }
      } else if (completion) await reportCompletion(completion);
      return 0;
    }));
  } catch (error) {
    if (fs.existsSync(RUN)) logCleanup(error);
    return 1;
  }
}
