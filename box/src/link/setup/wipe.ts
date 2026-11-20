import crypto from "node:crypto";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { installOffersLayoutReady, removalLayoutReady, removalOperationsLayoutReady } from "../../shared/folder-version";
import { isObject, parsed } from "../../shared/json";
import { exclusiveWhile, writeDurablePrivate } from "../../shared/files";
import { withCredentialCopyLock } from "../add/seed";
import { APP_FOLDER, APP_KEY_FILES, CHATS, ENGINE_CONFIG, ENGINE_DATA, homeFolder, IS_MAC, K, KEYS_ON_HOST, located, OWN_RUNTIME, RUN, SCRIPT } from "../home";
import { clearHostKeys } from "../keys";
import { macKeys, ownMacConfig, savedMacConfig } from "../mac/link";
import { LinkHost } from "../relay/host";
import { connectOnce } from "../relay/connection";
import { mailFor, openRemovalMail, ownKey, peersOf } from "../peers/known";
import { macLink, readLink, updateLink, type LinkState } from "../state";
import { clearAppOwnedLinkKeys, clearDesktopKeychain, logCleanup, stopAppOwnedKeeper, uninstall } from "../uninstall";
import { beforeMembershipWipe, invalidatedPreparation, preparedMoveDestination, verifySourceGeneration } from "./before-wipe";
import { confirmRemovalFiles } from "./removal-moves";
import { signDoc } from "../documents/format";
import { followedSetup } from "./record";
import { REMOVAL_ID, REMOVAL_EXPORT_REQUIRED, type LocalWipe } from "./removal-record";
import { newKey, publicOf } from "../crypto/keys";
import { appMemberWipeReceipt, forgetWipeApproval, isAppWipeFolder, keepWipeReceipt, prepareWipeDirectory, retainedWipeApproval,
  resumeWipeDirectory, wipeApprovedDirectory, wipeLock, wipeReceipts } from "./install-offer";
import { inside, roots } from "../backup/locations";
import { backupBefore, usesAutomaticRelayBackup } from "../backup/engine";
import { receiveRemovalMail, sendRemovalUpdates } from "./removal";

const APP_WIPE_ENV = "app-wipe-roots.json";
const ROOT_SETTINGS = ["KUVO_BOTS", "KUVO_WAKES", "KUVO_ENGINE_DATA", "KUVO_ENGINE_DB_FOLDER", "KUVO_ENGINE_CONFIG",
  "KUVO_AGENTS_HOME", "KUVO_GROUPS", "KUVO_ARTIFACTS", "KUVO_MAC_ATTACHMENTS", "KUVO_CHATS", "KUVO_KEYS", "KUVO_INTEGRATIONS",
  "KUVO_KEYS_ON_HOST"];

function appDataPath(root: string): string {
  try { return fs.realpathSync.native(root); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT" || fs.lstatSync(root, { throwIfNoEntry: false })) throw error;
    return path.join(appDataPath(path.dirname(root)), path.basename(root));
  }
}

function appWipeTarget(): string | null {
  const folder = APP_FOLDER;
  const support = path.join(located(homeFolder()), "Library", "Application Support");
  if (!folder || !isAppWipeFolder(folder) || path.join(folder, "box") !== path.resolve(K)
    || path.resolve(located(homeFolder()), path.relative(homeFolder(), folder)) !== path.join(support, path.basename(folder))
    || located(folder) !== path.join(support, path.basename(folder))
    || located(K) !== path.join(located(folder), "box")) return null;
  // Box-owned data beside the box must resolve inside the same App Support directory. A
  // custom path outside it is a different layout and needs its own erase contract.
  const stored = [...Object.values(roots), CHATS, ENGINE_CONFIG, ENGINE_DATA, APP_KEY_FILES, process.env.KUVO_INTEGRATIONS];
  try {
    if (stored.some(root => root && !inside(located(folder), appDataPath(path.resolve(root))))) return null;
  } catch { return null; }
  return fs.lstatSync(K, { throwIfNoEntry: false })?.isDirectory() ? located(folder) : null;
}

export function wipeSupportedHere(): boolean {
  if (APP_FOLDER) return installOffersLayoutReady(K) && appWipeTarget() !== null;
  return !APP_FOLDER && path.resolve(K) === path.resolve(homeFolder(), ".kuvo-code")
    && fs.lstatSync(K, { throwIfNoEntry: false })?.isDirectory() === true;
}

type MemberWipeReview = NonNullable<NonNullable<LinkState["removalSource"]>["review"]>;

/** Persist the fence before returning a refusal. A replay of the plain instruction must stay
 * blocked even if the local backup configuration later changes. */
