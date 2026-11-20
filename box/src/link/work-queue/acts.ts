import { fingerprint } from "../ownership/merge";
// What you do to an item from a client, on the box that keeps it (PROTOCOL.md, "The Work Queue"):
// add one, read it whole, change it, delete it or bring it back. The same whether your client asked this box
// (`kuvo/work-queue/items/…`) or another of your computers' boxes handed it on (`forward.ts`).

import { isObject } from "../../shared/json";
import { relationsOf, setRelation } from "./relations";
import { readAttachment } from "./attachment-file";
import { ownName } from "../letters/start";
import { callOffHanding, refuseLiveElsewhere, twinOf } from "./hand";
import { readSeen } from "./github";
import { numberWaiting, primaryOf } from "./numbers";
import { addItem, applyFileOps, stageClientFiles, changeItem, handItem, refuseClosedThere, refuseHanding, refuseMoving, refuseQueueOwner, removeItem, restoreItem, type Acting, type Fields } from "./ops";
import { assignable, assigneeFor, botSession } from "./assignees";
import { frozen } from "../moves/records";
import { allItems, changeQueues, deletionFor, departureFor, matching, MOST_HANDED_FILES, OWNER, Refusal, YOURS, type Book, type Item, type ItemRef } from "./store";
import { itemsElsewhere, tellQueuesAgain } from "./across";
import { canOf, factsOf, nameOf, ownersOf, refOf, summaryOf, titlesHere, visibleItem, wholeOf, type ItemSummary } from "./view";
import type { Act, Acted } from "./forward";
import type { Target } from "../../shared/records";

/** How long a client's ask waits for GitHub to say what its issues and pull requests are now (a
 * read that takes longer goes on, for the next ask or the view's next line). */
export const GITHUB_WAIT = 3_000;

const YOU = { user: true } as const;
// An item a client named that isn't here.
class Missing extends Refusal {}

/** A summary with what a client may do to it here (`canOf`): nothing while its conversation moves. */
export const withCan = <S extends ItemSummary>(summary: S, moving: Set<string> = frozen()): S => ({ ...summary, can: canOf(summary, { moving: moving.has(summary.owner) }) });

// What a client's own change says: the item as it is now, or why not.
function changed(work: () => Item): Acted {
  try {
    const item = work();
    // Your other computers hear of it at their next round, not after the usual wait.
    tellQueuesAgain();
    return { status: 200, value: { item: { ...withCan(summaryOf(item, new Set(), new Map(), assignable())), mentionTargets: item.mentionIndex?.targets ?? [] } } };
  } catch (error) {
    if (error instanceof Refusal) return { status: error instanceof Missing ? 404 : 400, value: { message: error.message } };
    throw error;
  }
}

/** What a client may say of an item it adds or changes: nothing else is taken from it. */
const fieldsOf = ({ title, body, status, priority, labels, assignee, due, before, after, top, files, removeInlineImages }: Record<string, unknown>): Fields => ({ title, body, status, priority, labels, assignee, due, before, after, top, files, removeInlineImages });

/** An item you add from a client (`POST kuvo/work-queue/items`): in the queue `owner` names (a
 * conversation's, the one you're looking at); else, for a bot of this box, in that bot's; else in
 * your own (`usr_you`). Todo and Normal unless said. */
