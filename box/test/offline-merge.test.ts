import "./safety"
import { expect, test } from "bun:test";
import { editView, mergeEdit, mergeText, observe, rememberEdit, replay } from "../src/link/ownership/merge";

test("a later revision applies, the first same-field value stands, and a resolution is explicit", () => {
  const first = observe(undefined, "record", { title: "Original", status: "todo" });
  const baseRevision = editView(first).revision;
  const history = observe(first, "record", { title: "One", status: "todo" });
  const edit = { operation: "second", baseRevision, fields: { title: "Two", status: "in-progress" } };
  const merged = mergeEdit(history, edit, () => false);
  expect(merged.status).toBe("choice");
  expect({ ...merged.changes }).toEqual({ status: "in-progress" });
  expect(merged.choices).toEqual([{ id: merged.added[0]!, field: "title", base: "Original", accepted: "One", proposed: "Two" }]);
  const after = observe(history, "record", { title: "One", status: "in-progress" });
  after.choices = merged.choices;
  const receipt = rememberEdit(after, edit.operation, "digest", "choice", merged.added);
  expect(replay(after, edit.operation, "digest")).toEqual(receipt);
  expect(replay(after, edit.operation, "other digest")).toBeFalse();

  const chosen = mergeEdit(after, { operation: "choose", baseRevision: editView(after).revision,
    resolve: { id: merged.added[0]!, use: "proposed" } }, () => false);
  expect({ ...chosen.changes }).toEqual({ title: "Two" });
  expect(chosen.choices).toEqual([]);
  const later = mergeEdit(after, { operation: "later", baseRevision: editView(after).revision, fields: { title: "Three" } }, () => false);
  expect({ ...later.changes }).toEqual({ title: "Three" });
  expect(later.status).toBe("committed");
});

test("independent passages merge and incompatible rewrites retain both wordings", () => {
  const original = "Alpha: red. Beta: green. Gamma: white.";
  expect(mergeText(original, "Alpha: blue. Beta: green. Gamma: black.", "Alpha: red. Beta: orange. Gamma: white.")).toEqual({
    accepted: "Alpha: blue. Beta: orange. Gamma: black.",
  });
  expect(mergeText(original, "Alpha: blue. Beta: green. Gamma: black.", "Alpha: yellow. Beta: orange. Gamma: white.")).toEqual({
    accepted: "Alpha: blue. Beta: orange. Gamma: black.",
    proposed: "Alpha: yellow. Beta: orange. Gamma: black.",
  });
  expect(mergeText("First. Last.", "First. Middle. Last.", "First. Middle. Last.")).toEqual({ accepted: "First. Middle. Last." });
});

test("a stale edit loses to deletion and cannot cross a restore or a retired base", () => {
  const item = observe(undefined, "record", { body: "Original" });
  const deleted = observe(item, "record", { body: "Original" }, true);
  const edit = { operation: "late", baseRevision: editView(item).revision, fields: { body: "Pending text" } };
  expect(mergeEdit(deleted, edit, () => true).status).toBe("deleted");
  const restored = observe(deleted, "record", { body: "Original" });
  expect(editView(restored).revision).not.toBe(editView(item).revision);
  expect(mergeEdit(restored, edit, () => true).status).toBe("refresh");
  expect(mergeEdit(observe(item, "record", {}, false, 1), edit, () => true).status).toBe("refresh");
});

test("a compact deletion travels with its queue, even when all the items are gone", async () => {
  const { K } = await import("../src/link/home");
  const { upgradeKuvoFolder } = await import("../src/shared/folder-version");
  const { changeQueues, deletionFor, KEPT_REMOVED, newItemID } = await import("../src/link/work-queue/store");
  const { carryWorkQueue, packWorkQueues, placeWorkQueues } = await import("../src/link/work-queue/moves");
  upgradeKuvoFolder(K);
  const from = "ses_expired";
  const to = "ses_carried";
  const id = newItemID();
  const now = Date.now();
  const by = { user: true } as const;
  changeQueues(book => book.queue(from).items.push({
    id, owner: from, title: "Expired", body: "", status: "todo", order: 0, labels: [],
    notes: [], files: [], links: [], sessions: [], removed: { at: now - KEPT_REMOVED - 1, by },
    created: { at: now - KEPT_REMOVED - 2, by }, updated: { at: now - KEPT_REMOVED - 1, by },
  }), now);
  changeQueues(() => {}, now);
  expect(deletionFor(id)?.queue).toBe(from);
  carryWorkQueue(from, to);
  expect(packWorkQueues([from])).toBeNull();
  const packed = packWorkQueues([to]);
  expect(packed?.items).toEqual([]);
  expect(packed?.deletions).toEqual([{ id, queue: to, at: now - KEPT_REMOVED - 1 }]);
  // Placing a compact fence is replay-safe and a replayed old payload cannot restore the item.
  placeWorkQueues(packed, [to], null);
  expect(deletionFor(id)?.queue).toBe(to);
  expect(() => placeWorkQueues({ items: [{ id, owner: to, title: "From an obsolete copy" }] }, [to], null)).toThrow(/deleted here/);
});


test("a former owner cannot recreate a handed-off item from a replayed creation", async () => {
  const { K } = await import("../src/link/home");
  const { upgradeKuvoFolder } = await import("../src/shared/folder-version");
  const { newItemID, noteDeparture } = await import("../src/link/work-queue/store");
  const { editQueue } = await import("../src/link/work-queue/offline");
  upgradeKuvoFolder(K);
  const id = newItemID();
  noteDeparture(id, { queue: "ses_new_home", computer: "destination" });
  const result = editQueue({
    ref: { kind: "queueItems", id, queue: "ses_old_home" }, operation: "create",
    create: true, baseRevision: null, fields: { title: "A replay" },
  }, () => true);
  expect(result).toMatchObject({ operation: "create", status: "refresh" });
});

test("a stale choice cannot resolve a later rewrite, and its refreshed option keeps independent passages", () => {
  const passage = () => true;
  const base = observe(undefined, "record", { body: "Alpha: red. Beta: green." });
  const first = observe(base, "record", { body: "Alpha: blue. Beta: green." });
  const clash = mergeEdit(first, { operation: "second", baseRevision: editView(base).revision,
    fields: { body: "Alpha: yellow. Beta: green." } }, passage);
  first.choices = clash.choices;
  const oldChoice = clash.added[0]!;
  const third = observe(first, "record", { body: "Alpha: white. Beta: orange." }, false, 32, passage);
  const view = editView(third);
  for (const use of ["accepted", "proposed"] as const) {
    const stale = mergeEdit(third, { operation: use, baseRevision: view.revision, resolve: { id: oldChoice, use } }, passage);
    expect(stale.status).toBe("refresh");
    expect(stale.changes).toEqual({});
  }
  expect(view.choices[0]).toMatchObject({ accepted: "Alpha: white. Beta: orange.", proposed: "Alpha: yellow. Beta: orange." });
  const resolved = mergeEdit(third, { operation: "refreshed", baseRevision: view.revision,
    resolve: { id: view.choices[0]!.id, use: "proposed" } }, passage);
  expect(resolved.changes).toEqual({ body: "Alpha: yellow. Beta: orange." });
});
