// The active rows and counts shared by the bot list and conversation activity.
import { keptList, type Kept } from "../lists/sessions";
import { readActivity } from "./activity-peers";
import { peerIsAway, readReach } from "../peers/reach";
import { movedOf, peersOf } from "../peers/known";
import { readLink } from "../state";
import type { ActivityFact } from "../../shared/records";
import type { Activity } from "../../shared/routes/work";

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

export function activityFacts(list: Kept | null = keptList()): (ActivityFact & { computer?: string })[] {
  const link = readLink();
  const moved = movedOf(link);
  const rows = new Map(localActivity(list).map(row => [row.session, row as ActivityFact & { computer?: string }]));
  const reach = readReach();
  const now = Date.now();
  const peers = peersOf(link).flatMap(peer => {
    const snapshot = readActivity(peer.key);
    return snapshot ? [{ key: peer.key, ...snapshot }] : [];
  }).sort((a, b) => b.heard - a.heard);
  for (const peer of peers) {
    if (peerIsAway(peer.key, peer.heard, reach, now)) continue;
    for (const row of peer.rows) {
      const home = moved.get(row.session);
      if (home ? home === peer.key : !rows.has(row.session)) rows.set(row.session, { ...row, computer: peer.key });
    }
  }
  return [...rows.values()];
}

/** The sessions at work under `root` (its own included) and the bot's; `besides`: what works for
 * `root` besides its own turn, so it's left out (a conversation's activity never lists itself). */
export function activityOf(rows: ReturnType<typeof activityFacts>, root?: string, bot?: string, besides = false): Activity {
  const sessions: Activity["sessions"][number][] = [];
  const counts = { working: 0, asking: 0 };
  for (const { session, title, state, line, bot: owner, computer, subagent } of rows) {
    if (besides && session === root) continue;
    if (!(root && (session === root || line.includes(root))) && !(bot && owner === bot)) continue;
    counts[state]++;
    sessions.push({ session, title, state, ...(computer ? { computer } : {}), ...(subagent ? { subagent } : {}) });
  }
  sessions.sort((a, b) => a.session.localeCompare(b.session));
  return { ...counts, sessions };
}
