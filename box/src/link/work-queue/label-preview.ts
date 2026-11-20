import crypto from "node:crypto";
import { isObject } from "../../shared/json";
import { LABEL_RESTORE_FEATURE } from "../../shared/routes/work-queue";
import { addressOf } from "../crypto/keys";
import { mailFor, openMail, peersOf, type MailSender } from "../peers/known";
import { Frame, readFrame, readRelay, relayHello } from "../relay/frames";
import type { LinkHost } from "../relay/host";
import { followedSetup } from "../setup/record";
import { liveLink, readLink, type LinkState } from "../state";
import { takesForwarded, type Acted } from "./forward";
import { restoreLabels } from "./labels";

type Request = { library: unknown; force: boolean };
const refused = (status: number, message: string): Acted => ({ status, value: { message } });

/** A preview is a read of the current main computer's library. Its request and reply live in memory. */
export async function previewLabelRestore(request: Request, signal?: AbortSignal): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  if (link && setup?.role === "member") {
    const primary = peersOf(link).find(peer => peer.key === setup.primary);
    if (!primary?.address || !link.relay || !liveLink()?.welcomed) return refused(503, "This box can't reach the main computer.");
    if (!takesForwarded(primary.key, LABEL_RESTORE_FEATURE)) return refused(409, "Update the box on the main computer before previewing a restore from another computer.");
    const id = crypto.randomBytes(16).toString("base64url");
    const mail = mailFor(primary, { labelRestorePreview: { id, setup: setup.id, epoch: setup.epoch, request } }, link, false, { trackDelivery: false });
    if (!mail) return refused(400, "The preview request is too large to send.");
    return askPreview(link.relay, mail, link, primary.key, id, signal);
  }
  return previewHere(request);
}

// The temporary relay address has no box reachability or phone trust state. The mail itself is
// still authenticated with the requesting box's key, and only the primary can seal its reply.
function askPreview(relay: string, mail: Buffer, link: LinkState, primary: string, id: string, signal?: AbortSignal): Promise<Acted> {
  return new Promise(resolve => {
    const unavailable = refused(503, "The main computer didn't answer the preview.");
    if (signal?.aborted) return resolve(unavailable);
    const secret = crypto.randomBytes(32);
    const address = addressOf(secret);
    let socket: WebSocket;
    try { socket = new WebSocket(relay); } catch { return resolve(unavailable); }
    socket.binaryType = "arraybuffer";
    const finish = (reply: Acted) => {
      clearTimeout(timer);
      signal?.removeEventListener("abort", unanswered);
      socket.onopen = socket.onmessage = null;
      socket.onclose = socket.onerror = null;
      socket.close();
      resolve(reply);
    };
    const unanswered = () => finish(unavailable);
    const timer = setTimeout(unanswered, 20_000);
    signal?.addEventListener("abort", unanswered, { once: true });
    socket.onclose = socket.onerror = unanswered;
    socket.onopen = () => socket.send(relayHello(secret));
    socket.onmessage = event => {
      const message = readRelay(Buffer.from(event.data));
      if (message?.kind === "welcome" && message.address.equals(address)) return socket.send(mail);
      if (message?.kind === "unreachable") return unanswered();
      if (message?.kind !== "deliver") return;
      const frame = readFrame(message.payload);
      if (frame?.kind !== Frame.mail) return;
      const opened = openMail(frame.body, link);
      const given = opened?.payload.labelRestorePreview;
      if (opened?.sender.key === primary && isObject(given) && given.id === id && isObject(given.reply)
        && typeof given.reply.status === "number") finish({ status: given.reply.status, value: given.reply.value });
    };
  });
}

function previewHere({ library, force }: Request): Acted {
  const outcome = restoreLabels(library, true, force);
  return "message" in outcome ? refused(outcome.status, outcome.message) : { status: 200, value: outcome.result };
}

export function takeLabelRestorePreview(sender: MailSender, given: Record<string, unknown>, link: LinkState | null, host: LinkHost | null | undefined, from: Buffer | undefined): void {
  const { id, request } = given;
  if (typeof id !== "string" || !/^[A-Za-z0-9_-]{22}$/.test(id)) return;
  const setup = followedSetup(link);
  if (!link || !from || setup?.role !== "primary" || !isObject(request) || typeof request.force !== "boolean") return;
  const peers = peersOf(link);
  const address = from.toString("base64url");
  if (!peers.some(peer => peer.key === sender.key) || peers.some(peer => peer.address === address)) return;
  const result = given.setup === setup.id && given.epoch === setup.epoch
    ? previewHere({ library: request.library, force: request.force })
    : refused(409, "The main computer changed. Try the preview again.");
  const mail = mailFor({ key: sender.key, address }, { labelRestorePreview: { id, reply: result } }, link);
  if (mail) host?.send(mail);
}
