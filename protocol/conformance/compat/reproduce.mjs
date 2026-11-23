// Compare two independent runs, allowing only paths and named external caches
// whose bytes depend on filesystem metadata or SQLite's WAL salt.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, lstatSync, readdirSync, readFileSync, readlinkSync, rmSync } from "node:fs";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";
import { readJSON, scratch } from "../adapter.mjs";
import { generateCurrent, generateHoldTranscript, generateRelease } from "./generate.mjs";
import { tags } from "./tagged.mjs";

function sqlite(file, normalize) {
  const root = scratch();
  const copy = path.join(root, "copy.sqlite");
  try {
    copyFileSync(file, copy);
    if (existsSync(file + "-wal")) copyFileSync(file + "-wal", copy + "-wal");
    const db = new DatabaseSync(copy, { readOnly: true });
    try {
      const schema = db.prepare("SELECT * FROM sqlite_schema ORDER BY name").all();
      const tables = schema.filter(row => row.type === "table").map(({ name }) => {
        const table = db.prepare('SELECT * FROM "' + name.replaceAll('"', '""') + '"');
        table.setReadBigInts(true);
        return table.all().map(row => normalize(JSON.stringify(row, (_, value) => typeof value === "bigint"
          ? { integer: value.toString() } : value))).sort();
      });
      return JSON.stringify({ schema, tables, version: db.prepare("PRAGMA user_version").get() });
    } finally {
      db.close();
    }
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function manifest(root) {
  const items = {};
  const replacements = new Map();
  for (const tag of tags) {
    const world = path.join(root, tag, "app-world.json");
    if (!existsSync(world)) continue;
    const mac = readJSON(world);
    for (const [name, file] of Object.entries({ project: mac.repository, worktree: mac.worktree, chat: mac.chat,
      plain: path.join(mac.home, "Projects", "plain") })) {
      for (const prefix of ["", "directory:"]) {
        replacements.set(createHash("sha1").update(prefix + file).digest("hex"), `<${tag}:${prefix}${name}>`);
      }
    }
  }
  function normalize(value) {
    let text = value.replaceAll("/private" + root, "<cache>").replaceAll(root, "<cache>");
    for (const [from, to] of replacements) text = text.replaceAll(from, to);
    return text;
  }
  function visit(folder) {
    for (const name of readdirSync(folder).sort()) {
      const file = path.join(folder, name);
      const relative = normalize(path.relative(root, file));
      const stat = lstatSync(file);
      if (stat.isDirectory()) {
        items[relative] = { mode: stat.mode };
        visit(file);
      } else if (stat.isSymbolicLink()) {
        items[relative] = { link: normalize(readlinkSync(file)) };
      } else {
        const data = readFileSync(file);
        assert.ok(stat.isFile(), relative);
        const variable = relative.endsWith(".git/index") || /^.*\/\.git\/worktrees\/[^/]+\/index$/.test(relative)
          ? "Git stat cache"
          : /\.(db|sqlite)-(wal|shm)$/.test(relative) ? "SQLite WAL salt or shared-memory index" : "";
        let content = /\.(db|sqlite)$/.test(relative) ? sqlite(file, normalize)
          : data.includes(0) ? data : normalize(data.toString("utf8"));
        if (relative === "v0.8.0-beta.1/machine/run/link.json") {
          const link = JSON.parse(content);
          assert.deepEqual(Object.keys(link.macSeen), [link.mac]);
          assert.ok(Number.isSafeInteger(link.macSeen[link.mac]));
          // The released setup writer stamps the time it last heard its Mac.
          link.macSeen[link.mac] = "<last heard>";
          content = JSON.stringify(link);
        }
        const digest = variable || createHash("sha256").update(content).digest("hex");
        items[relative] = { mode: stat.mode, digest };
      }
    }
  }
  visit(root);
  return items;
}

const roots = [scratch(), scratch()];
try {
  for (const root of roots) {
    for (const tag of tags) await generateRelease(tag, path.join(root, tag));
    generateCurrent(path.join(root, "current"));
    generateHoldTranscript(root);
  }
  const first = manifest(roots[0]);
  const second = manifest(roots[1]);
  assert.deepEqual(Object.keys(second).sort(), Object.keys(first).sort(), "generated paths differ");
  for (const name of Object.keys(first)) assert.deepEqual(second[name], first[name], name);
  console.log(`Two generations match across ${Object.keys(first).length} entries.`);
} finally {
  for (const root of roots) rmSync(root, { recursive: true, force: true });
}
