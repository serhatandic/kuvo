// The Work Queue's routes (PROTOCOL.md, "The Work Queue"): an owner's view and its events, one
// item whole and its files, what `#n` refs name (for chips), the statuses, priorities and labels, and
// what you do from a client: add an item, change what it says and where it stands, reorder, delete,
// bring back, keep the labels, and import an old backlog. Agents do the same through the tools
// (`kuvo/work-queue/tools`).

import fs from "node:fs";
import path from "node:path";
import { isObject, parsed } from "../../shared/json";
import { watchFolders } from "../follow";
import { targetsOf } from "../letters/tools";
import { atWorkNow } from "../work/background";
import { provedRequest } from "./vault";
import { addLabel, changeLabel, LABEL_COLORS, labels, LABELS_FILE, removeLabel, type Label } from "../work-queue/labels";
import { isClosed, PRIORITIES, statusName, STATUSES } from "../work-queue/statuses";
import { assignable } from "../work-queue/assignees";
import { stripLabel } from "../work-queue/ops";
import { frozen } from "../moves/records";
import { importItems, importWords, parseBacklog, readBacklogFiles } from "../work-queue/import";
import { primaryOf } from "../work-queue/numbers";
import { changedThere, itemElsewhere, itemsElsewhere, tellQueuesAgain } from "../work-queue/across";
import { carriedFiles, Refusal, refuseMoving, TooBig } from "../work-queue/ops";
import { TOLD, type ToldItem } from "../work-queue/told";
import { readLink } from "../state";
import { ownKey } from "../peers/known";
import { awaySince } from "../peers/reach";
import type { QueueCounts } from "../../shared/routes/work-queue";
import { allItems, changeQueues, matching, OWNER, queueFollowers, readAll, WORK_QUEUE, type Item } from "../work-queue/store";
import { readSeen, SEEN_FILE } from "../work-queue/github";
import { callQueueTool, QUEUE_INSTRUCTIONS, QUEUE_TOOLS } from "../work-queue/tools";
import { canOf, overdueOf, todayHere, factsOf, inView, linkedTo, nameOf, overseenBy, ownersOf, refOf, summaryOf, type Computer, type Facts, type ItemSummary } from "../work-queue/view";
import { ATTACHMENTS, forwardAct, seekElsewhere, takesForwarded, type Acted } from "../work-queue/forward";
import { actHere, addHere, fileOf, GITHUB_WAIT, MOST_FORWARDED_FILE, withCan } from "../work-queue/acts";
import { fromLocalApp, type AnswerPiece, type RouteContext } from "../server/context";
import type { Target } from "../../shared/records";

export const WORK_QUEUE_ROUTE = /^kuvo\/work-queue(?:\/(tools|events|refs|labels|import|items(?:\/([^/]+)(?:\/(restore|files\/([^/]+)))?)?))?$/;

// An item another of your computers keeps, with which one.
type Elsewhere = ToldItem & { computer: Computer };

const YOU = { user: true } as const;
// Every queue on the box at once, as the panel's All shows it.
const ALL = "all";
const ONLY_CLIENTS = "Only Kuvo Code on your Mac or iPhone can do this: agents change the Work Queue with their tools.";
// A Mac over SSH whose proof didn't hold (too old, its clock off, or a Mac that didn't set this one up).
const NOT_PROVED = "This machine couldn't tell the request was from the Mac that set it up (its proof was missing or too old; check both computers' clocks): try again.";

// A paired phone, this Mac's app, or Kuvo Code on another computer (`box route`) with its Mac's
// proof for this request alone, as the vault takes it (anyone on this computer can run `box route`,
// an agent's command too): you.
const notClient = ({ request }: RouteContext) => (request.fromMac && request.headers["x-kuvo-vault-proof"] ? NOT_PROVED : ONLY_CLIENTS);
const fromClient = ({ request, phone }: RouteContext) => Boolean(phone?.key || request.fromApp || (request.fromMac && provedRequest(request)));
const queryOf = (request: RouteContext["request"]) => new URLSearchParams(request.path.split("?")[1] ?? "");

