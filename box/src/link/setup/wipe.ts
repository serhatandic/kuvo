import crypto from "node:crypto";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { installOffersLayoutReady, removalLayoutReady, removalOperationsLayoutReady } from "../../shared/folder-version";
import { isObject, parsed } from "../../shared/json";
import { exclusiveWhile, writeDurablePrivate } from "../../shared/files";
import { withCredentialCopyLock } from "../add/seed";
import { APP_FOLDER, APP_KEY_FILES, CHATS, ENGINE_CONFIG, homeFolder, IS_MAC, K, KEYS_ON_HOST, located, OWN_RUNTIME, RUN, SCRIPT } from "../home";
import { clearHostKeys } from "../keys";
import { macKeys, ownMacConfig, savedMacConfig } from "../mac/link";
import { LinkHost } from "../relay/host";
import { connectOnce } from "../relay/connection";
import { mailFor, openRemovalMail, ownKey, peersOf } from "../peers/known";
import { macLink, readLink, updateLink, type LinkState } from "../state";
import { clearAppOwnedLinkKeys, clearDesktopKeychain, logCleanup, stopAppOwnedKeeper, uninstall } from "../uninstall";
import { beforeMembershipWipe, invalidatedPreparation, verifySourceGeneration } from "./before-wipe";
import { signDoc } from "../documents/format";
import { followedSetup } from "./record";
import { REMOVAL_ID, type LocalWipe } from "./removal-record";
import { newKey, publicOf } from "../crypto/keys";
import { forgetWipeApproval, isAppWipeFolder, keepWipeReceipt, prepareLastSetupWipe, retainedWipeApproval, wipeLastSetup, wipeLock } from "./install-offer";
import { inside, roots } from "../backup/locations";

const APP_WIPE_ENV = "app-wipe-roots.json";
const ROOT_SETTINGS = ["KUVO_BOTS", "KUVO_WAKES", "KUVO_ENGINE_DATA", "KUVO_ENGINE_DB_FOLDER", "KUVO_ENGINE_CONFIG",
  "KUVO_AGENTS_HOME", "KUVO_GROUPS", "KUVO_ARTIFACTS", "KUVO_MAC_ATTACHMENTS", "KUVO_CHATS", "KUVO_KEYS", "KUVO_INTEGRATIONS",
  "KUVO_KEYS_ON_HOST"];

function appWipeTarget(): string | null {
  const folder = APP_FOLDER;
  const support = path.join(located(homeFolder()), "Library", "Application Support");
  if (!folder || !isAppWipeFolder(folder) || path.join(folder, "box") !== path.resolve(K)
    || path.resolve(located(homeFolder()), path.relative(homeFolder(), folder)) !== path.join(support, path.basename(folder))
    || located(folder) !== path.join(support, path.basename(folder))
    || located(K) !== path.join(located(folder), "box")) return null;
  // Box-owned data beside the box must resolve inside the same App Support directory. A
  // custom path outside it is a different layout and needs its own erase contract.
  const stored = [...Object.values(roots), CHATS, ENGINE_CONFIG, APP_KEY_FILES, process.env.KUVO_INTEGRATIONS];
  if (stored.some(root => root && !inside(located(folder), located(root)))) return null;
  return fs.lstatSync(K, { throwIfNoEntry: false })?.isDirectory() ? located(folder) : null;
}

export function wipeSupportedHere(last = false): boolean {
  if (APP_FOLDER) return last && installOffersLayoutReady(K) && appWipeTarget() !== null;
  return !APP_FOLDER && path.resolve(K) === path.resolve(homeFolder(), ".kuvo-code")
    && fs.lstatSync(K, { throwIfNoEntry: false })?.isDirectory() === true;
}

/** The installer can retry after the keeper has been stopped. Keep only layout settings in the
 * same data tree, so the retry verifies the original roots and deletes its own instructions. */
export async function resumeAppWipe(folder: string): Promise<void> {
  const home = path.join(folder, "box");
  const stateFile = path.join(home, "run", "link.json");
  const configFile = path.join(home, "run", APP_WIPE_ENV);
  if (!SCRIPT || !isAppWipeFolder(folder) || located(folder) !== path.resolve(folder)) return;
  const config = fs.lstatSync(configFile, { throwIfNoEntry: false });
  if (!config?.isFile() || config.size > 64 * 1024 || !fs.lstatSync(stateFile, { throwIfNoEntry: false })?.isFile()) return;
  const kept = parsed(fs.readFileSync(stateFile));
  if (!isObject(kept) || !isObject(kept.localWipe)) return;
  const { setup, operation } = kept.localWipe;
  if (typeof setup !== "string" || typeof operation !== "string" || !retainedWipeApproval(folder, setup, operation)) return;
  const settings = parsed(fs.readFileSync(configFile));
  if (!isObject(settings)) return;
  const env = { ...process.env };
  for (const name of ROOT_SETTINGS) {
    delete env[name];
    if (typeof settings[name] === "string") env[name] = settings[name];
  }
  await new Promise<void>((resolve, reject) => {
    const worker = spawn(process.execPath, [...OWN_RUNTIME, SCRIPT, "wipe-member"], {
      cwd: path.dirname(folder), stdio: "ignore", env: { ...env, KUVO_HOME: home, KUVO_BOX: "mac", BUN_BE_BUN: "1" },
    });
    worker.once("error", reject);
    worker.once("exit", () => resolve());
  });
}

