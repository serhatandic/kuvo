// The same scheduler owns fleet updates on a Mac's server and on a machine's link.
import { readEngineFile } from "../engine/client";
import { watchFolders } from "../follow";
import { RUN } from "../home";
import { takesForwarded } from "../work-queue/forward";
import { updateAccess } from "../setup/access";
import { followedSetup } from "../setup/record";
import { readLink } from "../state";
import { peersOf } from "../peers/known";
import { announce } from "../peers/agents";
import type { LinkHost } from "../relay/host";
import { claimRetry } from "./attempt";
import { releaseChoice, releaseSetup } from "./choice";
import { clearIncoming, followOver, followPeers, HELD, heldRelease, retryTransfer, watchHeld } from "./fleet";
import { localFleetUpdate } from "./progress";
import {
  boxVersion, checkDrift, dropOldBackups, noteInterruptedOffer, noteLauncher, offerUpdate, offerWaits, releaseRound,
  startOfferedUpdate, startUpdate, updateState, updatesBlocked, UPDATE_KEYS,
} from "./update";

export function keepFleetUpdates(host: () => LinkHost | null | undefined): void {
  followOver(host);
  if (updatesBlocked()) return;
  clearIncoming();
  noteInterruptedOffer();
  let choice = "";
  let progress = "";
  const inform = () => {
    const relay = host();
    if (relay?.welcomed) announce(relay, readEngineFile).catch(() => {});
  };
  const changed = () => {
    let retry;
    try { retry = claimRetry(); } catch (error) {
      console.error(`Kuvo updates: Couldn’t claim the requested retry: ${(error as Error).message}`);
    }
    if (retry) {
      const selected = releaseChoice();
      if (selected?.version === retry.version && selected.sha256 === retry.sha256 && localFleetUpdate(retry.version)?.canRetry)
        retryTransfer(retry.version);
    }
    const accepted = JSON.stringify(releaseChoice());
    if (accepted !== choice) {
      choice = accepted;
      followPeers();
    }
    const update = localFleetUpdate();
    const state = JSON.stringify(update);
    if (state !== progress) {
      progress = state;
      console.error(JSON.stringify({ at: new Date().toISOString(), pid: process.pid, event: "fleet update changed", current: update?.current, target: update?.target, state: update?.state ?? "no release selected", stage: update && "stage" in update ? update.stage : undefined }));
      inform();
    }
  };
  watchHeld(() => { changed(); inform(); });
  watchFolders([RUN], changed, { recursive: false });
  setInterval(() => {
    followPeers();
    changed();
    if (offerWaits()) startOfferedUpdate().catch(error => console.error(`Kuvo updates: ${error.message}`));
  }, 15_000).unref?.();
  const rounds = () => {
    noteLauncher();
    try { checkDrift(); } catch {}
    try { dropOldBackups(); } catch {}
    const link = readLink();
    const leader = followedSetup(link);
    const peers = peersOf(link);
    if (leader?.role === "primary" && !releaseSetup() && peers.length && peers.every(peer => takesForwarded(peer.key, "setup.checkpoints"))) {
      updateAccess(current => current, { checkpoint: true });
    }
    const round = releaseRound();
    const setup = releaseSetup();
    if (setup && !setup.primary) return;
    if (setup?.primary && !releaseChoice() && !offerWaits() && heldRelease()?.version === boxVersion()) {
      if (offerUpdate(HELD).status === 202) return;
    }
    if (!setup && offerWaits()) return;
    if (updateState().round !== round && UPDATE_KEYS.length) startUpdate({ round });
  };
  setTimeout(rounds, 60_000).unref?.();
  setInterval(rounds, 600_000).unref?.();
}
