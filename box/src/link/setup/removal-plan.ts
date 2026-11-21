import fs from "node:fs";
import path from "node:path";
import type { MoveKind, MovePlace } from "../../shared/routes/moves";
import { askGroups } from "../areas/groups";
import { selectedProjects } from "../backup/engine";
import { previewProjects } from "../backup/projects";
import { ownComputerName } from "../computer-name";
import { botHere } from "../bots/here";
import { roots as storageRoots, storageInventory } from "../backup/locations";
import { BOTS } from "../home";
import { packQueues } from "../engine/queue";
import { verifyPayload } from "../moves/payload";
import { conversationOf } from "../engine/conversations";
import { bridgeTarget, readEngineFile } from "../engine/client";
import { isChatsFolder } from "../engine/chats";
import { isHiddenRun } from "../engine/sessions";
import { readSessions } from "../lists/sessions";
import { sessionInventory } from "../moves/session-files";
import { moveOf } from "../moves/records";
import { placesFor } from "../moves/move";
import { acceptedOwner, mayActivate } from "../ownership/resolve";
import { moveReference } from "../ownership/transfer";
import { ownKey } from "../peers/known";
import { readLink } from "../state";
import { packWorkQueues, unavailableQueuePayloads } from "../work-queue/moves";
import { startedByOf, worksFor } from "../work/tree";

import { allItems, queueDeletions } from "../work-queue/store";

type Item = { kind: MoveKind; id: string; name: string };
type Group = { group: { id: string; name: string; members?: string[] } };
const stays = ["unselected-project-files", "child-sessions", "native-agent-histories", "credentials-and-vault"] as const;

