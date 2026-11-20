import { clientTransportsLayoutReady } from "../../shared/folder-version";
import { isObject } from "../../shared/json";
import { IKResponder } from "../crypto/noise";
import { IS_MAC, K } from "../home";
import { keptDeviceConnections, macKeys, ownMacConfig, savedMacConfig } from "../mac/link";
import { KEY, ownKey } from "../peers/known";
import { deviceConnectionsAllowed, pairedPhone } from "../phones";
import { macLink, readLink } from "../state";
import { clientUpdateRequired } from "../updates/client";
import type { Asker } from "./context";

const PROLOGUE = Buffer.from("kuvo-ssh v1");
type Opening = { computer: string; given: Record<string, unknown>; actor: Asker; authorized: () => boolean } | { status: number; message: string };

/** The same paired device identity as on the relay, bound to this connection and this box. */
export async function acceptSshRoute(read: () => Promise<Record<string, unknown>>, say: (value: unknown) => void): Promise<Opening> {
  const opening = await read();
  const computer = opening?.computer;
  if (typeof computer !== "string" || !KEY.test(computer) || typeof opening.hello !== "string" || opening.hello.length > 4000) {
    return { status: 400, message: "Send the computer’s public key and a device handshake." };
  }
  if (!clientTransportsLayoutReady(K)) return { status: 503, message: "Wait for the Kuvo Code update to finish before connecting over SSH." };
  if (IS_MAC) {
    macLink.config ??= savedMacConfig() ?? ownMacConfig();
    if (!keptDeviceConnections()) return { status: 403, message: "“Let my devices connect” is off on this Mac." };
    if (!await macKeys({ needed: true })) return { status: 503, message: "Kuvo couldn’t verify this Mac’s identity." };
  }
  const here = () => {
    try {
      const link = readLink();
      return link?.key && ownKey(link) === computer && deviceConnectionsAllowed(link, !IS_MAC || keptDeviceConnections()) ? link : null;
    } catch { return null; }
  };
  const box = here();
  if (!box?.key) return { status: 409, message: "SSH reached a different computer or one being removed." };
  try {
    const responder = new IKResponder(Buffer.from(box.key, "base64url"), PROLOGUE);
    const hello: unknown = JSON.parse(responder.readMessage1(Buffer.from(opening.hello, "base64url")).toString("utf8"));
    if (!isObject(hello) || hello.v !== 1) return { status: 400, message: "The SSH handshake is not supported." };
    // SSH already proved access to the OS account. The application's current pairing still
    // has to be present here, so the command can't stand in for a paired client by itself.
    const key = responder.rs.toString("base64url");
    const pairing = pairedPhone(box, key);
    const refusal = pairing && clientUpdateRequired(hello, pairing.kind === "desktop");
    const { message, transport } = responder.writeMessage2(Buffer.from(JSON.stringify({ allowed: !!pairing && !refusal, ...(refusal || {}) })));
    say({ hello: message.toString("base64url") });
    if (!pairing) return { status: 403, message: "Pair this device before reaching this computer over SSH." };
    if (refusal) return { status: 426, ...refusal };
    const sealed = (await read())?.request;
    if (typeof sealed !== "string") return { status: 400, message: "Send the authenticated SSH request." };
    const given: unknown = JSON.parse(transport.open(Buffer.from(sealed, "base64url")).toString("utf8"));
    if (!isObject(given)) return { status: 400, message: "Send a routed request." };
    const currentPairing = () => pairedPhone(here(), key, pairing.pairing ?? "");
    const current = currentPairing();
    if (!current) return { status: 403, message: "This device’s pairing changed. Connect again." };
    return { computer, given, authorized: () => !!currentPairing(),
      actor: { key: responder.rs, pairing: current.pairing, name: current.name, ...(hello.client === "desktop" ? { kind: "desktop" } : {}) } };
  } catch {
    return { status: 403, message: "Kuvo couldn’t authenticate the SSH request." };
  }
}