function confirmedWipe(link: LinkState | null): LocalWipe | undefined {
  const wipe = link?.localWipe;
  if (!link || !wipe) return;
  const approved = !wipe.primary && retainedWipeApproval(appWipeTarget() ?? K, wipe.setup, wipe.operation);
  const setup = followedSetup(link) ?? (approved ? link.setup : null);
  if (!setup || wipe.setup !== setup.id || !REMOVAL_ID.test(wipe.operation)) return;
  if (wipe.primary) {
    if (setup.role === "member" && setup.primary === wipe.primary.key && setup.sign === wipe.primary.sign) return wipe;
  } else if (setup.role === "primary" && (setup.primary === ownKey(link) || approved)
    && !peersOf(link).some(peer => peer.key !== setup.primary) && !link.leadershipPending
    && link.sharedOutcomes?.[wipe.operation]?.status === "committed") return wipe;
  return undefined;
}

let working = false;
let retryAfter = 0;

/** Run apart from the server that answered the removal and the link that received the instruction. */
export function scheduleWipe(): void {
  if (working || Date.now() < retryAfter || !SCRIPT) return;
  const wipe = confirmedWipe(readLink());
  if (!wipe || !wipeSupportedHere(!wipe.primary)) return;
  working = true;
  setTimeout(() => {
    try {
      const child = spawn(process.execPath, [...OWN_RUNTIME, SCRIPT, "wipe-member"], {
        cwd: path.dirname(APP_FOLDER ?? K), detached: true, stdio: "ignore", env: { ...process.env, KUVO_HOME: K, BUN_BE_BUN: "1" },
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
  if (!removalLayoutReady(K)) return 1;
  try {
    const initial = readLink();
    if (!initial?.localWipe || !wipeSupportedHere(!initial.localWipe.primary)) return 1;
    const target = appWipeTarget() ?? K;
    if (IS_MAC && !macLink.overlay && !confirmedWipe(initial)) {
      macLink.config ??= savedMacConfig() ?? ownMacConfig();
      await macKeys({ needed: true });
    }
    return await exclusiveWhile(wipeLock(target), () => withCredentialCopyLock(async () => {
      const link = readLink();
      const wipe = confirmedWipe(link);
      if (!link || !wipe) return 1;
      if (!wipe.prepared) {
        const { backupBefore } = await import("../backup/engine");
        await backupBefore("wipe");
        const ready = updateLink(current => {
          const accepted = confirmedWipe(current);
          if (!current || !accepted || accepted.operation !== wipe.operation) return null;
          current.localWipe = { ...accepted, prepared: true };
          return current;
        }, { durable: true });
        if (!ready) return 1;
      }
      const last = !wipe.primary && installOffersLayoutReady(K);
      const own = last ? link.setup?.primary ?? ownKey(link) : ownKey(link);
      const invalidate = (error: unknown): never => {
        if (own && wipe.generation) {
          if (last) forgetWipeApproval(target, wipe.setup, wipe.operation);
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
      if (installOffersLayoutReady(K) && KEYS_ON_HOST && !wipe.hostKeysCleared) {
        await clearHostKeys();
        const cleared = updateLink(current => {
          const accepted = confirmedWipe(current);
          if (!current || !accepted || accepted.operation !== wipe.operation) return null;
          current.localWipe = { ...accepted, hostKeysCleared: true };
          return current;
        }, { durable: true });
        if (!cleared) return 1;
      }
      if (APP_FOLDER && appWipeTarget() !== target) throw new Error("The app’s erase layout has changed.");
      if (APP_FOLDER) {
        const settings = Object.fromEntries(ROOT_SETTINGS.map(name => [name, process.env[name]]));
        writeDurablePrivate(path.join(RUN, APP_WIPE_ENV), JSON.stringify(settings), K);
      }
      if (last) prepareLastSetupWipe(target, wipe.setup, wipe.operation);
      if (APP_FOLDER) await stopAppOwnedKeeper(target);
      const stopped = uninstall();
      if (!stopped.stopped || stopped.startsAgain.length) throw new Error(stopped.message);
      const completion = completionOf(link, wipe);
      const file = completion && removalOperationsLayoutReady(K) ? keepWipeReceipt(K, completion.operation + ".json", completion) : undefined;
      check();
      if (!(APP_FOLDER ? clearAppOwnedLinkKeys(target) : clearDesktopKeychain())) {
        throw new Error("The Box’s identity could not be removed from the Keychain.");
      }
      check();
      if (APP_FOLDER && appWipeTarget() !== target) throw new Error("The app’s erase layout has changed.");
      if (last) wipeLastSetup(target, wipe.setup, wipe.operation);
      else fs.rmSync(K, { recursive: true, force: true });
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
