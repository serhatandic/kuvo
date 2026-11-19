import { updateAccess } from "../setup/access";
import type { SaveReply, SavedReply, UnsavedReply } from "../../shared/routes/setup-save";
import { setupSaveReceipt } from "../../shared/setup-local";
import { awaitSharedSave, bindSave, queryOperation, removalReceipt, sharedSaveReply, withSaveOutcome } from "../setup/shared";
import { createHash } from "node:crypto";
import { readDoc } from "../documents/format";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { noteDocsChanged } from "../documents/changed";
import { macDocsChanged } from "../documents/exchange";
import { phoneTrustWait } from "../documents/phone-sync";
import { IS_MAC } from "../home";
import { ownName } from "../letters/start";
import { fits, pairDevice, pairedPhone, removeAsMac, removePhones } from "../phones";
import { mainElsewhere } from "../setup/main-only";
import { followedSetup } from "../setup/record";
import { macLink, readLink } from "../state";
import { forwardAct, takesForwarded } from "../work-queue/forward";
import type { LinkState } from "../state";
import type { Act, Acted } from "../work-queue/forward";
import { readPeerAgents } from "./directory";
import { macsOf, ownKey, peersOf } from "./known";
import type { MailSender } from "./known";

/** What a box's `kuvo/info` and its agents' mail say when it pairs Kuvo Code's desktop app this way. */
export const DEVICES_FEATURE = "devices.pair";
export const DEVICES_REMOVE_FEATURE = "devices.remove.forward";

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

type DeviceReply = SaveReply | UnsavedReply<200 | 400 | 403 | 404 | 409 | 503>;
const answer = <S extends 200 | 400 | 403 | 404 | 409 | 503>(status: S, value: UnsavedReply<S>["value"] & Record<string, unknown>): UnsavedReply<S> => ({ status, value });

/** What a removed device is told (`reason`, for the app to offer making a new key, on your word). */
const REMOVED = { message: "This app was removed from your devices.", reason: "removed" };
const NOT_GIVEN = { message: "Send the app's key (key), its name (name, up to 100 characters) and what it is (kind: desktop)." };

const switchOff = (): boolean => IS_MAC && !macLink.config?.phones;
const switchOffWords = () => ({ message: `“Let my devices connect” is off on ${ownName()}.` });

/** A primary pairs the desktop key sent beside it or forwarded by an enrolled member. */
export async function pairHere(device: AskingDevice, member?: string): Promise<DeviceReply> {
  if (switchOff()) return answer(409, switchOffWords());
  let made: ReturnType<typeof pairDevice> | null = null;
  let fresh = false;
  let refusal: UnsavedReply<400 | 409> | undefined;
  const { save, busy } = updateAccess(link => {
    const main = mainElsewhere(link);
    if (main) {
      refusal = answer(409, { message: `Pair new devices from ${main.name || "your main computer"}.`, primary: main.key });
      return null;
    }
    if (member) {
      if (!link || followedSetup(link)?.role !== "primary") {
        refusal = answer(409, { message: `${ownName()} isn't your setup's main computer, so it adds no devices for your other computers.` });
        return null;
      }
      if (!ownMachines(link).has(member)) {
        refusal = answer(409, { message: `Only computers in ${ownName()}'s own setup can add your devices.` });
        return null;
      }
    }
    if (!givenDevice(device, link)) {
      refusal = answer(400, NOT_GIVEN);
      return null;
    }
    if (!link?.relay || !link.secret) return null;
    const before = (link.phones ?? []).length;
    made = pairDevice(link, device);
    fresh = (link.phones ?? []).length !== before;
    return fresh ? link : null;
  });
  if (busy) return answer(409, { message: "An access change is already waiting to be saved." });
  const receipt = await awaitSharedSave(save);
  if (receipt && receipt.status !== "committed") return sharedSaveReply(receipt);
  if (refusal) return refusal;
  const paired = made as ReturnType<typeof pairDevice> | null;
  if (!paired) return answer(409, { message: IS_MAC ? `${ownName()} isn't ready for your devices yet. Try again in a moment.` : "This machine has no relay to pair through." });
  if ("removed" in paired) return answer(409, REMOVED);
  if ("full" in paired) return answer(409, { message: "You have as many paired devices as Kuvo Code keeps. Remove one you no longer use, then try again." });
  if (fresh) {
    noteDocsChanged();
    // (A Mac's link comes up for its first device, as for a phone's pairing.)
    macLink.wake?.();
  }
  const value = { paired: true as const, pairing: paired.pairing };
  if (receipt) return { status: 200, value: { ...value, ...receipt } } satisfies SavedReply<typeof value>;
  return answer(200, value);
}

const ownMachines = (link: LinkState): Set<string> =>
  new Set(peersOf(link).filter(peer => (link.setup?.managed || peer.kind === "machine") && peer.sign).map(peer => peer.key));

export type DeviceActor = { key: string; pairing: string };

