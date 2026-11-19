// What's there only for released Macs (PROTOCOL.md, "For released Macs") is marked where it is,
// on every side: the box's link and its agents bridge, its helper, OpenCodeKit and KuvoClient (not
// the retired Mac app, swift/KuvoCode, which is reference only). Each item the list names has its
// markers, and each marker names an item on the list, so a version's paths go together when it
// goes (scripts/compat.sh lists them).
//
// Run with `node --test box/bridge-tests/compat.test.js`.
import "./test-safety.js";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import "./at-end.js";

const repo = fileURLToPath(new URL("../../", import.meta.url));
const protocol = fs.readFileSync(path.join(repo, "protocol/PROTOCOL.md"), "utf8");
const from = protocol.indexOf("\n## For released Macs");
const to = protocol.indexOf("\n## ", from + 1);
const section = protocol.slice(from, to === -1 ? undefined : to);
const listed = [...section.matchAll(/<!-- compat: (0\.7|beta\.1|beta\.30|layout\.4) ([a-z0-9-]+) -->/g)].map(match => `${match[1]} ${match[2]}`);

// The code of every side, as written: not what's built from it (a bundle leaves comments out),
// nor the tests.
const ROOTS = ["box/src", "mac/helper/Sources", "swift/OpenCodeKit/Sources", "swift/KuvoClient/Sources", "swift/KuvoLink/Sources", "scripts"];
const BUILT = new Set(["box/dist/kuvo-agents.js", "box/dist/kuvo-browser.js", "scripts/compat.sh"]);
const CODE = /\.(swift|ts|js|mjs|sh|py)$/;
// The keeper's shell (box/src/keeper), which has no extension.
const SHELL = /^box\/src\/keeper\/[^/.]+$/;
function* files(folder) {
  let entries = [];
  try { entries = fs.readdirSync(path.join(repo, folder), { withFileTypes: true }); } catch { return; }
  for (const entry of entries) {
    const file = `${folder}/${entry.name}`;
    if (entry.isDirectory() && entry.name !== "node_modules" && !entry.name.startsWith(".")) yield* files(file);
    else if (entry.isFile() && (CODE.test(entry.name) || SHELL.test(file)) && !BUILT.has(file)) yield file;
  }
}
const markers = [];
for (const root of ROOTS) {
  for (const file of files(root)) {
    fs.readFileSync(path.join(repo, file), "utf8").split("\n").forEach((line, index) => {
      for (const match of line.matchAll(/COMPAT\(([^)]*)\)(.*)/g)) markers.push({ where: `${file}:${index + 1}`, tag: match[1], rest: match[2] });
    });
  }
}

test("PROTOCOL.md names the items that are there only for released Macs", () => {
  assert.ok(listed.length >= 10, `only ${listed.length} items in "For released Macs"`);
  assert.equal(new Set(listed).size, listed.length, "an item is named twice");
});

test("every item for released Macs is marked in the code", () => {
  const marked = new Set(markers.map(marker => marker.tag));
  assert.deepEqual(listed.filter(item => !marked.has(item)), []);
});

// When each one goes. Since 1.0 took 0.8.0's place (PROTOCOL.md, "What 1.0 keeps of this"), an item is
// one of three kinds, which protocol/compat-kinds.txt names: what keeps a box right while an older
// Mac still connects (`stays`: on the user's word); what no released client needs but isn't decided
// yet (`next`: #717's next part); and, unnamed there, what an earlier version left, taken in once,
// which carries its date from before until the box's move carries it (#713) or it goes.
const KEPT = "Goes on the user's word, once no Mac from before 1.0 connects.";
const NEXT = "Goes in #717's next part.";
const GOES = {
  "0.7": [KEPT, NEXT, "Goes 3 months after 0.8.0.", "Goes once a release with one is the newest.", "Goes on the user's word, once no 0.7 Mac is expected to update."],
  "beta.1": [KEPT, "Goes when 0.8.0 ships.", "Goes 3 months after 0.8.0."],
  // What a box from before beta.31 needs once to take this release (its updater, its keeper, its
  // servers from before): gone once none is.
  "beta.30": ["Goes once no box is from before beta.31."],
  "layout.4": ["Goes once supported upgrades and rollbacks no longer use layout 4 setup records."],
};
const kinds = new Map(fs.readFileSync(path.join(repo, "protocol/compat-kinds.txt"), "utf8").split("\n").filter(line => line && !line.startsWith("#")).map(line => {
  const [version, item, kind] = line.split(" ");
  return [`${version} ${item}`, kind];
}));
// Files whose bytes are the box's release, or a hash of them (the keeper's shell, the same shell
// held in Swift until the retired app goes, and the browser's keeper): their markers keep the
// words they had, so a re-marking doesn't change what machines run. An item's kind is still the
// list's, whatever such a marker says.
const FROZEN = file => SHELL.test(file) || file === "swift/OpenCodeKit/Sources/OpenCodeKit/Engine/RemoteEngine.swift" || file === "box/src/browser/kuvo-browser.js";

test("every marker names an item on the list, and says what it does and when it goes", () => {
  const known = new Set(listed);
  assert.deepEqual(markers.filter(marker => !known.has(marker.tag)).map(marker => `${marker.where} COMPAT(${marker.tag})`), []);
  assert.deepEqual(markers.filter(marker => !/^: \S.* Goes /.test(marker.rest)).map(marker => marker.where), []);
  const goes = marker => GOES[marker.tag.split(" ")[0]]?.some(words => marker.rest.trimEnd().endsWith(words));
  assert.deepEqual(markers.filter(marker => !goes(marker)).map(marker => `${marker.where} COMPAT(${marker.tag})${marker.rest}`), []);
});

test("an item's kind is one on the list, and its markers end as its kind does", () => {
  const known = new Set(listed);
  assert.deepEqual([...kinds].filter(([item, kind]) => !known.has(item) || !["stays", "next", "feed"].includes(kind)), []);
  const ends = { stays: KEPT, next: NEXT, feed: "Goes once a release with one is the newest." };
  const wrong = markers.filter(marker => !FROZEN(marker.where.split(":")[0])).filter(marker => {
    const end = ends[kinds.get(marker.tag)];
    const rest = marker.rest.trimEnd();
    // (One that's neither keeps a date from before: never one of the two.)
    return end ? !rest.endsWith(end) : rest.endsWith(KEPT) || rest.endsWith(NEXT);
  });
  assert.deepEqual(wrong.map(marker => `${marker.where} COMPAT(${marker.tag})${marker.rest}`), []);
});
