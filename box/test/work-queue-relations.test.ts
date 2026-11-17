import "./safety";
import { expect, test } from "bun:test";

const { actHere, addHere } = await import("../src/link/work-queue/acts");
const { references } = await import("../src/link/work-queue/relations");
const { allItems, changeQueues } = await import("../src/link/work-queue/store");
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
  expect(allItems().find(each => each.id === b)?.mentionLinks).toEqual([{ item: a, kind: "mentioned-in" }]);
  await change(a, { body: "The reference was removed." });
  expect(await read(b)).toEqual([]);
  expect(allItems().find(each => each.id === b)?.mentionLinks).toBeUndefined();
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
