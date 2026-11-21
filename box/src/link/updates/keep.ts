// The scheduler runs in the relay owner on each computer.
import { docsChanged } from "../documents/exchange";
import { readEngineFile } from "../engine/client";
import { watchFolders } from "../follow";
import { RUN } from "../home";
import { publishLegacyRelease } from "../setup/shared";
import { announce } from "../peers/agents";
import { peersOf } from "../peers/known";
import { readPeerAgents } from "../peers/directory";
import { readLink } from "../state";
import type { LinkHost } from "../relay/host";
import { consumeFleetRetry } from "./progress";
import { clearIncoming, consumeFleetCheck, fleetFacts, followOver, followPeers, heldKeys, isUpdateMain, runningKept, watchHeld } from "./fleet";
import { checkDrift, dropOldBackups, noteLauncher, newerVersion, releaseRound, startUpdate, switchPending, updateOnTrial, updateState, updatesBlocked, UPDATE_KEYS, verifyBoxes } from "./update";

/** After the main’s start guard passes, publish the running signed pack for older readers. */
export function keepLegacyRelease(): boolean {
  if (!isUpdateMain() || switchPending() || updateOnTrial()) return false;
  const told = readPeerAgents();
  const legacy = peersOf(readLink()).some(peer => !newerVersion(told[peer.key]?.computer?.kuvo, "0.8.0-beta.40"));
  const running = legacy && runningKept();
  if (!running) return false;
  const { layout, clientProtocol } = verifyBoxes(running.boxes, heldKeys());
  return publishLegacyRelease({ version: running.version, sha256: running.sha256,
    signed: Buffer.from(running.boxes).toString("base64"), layout, clientProtocol });
}

export function keepFleetUpdates(host: () => LinkHost | null | undefined): void {
  followOver(host);
  if (updatesBlocked()) return;
  clearIncoming();
  let facts = "";
  let queued: ReturnType<typeof setTimeout> | undefined;
  const changed = () => {
    queued = undefined;
    try {
      consumeFleetRetry();
      consumeFleetCheck();
      followPeers();
      const current = JSON.stringify(fleetFacts());
      const relay = host();
      if (relay?.welcomed && current !== facts) {
        facts = current;
        void announce(relay, readEngineFile).catch(() => { facts = ""; });
      }
      if (keepLegacyRelease()) docsChanged(relay, { tell: true });
    } catch (error) {
      console.error(`Kuvo updates: ${(error as Error).message}`);
    }
  };
  const schedule = () => { queued ??= setTimeout(changed, 100); };
  watchHeld(schedule);
  watchFolders([RUN], schedule, { recursive: false });
  setInterval(schedule, 5_000).unref?.();
  schedule();
  const rounds = () => {
    noteLauncher();
    try { checkDrift(); } catch {}
    try { dropOldBackups(); } catch {}
    const round = releaseRound();
    if (isUpdateMain() && updateState().round !== round && UPDATE_KEYS.length) startUpdate({ round });
  };
  setTimeout(rounds, 60_000).unref?.();
  setInterval(rounds, 600_000).unref?.();
}