// Whether every word of `q` is in what an item says: its title, its number or its issue's name,
// its labels' names and, of one kept here, its description and notes (one on another computer: its
// last note, as that's all of them this box was told).
function says(q: string, item: { title: string; number?: number; source?: { ref: string; name?: string }; labels: string[]; body?: string; notes?: { text: string }[]; lastNote?: { text: string } }, names: Map<string, string>): boolean {
  const text = [item.title, item.number !== undefined ? `#${item.number}` : "", item.source?.ref ?? "", item.source?.name ?? "", ...item.labels.map(each => names.get(each) ?? ""), item.body ?? "", ...(item.notes ?? []).map(each => each.text), item.lastNote?.text ?? ""].join("\n").toLowerCase();
  return q.toLowerCase().split(/\s+/).filter(Boolean).every(word => text.includes(word));
}

/** An owner's view, as `kuvo/work-queue` gives it: the statuses, priorities and labels, who an item
 * may be for, its items and those it oversees or handed on (summaries, each with what a client may
 * do to it), those linked to it that are someone else's (what it's working on: seen here, changed
 * only by their owner's side), whose each is, and what's deleted among them. `all` is every item
 * on the box, whoever's it is. With `q`, only those its words are in. Its issues and pull requests
 * are read from GitHub where that's due, waiting `wait` ms at most (null: not read, as last read). */
/** The filters a view is asked with (`computers`, `sources`, `owners`, `labels`: lists): an item
 * shows when, for each kind asked, it's one of those named. A computer by its key (`""`, or its own
 * key, for the box asked); a source by its kind (`none` for an item that's no outside source's); an
 * owner by its key; a label by its id (`none` for an item with no label). */
export interface Filters { computers?: string[]; sources?: string[]; owners?: string[]; labels?: string[] }

/** The filters in a request's query, as taken: each a list separated by commas, 50 at most. */
export function filtersOf(query: URLSearchParams): Filters {
  const filters: Filters = {};
  for (const kind of ["computers", "sources", "owners", "labels"] as const) {
    if (!query.has(kind)) continue;
    // (An empty name is the box asked, among computers; nothing, among the rest.)
    filters[kind] = [...new Set((query.get(kind) ?? "").split(",").map(each => each.trim()).filter(each => each.length <= 100 && (each || kind === "computers")))].slice(0, 50);
  }
  return filters;
}

