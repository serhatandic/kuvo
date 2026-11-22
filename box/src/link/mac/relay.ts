// This Mac's box on the relay (PROTOCOL.md, "This Mac's box: the relay")
// On a Mac, `serve` holds the relay connection a machine's `run` holds, in the same process: your
// phones' sessions, paired and pairing as on a machine, answered here. Its keys stay in your
// Keychain: the app gives them as the link starts (`link/identity`), and they're kept in memory
// only; without them the link stays off and says why, and never makes keys of its own. Its app says
// whether it's wanted (`kuvo/link/config`: the phone link on, machines to meet), which it keeps for
// when it starts again before the app opens. Your other computers' mail is taken here, as a machine
// takes it: documents, agents, letters, group chats'. What a phone asks it answers itself, but the
// Mac's own routes (`MAC_APP_ROUTES`) where there's an app to ask, which go to it through the host
// (`link/route`); of those it answers what it can of its machines while the app isn't there
// (`machines`). With only Kuvo Code's desktop app there's none, and it answers those too (`answersAlone`).

import type { RouteFeature } from "../../shared/routes/feature";
import { engineConnect } from "../areas/engine";
import { docsChanged } from "../documents/exchange";
import { noteSetupNow } from "../setup/lead";
import { authorizeDeviceConnection, refreshPhoneTrust } from "../documents/phone-sync";
import { peerPlaces, readEngineFile } from "../engine/client";
import { announce } from "../peers/agents";
import { peerMail, receiveMail } from "../peers/mail";
import { confirmPhone, currentPhonePairings, deviceConnectionsAllowed } from "../phones";
import { connectOnce, nextTry } from "../relay/connection";
import { LinkHost } from "../relay/host";
import { versionedRoutes } from "../server/format";
import { createBoxDispatcher } from "../server/router";
import { OWN_HOME_MAC } from "../home";
import { boxAnswers } from "./keeper-runs";
import { openAppHost } from "../host-interface";
import { macLink, readLink } from "../state";
import { appAnswer, passedOn } from "./app-route";
import { macKeys, macLinkCheck, macLinkWait, macNeeded, ownMacConfig, savedMacConfig, setLinkStatus, tellUntold } from "./link";
import { isMachinesRoute, isRetiredMachineRoute, machinesRoute } from "./machines";
import type { AppTiming } from "./machines";
import type { EngineRecord } from "../../shared/records";
import type { AnswerPiece, Asker, LinkHandler, LinkRequest } from "../server/context";
import type { BoxDispatcher } from "../server/router";
import type { LinkState } from "../state";

// What this Mac's app answers a phone, by area, where there's an app to ask (the retired Mac app,
// which ran this box in its own folder or registered its host with it): its machines (which this
// box answers in part with the app closed: machines.ts), the machines your phones reach directly,
// what its panel's browser asks you, its macOS services (`kuvo/mac`), and its engine's restart
// while the app runs this box itself (`boxAnswers`). The one list of them: the app keeps none, and
// hands its own requests for anything else to this box. Everything else a phone asks this box
// answers itself, as a machine's does, with the app open or closed; and with only Kuvo Code's
// desktop app, these too (`answersAlone`).
const MAC_APP_ROUTE_FEATURES = {
  "kuvo/machines": "machines",
  "kuvo/hosts": "hosts",
  "kuvo/asks": "asks",
  "kuvo/mac": "mac",
  "kuvo/engine": "engine.restart",
} as const satisfies Record<string, RouteFeature>;
export const MAC_APP_ROUTES = Object.keys(MAC_APP_ROUTE_FEATURES);
export const macAppAnswers = (route: string): boolean => MAC_APP_ROUTES.some(area => route === area || route.startsWith(`${area}/`)) && !isRetiredMachineRoute(route);

export { boxAnswers, keeperRuns } from "./keeper-runs";

const APP_NOT_ANSWERING = "Kuvo Code on this Mac isn't answering. Try again in a moment.";
/** A Mac's box in a home of its own, its app closed (it said so, or never registered): as its helper says it. */
const APP_CLOSED = "Kuvo Code isn't open on this Mac.";

// What a phone asks of the Mac's own routes, answered by the app (`link/route`), as it comes; 503,
// at once, when the app isn't answering, and a stream that ends when the app goes.
async function* toApp(request: LinkRequest, phone: Asker | undefined, signal?: AbortSignal): AsyncGenerator<AnswerPiece> {
  const response = await appAnswer(request, phone, signal);
  if (response) return yield* passedOn(response, signal);
  yield { head: { status: 503, headers: { "content-type": "application/json" } } };
  yield { body: Buffer.from(JSON.stringify({ message: OWN_HOME_MAC && !openAppHost() ? APP_CLOSED : APP_NOT_ANSWERING })) };
}

/** Whether this box answers the Mac's own routes itself: in a home of its own with no app's host
 * registered, which is every Mac that has only Kuvo Code's desktop app (a client: it answers
 * nothing for its box). Its machines and what your phones reach (`own-machines.ts`), While Agents
 * Work (`own-mac.ts`), what its browser asks you and its engine's restart, as a machine's box
 * answers them. The retired app, where it registered a host, is still asked while it's open. */