export function addHere(given: Record<string, unknown>, local: boolean, operation: string): Acted {
  const now = Date.now();
  const primary = primaryOf();
  const id = `wqi_${operation.slice(0, 20)}`;
  const digest = fingerprint({ owner: given.owner, ...fieldsOf(given) });
  const itemAnswer = (item: Item): Acted => ({ status: 200, value: {
    item: { ...withCan(summaryOf(item, new Set(), new Map(), assignable())), mentionTargets: item.mentionIndex?.targets ?? [] },
  } });
  let staged: ReturnType<typeof stageClientFiles> | undefined;
  try {
    const result = changeQueues(book => {
      const existing = book.find({ id });
      const deletion = deletionFor(id);
      const departure = departureFor(id);
      const receipt = existing?.creationReceipt ?? deletion?.creationReceipt ?? departure?.creationReceipt;
      if (receipt?.operation === operation && receipt && receipt.digest === digest) return receipt.answer;
      if (existing || deletion || departure) return { status: 409, value: { message: "That operation id is in use or its item has moved." } };
      if (given.owner !== undefined && (typeof given.owner !== "string" || !OWNER.test(given.owner)))
        throw new Refusal("That's no queue to add it to (owner): a conversation's address, or a group chat's id.");
      const assignee = given.assignee === undefined || given.assignee === null ? null : assigneeFor(given.assignee);
      const owner = (given.owner as string | undefined) ?? botSession(assignee ?? undefined) ?? YOURS;
      refuseQueueOwner(owner);
      refuseMoving([owner]);
      staged = stageClientFiles(given.files, local);
      const { before: _, after: __, ...fields } = fieldsOf(given);
      const acting: Acting = { who: YOU, now, staged: staged.staged, fileOps: [] };
      const item = addItem(book, owner, fields, acting, primary, undefined, id);
      applyFileOps(acting);
      numberWaiting(book, primary, null);
      book.recordCreation(item, () => ({ operation, digest, answer: itemAnswer(item) }));
      return { item };
    }, now, { durable: true });
    const answer = "item" in result ? result.item.creationReceipt?.answer ?? itemAnswer(result.item) : result;
    if (answer.status === 200) tellQueuesAgain();
    return answer;
  } catch (error) {
    if (error instanceof Refusal) return { status: 400, value: { message: error.message } };
    throw error;
  } finally { staged?.clear(); }
}

/** What a client does to an item this box keeps (`ref`), as the route answers it: read whole, delete,
 * bring back or change (`given`: what it says, where it stands, its place). */
export async function actHere({ method, action, ref, given = {}, targets, signal, local = false }: { method: Act["method"]; action?: "restore"; ref: ItemRef; given?: Record<string, unknown>; targets: Target[]; signal?: AbortSignal | null; local?: boolean }): Promise<Acted> {
  const find = (items: Item[]) => matching(items, ref)[0] ?? null;
  if (method === "GET") {
    const items = allItems();
    const item = find(items.filter(each => visibleItem(each, { owner: "all", deleted: true })));
    if (!item) return { status: 404, value: { message: "There's no such item." } };
    const facts = await factsOf(targets, signal ?? undefined);
    await readSeen([item], GITHUB_WAIT);
    const { titles } = await ownersOf(item.owner, [item], facts, targets);
    const whole = wholeOf(item, facts.working, titles, assignable());
    const elsewhere = itemsElsewhere("all", undefined,
      new Set(items.filter(each => !each.removed).map(each => each.id)),
      new Set(items.filter(each => each.removed).map(each => each.id))).items;
    const notes = whole.notes.map((note, at) => ({ ...note, mentions: item.mentionIndex?.notes[at]?.filter(range => range.item) ?? [] }));
    const bodyMentions = item.mentionIndex?.body.filter(range => range.item) ?? [];
    return { status: 200, value: { item: { ...withCan(whole), notes, itemRelations: relationsOf(item, [...items, ...elsewhere]), bodyMentions, canLinkItems: !item.removed && !item.handing && !frozen().has(item.owner) } } };
  }
  const now = Date.now();
  const primary = primaryOf();
  const inBook = (work: (item: Item, book: Book) => void, { handing = false } = {}) => () => changeQueues(book => {
    const item = find(book.items());
    if (!item) throw new Missing("There's no item like that.");
    refuseQueueOwner(item.owner, item.id);
    refuseMoving([item.owner]);
    if (!handing) refuseHanding(item);
    work(item, book);
    numberWaiting(book, primary, null);
    return item;
  });
  // (Deleting one on its way to another computer calls its hand-off off: one stuck going to a
  // computer that doesn't answer is yours to let go of.)
  if (method === "DELETE") {
    return changed(inBook(item => {
      if (item.removed) throw new Refusal("It's deleted already.");
      callOffHanding(item, YOU, now);
      removeItem(item, YOU, now);
    }, { handing: true }));
  }
  if (action === "restore") {
    // (Another item here its issue: named by its conversation's title, as you know it, looked up
    // only then.)
    const deleted = find(allItems());
    const twin = deleted?.removed ? twinOf(deleted) : null;
    const titled = twin && !("computer" in twin) ? await titlesHere([twin.owner], targets) : undefined;
    return changed(inBook((item, book) => {
      if (!item.removed) throw new Refusal("It isn't deleted.");
      refuseLiveElsewhere(item, book.items(), titled);
      restoreItem(book, item, YOU, now);
    }));
  }
  if (given.itemRelation !== undefined) {
    const relation = given.itemRelation;
    const applied = changed(inBook((item, book) => {
      if (item.removed || !isObject(relation) || typeof relation.item !== "string") throw new Refusal("Choose two items that are still in the Work Queue.");
      const other = book.items().find(each => each.id === relation.item && !each.removed);
      if (!other) throw new Refusal("Choose an item kept on the same computer and try again.");
      refuseMoving([other.owner]);
      refuseHanding(other);
      setRelation(item, other, relation.kind);
    }));
    if (applied.status === 200 && isObject(applied.value)) applied.value.itemRelationApplied = true;
    return applied;
  }
  let staged: ReturnType<typeof stageClientFiles> | undefined;
  try {
    const applied = changed(() => {
      staged = stageClientFiles(given.files, local);
      return inBook((item, book) => {
        if (item.removed) throw new Refusal("It's deleted: bring it back first.");
        const fields = fieldsOf(given);
        if ([fields.before, fields.after].some(each => each !== undefined && refOf(each) === null)) throw new Refusal("There's no item like that to put it beside.");
        if (item.source && (fields.title !== undefined || fields.body !== undefined || fields.removeInlineImages !== undefined)) throw new Refusal(`${nameOf(item)} takes its title and description from GitHub: change them there.`);
        // (As its `can.attach` says: files go with what you write of it.)
        if (item.source && staged!.staged.length) throw new Refusal(`${nameOf(item)} takes its description from GitHub: attach files there.`);
        // (An issue GitHub closed stands as it is until it's reopened there: nothing of it is set here.)
        refuseClosedThere(item);
        const acting: Acting = { who: YOU, now, staged: staged!.staged, fileOps: [] };
        changeItem(book, item, fields, acting);
        // (One of your own given to a bot here goes into that bot's queue, where its agent sees it.)
        const to = item.owner === YOURS && fields.assignee !== undefined ? botSession(item.assignee) : null;
        if (to) {
          refuseMoving([to]);
          handItem(book, item, to, YOU, now);
        }
        applyFileOps(acting);
      })();
    });
    if (given.removeInlineImages !== undefined && applied.status === 200 && isObject(applied.value)) applied.value.inlineImagesApplied = true;
    return applied;
  } finally { staged?.clear(); }
}