/** Apply both local and forwarded removals at the authority. Any relay actor must still
 * hold the same pairing when the primary takes the lock. */
export async function removeDevicesHere(entries: unknown, actor: DeviceActor | null, member?: string, operation?: string): Promise<DeviceReply> {
  const { changed, save, busy } = updateAccess(link => {
    if (mainElsewhere(link) || (member && (!link || followedSetup(link)?.role !== "primary" || !ownMachines(link).has(member)))) return null;
    const kept = link ?? (!actor && !member ? { phones: [], removed: [] } : null);
    if (!kept || (actor && !pairedPhone(kept, actor.key, actor.pairing))) return null;
    const staging = Boolean(kept.setup?.standby);
    return (actor ? removePhones(kept, actor.key, entries, staging) : removeAsMac(kept, entries, staging)) ? kept : null;
  }, { operation });
  if (busy) return answer(409, { message: "An access change is already waiting to be saved." });
  if (!changed) return answer(400, { message: "Send devices from your setup to remove." });
  const receipt = await awaitSharedSave(save && member ? bindSave(save, changed.sign!, { requestingMember: member }) : save);
  if (receipt && receipt.status !== "committed") return sharedSaveReply(receipt);
  noteDocsChanged();
  // Let a device that removed itself receive the response.
  setTimeout(() => macDocsChanged({ closeRemoved: readLink() }), 200);
  return receipt ? sharedSaveReply(receipt) : answer(200, {});
}

export async function deviceAct(sender: MailSender, act: Act): Promise<DeviceReply> {
  if (act.item === sender.key && act.method === "GET") {
    const receipt = setupSaveReceipt(act.body?.save);
    const ticket = readDoc(receipt?.proof);
    if (!receipt || (ticket?.body.requestingMember !== sender.key && ticket?.body.revokedVoucher !== sender.key)) return answer(404, {});
    const outcome = queryOperation(receipt.setup, receipt.operation, receipt.proof, "membershipAndAccess");
    if (!outcome) return answer(404, {});
    return sharedSaveReply(withSaveOutcome({ ...receipt, epoch: Number(outcome.epoch) }, outcome.status));
  }
  if (act.item === sender.key && act.method === "DELETE") {
    const actor = act.body?.actor;
    if (actor !== null && (!isObject(actor) || !fits(actor, []) || typeof actor.pairing !== "string")) {
      return answer(400, { message: "Send the requesting device and its pairing." });
    }
    const operation = createHash("sha256").update(JSON.stringify([sender.key, act.id])).digest("base64url");
    const state = readLink();
    if (state?.sign && followedSetup(state)?.role === "primary" && ownMachines(state).has(sender.key)) {
      const prior = removalReceipt(state, operation);
      if (prior) {
        const receipt = await awaitSharedSave(bindSave(prior, state.sign, { requestingMember: sender.key }));
        return sharedSaveReply(receipt!);
      }
    }
    return removeDevicesHere(act.body?.phones, actor as DeviceActor | null, sender.key, operation);
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
  if (link?.setup?.aside) return answer(409, { message: `${named} doesn't list this computer in its setup now. Re-add this computer from ${named} to pair this app.` });
  // Let in already (the main computer's phones document lists it): the same answer, nothing asked.
  const known = (link?.phones ?? []).find(phone => phone.key === device.key);
  if (known) {
    const waiting = phoneTrustWait(link, device.key, IS_MAC ? macLink.host : undefined);
    return waiting ? answer(waiting.reason === "pairing-readd" ? 409 : 503, waiting) : answer(200, { paired: true, pairing: known.pairing ?? "" });
  }
  if ((link?.removed ?? []).some(each => each.key === device.key)) return answer(409, REMOVED);
  const waiting = { message: `${named} isn't answering. This app reaches your other computers once it does.` };
  const peer = peersOf(link).find(each => each.key === main.key);
  if (!peer) return answer(503, waiting);
  // (Not heard from yet, so what it takes isn't known: asked again once it is.)
  if (!readPeerAgents()[main.key]?.features) return answer(503, waiting);
  if (!takesForwarded(main.key, DEVICES_FEATURE)) return answer(409, { message: `${named} needs a newer Kuvo Code to add this app to your devices. Update it, then try again.` });
  const there = await forwardAct(peer, device.name, { area: "devices", method: "POST", item: device.key, body: { name: device.name, kind: device.kind } }, signal, link);
  if ("refused" in there) return answer(503, waiting);
  if (there.status === 202 || setupSaveReceipt(there.value)) return there;
  const said = isObject(there.value) ? there.value : {};
  if (there.status === 200 && said.paired === true && typeof said.pairing === "string") return answer(200, { paired: true, pairing: said.pairing });
  const message = typeof said.message === "string" && said.message ? said.message.slice(0, 700) : `${named} didn't add this app to your devices.`;
  return answer(409, { message, ...((said.reason === "removed" || said.reason === "blocked") ? { reason: said.reason } : {}) });
}
