import type { MoveKind, MovePlace } from "../../shared/routes/moves";
import { askGroups } from "../areas/groups";
import { selectedProjects } from "../backup/engine";
import { ownComputerName } from "../computer-name";
import { listBots } from "../bots/list";
import { bridgeTarget, readEngineFile } from "../engine/client";
import { isChatsFolder } from "../engine/chats";
import { isHiddenRun } from "../engine/sessions";
import { readSessions } from "../lists/sessions";
import { moveOf } from "../moves/records";
import { placesFor } from "../moves/move";
import { acceptedOwner, mayActivate } from "../ownership/resolve";
import { moveReference } from "../ownership/transfer";
import { ownKey } from "../peers/known";
import { readLink } from "../state";
import { startedByOf, worksFor } from "../work/tree";

type Item = { kind: MoveKind; id: string; name: string };
type Group = { group: { id: string; name: string; members?: string[] } };
const stays = ["unselected-project-files", "child-sessions", "native-agent-histories", "credentials-and-vault"] as const;

export async function removalPlan(projects?: unknown) {
  const selected = selectedProjects(projects);
  const link = readLink();
  const own = ownKey(link ?? {});
  if (!own) throw new Error("This computer has no setup identity.");
  const bridge = bridgeTarget();
  const signal = AbortSignal.timeout(15_000);
  const [engineSessions, agentSessions, groups] = await Promise.all([
    Promise.resolve().then(() => readSessions(readEngineFile(), signal)).catch(() => null),
    bridge ? readSessions(bridge, signal) : null,
    bridge ? askGroups<{ data?: Group[] }>(bridge, "GET", "", undefined, signal).catch(() => null) : null,
  ]);
  const groupList = groups?.status === 200 && Array.isArray(groups.value?.data) ? groups.value.data : null;
  const bots = listBots();
  const roots = new Set(bots.map(bot => bot.session));
  const sourceOnly: Item[] = [];
  const exportOnly: Item[] = [];
  const candidates: Item[] = bots.map(({ id, name }) => ({ kind: "bot", id, name }));
  for (const { group } of groupList ?? []) {
    if (group?.id) candidates.push({ kind: "group", id: group.id, name: group.name || group.id });
  }
  const sessions = new Map([...(engineSessions ?? []), ...(agentSessions ?? [])].map(session => [session.id, session]));
  const parents = new Set([...sessions.values()].map(session => session.parentID).filter(Boolean));
  for (const session of sessions.values()) {
    if (roots.has(session.id) || isHiddenRun(session)) continue;
    const item: Item = { kind: "chat", id: session.id, name: session.title || session.id };
    if (session.parentID || startedByOf(session) || worksFor(session)) sourceOnly.push(item);
    else if (!isChatsFolder(session.location?.directory ?? session.directory)) exportOnly.push(item);
    else candidates.push(item);
  }
  const local = ({ kind, id }: Item) => {
    const owner = acceptedOwner(link, moveReference(kind, id));
    return !owner || owner === own;
  };
  const movable: (Item & { destinations: Pick<MovePlace, "key" | "name" | "kind">[] })[] = [];
  const moving: Item[] = [];
  const canExport = exportOnly.filter(local);
  const botSessions = new Map(bots.map(bot => [bot.id, bot.session]));
  for (const item of candidates.filter(local)) {
    const ref = moveReference(item.kind, item.id);
    if (moveOf(item.id)) { moving.push(item); continue; }
    if (item.kind !== "group" && parents.has(item.kind === "bot" ? botSessions.get(item.id) : item.id)) {
      canExport.push(item);
      continue;
    }
    const places = mayActivate(ref, { link }) ? await placesFor(item.kind, item.id, signal, false) : null;
    const destinations = places?.places.filter(place => place.ok).map(({ key, name, kind }) => ({ key, name, kind })) ?? [];
    if (destinations.length) movable.push({ ...item, destinations });
    else canExport.push(item);
  }
  return { source: { key: own, name: ownComputerName() }, complete: Boolean(engineSessions && agentSessions && groupList && !signal.aborted), movable, moving,
    exportOnly: canExport, stays, childSessions: sourceOnly.filter(local), ...(selected.length ? { projects: selected } : {}) };
}
