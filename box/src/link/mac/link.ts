// This Mac's box's link (PROTOCOL.md, "This Mac's box: the relay"): whether it's wanted, how it's
// doing, its keys from the app, and the machines told they're let go. mac/relay runs it.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { isObject } from "../../shared/json";
import { macPeersBody, ownDoc } from "../documents/work";
import { writePrivate } from "../files";
import { plainName } from "../../shared/text";
import { accountHome, K, located, MAC_APP_VERSION, OWN_HOME_MAC, RUN } from "../home";
import { linkHost } from "../host-interface";
import { cleanPeer, fitsPeer, mailFor, peersOf } from "../peers/known";
import { macLink, readLink, updateLink } from "../state";
import { anyMachines } from "./machines";
import type { SignedDoc } from "../../shared/documents";
import type { Peer } from "../peers/known";
import type { LinkHost } from "../relay/host";
import type { LinkState, MacLinkConfig } from "../state";

/** How this Mac's link stands: off, on its way to the relay, on it, or failed (with why). */
export type LinkStatusState = "off" | "connecting" | "connected" | "failed";

/** link.json with the machines this Mac removed that haven't heard it yet (`untold`), which
 * `LinkState` doesn't list. */

// Whether the link's wanted now: the phone link on, with a phone paired or a pairing waiting; or
// machines to meet.
export function macNeeded(link: LinkState | null | undefined, now = Date.now()): boolean {
  const config = macLink.config;
  if (!config) return false;
  if (config.machines) return true;
  const pairing = (["pairing", "appPairing"] as const).some(slot => link?.[slot] && now < link[slot]!.expires);
  return Boolean(config.phones && ((link?.phones ?? []).length || pairing));
}

// What its app last said of the link (`kuvo/link/config`), kept in a file of the box's own
// (`run/mac-link.json`, readable by you alone; never in link.json, which keeps no relay or name on a
// Mac): whether the phone link is on, the relay, this Mac's name and its app's version. So a box
// started again with the app closed (by the keeper, at login with Background Work on) is back on
// the relay as it was, before the app opens, if it ever does. Whether there are machines to meet
// isn't kept: the box reads the app's list of them (`machines.json`) as it takes this, so machines
// you've removed since don't bring the link back. Its keys stay in your Keychain, given through the
// keeper's host interface as through the app's.
export const MAC_LINK_FILE = path.join(RUN, "mac-link.json");

/** `own`: the box's own settings with only the switch yours (Settings › This Mac, no app telling
 * it): the relay and this Mac's name are worked out again at each start, the switch kept. */
export function saveMacConfig(config: MacLinkConfig, own = false): void {
  const { phones, relay, name, app } = config;
  try { writePrivate(MAC_LINK_FILE, JSON.stringify({ phones, relay, name, app, ...(own ? { own: true } : {}) })); } catch (error) {
    console.error(`This Mac's link settings couldn't be kept: ${(error as Error).message}`);
  }
}

/** Whether what's kept is the box's own settings (none kept yet, or kept with only the switch
 * yours), not what an app said. */
export function keepsOwnConfig(file = MAC_LINK_FILE): boolean {
  let kept: unknown;
  try { kept = JSON.parse(fs.readFileSync(file, "utf8")); } catch { return !fs.existsSync(file); }
  return isObject(kept) && kept.own === true;
}

/** What the app last said, as kept, with whether there are machines now: null for none kept (or
 * what isn't what it keeps). Kept as the box's own (`own`), it's the box's own settings now with
 * the switch as you left it. */
export function savedMacConfig(file = MAC_LINK_FILE): MacLinkConfig | null {
  let kept: unknown;
  try { kept = JSON.parse(fs.readFileSync(file, "utf8")); } catch { return null; }
  if (!isObject(kept) || typeof kept.phones !== "boolean" || typeof kept.relay !== "string") return null;
  try {
    if (!["ws:", "wss:"].includes(new URL(kept.relay).protocol)) return null;
  } catch {
    return null;
  }
  const own = kept.own === true ? ownMacConfig() : null;
  if (own) return { ...own, phones: kept.phones };
  return { phones: kept.phones, machines: anyMachines(), relay: kept.relay, name: typeof kept.name === "string" ? kept.name : "Mac",
    app: MAC_APP_VERSION ?? (typeof kept.app === "string" ? kept.app : "") };
}

/** The relay your computers and devices meet at, where no app of this Mac's said another (the
 * retired Mac app's own default, `PhoneLink.defaultRelay`). */
export const DEFAULT_RELAY = "wss://kuvolink.porigam.com.tr";

/** This computer's name: a Mac's as you named it in System Settings, else its host's. */
export function computerName(): string {
  if (process.platform === "darwin") {
    const asked = spawnSync("/usr/sbin/scutil", ["--get", "ComputerName"], { encoding: "utf8", timeout: 5_000, stdio: ["ignore", "pipe", "ignore"] });
    const said = asked.status === 0 ? plainName(asked.stdout).slice(0, 100) : "";
    if (said) return said;
  }
  return plainName(os.hostname().replace(/\.(local|lan|home)$/i, "")).slice(0, 100) || (process.platform === "darwin" ? "Mac" : "computer");
}

/** The link's settings of a Mac's box in a home of its own that no app ever told them (#713: the
 * desktop app says none, and a Mac that never ran the retired app has none kept): the box's own.
 * The relay everyone's on, this Mac's name, and your devices let in ("Let my devices connect" on,
 * as a machine's box always has it: nothing connects until you pair a device or add a computer).
 * What an app said before (`mac-link.json`) is kept instead, its switch as you left it. A test's
 * or a demo copy's folder never takes the real relay by itself: only the one it's given
 * (`KUVO_TEST_RELAY`), else none, as before. Null for a box in its app's folder, which its app tells. */
