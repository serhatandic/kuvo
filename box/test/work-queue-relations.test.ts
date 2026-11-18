import "./safety"
import { expect, spyOn, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";

const { actHere, addHere, fileOf } = await import("../src/link/work-queue/acts");
const { references } = await import("../src/link/work-queue/relations");
const { allItems, changeQueues, filesOf, WORK_QUEUE, KEPT_REMOVED } = await import("../src/link/work-queue/store");
const { nameOf } = await import("../src/link/work-queue/view");

test("references follow the shared Work Queue rule and preserve UTF-16 offsets", () => {
  const source = "🖼 See #821 and (#82). Ranked #1, #2 in search, PR #3, #4seller, gh:acme/project-#5, `#6`, [#7](https://example.com), https://example.com/#8.\n\n```\n#92\n";
  expect(references(source).map(({ start, end, number }) => [source.slice(start, end), number])).toEqual([["#821", 821], ["#82", 82]]);
  expect(references("🖼 #9000")).toEqual([{ start: 3, end: 8, number: 9000 }]);
});

test("mentions from descriptions and notes, and a stored relationship read from both ends", async () => {
  const make = (title: string, number: number) => {
    const added = addHere({ title });
    expect(added.status).toBe(200);
    const item = allItems().find(each => each.title === title)!;
    changeQueues(book => { book.items().find(each => each.id === item.id)!.number = number; });
    return item.id;
  };
  const a = make("Picker UX", 8101);
  const b = make("Preview release", 8102);
  const c = make("QA checklist", 8103);
  const change = (id: string, given: Record<string, unknown>) => actHere({ method: "PATCH", ref: { id }, targets: [], given });
  const read = async (id: string) => {
    const read = await actHere({ method: "GET", ref: { id }, targets: [] });
    expect(read.status).toBe(200);
    return (read.value as { item: { itemRelations: unknown[] } }).item.itemRelations;
  };
  expect((await change(a, { body: "Coordinate with #8102. Ignore #999999." })).status).toBe(200);
  changeQueues(book => { book.items().find(each => each.id === a)!.notes.push({ at: 1, by: { user: true }, text: "See #8103" }); });
  expect(await read(a)).toEqual([
    { id: b, number: 8102, title: "Preview release", kind: "mentions" },
    { id: c, number: 8103, title: "QA checklist", kind: "mentions" },
  ]);
  expect(await read(b)).toEqual([{ id: a, number: 8101, title: "Picker UX", kind: "mentioned-in" }]);
  expect(allItems().find(each => each.id === a)?.mentionIndex?.targets).toEqual([b, c]);
  await change(a, { body: "The reference was removed." });
  expect(await read(b)).toEqual([]);
  expect(allItems().find(each => each.id === a)?.mentionIndex?.targets).toEqual([c]);
  expect(await read(c)).toEqual([{ id: a, number: 8101, title: "Picker UX", kind: "mentioned-in" }]);
  changeQueues(book => { book.items().find(each => each.id === a)!.notes = []; });
  expect(await read(c)).toEqual([]);

  expect((await change(a, { itemRelation: { item: b, kind: "blocks" } })).status).toBe(200);
  const [holder, other] = [a, b].sort();
  expect(allItems().find(each => each.id === holder)?.relations).toEqual([{ item: other, kind: holder === a ? "blocks" : "blocked-by" }]);
  expect(await read(a)).toEqual([{ id: b, number: 8102, title: "Preview release", kind: "blocks" }]);
  expect(await read(b)).toEqual([{ id: a, number: 8101, title: "Picker UX", kind: "blocked-by" }]);
  expect((await change(b, { itemRelation: { item: a, kind: null } })).status).toBe(200);
  expect(await read(a)).toEqual([]);
  expect(await read(b)).toEqual([]);
  expect((await change(a, { itemRelation: { item: a, kind: "related" } })).status).toBe(400);
  expect(nameOf(allItems().find(each => each.id === a)!)).toContain("#8101");
});

test("inline attachment anchors follow the stored files and their collision-safe names", async () => {
  const file = { name: "Login [wide].png", uri: "data:image/png;base64,YWJj" };
  const marker = (index: number) => `![Attachment](kuvo-attachment:draft/${index}/Login%20%5Bwide%5D.png)`;
  const result = addHere({ title: "Two captures", body: `Before\n${marker(1)}\nBetween\n${marker(0)}\nAfter`, files: [file, file] });
  expect(result.status).toBe(200);
  const saved = allItems().find(item => item.title === "Two captures")!;
  const [first, second] = saved.files;
  expect(first.name).toBe("Login [wide].png");
  expect(second.name).toBe("Login [wide] 2.png");
  expect(first.anchor).toMatch(/^[a-f0-9]{24}$/);
  expect(first.anchor).not.toBe(second.anchor);
  expect(saved.body).toBe(`Before\n![Login \\[wide\\] 2.png](kuvo-attachment:file/${second.anchor})\nBetween\n![Login \\[wide\\].png](kuvo-attachment:file/${first.anchor})\nAfter`);
  const edit = await actHere({ method: "PATCH", ref: { id: saved.id }, targets: [], given: { body: saved.body + "\n" + marker(0), files: [file] } });
  expect(edit.status).toBe(200);
  const edited = allItems().find(item => item.id === saved.id)!;
  expect(edited.files.map(each => each.name)).toEqual(["Login [wide].png", "Login [wide] 2.png", "Login [wide] 3.png"]);
  expect(edited.body).toContain(saved.body);
  expect(edited.body.endsWith(`![Login \\[wide\\] 3.png](kuvo-attachment:file/${edited.files[2].anchor})`)).toBe(true);
});


test("Markdown fences, delimiter lengths, nested destinations and RTL preserve mention offsets", () => {
  const source = "~~~ markdown\n#41\n~~~~\n\n`` See #42 ` followed by #43 ``\n\n" +
    "[See #44](https://example.test/path(a(b))) [word #45][target]\n\n[target]: https://example.test\n\n" +
    "🖼 עברית #46 ثم #47";
  expect(references(source)).toEqual([46, 47].map(number => {
    const start = source.indexOf(`#${number}`);
    return { start, end: start + 3, number };
  }));
});

const patch = (id: string, given: Record<string, unknown>) => actHere({ method: "PATCH", ref: { id }, targets: [], given });
const find = (id: string) => allItems().find(item => item.id === id)!;
const read = async (id: string) => {
  const answer = await actHere({ method: "GET", ref: { id }, targets: [] });
  expect(answer.status).toBe(200);
  return (answer.value as { item: {
    body: string; files: { name: string; anchor?: string }[];
    bodyMentions: { item: string }[];
    notes: { text: string; mentions: { item: string; start: number; end: number; number: number }[] }[];
    itemRelations: { id: string; kind: string }[];
  } }).item;
};
const add = (title: string, given = {}) => {
  const answer = addHere({ title, ...given });
  expect(answer.status).toBe(200);
  return (answer.value as { item: { id: string } }).item.id;
};
const png = (name: string, bytes: string) => ({ name, uri: "data:image/png;base64," + Buffer.from(bytes).toString("base64") });
const marker = (index: number) => `![Attachment](kuvo-attachment:draft/${index}/photo.png)`;

test("a typed note returns the same stable mention and offsets as its backlink", async () => {
  const target = add("Note destination");
  const source = add("Note source");
  changeQueues(book => {
    book.find({ id: target })!.number = 749210;
    book.find({ id: source })!.notes.push({ text: "🎨 See #749210", by: { user: true }, at: Date.now() });
  });
  expect((await read(source)).notes[0].mentions).toEqual([{ item: target, number: 749210, start: 7, end: 14 }]);
  expect((await read(target)).itemRelations).toEqual([expect.objectContaining({ id: source, kind: "mentioned-in" })]);
});

test("a saved attachment read rejects symlink replacements for the file and the item folder", () => {
  const id = add("Attachment containment", { files: [png("photo.png", "legitimate")] });
  expect(fileOf({ id }, "photo.png")?.bytes.toString()).toBe("legitimate");
  const outside = fs.mkdtempSync(path.join(WORK_QUEUE, "outside-attachments-"));
  const file = path.join(outside, "photo.png");
  fs.writeFileSync(file, "outside");
  try {
    const saved = path.join(filesOf(id), "photo.png");
    fs.unlinkSync(saved);
    fs.symlinkSync(file, saved);
    expect(fileOf({ id }, "photo.png")).toBeNull();
    fs.unlinkSync(saved);
    fs.rmdirSync(filesOf(id));
    fs.symlinkSync(outside, filesOf(id), "dir");
    expect(fileOf({ id }, "photo.png")).toBeNull();
  } finally {
    fs.rmSync(filesOf(id), { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  }
});

test("case collisions and names occupied on disk keep both anchors on their own bytes", async () => {
  const id = add("Case collisions", { body: marker(0), files: [png("Photo.png", "first")] });
  const occupied = path.join(filesOf(id), "photo 2.png");
  fs.writeFileSync(occupied, "already there", { flag: "wx" });
  const before = find(id);
  expect((await patch(id, { body: before.body + "\n" + marker(0), files: [png("photo.png", "second")] })).status).toBe(200);
  const saved = find(id);
  expect(saved.files.map(file => file.name)).toEqual(["Photo.png", "photo 3.png"]);
  expect(fs.readFileSync(occupied, "utf8")).toBe("already there");
  const byAnchor = new Map(saved.files.map(file => [file.anchor, fileOf({ id }, file.name)?.bytes.toString()]));
  expect([...saved.body.matchAll(/kuvo-attachment:file\/([a-f0-9]+)/g)].map(match => byAnchor.get(match[1]))).toEqual(["first", "second"]);
});

test("removing a picture marker preserves the latest description and other attachments", async () => {
  const id = add("Keep the later edit", { body: "Earlier " + marker(0) + " / " + marker(1), files: [png("a.png", "a"), png("b.png", "b")] });
  const snapshot = await read(id);
  const first = snapshot.files[0], second = snapshot.files[1];
  const url = `kuvo-attachment:file/${first.anchor}`;
  const code = `\`![a.png](${url})\``;
  const late = "The bot edited the text. " + snapshot.body + "\n\n" + code;
  expect((await patch(id, { body: late })).status).toBe(200);
  const request = { removeInlineImages: [{ url, name: first.name }] };
  const removed = await patch(id, request);
  expect(removed.status).toBe(200);
  expect(removed.value).toMatchObject({ inlineImagesApplied: true });
  expect((await patch(id, request)).status).toBe(200);
  expect((await read(id)).body).toBe(`The bot edited the text. Earlier  / ![b.png](kuvo-attachment:file/${second.anchor})\n\n${code}`);
  expect(fileOf({ id }, second.name)?.bytes.toString()).toBe("b");
});

test("soft deletion hides a manual link at both ends and purging prunes its stored edge", async () => {
  const [holder, target] = [add("Purge link first"), add("Purge link second")].sort();
  expect((await patch(holder, { itemRelation: { item: target, kind: "blocks" } })).status).toBe(200);
  expect((await actHere({ method: "DELETE", ref: { id: holder }, targets: [] })).status).toBe(200);
  expect((await read(holder)).itemRelations).toEqual([]);
  expect((await read(target)).itemRelations).toEqual([]);
  expect((await actHere({ method: "PATCH", action: "restore", ref: { id: holder }, targets: [] })).status).toBe(200);
  expect((await read(target)).itemRelations).toEqual([expect.objectContaining({ id: holder, kind: "blocked-by" })]);
  expect((await actHere({ method: "DELETE", ref: { id: target }, targets: [] })).status).toBe(200);
  changeQueues(() => {}, Date.now() + KEPT_REMOVED + 1000);
  expect(find(holder).relations).toBeUndefined();
});

test("unrelated writes keep the saved mention index and recovery rebuilds only a missing index", async () => {
  const target = add("Indexed target");
  const source = add("Indexed source");
  const unrelated = add("Unrelated change");
  changeQueues(book => { book.find({ id: target })!.number = 749211; });
  await patch(source, { body: "See #749211" });
  const index = find(source).mentionIndex;
  expect(index?.targets).toEqual([target]);
  const parsed = spyOn(await import("../src/link/work-queue/markdown"), "protectedRanges");
  try {
    await patch(unrelated, { priority: "high" });
    expect(find(source).mentionIndex).toEqual(index);
    expect((await read(source)).itemRelations).toEqual([expect.objectContaining({ id: target })]);
    expect(parsed).not.toHaveBeenCalled();
  } finally { parsed.mockRestore(); }
  // Simulate an old queue file imported without the index. A read can recover the link, and the
  // next write saves the recovered ranges instead of rebuilding on subsequent reads.
  const file = path.join(WORK_QUEUE, "queues", "usr_you.json");
  const stored = JSON.parse(fs.readFileSync(file, "utf8"));
  delete stored.items.find((item: { id: string }) => item.id === source).mentionIndex;
  fs.writeFileSync(file, JSON.stringify(stored));
  expect((await read(source)).bodyMentions[0].item).toBe(target);
  await patch(unrelated, { priority: "low" });
  expect(JSON.parse(fs.readFileSync(file, "utf8")).items.find((item: { id: string }) => item.id === source).mentionIndex).toEqual(index);
});