export function holdMemberWipe(setup: string, operation: string, review: MemberWipeReview, prior?: LocalWipe) {
  return updateLink(current => {
    if (!current || followedSetup(current)?.id !== setup || current.setup?.role !== "member" || current.localUnlink ||
      (prior ? JSON.stringify(current.localWipe) !== JSON.stringify(prior) : current.localWipe)) return null;
    current.removalSource = { setup, operation, review };
    delete current.localWipe;
    current.phones = [];
    delete current.pairing;
    delete current.appPairing;
    return current;
  }, { durable: true });
}

/** The installer can retry after the keeper has been stopped. Keep only layout settings in the
 * same data tree, so the retry verifies the original roots and deletes its own instructions. */
export async function resumeAppWipe(folder: string): Promise<void> {
  const home = path.join(folder, "box");
  const stateFile = path.join(home, "run", "link.json");
  const configFile = path.join(home, "run", APP_WIPE_ENV);
  if (!SCRIPT || !isAppWipeFolder(folder) || located(folder) !== path.resolve(folder)) return;
  await resumeWipeDirectory(folder, true, () => clearMemberAppIdentity(folder));
  const erased = appMemberWipeReceipt(folder);
  if (erased) {
    const kept = savedCompletion(folder, erased);
    if (kept) {
      const file = publishCompletion(folder, kept);
      await reportCompletion(kept, file, 2_000);
    }
  }
  const config = fs.lstatSync(configFile, { throwIfNoEntry: false });
  if (!config?.isFile() || config.size > 64 * 1024 || !fs.lstatSync(stateFile, { throwIfNoEntry: false })?.isFile()) return;
  const kept = parsed(fs.readFileSync(stateFile));
  if (!isObject(kept) || !isObject(kept.localWipe)) return;
  const { setup, operation } = kept.localWipe;
  if (typeof setup !== "string" || typeof operation !== "string" || !retainedWipeApproval(folder, setup, operation, isObject(kept.localWipe.primary))) return;
  const settings = parsed(fs.readFileSync(configFile));
  if (!isObject(settings)) return;
  const env = { ...process.env };
  for (const name of ROOT_SETTINGS) {
    delete env[name];
    if (typeof settings[name] === "string") env[name] = settings[name];
  }
  await new Promise<void>((resolve, reject) => {
    const member = isObject(kept.localWipe) && isObject(kept.localWipe.primary);
    const worker = spawn(process.execPath, [...OWN_RUNTIME, SCRIPT, "wipe-member"], {
      cwd: path.dirname(folder), stdio: "ignore", detached: member, env: { ...env, KUVO_HOME: home, KUVO_BOX: "mac", BUN_BE_BUN: "1" },
    });
    worker.once("error", reject);
    if (member) worker.once("spawn", () => { worker.unref(); resolve(); });
    else worker.once("exit", () => resolve());
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
  if (!wipe || !wipeSupportedHere()) return;
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
type Completion = { setup: string; operation: string; generation?: string; primary: string; relay: string; reply: string; secret: string; done: string; proof?: ReturnType<typeof signDoc>; deleted?: true };

function completionOf(link: LinkState, wipe: LocalWipe): Completion | undefined {
  const peer = wipe.primary;
  if (!peer || !link.relay || !link.key) return;
  const reply = newKey();
  const done = mailFor(peer, { done: wipe.operation, setup: wipe.setup, generation: wipe.generation, receipt: publicOf(reply).toString("base64url") }, link, true);
  if (!done) throw new Error("Could not save the wipe completion receipt.");
  return { setup: wipe.setup, operation: wipe.operation, generation: wipe.generation, primary: peer.key, relay: link.relay,
    ...(link.sign ? { proof: signDoc(link.sign, "setup-shared", 1, { removal: "completed", setup: wipe.setup,
      operation: wipe.operation, source: ownKey(link), generation: wipe.generation }) } : {}),
    reply: reply.toString("base64url"), secret: crypto.randomBytes(32).toString("base64url"), done: done.toString("base64url") };
}

function savedCompletion(target: string, wipe: Pick<LocalWipe, "operation" | "setup">): Completion | undefined {
  if (!REMOVAL_ID.test(wipe.operation)) return;
  const file = path.join(wipeReceipts(target), wipe.operation + ".json");
  const info = fs.lstatSync(file, { throwIfNoEntry: false });
  if (!info?.isFile() || info.size > 4096 || (info.mode & 0o077) !== 0 || (process.getuid && info.uid !== process.getuid())) return;
  const value: unknown = parsed(fs.readFileSync(file));
  if (!isObject(value) || value.setup !== wipe.setup || value.operation !== wipe.operation ||
    typeof value.primary !== "string" || typeof value.relay !== "string" || typeof value.reply !== "string" ||
    typeof value.secret !== "string" || typeof value.done !== "string") return;
  return value as Completion;
}

function publishCompletion(target: string, value: Completion): string {
  if (value.proof) keepWipeReceipt(target, value.operation + ".ack", value.proof);
  return keepWipeReceipt(target, value.operation + ".json", { ...value, deleted: true });
}

function clearMemberAppIdentity(target: string) {
  if (fs.existsSync(target) || !clearAppOwnedLinkKeys(target)) {
    throw new Error("The app-owned Box’s identity could not be removed safely.");
  }
}

/** A shutdown write or a failed destination check can invalidate the grant after writers stop.
 * Keep just removal mail available with the existing in-memory identity until the next grant. */
async function awaitWipeRetry(link: LinkState, wipe: LocalWipe): Promise<boolean> {
  if (!link.key || !link.relay || !link.secret) return false;
  const pending = () => {
    const current = readLink();
    return !current?.localWipe && (current?.removalSource?.setup === wipe.setup || current?.localUnlink?.setup === wipe.setup);
  };
  working = true;
  try {
    while (pending()) {
      const host = new LinkHost({
        key: Buffer.from(link.key, "base64url"),
        hello: { v: 1, kind: "machine", name: "", app: "", engine: null },
        authorize: () => false, handle: async function* () {}, send: () => {},
        mail: (from, body) => {
          const mail = openRemovalMail(body, readLink());
          if (mail) receiveRemovalMail(mail.key, mail.message, host, from);
          if (readLink()?.localWipe) host.disconnect?.();
        },
      });
      const check = setInterval(() => {
        if (!pending()) host.disconnect?.();
        else if (readLink()?.localUnlink) sendRemovalUpdates(host);
      }, 1_000);
      try { await connectOnce(link.relay, Buffer.from(link.secret, "base64url"), host); }
      finally { clearInterval(check); }
      if (pending()) await new Promise(resolve => setTimeout(resolve, 2_000));
    }
    return Boolean(readLink()?.localWipe);
  } finally { working = false; }
}

/** Writers have stopped. Use only the scoped removal channel for the final file check. */
async function verifyMovedBeforeErasing(link: LinkState, wipe: LocalWipe, own: string) {
  const destination = preparedMoveDestination(wipe, own);
  if (!destination) return;
  if (!link.key || !link.relay || !link.secret) throw new Error("The move destination could not be reached for the final file check.");
  let verified: boolean | undefined;
  const host = new LinkHost({
    key: Buffer.from(link.key, "base64url"),
    hello: { v: 1, kind: "machine", name: "", app: "", engine: null },
    authorize: () => false, handle: async function* () {}, send: () => {},
    mail: (_from, body) => {
      const mail = openRemovalMail(body, link);
      if (mail) receiveRemovalMail(mail.key, mail.message, host);
    },
  });
  await connectOnce(link.relay, Buffer.from(link.secret, "base64url"), host, () => {
    void confirmRemovalFiles(link, destination, wipe.operation, host).then(result => {
      verified = result;
      host.disconnect?.();
    });
  });
  if (!verified) throw new Error("The destination could not verify the moved files before erase. Review the removal plan.");
}

async function reportCompletion(value: Completion, file?: string, timeoutMs = 8_000): Promise<boolean> {
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
  const timeout = setTimeout(() => host.disconnect?.(), timeoutMs);
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
  let retry: { link: LinkState; wipe: LocalWipe } | undefined;
  try {
    const initial = readLink();
    if (!initial?.localWipe || !wipeSupportedHere()) return 1;
    const target = APP_FOLDER ? appWipeTarget() : K;
    if (!target) return 1;
    if (IS_MAC && !macLink.overlay && (!confirmedWipe(initial) || (initial.localWipe.primary && !initial.key))) {
      macLink.config ??= savedMacConfig() ?? ownMacConfig();
      await macKeys({ needed: true });
    }
    return await exclusiveWhile(wipeLock(target), () => withCredentialCopyLock(async () => {
      const link = readLink();
      const wipe = confirmedWipe(link);
      if (!link || !wipe) return 1;
      const last = !wipe.primary && installOffersLayoutReady(K);
      const appMember = Boolean(APP_FOLDER && wipe.primary);
      const journaled = last || appMember;
      const own = wipe.source ?? ownKey(link) ?? (last ? link.setup?.primary : undefined);
      if (wipe.requiresPreparation && !own) throw new Error("The wipe’s source identity is unavailable.");
      let stopping = appMember && retainedWipeApproval(target, wipe.setup, wipe.operation, true);
      let hostKeysCleared = wipe.hostKeysCleared;
      const invalidate = (error: unknown): never => {
        if (own && wipe.generation) {
          invalidatedPreparation(wipe, own, error instanceof Error ? error.message.slice(0, 500) : undefined);
          const changed = updateLink(current => {
            if (!current || current.localWipe?.operation !== wipe.operation) return null;
            if (stopping && current.removalSource?.setup === wipe.setup && current.removalSource.operation === wipe.operation) {
              current.removalSource.stopped = true;
              if (hostKeysCleared) current.removalSource.hostKeysCleared = true;
            }
            delete current.localWipe;
            return current;
          }, { durable: true });
          if (changed) {
            if (journaled) forgetWipeApproval(target, wipe.setup, wipe.operation, appMember);
            if (stopping && link.removalSource) retry = { link, wipe };
          }
        }
        throw error;
      };
      const check = () => {
        try { if (own) verifySourceGeneration(wipe, own); }
        catch (error) { invalidate(error); }
      };
      let verifiedExport = false;
      if (!wipe.prepared) {
        if (wipe.primary && usesAutomaticRelayBackup()) {
          verifiedExport = Boolean(own && await beforeMembershipWipe(wipe, own).catch(invalidate));
          if (!verifiedExport) {
            if (wipe.requiresPreparation) invalidate(new Error(REMOVAL_EXPORT_REQUIRED));
            else holdMemberWipe(wipe.setup, wipe.operation, "choose-export", wipe);
            return 1;
          }
        } else await backupBefore("wipe");
        const ready = updateLink(current => {
          const accepted = confirmedWipe(current);
          if (!current || !accepted || accepted.operation !== wipe.operation) return null;
          current.localWipe = { ...accepted, prepared: true };
          return current;
        }, { durable: true });
        if (!ready) return 1;
      }
      if (own && !verifiedExport) await beforeMembershipWipe(wipe, own).catch(invalidate);
      if (installOffersLayoutReady(K) && KEYS_ON_HOST && !hostKeysCleared) {
        await clearHostKeys();
        const cleared = updateLink(current => {
          const accepted = confirmedWipe(current);
          if (!current || !accepted || accepted.operation !== wipe.operation) return null;
          current.localWipe = { ...accepted, hostKeysCleared: true };
          return current;
        }, { durable: true });
        if (!cleared) return 1;
        hostKeysCleared = true;
      }
      if (APP_FOLDER && appWipeTarget() !== target) throw new Error("The app’s erase layout has changed.");
      if (APP_FOLDER) {
        const settings = Object.fromEntries(ROOT_SETTINGS.map(name => [name, process.env[name]]));
        writeDurablePrivate(path.join(RUN, APP_WIPE_ENV), JSON.stringify(settings), K);
      }
      const kept = appMember && retainedWipeApproval(target, wipe.setup, wipe.operation, true) ? savedCompletion(target, wipe) : undefined;
      const completion = kept && kept.generation === wipe.generation && kept.primary === wipe.primary?.key
        ? kept : completionOf(link, wipe);
      if (appMember && !completion) throw new Error("The wipe’s completion receipt is unavailable.");
      const file = completion && removalOperationsLayoutReady(K)
        ? keepWipeReceipt(target, completion.operation + ".json", completion) : undefined;
      if (journaled) prepareWipeDirectory(target, wipe.setup, wipe.operation, appMember);
      stopping = true;
      if (APP_FOLDER) await stopAppOwnedKeeper(target);
      const stopped = uninstall();
      if (!stopped.stopped || stopped.startsAgain.length) throw new Error(stopped.message);
      check();
      if (own) await verifyMovedBeforeErasing(link, wipe, own).catch(invalidate);
      if (!appMember && !(APP_FOLDER ? clearAppOwnedLinkKeys(target) : clearDesktopKeychain())) {
        throw new Error("The Box’s identity could not be removed from the Keychain.");
      }
      check();
      if (APP_FOLDER && appWipeTarget() !== target) throw new Error("The app’s erase layout has changed.");
      if (journaled) wipeApprovedDirectory(target, wipe.setup, wipe.operation, appMember,
        appMember ? () => clearMemberAppIdentity(target) : undefined);
      else fs.rmSync(K, { recursive: true, force: true });
      if (completion && file) {
        const value = completion;
        publishCompletion(target, value);
        for (let attempt = 0; attempt < 15 && fs.existsSync(file); attempt++) {
          if (await reportCompletion(value, file)) break;
          await new Promise(resolve => setTimeout(resolve, 2_000));
        }
      } else if (completion) await reportCompletion(completion);
      return 0;
    }));
  } catch (error) {
    if (fs.existsSync(RUN)) logCleanup(error);
    if (retry && await awaitWipeRetry(retry.link, retry.wipe).catch(logCleanup)) return wipeCommand();
    return 1;
  }
}
