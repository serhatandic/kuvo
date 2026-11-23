// The bots of your other computers, as each last told this box (`elsewhere` in `kuvo/bots`,
// PROTOCOL.md, `bots.elsewhere`): what a box already hears from its peers (their agents, "Peers"),
// as a list a client can draw, and change through this box (`PATCH` and `DELETE kuvo/bots/<id>`,
// "Held for another computer").

import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { LOOKS } from "../../shared/routes/bots";
import { computerName } from "../peers/computer-name";
import { readPeerAgents } from "../peers/directory";
import { peersOf } from "../peers/known";
import { standingOf } from "../peers/lists";
import { peerIsAway, readReach } from "../peers/reach";
import { readLink } from "../state";
import { whatDeleteTakes } from "./deletes";
import { awaySessions, readMoves } from "../moves/records";
import { botHere } from "./here";
import type { ToldAgents } from "../../shared/records";
import type { BotElsewhere, BotLook, HeldEdit, RefusedEdit } from "../../shared/routes/bots";
import type { Peer } from "../peers/known";
import type { LinkState } from "../state";

/** A computer as a client shows it: a machine by its name (its id on its Mac is how SSH reaches it). */
export const shownName = (peer: Peer): string => (peer.kind === "machine" && plainName(peer.name)) || computerName(peer);

/** A bot as this box knows it from the computer it lives on: its name and look. */
export interface KnownBot {
  name: string;
  look?: BotLook;
}

/** A bot one of your other computers lists, as that computer last told this box. */
export type ListedElsewhere = BotElsewhere & { session: string; name: string; kind: "mac" | "machine"; deletes: NonNullable<BotElsewhere["deletes"]> };

/** The bots your other computers list, each once: one two of them list (it's moving from one to the
 * other) is the one's that told this box last, as a held edit goes (`keeperOf`); one that lives
 * here is this box's own, whatever another still says. By computer, as this box knows them, then as
 * each listed its own. */
export function botsElsewhere(link: LinkState | null = readLink(), told: Record<string, ToldAgents> = readPeerAgents(), now = Date.now()): ListedElsewhere[] {
  const heard = (peer: Peer): number => told[peer.key]?.heard ?? told[peer.key]?.at ?? 0;
  const peers = peersOf(link).filter(peer => told[peer.key]);
  // (One its computer gave no name isn't listed by it: nothing to show it by.)
  const bots = (peer: Peer) => (told[peer.key]?.list ?? []).filter(agent => agent.kind === "bot" && agent.id && agent.name);
  const latest = new Map<string, Peer>();
  for (const peer of peers) {
    for (const agent of bots(peer)) {
      const other = latest.get(agent.id!);
      if (!other || heard(peer) > heard(other)) latest.set(agent.id!, peer);
    }
  }
  const retired = new Map(readMoves().filter(move => move.kind === "bot" && move.role === "from" && move.stage === "finishing").map(move => [move.thing, move.to]));
  const listed: ListedElsewhere[] = [];
  const away = new Set(awaySessions());
  const reach = readReach();
  for (const peer of peers) {
    const computer = shownName(peer);
    // Its computer not answering: said so, with when it was last heard from; only switching to a
    // new release, or quiet under a minute, it isn't away (`standingOf`).
    const seen = told[peer.key]?.heard ?? told[peer.key]?.at;
    const standing = standingOf(peer.key, peerIsAway(peer.key, told[peer.key]?.heard, reach), seen, reach, told, now);
    const off = standing.away ? { away: true as const, ...(standing.lastSeen !== undefined ? { lastSeen: standing.lastSeen } : {}) }
      : standing.updating ? { updating: true as const } : {};
    // Where its deleted bots' folders go, as it said; one that hasn't (from before it did) is said both ways.
    const trash = told[peer.key]?.trash;
    for (const agent of bots(peer)) {
      const destination = retired.get(agent.id!);
      const local = botHere(agent.id);
      if (destination ? destination !== peer.key : (latest.get(agent.id!)?.key !== peer.key || (local && !away.has(local.session)))) continue;
      // (Listed once by each computer; one that names a bot twice is told once.)
      latest.delete(agent.id!);
      listed.push({
        id: agent.id!,
        session: agent.session,
        name: agent.name,
        ...(agent.look ? { look: agent.look } : {}),
        ...(agent.about ? { job: agent.about } : {}),
        box: peer.key,
        computer,
        kind: peer.kind,
        // (Its wake-ups aren't told between computers: said without a count.)
        deletes: whatDeleteTakes({ computer, folder: trash ? { to: trash } : { to: "unsaid" } }),
        ...off,
      });
    }
  }
  return listed;
}

/** Each bot your other computers list, by its id: its name and look as last told. */
export const knownOf = (listed: ListedElsewhere[]): Map<string, KnownBot> => new Map(listed.map(bot => [bot.id, { name: bot.name, ...(bot.look ? { look: bot.look } : {}) }]));
export const knownElsewhere = (link: LinkState | null = readLink(), told: Record<string, ToldAgents> = readPeerAgents()): Map<string, KnownBot> => knownOf(botsElsewhere(link, told));

const LOOK_WORDS: { [Part in keyof BotLook]: readonly string[] } = { color: LOOKS.colors, shape: LOOKS.shapes, face: LOOKS.faces };
const lookIn = (change: Record<string, unknown> | undefined): BotLook | undefined => {
  const look = change?.look;
  return isObject(look) && (Object.keys(LOOK_WORDS) as (keyof BotLook)[]).every(part => LOOK_WORDS[part].includes(look[part] as string)) ? { color: look.color, shape: look.shape, face: look.face } as BotLook : undefined;
};

/** The bots of your other computers as `kuvo/bots` says them, for a client to draw as they are:
 * those listed, each as you left it (the name and look a change held here gives it over its
 * computer's) and marked `waiting` or `refused`; then any this box only holds an edit for, or heard
 * a refusal of, that no computer lists now (`unlisted`). Never one that lives here. */
export function elsewhereSaid(listed: ListedElsewhere[], held: HeldEdit[], refused: RefusedEdit[]): BotElsewhere[] {
  const heldFor = new Map(held.map(each => [each.bot, each]));
  const refusedFor = new Map(refused.map(each => [each.bot, each]));
  const left = (bot: { name?: string; look?: BotLook }, edit: HeldEdit | undefined) => {
    const name = typeof edit?.change?.name === "string" && edit.change.name ? edit.change.name : bot.name;
    const look = lookIn(edit?.change) ?? bot.look;
    return { ...(name ? { name } : {}), ...(look ? { look } : {}) };
  };
  const marks = (id: string) => ({ ...(heldFor.has(id) ? { waiting: true as const } : {}), ...(refusedFor.has(id) ? { refused: true as const } : {}) });
  const all: BotElsewhere[] = listed.map(({ name: _name, look: _look, ...bot }) => ({ ...bot, ...left({ name: _name, look: _look }, heldFor.get(bot.id)), ...marks(bot.id) }));
  const seen = new Set(listed.map(bot => bot.id));
  for (const each of [...held, ...refused]) {
    if (seen.has(each.bot) || botHere(each.bot)) continue;
    seen.add(each.bot);
    all.push({ id: each.bot, ...left(each, heldFor.get(each.bot)), box: each.box, computer: each.computer, ...marks(each.bot), unlisted: true });
  }
  return all;
}
