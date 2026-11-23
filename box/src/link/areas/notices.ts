// What this box told you lately (PROTOCOL.md, "kuvo/notices"): its notices, as it made them, for
// clients to show as their own notifications, so what's news and how it's said is the box's alone.
// `telling`: whether the box is telling its news now (this Mac's box always; a machine's while a
// bridge that does runs). A client shows its own only where it isn't.

import { NOTICE_COPY } from "../../shared/routes/box";
import { listComputer } from "../lists/computer";
import { peersOf } from "../peers/known";
import { peerSnapshots } from "../peers/lists";
import { pendingComputers } from "../peers/pending";
import { readReach } from "../peers/reach";
import { readLink } from "../state";
import type { LinkHost } from "../relay/host";
import { followed, followedLive } from "../follow";
import { TOLD_FOLDER, tellingNow, toldFollowers, toldLately, noticeIsRecent } from "../notices/told";
import type { AnswerPiece, RouteContext } from "../server/context";
import { copyStanding } from "../peers/lists";

const lately = () => ({ telling: tellingNow(), ...toldLately() });

/** `GET kuvo/notices`: what's been told lately; `GET kuvo/notices/events`: that, a line now and one each time it changes. */
export async function* noticesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, route, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  if (route === "kuvo/notices") return yield* json(200, lately());
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  return yield* followed(lately, toldFollowers, signal, { folders: [TOLD_FOLDER], recursive: false });
}

type NoticeCopy = ReturnType<typeof lately>;
export const noticeCopies = peerSnapshots<NoticeCopy>("noticeCopy", "notices-v1.json", "line.merged-notices", NOTICE_COPY, undefined, 0);

// The notice and its id stay the publisher's, even if a client changes which box it follows.
export const shareNotices = (host: Pick<LinkHost, "send">) => noticeCopies.push(host, async () => lately());

export async function* setupNoticesRoute(context: RouteContext): AsyncGenerator<AnswerPiece> {
  const { request, signal, json } = context;
  if (request.method.toUpperCase() !== "GET") return yield* json(405, {});
  yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
  yield* followedLive(async () => {
    const link = readLink();
    const reach = readReach();
    const now = Date.now();
    const here = { computer: listComputer(link), away: false, ...lately() };
    const peers = await Promise.all(peersOf(link).map(async peer => {
      const saved = await noticeCopies.read(peer.key);
      if (!saved) return [];
      // (Away by the one rule, `standingOf`.)
      const { away, lastSeen } = copyStanding(peer.key, saved.seen, reach, undefined, now);
      return [{
        computer: { id: peer.key, name: peer.name }, away, ...(away ? { lastSeen } : {}),
        telling: !away && saved.telling, notices: saved.notices.filter(notice => noticeIsRecent(notice, now)),
      }];
    }));
    const copies = [here, ...peers.flat()];
    return {
      pendingComputers: pendingComputers("line.merged-notices", key => copies.some(copy => copy.computer.id === key), link),
      origins: copies.map(({ notices: _, ...origin }) => origin),
      notices: copies.flatMap(({ notices, telling: _, ...origin }) => notices.map(notice => ({ ...notice, ...origin })))
        .sort((a, b) => a.at - b.at || a.computer.id.localeCompare(b.computer.id) || a.id.localeCompare(b.id)),
    };
  }, tell => {
    toldFollowers.add(tell);
    noticeCopies.followers.add(tell);
    return () => { toldFollowers.delete(tell); noticeCopies.followers.delete(tell); };
  }, signal, { every: 3_000 });
}