/** The bytes of a file kept with an item here, by its name, or null when there's no such file. */
export function fileOf(ref: ItemRef, name: string): { type: string; bytes: Buffer } | null {
  const item = matching(allItems(), ref).find(each => visibleItem(each, { owner: "all", deleted: true }));
  const kept = item?.files.find(each => each.name === name);
  if (!item || !kept) return null;
  const bytes = readAttachment(item.id, name);
  return bytes === null ? null : { type: kept.type, bytes };
}

/** What another of your computers' boxes asked this one to do for its client, to an item kept here
 * (`forward.ts`): done as this box's own client's, and answered as the route would; a file's bytes
 * in base64 (`{type, data}`), up to a few MB. One that isn't here (it moved on meanwhile) isn't
 * handed on again. */
export async function actForPeer(act: Act, targets: Target[]): Promise<Acted> {
  const ref = refOf(act.item);
  const here = ownName();
  const gone: Acted = { status: 404, value: { message: `It's no longer on ${here}: it may have moved. Look again in a moment.` } };
  if (!ref || !matching(allItems(), ref)[0]) return gone;
  if (act.action === "file") {
    const file = fileOf(ref, act.name ?? "");
    if (!file) return { status: 404, value: { message: "There's no such file." } };
    // (Its size rounded up to a tenth, so it never reads as the most it may be.)
    if (file.bytes.length > MOST_FORWARDED_FILE) return { status: 413, value: { message: `${act.name} is ${(Math.ceil(file.bytes.length / 100_000) / 10).toFixed(1)} MB: too big to bring from ${here} (${MOST_FORWARDED_FILE / 1_000_000} MB at most). Open it there.` } };
    return { status: 200, value: { type: file.type, data: file.bytes.toString("base64") } };
  }
  const done = await actHere({ method: act.method, action: act.action === "restore" ? act.action : undefined, ref, given: act.body ?? {}, targets });
  // (Gone between the look and the change.)
  return done.status === 404 ? gone : done;
}

/** The most of a file's bytes another of your computers' boxes is given for its client (as an item
 * handed on takes). */
export const MOST_FORWARDED_FILE = MOST_HANDED_FILES;
