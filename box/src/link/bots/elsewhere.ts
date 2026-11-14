// The bots of your other computers, as each last told this box (`elsewhere` in `kuvo/bots`,
// PROTOCOL.md, `bots.elsewhere`): what a box already hears from its peers (their agents, "Peers"),
// as a list a client can draw, and change through this box (`PATCH` and `DELETE kuvo/bots/<id>`,
// "Held for another computer"). Only from your own computers: nothing from one that isn't only
// yours (`isApart`), whatever its list says.

import { plainName } from "../../shared/text";
import { computerName } from "../peers/computer-name";
import { isApart } from "../peers/apart";
import { readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { readLink } from "../state";
import { whatDeleteTakes } from "./deletes";
import { botHere } from "./here";
import type { ToldAgents } from "../../shared/records";
import type { BotElsewhere, BotLook } from "../../shared/routes/bots";
import type { Peer } from "../peers/known";
import type { LinkState } from "../state";

/** A computer as a client shows it: a machine by its name (its id on its Mac is how SSH reaches it). */
export const shownName = (peer: Peer): string => (peer.kind === "machine" && plainName(peer.name)) || computerName(peer);

/** A bot as this box knows it from the computer it lives on: its name and look. */
export interface KnownBot {
  name: string;
  look?: BotLook;
}

/** The bots your other computers list, each once: one two of them list (it's moving from one to the
 * other) is the one's that told this box last, as a held edit goes (`keeperOf`); one that lives
 * here is this box's own, whatever another still says. By computer, as this box knows them, then as
 * each listed its own. */
export function botsElsewhere(link: LinkState | null = readLink(), told: Record<string, ToldAgents> = readPeerAgents()): BotElsewhere[] {
  const heard = (peer: Peer): number => told[peer.key]?.heard ?? told[peer.key]?.at ?? 0;
  const peers = peersOf(link).filter(peer => told[peer.key] && !isApart(peer.key, link, told));
  const latest = new Map<string, Peer>();
  for (const peer of peers) {
    for (const agent of told[peer.key]?.list ?? []) {
      if (agent.kind !== "bot" || !agent.id) continue;
      const other = latest.get(agent.id);
      if (!other || heard(peer) > heard(other)) latest.set(agent.id, peer);
    }
  }
  const bots: BotElsewhere[] = [];
  for (const peer of peers) {
    const computer = shownName(peer);
    for (const agent of told[peer.key]?.list ?? []) {
      if (agent.kind !== "bot" || !agent.id || latest.get(agent.id)?.key !== peer.key || botHere(agent.id)) continue;
      // (Listed once by each computer; one that names a bot twice is told once.)
      latest.delete(agent.id);
      bots.push({
        id: agent.id,
        session: agent.session,
        name: agent.name,
        ...(agent.look ? { look: agent.look } : {}),
        ...(agent.about ? { job: agent.about } : {}),
        box: peer.key,
        computer,
        kind: peer.kind,
        ...(heard(peer) ? { heard: heard(peer) } : {}),
        // (Its wake-ups aren't told between computers: said without a count. A Mac's app takes
        // its folder on to the Trash; a machine keeps it in Kuvo Code's trash folder.)
        deletes: whatDeleteTakes({ computer, folder: peer.kind === "mac" ? { to: "trash" } : { to: "kuvo" } }),
      });
    }
  }
  return bots;
}

/** Each bot your other computers list, by its id: its name and look as last told. */
export function knownElsewhere(link: LinkState | null = readLink(), told: Record<string, ToldAgents> = readPeerAgents()): Map<string, KnownBot> {
  return new Map(botsElsewhere(link, told).map(bot => [bot.id, { name: bot.name, ...(bot.look ? { look: bot.look } : {}) }]));
}