export const answersAlone = (): boolean => OWN_HOME_MAC && !openAppHost();

// A phone's request on this Mac: the app's own areas from the app (its machines answered here too
// while it isn't there), everything else here (from #191 step 2; before, the box answered only the
// areas that had moved onto it, and the app the rest). In a home of its own with no app to ask,
// all of it here (`answersAlone`).
export function macCompatibilityFront(handle: LinkHandler, timing?: AppTiming): LinkHandler {
  return async function* (request, phone, signal) {
    const route = String(request.path ?? "").split("?")[0];
    if (!macAppAnswers(route) || boxAnswers(route) || answersAlone()) return yield* handle(request, phone, signal);
    if (isMachinesRoute(route)) return readLink()?.setup?.checkpointed ? yield* handle(request, phone, signal) : yield* machinesRoute(request, phone, signal, route, timing);
    yield* toApp(request, phone, signal);
  };
}

export function macFront(handle?: LinkHandler, timing?: AppTiming): LinkHandler {
  const front = (next: LinkHandler) => macCompatibilityFront(next, timing);
  return handle ? versionedRoutes(front(handle)) : createBoxDispatcher().entry({ front });
}

// This Mac's box's link: up while it's wanted, its keys from the app, your phones answered here,
// what's the app's yet passed to it. Nothing it meets (the relay gone, a frame that isn't one, the
// app away) stops the box's own routes.
export async function runMacLink(dispatcher: BoxDispatcher = createBoxDispatcher()): Promise<void> {
  let attempt = 0;
  macLink.stopped = false;
  // As its app last said, until it says again: the link comes back after a restart, the app closed.
  // In a home of its own with nothing an app said, the box's own (`ownMacConfig`).
  macLink.config ??= savedMacConfig() ?? ownMacConfig();
  while (!macLink.stopped) {
    try {
      const config = macLink.config;
      // Its keys as soon as it's on (a pairing link needs them before any phone's paired).
      const hadIdentity = !!macLink.overlay;
      if (!(await macKeys())) {
        await macLinkWait(30_000);
        continue;
      }
      if (macLink.stopped) break;
      if (!hadIdentity && macLink.overlay) noteSetupNow(Date.now(), { soon: 0 });
      if (!macNeeded(readLink())) {
        setLinkStatus("off");
        attempt = 0;
        await macLinkWait(60_000);
        continue;
      }
      // (Wanted, with its keys: the app's overlay gives it these, and its config is there, as
      // macNeeded says no without one.)
      const link = readLink() as LinkState & Required<Pick<LinkState, "key" | "secret" | "relay">>;
      setLinkStatus("connecting");
      let engine: Partial<EngineRecord> = {};
      try { engine = readEngineFile(); } catch {}
      const host: LinkHost = new LinkHost({
        key: Buffer.from(link.key, "base64url"),
        hello: { v: 1, kind: "mac", name: config!.name, app: config!.app, engine: engine.version ?? null },
        // Phones only while the phone link's on (the relay can be up for your machines alone).
        authorize: (key, hello, address, signal) => deviceConnectionsAllowed(readLink(), !!macLink.config?.phones) && authorizeDeviceConnection(host, key, hello, address, signal),
        confirmed: confirmPhone,
        // Mail from your machines, taken here as a machine takes it (`peerMail`).
        mail: (from, body) => receiveMail(body, host, (sender, payload) => peerMail(sender, payload, host, peerPlaces), undefined, from),
        handle: dispatcher.entry({
          front: macCompatibilityFront,
          info: { name: config!.name },
          phonesChanged: changed => host.keepOnly(currentPhonePairings(changed)),
          hold: macLink.hold,
        }),
        connect: engineConnect(readEngineFile, (): number => host.unsent?.() ?? 0),
        send: () => {},
      });
      macLink.host = host;
      // A pairing can run out while it's up: looked at now and then.
      const looking = setInterval(macLinkCheck, 30_000);
      const started = Date.now();
      const welcomed = await connectOnce(link.relay, Buffer.from(link.secret, "base64url"), host, () => {
        setLinkStatus("connected");
        // Meeting your machines: who this Mac's agents are, asking for theirs; this Mac's
        // documents as they are now, and its digest to each; and those removed while it was away
        // hear it.
        announce(host, readEngineFile, undefined, undefined, true).catch(() => {});
        try { docsChanged(host, { greet: true }); } catch {}
        try { refreshPhoneTrust(host); } catch {}
        try { tellUntold(); } catch {}
      });
      clearInterval(looking);
      if (macLink.host === host) macLink.host = null;
      if (macLink.stopped) break;
      // Not wanted any more: off now, not after a wait.
      if (!macNeeded(readLink())) {
        setLinkStatus("off");
        attempt = 0;
        continue;
      }
      const next = nextTry(attempt, welcomed, Date.now() - started);
      attempt = next.attempt;
      if (!welcomed) setLinkStatus("failed", `Can't reach the relay at ${new URL(link.relay).host}.`);
      else setLinkStatus("connecting");
      await macLinkWait(next.wait);
    } catch (error) {
      macLink.host = null;
      setLinkStatus("failed", String((error as Error | undefined)?.message ?? error));
      await macLinkWait(5_000);
    }
  }
  setLinkStatus("off");
}
