// Rows the engine's session API doesn't expose: message activity and the engine's older worktree records.
import { readEngineDatabases, TABLES_SQL } from "../../usage/engine-db";
import { ENGINE_DATA } from "../home";
import type { EngineSession } from "./sessions";

export interface EngineListFacts {
  activity: Map<string, number>;
  available: boolean;
  worktrees: Record<string, string>;
}

const legacyActivity = `SELECT id, coalesce((SELECT time_created FROM message WHERE session_id = session.id
  ORDER BY time_created DESC LIMIT 1), time_created) AS seen FROM session`;
const activity = `SELECT id, coalesce((SELECT time_created FROM session_message WHERE session_id = session_v2.id
  ORDER BY seq DESC LIMIT 1), time_created) AS seen FROM session_v2`;
const trees = `SELECT worktree.directory, project.worktree AS project FROM worktree
  JOIN project ON project.id = worktree.project_id WHERE project.vcs = 'git'`;
const json = (activity: string, worktrees = "SELECT NULL AS directory, NULL AS project WHERE 0") => `SELECT json_object(
  'activity', (SELECT json_group_object(id, seen) FROM (${activity})),
  'worktrees', (SELECT json_group_object(directory, project) FROM (${worktrees}))
)`;

export async function engineListFacts(): Promise<EngineListFacts> {
  const facts: EngineListFacts = { activity: new Map(), available: false, worktrees: Object.create(null) };
  try {
    const answers = await readEngineDatabases(ENGINE_DATA, {
      tables: TABLES_SQL, v1: json(legacyActivity), v2: json(activity, trees), both: json(`${legacyActivity} UNION ALL ${activity}`, trees),
    });
    for (const answer of answers ?? []) {
      if (!answer.json) continue;
      const { activity, worktrees } = JSON.parse(answer.json);
      facts.available = true;
      for (const [id, at] of Object.entries(activity)) {
        if (typeof at === "number" && at > (facts.activity.get(id) ?? 0)) facts.activity.set(id, at);
      }
      for (const [folder, project] of Object.entries(worktrees)) {
        if (typeof project === "string" && project !== folder) facts.worktrees[folder] = project;
      }
    }
  } catch {}
  return facts;
}

export function withEngineActivity(sessions: EngineSession[], facts: EngineListFacts): EngineSession[] {
  return sessions.map(session => {
    const activity = facts.activity.get(session.id);
    return activity === undefined ? session : { ...session, time: { ...session.time, activity } };
  });
}
