// The active rows and counts shared by the bot list and conversation activity.
import { pendingComputers } from "../peers/pending";
import { keptList, type Kept } from "../lists/sessions";
import { readActivity } from "./activity-peers";
import { peerIsAway, readReach } from "../peers/reach";
import { movedOf, peersOf } from "../peers/known";
import { readLink } from "../state";
import type { ActivityFact } from "../../shared/records";
import type { Activity } from "../../shared/routes/work";
import { standingOf } from "../peers/lists";
import { readPeerAgents } from "../peers/directory";

export function localActivity(list: Kept | null = keptList()): ActivityFact[] {
  const sessions = list?.sessions ?? {};
  return Object.entries(sessions).flatMap(([session, { facts }]) => {
    const asking = facts.ownAsk || (facts.asked !== undefined && !facts.childAsk);
    const working = facts.running && !facts.childOnly && !facts.childAsk;
    if (facts.away || facts.archived || facts.kind === "hidden" || facts.reviewPart || (!asking && !working)) return [];
    const line = new Set<string>();
    const parents = [facts.parent, ...(facts.line ?? [])];
    let bot = facts.bot;
    for (let i = 0; i < parents.length && line.size < 24; i++) {
      const parent = parents[i];
      if (!parent || parent === session || line.has(parent)) continue;
      line.add(parent);
      const above = sessions[parent]?.facts;
      bot ??= above?.bot;
      parents.push(above?.parent, ...(above?.line ?? []));
    }
    return [{
      session, line: [...line], ...(bot ? { bot } : {}), ...(facts.kind === "subagent" ? { subagent: true } : {}),
      title: facts.title ?? (facts.kind === "subagent" ? "Subagent" : "New Session"),
      state: asking ? "asking" as const : "working" as const,
    }];
  });
}

type ShownActivity = ActivityFact & { computer?: string; lastSeen?: number };

export function activityFacts(list: Kept | null = keptList()): ShownActivity[] {
  const link = readLink();
  const moved = movedOf(link);
  const rows = new Map(localActivity(list).map(row => [row.session, row as ShownActivity]));
  const reach = readReach();
  const told = readPeerAgents();
  const now = Date.now();
  const peers = peersOf(link).flatMap(peer => {
    const snapshot = readActivity(peer.key);
    // (Away by the one rule, `standingOf`.)
    return snapshot ? [{ key: peer.key, ...snapshot, away: standingOf(peer.key, peerIsAway(peer.key, snapshot.heard, reach, now), snapshot.heard, reach, told, now).away }] : [];
  }).sort((a, b) => Number(a.away) - Number(b.away) || b.heard - a.heard);
  for (const peer of peers) {
    for (const row of peer.rows) {
      const home = moved.get(row.session);
      if (home ? home === peer.key : !rows.has(row.session)) {
        rows.set(row.session, { ...row, computer: peer.key, ...(peer.away ? { lastSeen: peer.heard } : {}) });
      }
    }
  }
  return [...rows.values()];
}

/** The sessions at work under `root` (its own included) and the bot's; `besides`: what works for
 * `root` besides its own turn, so it's left out (a conversation's activity never lists itself). */
export function activityOf(rows: ReturnType<typeof activityFacts>, root?: string, bot?: string, besides = false): Activity {
  const sessions: Activity["sessions"][number][] = [];
  const lastSeenSessions: NonNullable<Activity["lastSeenSessions"]>[number][] = [];
  const counts = { working: 0, asking: 0 };
  for (const { session, title, state, line, bot: owner, computer, subagent, lastSeen } of rows) {
    if (besides && session === root) continue;
    if (!(root && (session === root || line.includes(root))) && !(bot && owner === bot)) continue;
    const row = { session, title, state, ...(computer ? { computer } : {}), ...(subagent ? { subagent } : {}) };
    if (lastSeen !== undefined && computer) {
      lastSeenSessions.push({ ...row, computer, lastSeen });
    } else {
      counts[state]++;
      sessions.push(row);
    }
  }
  sessions.sort((a, b) => a.session.localeCompare(b.session));
  lastSeenSessions.sort((a, b) => a.session.localeCompare(b.session));
  const pending = pendingComputers("line.pending-computers", key => readActivity(key) !== undefined);
  return { ...counts, sessions, ...(pending.length ? { pendingComputers: pending } : {}), ...(lastSeenSessions.length ? { lastSeenSessions } : {}) };
}
