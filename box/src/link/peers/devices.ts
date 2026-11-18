// Kuvo Code's desktop app, paired as one of your devices with no link to paste (PROTOCOL.md,
// "Paired phones", `kuvo/devices/pair`). The app asks the box on its own computer. Where that box
// pairs phones (your setup's main computer, or a box in no setup), it pairs the app's key at once.
// A member of a setup vouches for it instead: it hands the ask to the main computer's box by mail
// (`work-queue/forward.ts`, as what you do to one of your machines is handed on), which pairs it
// only for a box its own peers document names as one of its machines, and only while it lets your devices in. Every box of the setup then lets the
// device in from the main computer's phones document, as it does a phone. A removed key stays out,
// and so does the computer that vouched for it: removing a device a box vouched for keeps that box
// from vouching for another (`vouchBlocked`, phones.ts), until you allow it again on the main
// computer (`kuvo/computers/<key>/allow-devices`, areas/computers.ts), so nobody at that computer
// gets back in by having its app make a new key.
// Platform: the same on macOS and Linux; only a Mac's box has the switch ("Let my devices connect").

import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { noteDocsChanged } from "../documents/changed";
import { macDocsChanged } from "../documents/exchange";
import { macPeersBody } from "../documents/work";
import { IS_MAC } from "../home";
import { ownName } from "../letters/start";
import { fits, pairDevice, revokeVoucher, vouchBlockOf } from "../phones";
import { mainElsewhere } from "../setup/main-only";
import { followedSetup } from "../setup/record";
import { macLink, readLink, updateLink } from "../state";
import { forwardAct, takesForwarded } from "../work-queue/forward";
import type { LinkState } from "../state";
import type { Act, Acted } from "../work-queue/forward";
import { readPeerAgents } from "./directory";
import { macsOf, ownKey, peersOf } from "./known";
import type { MailSender } from "./known";

/** What a box's `kuvo/info` and its agents' mail say when it pairs Kuvo Code's desktop app this way. */
export const DEVICES_FEATURE = "devices.pair";

/** A removed device as its removal is said: by its name, or as what it is where it gave none. */
const namedDevice = (device: string): string => device || "Kuvo Code's app";

/** What the box of `computer` is told when it vouches while kept from it (`device`, the one whose
 * removal did that), for its app to show as it is. `here`: this box, the main computer. */
export const blockedVouch = (device: string, computer: string, here: string) => ({
  message: `${namedDevice(device)} on ${computer} was removed from your devices, so ${computer} can't add Kuvo Code's app by itself now. Allow it again from Kuvo Code on ${here} or another computer's app: Computers, ${computer}.`,
  reason: "blocked",
});

/** What a client shows on a computer kept from vouching (`vouching.words` in `kuvo/computers`). */
export const blockedWords = (device: string, computer: string): string => `${namedDevice(device)} was removed from your devices. ${computer} can't add Kuvo Code's app again until you allow it.`;

/** The longest name a device is kept with. */
const MOST_NAME = 100;

/** The app asking to be paired: its static key, the name it gave, and what it is. */
export interface AskingDevice {
  key: string;
  name: string;
  kind: "desktop";
}

/** What the app (or a member's box, for it) sent, checked: a 32-byte key that isn't a box's, a name
 * of up to 100 characters, and the one kind paired this way; null for anything else. */
export function givenDevice(given: unknown, link: LinkState | null = readLink()): AskingDevice | null {
  if (!isObject(given) || given.kind !== "desktop" || typeof given.name !== "string" || given.name.length > MOST_NAME) return null;
  const boxes = link ? [...macsOf(link), ...peersOf(link).map(peer => peer.key), ownKey(link) ?? ""] : [];
  if (!fits({ key: given.key }, boxes)) return null;
  return { key: given.key as string, name: plainName(given.name).slice(0, MOST_NAME), kind: "desktop" };
}

const answer = (status: number, value: unknown): Acted => ({ status, value });

/** What a removed device is told (`reason`, for the app to offer making a new key, on your word). */
const REMOVED = { message: "This app was removed from your devices.", reason: "removed" };
const NOT_GIVEN = { message: "Send the app's key (key), its name (name, up to 100 characters) and what it is (kind: desktop)." };

/** Whether this Mac lets no device in (its one switch, "Let my devices connect"): it pairs none
 * then, and vouches for none either, whatever its part in the setup. A machine has no switch. */
const switchOff = (): boolean => IS_MAC && !macLink.config?.phones;
const switchOffWords = () => ({ message: `“Let my devices connect” is off on ${ownName()}.` });

/** `device` paired on this box, which pairs phones itself: at once, as a paired device of its own.
 * Refused as a phone's pairing is: a Mac that lets no device in, or a box with no relay. `by`: the
 * member's box key. Its membership and permission to vouch are checked under the pairing lock. */