export async function viewOf(owner: string, targets: Target[], facts?: Facts, signal?: AbortSignal, cache?: Map<string, unknown>, wait: number | null = GITHUB_WAIT, q = "", filters: Filters = {}) {
  const known = facts ?? await factsOf(targets, signal);
  const overseen = owner === ALL ? new Set<string>() : overseenBy(owner, known.all);
  const here = allItems();
  // (A queue of a kind a newer box keeps isn't one to show.)
  const all = here.filter(item => owner === ALL || inView(owner, item, overseen) || linkedTo(owner, item));
  // (What's shown, here or told by another computer: GitHub is read for it, only now.)
  // (Another computer's as it told them: when it last asked GitHub decides whether this box does.)
  if (wait !== null) await readSeen([...all, ...itemsElsewhere(owner, readLink(), undefined, undefined, { asTold: true, linked: true }).items], wait);
  const { owners, titles } = await ownersOf(owner, all, known, targets, cache);
  // Your other computers' items in this view, as each last told them (one deleted here that's live
  // there, a hand-off called off that had got there, is theirs).
  const elsewhere = itemsElsewhere(owner, readLink(), new Set(here.filter(item => !item.removed).map(item => item.id)), new Set(here.filter(item => item.removed).map(item => item.id)), { linked: true });
  const named = new Set(owners.map(each => each.owner));
  // The labels: this box's, then those only another computer has.
  const own = labels();
  const shown: Label[] = [...own, ...elsewhere.labels.filter(each => !own.some(mine => mine.id === each.id))];
  const names = new Map(shown.map(each => [each.id, each.name]));
  const place = new Map(STATUSES.map((each, index) => [each.id, index]));
  // (In the statuses' order, then each one's own.)
  // (Each computer keeps its own order: within a status, this box's first, then each other
  // computer's, by name.)
  const computerOf = (item: { computer?: { name: string; key: string } }) => (item.computer ? `1 ${item.computer.name} ${item.computer.key}` : "0");
  const byPlace = <T extends { status: string; order: number; computer?: { name: string; key: string } }>(items: T[]) =>
    items.sort((a, b) => (place.get(a.status) ?? 99) - (place.get(b.status) ?? 99) || computerOf(a).localeCompare(computerOf(b)) || a.order - b.order);
  const moving = frozen();
  const who = assignable();
  // (Nothing is handed to one that doesn't take what a client does from another box.)
  // (A label an item names that no computer lists any more isn't one of its labels.)
  const summary = (item: Item): ItemSummary => {
    const said = withCan(summaryOf(item, known.working, titles, who), moving);
    return said.labels.every(each => names.has(each)) ? said : { ...said, labels: said.labels.filter(each => names.has(each)) };
  };
  // (One on another computer: nothing to do to it while that one isn't answering, or doesn't take
  // what a client does from another box.)
  const away = new Set(elsewhere.computers.filter(each => each.away !== undefined).map(each => each.key));
  const ownKeyHere = (() => { const link = readLink(); return (link && ownKey(link)) || ""; })();
  const kept = (item: { owner: string; labels: string[]; source?: { kind: string }; computer?: { key: string } }) =>
    (!filters.computers || filters.computers.some(each => (item.computer ? each === item.computer.key : each === "" || each === ownKeyHere)))
    && (!filters.sources || filters.sources.includes(item.source?.kind ?? "none"))
    && (!filters.owners || filters.owners.includes(item.owner))
    && (!filters.labels || (item.labels.length ? item.labels.some(each => filters.labels!.includes(each)) : filters.labels.includes("none")));
  const found = (item: Parameters<typeof says>[1]) => !q || says(q, item, names);
  return {
    owner,
    statuses: [...STATUSES],
    priorities: [...PRIORITIES],
    labels: shown,
    // (The colours a label may be, in the order a picker offers them: no client keeps its own list.)
    colors: [...LABEL_COLORS],
    assignees: who,
    new: { status: "todo", priority: "normal" },
    today: todayHere(),
    items: byPlace([
      ...all.filter(item => !item.removed && found(item)).map(summary).filter(kept),
      ...elsewhere.items.filter(item => found(item) && kept(item)).map(({ handedFrom: _, overdue: __, ...item }) => ({ ...item, ...(overdueOf(item) ? { overdue: true } : {}), labels: item.labels.filter(each => names.has(each)), can: canOf(item, { away: away.has(item.computer.key), apart: !takesForwarded(item.computer.key), attaching: takesForwarded(item.computer.key, ATTACHMENTS) }) })),
    ]),
    owners: [...owners, ...elsewhere.owners.filter(each => !named.has(each.owner))],
    removed: all.filter(item => item.removed && found(item)).sort((a, b) => b.removed!.at - a.removed!.at).map(summary).filter(kept),
    computers: elsewhere.computers,
    ...(q ? { q } : {}),
    ...(Object.keys(filters).length ? { filters } : {}),
  };
}

/** A view's counts (PROTOCOL.md, "The Work Queue", `counts=1`), of the items it lists (with `q`,
 * those the words are in): how many are open, of each status, priority and label (every one of the
 * view's, those with none too), from each outside source (by its kind), and on each computer (by its
 * key; this box's own under its own, `""` while it has none), and how many are deleted. Counted
 * here, so a client counts nothing. */
export function countsOf(view: Awaited<ReturnType<typeof viewOf>>, own = ""): QueueCounts {
  const none = (each: { id: string }[]) => Object.fromEntries(each.map(one => [one.id, 0])) as Record<string, number>;
  const statuses = none(view.statuses);
  const priorities = none(view.priorities);
  const labeled = none(view.labels);
  const sources: Record<string, number> = {};
  const computers: Record<string, number> = { [own]: 0 };
  for (const computer of view.computers) computers[computer.key] = 0;
  let open = 0;
  for (const item of view.items) {
    if (!isClosed(item.status)) open += 1;
    statuses[item.status] = (statuses[item.status] ?? 0) + 1;
    priorities[item.priority] = (priorities[item.priority] ?? 0) + 1;
    for (const label of item.labels) labeled[label] = (labeled[label] ?? 0) + 1;
    if (item.source) sources[item.source.kind] = (sources[item.source.kind] ?? 0) + 1;
    const key = item.computer?.key ?? own;
    computers[key] = (computers[key] ?? 0) + 1;
  }
  return { open, statuses, priorities, labels: labeled, sources, computers, removed: view.removed.length };
}

