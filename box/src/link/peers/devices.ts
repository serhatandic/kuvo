// Kuvo Code's desktop app, paired as one of your devices with no link to paste (PROTOCOL.md,
// "Paired phones", `kuvo/devices/pair`). The app asks the box on its own computer. Where that box
// pairs phones (your setup's main computer, or a box in no setup), it pairs the app's key at once.
// A member of a setup vouches for it instead: it hands the ask to the main computer's box by mail
// (`work-queue/forward.ts`, as what you do to one of your machines is handed on), which pairs it
// only for a box its own peers document names as one of its machines, never one shared with
// another setup, and only while it lets your devices in. Every box of the setup then lets the
// device in from the main computer's phones document, as it does a phone. A removed key stays out.
// Platform: the same on macOS and Linux; only a Mac's box has the switch ("Let my devices connect").

import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { noteDocsChanged } from "../documents/changed";
import { macPeersBody } from "../documents/work";
import { IS_MAC } from "../home";
import { ownName } from "../letters/start";
import { fits, pairDevice } from "../phones";
import { mainElsewhere } from "../setup/main-only";
import { followedSetup } from "../setup/record";
import { sharedMachine } from "../setup/settle";
import { macLink, readLink, updateLink } from "../state";
import { forwardAct, takesForwarded } from "../work-queue/forward";
import type { LinkState } from "../state";
import type { Act, Acted } from "../work-queue/forward";
import { isApart } from "./apart";
import { readPeerAgents } from "./directory";
import { macsOf, ownKey, peersOf } from "./known";
import type { MailSender } from "./known";

/** What a box's `kuvo/info` and its agents' mail say when it pairs Kuvo Code's desktop app this way. */
export const DEVICES_FEATURE = "devices.pair";

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
 * Refused as a phone's pairing is: a Mac that lets no device in, or a box with no relay. */
export function pairHere(device: AskingDevice): Acted {
  if (switchOff()) return answer(409, switchOffWords());
  let made: ReturnType<typeof pairDevice> | null = null;
  let fresh = false;
  updateLink(link => {
    if (!link?.relay || !link.secret) return null;
    const before = (link.phones ?? []).length;
    made = pairDevice(link, device);
    fresh = (link.phones ?? []).length !== before;
    return fresh ? link : null;
  });
  const paired = made as ReturnType<typeof pairDevice> | null;
  if (!paired) return answer(409, { message: IS_MAC ? `${ownName()} isn't ready for your devices yet. Try again in a moment.` : "This machine has no relay to pair through." });
  if ("removed" in paired) return answer(409, REMOVED);
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
 * only on the setup's main computer, for a box its own peers document names, that isn't shared
 * with another setup (as it says, or as this box knows it). */
export async function deviceAct(sender: MailSender, act: Act): Promise<Acted> {
  const here = ownName();
  const link = readLink();
  const device = act.method === "POST" ? givenDevice({ key: act.item, ...(act.body ?? {}) }, link) : null;
  if (!device) return answer(400, NOT_GIVEN);
  const setup = followedSetup(link);
  if (!link || setup?.role !== "primary") return answer(409, { message: `${here} isn't your setup's main computer, so it adds no devices for your other computers.` });
  if (sharedMachine(link) || isApart(sender.key, link) || !ownMachines(link).has(sender.key)) return answer(409, { message: `Only computers in ${here}'s own setup can add your devices.` });
  return pairHere(device);
}

/** `kuvo/devices/pair` as this box answers Kuvo Code's app beside it: paired here, or, on a member
 * of a setup, by its main computer, which this box asks and waits for. */
export async function pairDeviceFromHere(given: unknown, signal?: AbortSignal | null): Promise<Acted> {
  const link = readLink();
  const device = givenDevice(given, link);
  if (!device) return answer(400, NOT_GIVEN);
  if (sharedMachine(link)) return answer(409, { message: "This computer is shared with another setup, so it can't add your devices by itself." });
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
  const message = typeof said.message === "string" && said.message ? said.message.slice(0, 300) : `${named} didn't add this app to your devices.`;
  return answer(409, { message, ...(said.reason === "removed" ? { reason: "removed" } : {}) });
}