export function ownMacConfig(): MacLinkConfig | null {
  if (!OWN_HOME_MAC) return null;
  // (The real one: your account's own `~/.kuvo-code`, the only home a Mac's helper runs.)
  const real = located(K) === located(path.join(accountHome(), ".kuvo-code"));
  let relay = real ? DEFAULT_RELAY : null;
  try {
    const given = real ? undefined : process.env.KUVO_TEST_RELAY;
    if (given && ["ws:", "wss:"].includes(new URL(given).protocol)) relay = given.replace(/\/$/, "");
  } catch {}
  if (!relay) return null;
  return { phones: true, machines: anyMachines(), relay, name: computerName(), app: MAC_APP_VERSION ?? "" };
}

// Off the relay once it's no longer wanted (the phone link turned off, the last phone gone, a
// pairing that ran out, no machines any more).
export function macLinkCheck(): void {
  if (macLink.host && !macNeeded(readLink())) macLink.host.disconnect?.();
}

export function setLinkStatus(state: LinkStatusState, reason?: string): void {
  const next = reason ? { state, reason } : { state };
  if (JSON.stringify(next) === JSON.stringify(macLink.status)) return;
  macLink.status = next;
  for (const tell of macLink.statusFollowers) tell();
}

// Waits `ms`, or until the link's told something changed (`macLink.wake`).
export function macLinkWait(ms: number): Promise<void> {
  return new Promise<void>(resolve => {
    const timer = setTimeout(done, ms);
    function done() {
      clearTimeout(timer);
      if (macLink.wake === done) macLink.wake = null;
      resolve();
    }
    macLink.wake = done;
  });
}

const KEY_TEXT = /^[A-Za-z0-9_-]{43}$/;

// This Mac's keys, from its app (your Keychain): its link key, relay secret and signing key, kept
// with the relay and name it says once it's on. False when the app couldn't give them (the link
// says so).
export async function macKeys({ needed = false } = {}): Promise<boolean> {
  const config = macLink.config;
  // (`needed`: asked for whether or not the link is wanted yet, as adding this Mac's first
  // computer does: it's signed in with these keys before there's any machine to meet.)
  if (macLink.overlay || !config || !(needed || config.phones || config.machines)) return true;
  const given = await linkHost("link/identity", {});
  if (!given || !["key", "secret", "sign"].every(name => KEY_TEXT.test((given[name] ?? "") as string))) {
    // (Asked for ahead of the link being wanted: the link itself hasn't failed, and says nothing.)
    if (config.phones || config.machines) setLinkStatus("failed", "Kuvo Code couldn't give this Mac's link its keys from your Keychain. It tries again in a moment.");
    return false;
  }
  macLink.overlay = { key: given.key as string, secret: given.secret as string, sign: given.sign as string, relay: macLink.config!.relay, name: macLink.config!.name };
  // What it said of the keys before isn't so any more.
  if (macLink.status.state === "failed") setLinkStatus(macNeeded(readLink()) ? "connecting" : "off");
  return true;
}

// Machines this Mac removed hear its peers document once, while it still knows where they are: they
// let this Mac go. Those it couldn't tell now (off the relay, or the connection closing) hear it
// once it's back (`link.untold`), each kept until it went.
export function tellGone(gone: Peer[]): void {
  if (!gone.length) return;
  const went = sendGone(gone);
  const left = gone.filter(peer => !went.includes(peer.key));
  updateLink((link: LinkState | null) => {
    if (!link) return null;
    const untold = (Array.isArray(link.untold) ? link.untold : []).filter(peer => !gone.some(each => each.key === peer.key));
    if (!left.length && untold.length === (link.untold?.length ?? 0)) return null;
    link.untold = [...untold, ...left].slice(-64);
    if (!link.untold.length) delete link.untold;
    return link;
  });
}

// Mails `gone` this Mac's peers document; the keys of those it went to.
function sendGone(gone: Peer[]): string[] {
  const host: LinkHost | null = macLink.host;
  if (!host?.welcomed) return [];
  let envelope: SignedDoc | null = null;
  const link = updateLink(link => {
    const made = link?.sign ? ownDoc(link, "peers", macPeersBody(link)) : null;
    envelope = made?.envelope ?? null;
    return made?.changed ? link : null;
  }) ?? readLink();
  if (!envelope || !link?.key) return [];
  const went: string[] = [];
  for (const peer of gone) {
    // (One that signs no documents takes none: nothing to tell it.)
    if (!peer.sign) {
      went.push(peer.key);
      continue;
    }
    const bytes = mailFor(peer, { docs: { give: [envelope] }, at: Date.now() }, link);
    try { if (bytes && host.send(bytes)) went.push(peer.key); } catch {}
  }
  return went;
}

// On the relay again: the machines removed meanwhile hear it now (one set up again since, not).
export function tellUntold(): void {
  const link: LinkState | null = readLink();
  const untold = (Array.isArray(link?.untold) ? link.untold : []).filter(fitsPeer);
  const again = untold.filter(peer => peersOf(link).some(each => each.key === peer.key));
  if (again.length) {
    updateLink((link: LinkState | null) => {
      if (!link?.untold) return null;
      link.untold = link.untold.filter(peer => !again.some(each => each.key === peer.key));
      return link;
    });
  }
  tellGone(untold.filter(peer => !again.includes(peer)).map(cleanPeer));
}

// Stops it (a test's, as it ends): what's open goes, and the loop with it.
export function stopMacLink(): void {
  macLink.stopped = true;
  macLink.host?.disconnect?.();
  macLink.wake?.();
}