const withCounts = (view: Awaited<ReturnType<typeof viewOf>>, counts: boolean) => {
  if (!counts) return view;
  const link = readLink();
  return { ...view, counts: countsOf(view, (link && ownKey(link)) || "") };
};

// What a chip shows of an item (a summary): its number (or its outside source, and what GitHub says
// of it), title, where it stands (its status), whose.
const refFacts = (item: Pick<ItemSummary, "id" | "number" | "title" | "status" | "owner" | "source" | "seen" | "struck" | "closesOn" | "removed">) => ({
  id: item.id,
  ...(item.number !== undefined ? { number: item.number } : {}),
  ...(item.source ? { source: item.source } : {}),
  ...(item.seen ? { seen: item.seen } : {}),
  ...(item.struck ? { struck: true } : {}),
  ...(item.closesOn ? { closesOn: item.closesOn } : {}),
  title: item.title,
  status: item.status,
  owner: item.owner,
  ...(item.removed ? { removed: true } : {}),
});

export async function* workQueueRoute(context: RouteContext, match: RegExpMatchArray): AsyncGenerator<AnswerPiece> {
  const { request, json, signal, readEngine, readBridge, bridge } = context;
  const method = request.method.toUpperCase();
  const [, part, rawItem, action, rawFile] = match;
  const targets = targetsOf({ readEngine, bridge });
  const body = parsed(request.body);
  const given = isObject(body) ? body : {};

  if (part === "tools") {
    if (method === "GET") return yield* json(200, { tools: QUEUE_TOOLS, instructions: QUEUE_INSTRUCTIONS });
    if (method !== "POST") return yield* json(405, {});
    const args = isObject(given.arguments) ? given.arguments : {};
    return yield* json(200, await callQueueTool(given.name, args, given.session, readEngine, readBridge, targets, signal ?? undefined));
  }

  if (part === "refs") {
    // (GET with `refs=243,12`; POST `{refs: [...], conversation?}`, a list, so a ref with a comma in
    // it, an outside one later, goes whole.)
    if (method !== "GET" && method !== "POST") return yield* json(405, {});
    const listed = method === "POST" ? (Array.isArray(given.refs) ? given.refs : []) : (queryOf(request).get("refs") ?? "").split(",");
    const wanted = listed.filter((each): each is string => typeof each === "string").map(each => each.trim()).filter(each => each && each.length <= 300).slice(0, 200);
    const items = allItems();
    const facts = await factsOf(targets, signal ?? undefined);
    const found = wanted.map(ref => {
      const named = refOf(ref);
      const here = named ? matching(items, named)[0] : undefined;
      // One on another of your computers, as it last told it: before one deleted here.
      const far = (!here || here.removed) && named ? itemElsewhere(named, readLink(), { asTold: true }) : null;
      return { ref, item: far ? undefined : here, far, here };
    });
    // (Their issues and pull requests read from GitHub where that's due: the chips are on screen.)
    await readSeen(found.flatMap(each => [each.item ?? each.far].filter(Boolean) as { source?: Item["source"] }[]), GITHUB_WAIT);
    const refs: Record<string, unknown> = {};
    for (const { ref, far, here } of found) {
      const again = far ? itemElsewhere({ id: far.id }) : null;
      const item = again ? undefined : here;
      refs[ref] = item ? refFacts(summaryOf(item, facts.working, new Map())) : again ? { ...refFacts(again), computer: again.computer } : null;
    }
    return yield* json(200, { refs });
  }

  if (part === "labels") {
    if (method === "GET") return yield* json(200, { labels: labels() });
    if (method !== "POST") return yield* json(405, {});
    if (!fromClient(context)) return yield* json(403, { message: notClient(context) });
    // (Removed, it's taken off every item here first, deleted ones too.)
    const outcome = typeof given.add === "string" ? addLabel(given.add, given.color)
      : given.change !== undefined ? changeLabel(given.change, { name: given.name, color: given.color })
      : given.remove !== undefined ? removeLabel(given.remove, stripLabel)
      : { refused: "Send {add, color?}, {change, name?, color?} or {remove}." };
    if ("refused" in outcome) return yield* json(400, { message: outcome.refused });
    tellQueuesAgain();
    return yield* json(200, { labels: labels(), ...(given.remove === undefined ? { label: outcome.done } : {}) });
  }

  if (part === "items") {
    if (method !== "POST") return yield* json(405, {});
    if (!fromClient(context)) return yield* json(403, { message: notClient(context) });
    return yield* answered(addHere(given, fromLocalApp(context.request)), json);
  }

  if (part === "import") {
    if (method !== "POST") return yield* json(405, {});
    if (!fromClient(context)) return yield* json(403, { message: notClient(context) });
    const owner = typeof given.owner === "string" && OWNER.test(given.owner) ? given.owner : null;
    if (!owner) return yield* json(400, { message: "Say whose queue it goes in (owner): a conversation's address." });
    try {
      refuseMoving([owner]);
    } catch (error) {
      return yield* json(409, { message: (error as Error).message });
    }
    const read = readBacklogFiles(given.files);
    if ("refused" in read) return yield* json(400, { message: read.refused });
    const { items, skipped } = parseBacklog(read.texts);
    let outcome: ReturnType<typeof importItems>;
    try {
      outcome = changeQueues(book => importItems(book, owner, items, YOU, primaryOf()));
    } catch (error) {
      if (error instanceof Refusal) return yield* json(400, { message: error.message });
      throw error;
    }
    const names = statusName;
    return yield* json(200, { added: outcome.added, byStatus: outcome.byStatus, waiting: outcome.waiting, skipped: [...skipped, ...outcome.already.map(n => `#${n} (already in the Work Queue here)`), ...outcome.skipped], said: importWords(outcome, skipped, names) });
  }

  if (part === "events" || part === undefined) {
    if (method !== "GET") return yield* json(405, {});
    const owner = queryOf(request).get("owner") ?? "";
    if (!OWNER.test(owner) && owner !== ALL) return yield* json(400, { message: "Say whose queue (owner): a conversation's address, a group chat's id, or all." });
    // (A glance at it, a count say, as the phone's ••• shows: GitHub isn't read for it.)
    const glance = queryOf(request).get("glance") === "1";
    // (Its counts too, for a client that draws them: the sidebar's, `counts=1`.)
    const counts = queryOf(request).get("counts") === "1";
    // (Only what these words are in: a search, `q`.)
    const q = (queryOf(request).get("q") ?? "").trim().slice(0, 200);
    // (And only those of these computers, sources, owners or labels: its filters.)
    const filters = filtersOf(queryOf(request));
    if (part === undefined) return yield* json(200, withCounts(await viewOf(owner, targets, undefined, signal ?? undefined, undefined, glance ? null : GITHUB_WAIT, q, filters), counts));
    yield { head: { status: 200, headers: { "content-type": "application/x-ndjson" } } };
    return yield* followView(owner, targets, signal ?? undefined, glance, counts, q, filters);
  }

  // One item.
  let named = "";
  try { named = decodeURIComponent(rawItem ?? ""); } catch {}
  const ref = refOf(named);
  if (!ref) return yield* json(404, { message: "There's no such item." });
  const find = (items: Item[]) => matching(items, ref)[0] ?? null;
  // (Who it's for is set on the computer that keeps it: that box's `can` is for its own client,
  // whose bots are its own.)
  // (Attaching too only where this box knows that computer keeps them: it's what it sends them through.)
  const fromThere = (item: Record<string, unknown>, computer: Computer) => ({ ...item, ...(isObject(item.can) ? { can: { ...item.can, assign: false, attach: item.can.attach === true && takesForwarded(computer.key, ATTACHMENTS) } } : {}), computer });
  // One that's neither here nor told by any of your computers (an old Done one, a deleted one), for
  // a client: asked of each of them, so a client asks one box whichever keeps it. Null when none
  // has it; `refuse` when some couldn't say, so it may be there.
  const seek = async (): Promise<{ far: Elsewhere; whole: Record<string, unknown> } | { refuse: { message: string; computers: Computer[] } } | null> => {
    if (!fromClient(context)) return null;
    const sought = await seekElsewhere(named, ref, signal);
    if ("missing" in sought) return null;
    if ("unsure" in sought) return { refuse: { message: sought.message, computers: sought.unsure } };
    return { far: { ...sought.told, computer: sought.computer }, whole: sought.item };
  };
  if (action === undefined && method === "GET") {
    if (find(allItems())) return yield* answered(await actHere({ method: "GET", ref, targets, signal }), json);
    // One on another of your computers: whole, as that computer's box answers for a client (`told:
    // false`); else as it last told it (`told: true`), without what only it keeps (its description,
    // notes, what happened to it, files, links).
    let far = itemElsewhere(ref, readLink(), { deleted: fromClient(context) });
    if (!far) {
      const sought = await seek();
      if (!sought) return yield* json(404, { message: "There's no such item." });
      if ("refuse" in sought) return yield* json(503, sought.refuse);
      return yield* json(200, { item: { ...fromThere(sought.whole, sought.far.computer), told: false } });
    }
    if (fromClient(context)) {
      const there = await forwardAct(far.computer, nameOf(far), { method: "GET", item: far.id }, signal);
      const whole = there.status === 200 && "value" in there && isObject(there.value) && isObject(there.value.item) ? there.value.item : null;
      if (whole) return yield* json(200, { item: { ...fromThere(whole, far.computer), told: false } });
    }
    // (As it was told: nothing to do to it from here until its computer answers.)
    const cant = { away: awaySince(far.computer.key) !== null, apart: !fromClient(context) || !takesForwarded(far.computer.key), attaching: takesForwarded(far.computer.key, ATTACHMENTS) };
    await readSeen([itemElsewhere(ref, readLink(), { asTold: true }) ?? far], GITHUB_WAIT);
    far = itemElsewhere({ id: far.id }) ?? far;
    const { handedFrom: _, ...summary } = far;
    return yield* json(200, { item: { ...summary, can: canOf(summary, cant), body: "", notes: [], events: [], files: [], links: [], handed: [], told: true } });
  }
  if (action?.startsWith("files/") && method === "GET") {
    let name = "";
    try { name = decodeURIComponent(rawFile ?? ""); } catch {}
    // One kept with an item on another of your computers: from there, for a client (up to a few MB).
    const forClient = !find(allItems()) && fromClient(context) && Boolean(name) && name.length <= 300;
    let far: Elsewhere | null = forClient ? itemElsewhere(ref) : null;
    if (forClient && !far) {
      const sought = await seek();
      if (sought && "refuse" in sought) return yield* json(503, sought.refuse);
      far = sought?.far ?? null;
    }
    let file = far ? null : fileOf(ref, name);
    if (far) {
      const there = await forwardAct(far.computer, nameOf(far), { method: "GET", item: far.id, action: "file", name }, signal);
      if ("refused" in there) return yield* json(there.status, { message: there.refused, computer: far.computer });
      const value = isObject(there.value) ? there.value : {};
      if (there.status !== 200) return yield* json(there.status, { ...value, computer: far.computer });
      if (typeof value.data !== "string" || typeof value.type !== "string") return yield* json(502, { message: `${far.computer.name} answered something this computer can't read.`, computer: far.computer });
      file = { type: value.type, bytes: Buffer.from(value.data, "base64") };
    }
    if (!file) return yield* json(404, { message: "There's no such file." });
    yield { head: { status: 200, headers: { "content-type": file.type } } };
    return yield { body: file.bytes };
  }
  const asked: { method: "DELETE" | "POST" | "PATCH"; action?: "restore" } | null = method === "DELETE" && action === undefined ? { method: "DELETE" }
    : method === "POST" && action === "restore" ? { method: "POST", action }
    : method === "PATCH" && action === undefined ? { method: "PATCH" }
    : null;
  if (!asked) return yield* json(405, {});
  if (!fromClient(context)) return yield* json(403, { message: notClient(context) });
  // One on another of your computers is done there, by that computer's box: this one hands it on
  // and answers with what it said (a client never works out where an item is).
  let far: Elsewhere | null = null;
  if (!find(allItems())) {
    far = itemElsewhere(ref, readLink(), { deleted: asked.action === "restore" });
    if (!far) {
      const sought = await seek();
      if (sought && "refuse" in sought) return yield* json(503, sought.refuse);
      far = sought?.far ?? null;
    }
  }
  if (!far) return yield* answered(await actHere({ ...asked, ref, given, targets, signal, local: fromLocalApp(context.request) }), json);
  const { title, body: text, status, priority, labels: labeled, assignee, due, before, after, top } = given;
  // (Files go to that computer's box with their bytes: it never reads a path it's sent.)
  // (Asked first, before any file is read: that computer keeps them at all.)
  const attaching = Array.isArray(given.files) && given.files.length > 0;
  if (attaching && !takesForwarded(far.computer.key, ATTACHMENTS)) return yield* json(409, { message: `${far.computer.name} needs an update before it can keep attached files. Update it and try again.`, computer: far.computer });
  let files: unknown;
  try { files = attaching ? carriedFiles(given.files, fromLocalApp(context.request), far.computer.name, MOST_FORWARDED_FILE) : given.files; } catch (error) {
    if (error instanceof Refusal) return yield* json(error instanceof TooBig ? 413 : 400, { message: error.message, computer: far.computer });
    throw error;
  }
  const there = await forwardAct(far.computer, nameOf(far), { ...asked, item: far.id, ...(asked.method === "PATCH" ? { body: { title, body: text, status, priority, labels: labeled, assignee, due, before, after, top, files } } : {}) }, signal);
  if ("refused" in there) return yield* json(there.status, { message: there.refused, computer: far.computer });
  // What this box has of it now, as that computer's box answered (not when it next tells).
  if (there.status === 200 && isObject(there.value) && isObject(there.value.item) && there.value.item.id === far.id) changedThere(far.computer.key, asked.method === "DELETE" ? "delete" : asked.action ?? "change", there.value.item);
  // (As this box names that computer, so a client knows where it is.)
  const value = !isObject(there.value) ? there.value
    : isObject(there.value.item) ? { ...there.value, item: fromThere(there.value.item, far.computer) }
    : there.status === 200 ? there.value : { ...there.value, computer: far.computer };
  return yield* json(there.status, value);
}

