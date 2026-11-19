import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { removalLayoutReady } from "../../shared/folder-version";
import { exclusiveWhile } from "../../shared/files";
import { APP_FOLDER, homeFolder, K, OWN_RUNTIME, RUN, SCRIPT } from "../home";
import { LinkHost } from "../relay/host";
import { connectOnce } from "../relay/connection";
import { openRemovalMail, ownKey, peersOf } from "../peers/known";
import { readLink, updateLink, type LinkState } from "../state";
import { clearDesktopKeychain, logCleanup, uninstall } from "../uninstall";
import { beforeMembershipWipe } from "./before-wipe";
import { followedSetup } from "./record";
import { REMOVAL_ID, type LocalWipe } from "./removal-record";
import { sendRemoval } from "./removal";

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

async function confirmDelivery(link: LinkState, wipe: LocalWipe): Promise<void> {
  const peer = wipe.primary;
  if (!peer || !link.relay || !link.key || !link.secret) return;
  const host = new LinkHost({
    key: Buffer.from(link.key, "base64url"),
    hello: { v: 1, kind: "machine", name: "", app: "", engine: null },
    authorize: () => false, handle: async function* () {}, send: () => {},
    mail: (_from, body) => {
      const reply = openRemovalMail(body, link);
      if (reply?.key === peer.key && reply.message.recorded === wipe.operation) host.disconnect?.();
    },
  });
  const send = () => sendRemoval(host, peer, link, { done: wipe.operation, setup: wipe.setup });
  const repeat = setInterval(send, 500);
  const timeout = setTimeout(() => host.disconnect?.(), 8_000);
  try { await connectOnce(link.relay, Buffer.from(link.secret, "base64url"), host, send); }
  finally { clearInterval(repeat); clearTimeout(timeout); }
}

export async function wipeCommand(): Promise<number> {
  if (!removalLayoutReady(K) || !wipeSupportedHere() || !confirmedWipe(readLink())) return 1;
  try {
    return await exclusiveWhile(path.resolve(K) + "-wipe.lock", async () => {
      const link = readLink();
      const wipe = confirmedWipe(link);
      if (!link || !wipe) return 1;
      if (!wipe.prepared) {
        await beforeMembershipWipe({ phase: "before-wipe", ...wipe });
        const { backupBefore } = await import("../backup/engine");
        await backupBefore("wipe");
        const ready = updateLink(current => {
          if (!current || confirmedWipe(current)?.operation !== wipe.operation) return null;
          current.localWipe = { ...wipe, prepared: true };
          return current;
        }, { durable: true });
        if (!ready) return 1;
      }
      const stopped = uninstall();
      if (!stopped.stopped || stopped.startsAgain.length || !clearDesktopKeychain()) throw new Error(stopped.message);
      fs.rmSync(K, { recursive: true, force: true });
      await confirmDelivery(link, wipe);
      return 0;
    });
  } catch (error) {
    if (fs.existsSync(RUN)) logCleanup(error);
    return 1;
  }
}