export async function removalPlan(projects?: unknown, moveTo?: string, scopedDestination?: string, verifiedSessions: ReadonlySet<string> = new Set()) {
  const selected = selectedProjects(projects);
  const link = readLink();
  const own = ownKey(link ?? {});
  if (!own) throw new Error("This computer has no setup identity.");
  const bridge = bridgeTarget();
  const signal = AbortSignal.timeout(15_000);
  const [engineSessions, agentSessions, groups] = await Promise.all([
    Promise.resolve().then(() => readSessions(readEngineFile(), signal, true)).catch(() => null),
    bridge ? readSessions(bridge, signal, true) : null,
    bridge ? askGroups<{ data?: Group[] }>(bridge, "GET", "", undefined, signal).catch(() => null) : null,
  ]);
  const groupList = groups?.status === 200 && Array.isArray(groups.value?.data) ? groups.value.data : null;
  const unresolved: string[] = [];
  let botIDs: string[] = [];
  try { botIDs = fs.readdirSync(BOTS); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "ENOENT") unresolved.push("bots"); }
  const bots = botIDs.flatMap(id => {
    if (!/^bot_[A-Za-z0-9]+$/.test(id)) return [];
    const bot = botHere(id);
    if (!bot) { unresolved.push(id); return []; }
    return [{ id, name: bot.described.name, session: bot.session }];
  });
  const roots = new Set(bots.map(bot => bot.session));
  const sourceOnly: Item[] = [];
  const exportOnly: Item[] = [];
  const candidates: Item[] = bots.map(({ id, name }) => ({ kind: "bot", id, name }));
  for (const { group } of groupList ?? []) {
    if (group?.id) candidates.push({ kind: "group", id: group.id, name: group.name || group.id });
    else unresolved.push("groups");
  }
  const sessions = new Map([...(engineSessions ?? []), ...(agentSessions ?? [])].map(session => [session.id, session]));
  const agents = new Set((agentSessions ?? []).map(session => session.id));
  const childIDs = new Map<string, string[]>();
  for (const session of sessions.values()) if (session.parentID) {
    const children = childIDs.get(session.parentID) ?? [];
    children.push(session.id);
    childIDs.set(session.parentID, children);
  }
  const familyOf = (root: string) => {
    const family = new Set([root]);
    for (const parent of family) for (const child of childIDs.get(parent) ?? []) family.add(child);
    return [...family];
  };
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
  const files: { item: string; name: string; status: "unavailable" | "export-only" }[] = [];
  const carriedChildren = new Set<string>();
  const coveredPaths = new Set<string>();
  const coveredQueues = new Set(verifiedSessions);
  const artifacts = [...storageInventory(undefined, kind => kind === "managedArtifacts")];
  if (artifacts.length) files.push({ item: "artifacts", name: "Artifact history and retained versions", status: "export-only" });
  for (const artifact of artifacts) {
    if (artifact.expectedSize === undefined || artifact.expectedSHA === undefined) continue;
    try {
      verifyPayload(storageRoots.artifacts!, artifact.name.slice("artifacts/".length), { size: artifact.expectedSize, sha256: artifact.expectedSHA });
    } catch {
      files.push({ item: "artifacts", name: `Retained artifact version ${path.basename(artifact.name).slice(0, 12)}`, status: "unavailable" });
      unresolved.push("artifacts");
    }
  }
  const canExport = exportOnly.filter(local);
  const botSessions = new Map(bots.map(bot => [bot.id, bot.session]));
  for (const item of candidates.filter(local)) {
    const ref = moveReference(item.kind, item.id);
    if (moveOf(item.id)) { moving.push(item); continue; }
    const root = item.kind === "bot" ? botSessions.get(item.id) : item.kind === "chat" ? item.id : undefined;
    const family = root ? familyOf(root) : [];
    if (family.length) {
      try {
        signal.throwIfAborted();
        const target = root && agents.has(root) ? bridge! : readEngineFile();
        if (family.some(id => agents.has(id) !== agents.has(root!))) {
          canExport.push(item);
          continue;
        }
        const records = await Promise.all(family.map(id => conversationOf(target, id, signal)));
        const waiting = await packQueues(target, family);
        const inventory = sessionInventory(family, records, waiting);
        for (const file of inventory.entries.keys()) coveredPaths.add(file);
        files.push(...inventory.unavailable.map(name => ({ item: item.id, name, status: "unavailable" as const })),
          ...inventory.exportOnly.map(name => ({ item: item.id, name, status: "export-only" as const })));
        if (inventory.unavailable.length || inventory.exportOnly.length) {
          unresolved.push(item.id);
          continue;
        }
      } catch { unresolved.push(item.id); continue; }
    }
    try {
      const unavailable = unavailableQueuePayloads(packWorkQueues(item.kind === "group" ? [item.id] : family));
      if (unavailable.length) {
        files.push(...unavailable.map(name => ({ item: item.id, name, status: "unavailable" as const })));
        unresolved.push(item.id);
        continue;
      }
    } catch { unresolved.push(item.id); continue; }
    const places = mayActivate(ref, { link }) ? await placesFor(item.kind, item.id, signal, false, scopedDestination).catch(() => null) : null;
    if (!places) { unresolved.push(item.id); continue; }
    const destinations = places.places.filter(place => place.ok).map(({ key, name, kind }) => ({ key, name, kind })) ?? [];
    if (moveTo && !destinations.some(place => place.key === moveTo) && !places.restricted) unresolved.push(item.id);
    else if (destinations.length) {
      movable.push({ ...item, destinations });
      for (const owner of item.kind === "group" ? [item.id] : family) coveredQueues.add(owner);
      family.slice(1).forEach(child => carriedChildren.add(child));
    }
    else if (places.restricted) canExport.push(item);
    else unresolved.push(item.id);
  }
  for (const attachment of storageInventory(undefined, kind => kind === "sessionFiles")) {
    const owner = attachment.name.split("/")[1]!;
    if (!verifiedSessions.has(owner) && (!attachment.file || !coveredPaths.has(attachment.file))) {
      files.push({ item: owner, name: path.basename(attachment.name), status: "export-only" });
    }
  }
  const queueItems = allItems();
  const unselectedQueues = new Set([
    ...queueItems.map(item => item.owner),
    ...queueDeletions().map(deletion => deletion.queue),
  ].filter(owner => !coveredQueues.has(owner)));
  for (const owner of unselectedQueues) files.push({ item: owner, name: "Work Queue items and history", status: "export-only" });
  const knownItems = new Set(queueItems.map(item => item.id));
  for (const entry of storageInventory(undefined, kind => kind === "queueFiles")) {
    const [, , , , item, leaf] = entry.name.split("/");
    if (item && leaf && !knownItems.has(item)) files.push({ item, name: leaf, status: "export-only" });
  }
  if (!engineSessions) unresolved.push("engine-sessions");
  if (!agentSessions) unresolved.push("agent-sessions");
  if (!groupList) unresolved.push("groups");
  return { unresolved: [...new Set(unresolved)], source: { key: own, name: ownComputerName() }, complete: !unresolved.length && !signal.aborted, movable, moving,
    exportOnly: canExport, stays, ...(files.length ? { files } : {}), childSessions: sourceOnly.filter(item => local(item) && !carriedChildren.has(item.id)),
    ...(selected.length ? { projects: selected, projectPreview: previewProjects(selected) } : {}) };
}