const answered = (answer: Acted, json: RouteContext["json"]) => json(answer.status, answer.value);

// An owner's view, a line now and one each time it changes: a change to the queues (here, or by
// another process: its files), or a session linked to one of its items starting or stopping work
// (looked at every few seconds).
async function* followView(owner: string, targets: Target[], signal?: AbortSignal, glance = false, counts = false, q = "", filters: Filters = {}): AsyncGenerator<AnswerPiece> {
  let wake: (() => void) | null = null;
  let dirty = true;
  const told = () => {
    dirty = true;
    wake?.();
  };
  queueFollowers.add(told);
  // (The queues, what your other computers told of theirs, and the labels beside them: not the
  // locks and the numbers there.)
  try { fs.mkdirSync(TOLD, { recursive: true, mode: 0o700 }); } catch {}
  // (What a watch may have missed, as one started or stopped: read again now, as every few seconds,
  // but who's under whom only as they change.)
  const looked = () => wake?.();
  const queuesWatch = watchFolders([path.join(WORK_QUEUE, "queues"), TOLD], told, { recursive: false, missed: looked });
  // (And what GitHub said of its issues and pull requests, read as it's shown.)
  const categoriesWatch = watchFolders([WORK_QUEUE], ((_: unknown, name: unknown) => { if ([path.basename(LABELS_FILE), path.basename(SEEN_FILE)].includes(String(name ?? ""))) told(); }) as () => void, { recursive: false, missed: looked });
  const watching = { close: () => { queuesWatch.close(); categoriesWatch.close(); } };
  signal?.addEventListener("abort", () => wake?.(), { once: true });
  let last = "";
  let facts: Facts | null = null;
  let factsAt = 0;
  // Titles of sessions the list doesn't have, looked up once a half minute at most.
  let titles = new Map<string, unknown>();
  try {
    while (!signal?.aborted) {
      // Who's under whom, looked at again with each change and every half minute; who's at work,
      // each time.
      if (!facts || dirty || Date.now() - factsAt > 30_000) {
        facts = await factsOf(targets, signal);
        factsAt = Date.now();
        titles = new Map();
      } else {
        const known: Facts = facts;
        facts = { all: known.all, working: await atWorkNow(targets, signal).catch(() => known.working) };
      }
      dirty = false;
      // (GitHub isn't waited for: what it says comes as a line of its own once it's read.)
      const line = JSON.stringify(withCounts(await viewOf(owner, targets, facts, signal, titles, glance ? null : 0, q, filters), counts));
      if (line !== last) {
        yield { body: Buffer.from(line + "\n") };
        last = line;
      }
      if (signal?.aborted || dirty) continue;
      await new Promise<void>(done => {
        const timer = setTimeout(done, 3000);
        wake = () => { clearTimeout(timer); done(); };
      });
      wake = null;
    }
  } finally {
    queueFollowers.delete(told);
    watching.close();
  }
}