export function pairHere(device: AskingDevice, by?: string): Acted {
  if (switchOff()) return answer(409, switchOffWords());
  let made: ReturnType<typeof pairDevice> | null = null;
  let fresh = false;
  let refusal: Acted | undefined;
  let computer: string | undefined;
  updateLink(link => {
    if (by) {
      if (!link || followedSetup(link)?.role !== "primary") {
        refusal = answer(409, { message: `${ownName()} isn't your setup's main computer, so it adds no devices for your other computers.` });
        return null;
      }
      if (link.removedHere?.includes(by) || !ownMachines(link).has(by)) {
        refusal = answer(409, { message: `Only computers in ${ownName()}'s own setup can add your devices.` });
        return null;
      }
      computer = peersOf(link).find(peer => peer.key === by)?.name;
    }
    if (!givenDevice(device, link)) {
      refusal = answer(400, NOT_GIVEN);
      return null;
    }
    if (!link?.relay || !link.secret) return null;
    const before = (link.phones ?? []).length;
    made = pairDevice(link, { ...device, by });
    fresh = (link.phones ?? []).length !== before;
    return fresh ? link : null;
  });
  if (refusal) return refusal;
  const paired = made as ReturnType<typeof pairDevice> | null;
  if (!paired) return answer(409, { message: IS_MAC ? `${ownName()} isn't ready for your devices yet. Try again in a moment.` : "This machine has no relay to pair through." });
  if ("removed" in paired) return answer(409, REMOVED);
  if ("blocked" in paired) return answer(409, blockedVouch(paired.blocked.device, computer || "that computer", ownName()));
  if ("full" in paired) return answer(409, { message: "You have as many desktop apps among your devices as Kuvo Code keeps. Remove one you no longer use, then try again." });
  if (fresh) {
    noteDocsChanged();
    // (A Mac's link comes up for its first device, as for a phone's pairing.)
    macLink.wake?.();
  }
  return answer(200, { paired: true, pairing: paired.pairing });
}

/** The boxes this box's own peers document names as its machines (the members of the setup it
 * leads), by key: only a Mac's box signs one. */
const ownMachines = (link: LinkState): Set<string> => new Set(IS_MAC ? macPeersBody(link).boxes.filter(box => box.kind === "machine").map(box => box.key as string) : []);

/** A member's box vouching for the app on its computer (`area: "devices"`, by mail): paired here
 * only on the setup's main computer, for a box its own peers document names. */
export async function deviceAct(sender: MailSender, act: Act): Promise<Acted> {
  if (act.method === "DELETE") {
    if (act.item !== sender.key) return answer(403, { message: "A computer can only revoke its own devices." });
    const changed = updateLink(link => {
      if (!link || followedSetup(link)?.role !== "primary" || !ownMachines(link).has(sender.key)) return null;
      const device = link.phones?.find(phone => phone.by === sender.key)?.name ?? vouchBlockOf(link, sender.key)?.device;
      revokeVoucher(link, sender.key, device);
      return link;
    });
    if (!changed) return answer(409, { message: "This computer couldn’t confirm the devices were revoked." });
    macDocsChanged({ closeRemoved: changed });
    return answer(200, {});
  }
  const device = act.method === "POST" ? givenDevice({ ...(act.body ?? {}), key: act.item }, null) : null;
  if (!device) return answer(400, NOT_GIVEN);
  return pairHere(device, sender.key);
}

/** `kuvo/devices/pair` as this box answers Kuvo Code's app beside it: paired here, or, on a member
 * of a setup, by its main computer, which this box asks and waits for. */
export async function pairDeviceFromHere(given: unknown, signal?: AbortSignal | null): Promise<Acted> {
  const link = readLink();
  const device = givenDevice(given, link);
  if (!device) return answer(400, NOT_GIVEN);
  // Each Mac's own switch counts: one that lets no device in asks its main computer for none.
  if (switchOff()) return answer(409, switchOffWords());
  const main = mainElsewhere(link);
  if (!main) return pairHere(device);
  const named = main.name || peersOf(link).find(peer => peer.key === main.key)?.name || "your main computer";
  // (Set aside: its main computer doesn't name it now. Stepping aside never opens pairing again.)
  if (link?.setup?.aside) return answer(409, { message: `${named} doesn't list this computer in its setup now, so it can't add your devices by itself.` });
  // Let in already (the main computer's phones document lists it): the same answer, nothing asked.
  const known = (link?.phones ?? []).find(phone => phone.key === device.key);
  if (known) return answer(200, { paired: true, pairing: known.pairing ?? "" });
  if ((link?.removed ?? []).some(each => each.key === device.key)) return answer(409, REMOVED);
  const waiting = { message: `${named} isn't answering. This app reaches your other computers once it does.` };
  const peer = peersOf(link).find(each => each.key === main.key);
  if (!peer) return answer(503, waiting);
  // (Not heard from yet, so what it takes isn't known: asked again once it is.)
  if (!readPeerAgents()[main.key]?.features) return answer(503, waiting);
  if (!takesForwarded(main.key, DEVICES_FEATURE)) return answer(409, { message: `${named} needs a newer Kuvo Code to add this app to your devices. Update it, then try again.` });
  const there = await forwardAct({ key: main.key, name: named, kind: "mac" }, device.name, { area: "devices", method: "POST", item: device.key, body: { name: device.name, kind: device.kind } }, signal, link);
  if ("refused" in there) return answer(503, waiting);
  const said = isObject(there.value) ? there.value : {};
  if (there.status === 200 && said.paired === true && typeof said.pairing === "string") return answer(200, { paired: true, pairing: said.pairing });
  const message = typeof said.message === "string" && said.message ? said.message.slice(0, 700) : `${named} didn't add this app to your devices.`;
  // (Why, where the app goes by it: its key was removed, or this computer is kept from vouching.)
  return answer(409, { message, ...(said.reason === "removed" || said.reason === "blocked" ? { reason: said.reason } : {}) });
}
