// The phones paired with this box (PROTOCOL.md, "Paired phones"): listed, removed, and a link to
// pair another; and Kuvo Code's desktop app on this computer paired as one of your devices
// (`kuvo/devices/pair`, peers/devices.ts).

import { parsed } from "../../shared/json";
import type { Body } from "../../shared/json";
import type { PAIRING } from "../../shared/routes/box";
import type { Infer } from "../../shared/shape";
import { noteDocsChanged } from "../documents/changed";
import { IS_MAC } from "../home";
import { newPairingToken, pairingLink, removeAsMac, removePhones } from "../phones";
import { mainElsewhere, pairElsewhere } from "../setup/main-only";
import { pairDeviceFromHere } from "../peers/devices";
import { fromLocalApp } from "../server/context";
import type { AnswerPiece, RouteContext } from "../server/context";
import { macLink, readLink, updateLink } from "../state";
import type { LinkState, Pairing } from "../state";

// The phones paired with this machine, and removing some (a lost one, say), from any phone
// paired with it: what a removal takes off stays off (see `removePhones`).
export async function* phonesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { phone, json } = context;
  const link = readLink();
  const asking = phone?.key?.toString("base64url");
  // TODO(types): not held to `Phones` (shared/routes/box.ts): a phone kept here may have no name
  // (`Phone.name` is optional in state.ts), which the shape's `name: string` doesn't allow.
  return yield* json(200, {
    phones: (link?.phones ?? []).map(each => ({ key: each.key, name: each.name, added: each.added ?? null, pairing: each.pairing ?? "", kind: each.kind === "desktop" ? "desktop" as const : "phone" as const, you: each.key === asking })),
  });
}

// Removing some, by a device paired here (a phone, or the desktop app over the link), or Kuvo Code's
// app on this computer, beside any box.
export async function* phonesRemoveRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, phone, phonesChanged, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  const given = parsed(request.body) as Body<"phones">;
  // This Mac's app removes as the Mac does (Settings › Phone, or a removal a machine's
  // document brought), and so does Kuvo Code beside a machine's box, over its own server: any
  // device, never one of the Macs that set the machine up.
  const asMac = (IS_MAC && request.fromApp === true) || fromLocalApp(request);
  // On this Mac, one before any phone's paired here (no link file yet) is kept all the same,
  // so the phone stays out once it's taken.
  const changed = updateLink(kept => {
    const link: LinkState | null = kept ?? (asMac ? { phones: [], removed: [] } : null);
    return link && (asMac ? removeAsMac(link, given?.phones) : removePhones(link, phone?.key?.toString("base64url"), given?.phones)) ? link : null;
  });
  if (!changed) return yield* json(400, { message: "Only phones paired here can be removed, and never your Mac." });
  noteDocsChanged();
  // Once this answer is on its way: a phone that removed itself gets it too.
  setTimeout(() => phonesChanged(readLink() ?? changed), 200);
  return yield* json(200, {});
}

/** What a Mac says while it lets no device in: the one switch for phones and desktop apps alike. */
const SWITCH_OFF = "“Let my devices connect” is off on this Mac.";

// A link for another phone to pair with this machine, as `kuvo-link.js pair` shows one: good
// once, for a while; a new one takes the place of the last. DELETE ends it.
export async function* pairingRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, json } = context;
  const method = request.method.toUpperCase();
  // This Mac's Settings › Phone has one of its own, apart from a phone's.
  const slot = IS_MAC && request.fromApp ? "appPairing" : "pairing";
  if (method === "DELETE") {
    // `?token=`: that link alone, so one closed late never ends a newer one.
    const token = new URLSearchParams(String(request.path ?? "").split("?")[1] ?? "").get("token");
    updateLink(link => {
      if (!link?.[slot] || (token && link[slot].token !== token)) return null;
      delete link[slot];
      return link;
    });
    macLink.wake?.();
    return yield* json(200, {});
  }
  if (method !== "POST") return yield* json(405, {});
  // Only your setup's main computer pairs phones: a member names it (PROTOCOL.md, "Your setup").
  const main = mainElsewhere(readLink());
  if (main) return yield* json(409, pairElsewhere(main));
  if (IS_MAC && !macLink.config?.phones) return yield* json(409, { message: SWITCH_OFF });
  const link = updateLink(current => current?.relay && current?.secret ? { ...current, [slot]: newPairingToken() } : null);
  if (!link) return yield* json(409, { message: IS_MAC ? "This Mac isn't ready for your devices yet. Try again in a moment." : "This machine has no relay to pair through." });
  macLink.wake?.();
  // (A box with a relay to pair through has its keys and name too.)
  return yield* json(200, { link: pairingLink({ ...link, pairing: link[slot] } as LinkState & { relay: string; secret: string; key: string; name: string; pairing: Pairing }), ends: Math.floor(link[slot]!.expires / 1000) } satisfies Infer<typeof PAIRING>);
}

// `POST kuvo/devices/pair {key, name, kind: "desktop"}`: Kuvo Code's desktop app on this computer
// paired as one of your devices, with no link to paste: here where this box pairs phones, else by
// the setup's main computer, which this box vouches to (peers/devices.ts). Only for the app beside
// this box, over the box's own server (`fromLocalApp`): never a client over the relay, nor `box
// route`, which an agent's command here could run. It uses no pairing token, and ends none.
export async function* devicesPairRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "POST") return yield* json(405, {});
  if (!fromLocalApp(request)) return yield* json(403, { message: "Only Kuvo Code on this computer adds itself to your devices." });
  const done = await pairDeviceFromHere(parsed(request.body), signal);
  return yield* json(done.status, done.value);
}
