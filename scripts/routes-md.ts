// protocol/ROUTES.md, as scripts/route-shapes.ts writes it (scripts/build-built.sh): every
// route of a box, by area (one file of box/src/shared/routes each), with its feature, what it takes
// and answers, and the comment written over it in its file. It's a readable list, never kept in
// git: a route is documented where it's defined, and PROTOCOL.md holds only the rules a route can't say.

import fs from "node:fs";
import path from "node:path";

type Shape = string | { list: Shape } | { map: Shape } | { object: Record<string, Shape> } | { oneOf: Shape[] } | { literal: string | number | boolean };
interface Route { route: string; feature: string; body?: Shape; answers: Record<string, Shape> }

// The source's top-level pieces between `[` and its `]`, each with the comments written before it
// (or after it on its line), skipping what's in strings and comments.
function entries(text: string, open: number): { text: string; doc: string }[] {
  const found: { text: string; doc: string }[] = [];
  let depth = 0, i = open, start = -1, notes: string[] = [];
  const close = () => {
    if (start < 0) return;
    const piece = text.slice(start, i).trim();
    if (piece) found.push({ text: piece, doc: notes.join(" ") });
    start = -1;
    notes = [];
  };
  for (; i < text.length; i++) {
    const c = text[i];
    if (c === "/" && text[i + 1] === "/") {
      const end = text.indexOf("\n", i);
      const note = text.slice(i + 2, end).trim();
      // A comment after an entry on its own line belongs to that entry.
      const lineStart = text.lastIndexOf("\n", i) + 1;
      if (depth === 1 && start < 0 && found.length && text.slice(lineStart, i).trim().replace(/,$/, "").length) {
        const last = found[found.length - 1];
        last.doc = [last.doc, note].filter(Boolean).join(" ");
      } else if (depth === 1 && start < 0) notes.push(note);
      i = end;
      continue;
    }
    if (c === "/" && text[i + 1] === "*") {
      const end = text.indexOf("*/", i);
      if (depth === 1 && start < 0) notes.push(text.slice(i + 2, end).replace(/^\s*\*+/gm, "").replace(/^\*/, "").trim());
      i = end + 1;
      continue;
    }
    if (c === '"' || c === "'" || c === "`") {
      if (depth === 1 && start < 0) start = i;
      for (i++; i < text.length && text[i] !== c; i++) if (text[i] === "\\") i++;
      continue;
    }
    if (c === "[" || c === "{" || c === "(") {
      if (depth === 1 && start < 0) start = i;
      depth++;
      continue;
    }
    if (c === "]" || c === "}" || c === ")") {
      depth--;
      if (depth === 0) { close(); break; }
      continue;
    }
    if (depth === 1 && c === ",") { close(); continue; }
    if (depth === 1 && start < 0 && !/\s/.test(c)) start = i;
  }
  return found;
}

// What's written in the entry for each status, where it's a name (`200: STARTED`).
function namedAnswers(entry: string): Record<string, string> {
  const named: Record<string, string> = {};
  for (const [, status, name] of entry.matchAll(/(?:[{,]\s*)(\d{3}):\s*([A-Z][A-Z0-9_]*)\s*(?=[,}])/g)) named[status] = name;
  return named;
}

const plain = (s: string) => s.replace(/\s+/g, " ").trim();

// A shape in a few words: an object by its fields' names (one level deep), a list as [ ], a map as
// {*: }, one of several with |.
function brief(shape: Shape, depth = 0): string {
  if (typeof shape === "string") return shape;
  if ("literal" in shape) return JSON.stringify(shape.literal);
  if ("list" in shape) return `[${brief(shape.list, depth)}]`;
  if ("map" in shape) return `{*: ${brief(shape.map, depth + 1)}}`;
  if ("oneOf" in shape) {
    const each = shape.oneOf.map(one => brief(one, depth));
    return each.length > 6 && shape.oneOf.every(one => typeof one === "object" && "literal" in one) ? `${each.slice(0, 6).join(" | ")} | …` : each.join(" | ");
  }
  const fields = Object.keys(shape.object);
  if (!fields.length) return "{}";
  if (depth > 0) return "{…}";
  return `{${fields.slice(0, 14).join(", ")}${fields.length > 14 ? ", …" : ""}}`;
}

// A refusal that says only what every refusal may (`ERROR`'s fields).
const ERROR_FIELDS = new Set(["message", "message?", "problem?", "reason?", "action?"]);
const isPlainRefusal = (shape: Shape) => typeof shape === "object" && "object" in shape && Object.keys(shape.object).every(field => ERROR_FIELDS.has(field));

// A shape said in at most this many characters, the rest left to its file.
const LONGEST = 160;
const cut = (said: string) => said.length > LONGEST ? `${said.slice(0, LONGEST - 1)}…` : said;

function answersLine(route: Route, named: Record<string, string>): string {
  // Statuses that answer alike are said once, together.
  const alike = new Map<string, string[]>();
  const refusals: string[] = [];
  for (const [status, shape] of Object.entries(route.answers)) {
    if (Number(status) >= 400 && isPlainRefusal(shape)) { refusals.push(status); continue; }
    const name = named[status];
    const said = `${name ? `\`${name}\` ` : ""}\`${cut(brief(shape))}\``;
    alike.set(said, [...(alike.get(said) ?? []), status]);
  }
  const parts = [...alike].map(([said, statuses]) => `${statuses.join(" ")} ${said}`);
  if (refusals.length) parts.push(`refuses ${refusals.join(" ")}`);
  if (route.body) parts.unshift(`takes \`${cut(brief(route.body))}\``);
  return parts.join("; ");
}

export async function routesMarkdown(root: string): Promise<string> {
  const dir = path.join(root, "box/src/shared/routes");
  const lines = [
    "# Routes",
    "",
    "Every route a box answers, by area: written from box/src/shared/routes by `scripts/build-built.sh` (scripts/route-shapes.sh); not kept in git, never edited. Each route says its feature (a client checks `kuvo/info`'s `features` before it uses the route), what it takes, what it answers by status (the shape's name in its file, and its fields; `?` may be left out) and the comment written over it in its file. A refusal that says only `message` (and `problem`, `reason`, `action`: index.ts's `ERROR`) is listed by status alone. The rules a route can't say are in [PROTOCOL.md](PROTOCOL.md).",
    "",
  ];
  const files = fs.readdirSync(dir).filter(name => name.endsWith(".ts") && name !== "index.ts").sort();
  let total = 0;
  for (const file of files) {
    const source = fs.readFileSync(path.join(dir, file), "utf8");
    const module = await import(path.join(dir, file));
    const arrays = Object.keys(module).filter(name => name.endsWith("_ROUTES") && Array.isArray(module[name]));
    if (!arrays.length) continue;
    const header = plain(source.split("\n").filter((line, at, all) => all.slice(0, at + 1).every(each => each.startsWith("//"))).map(line => line.replace(/^\/\/\s?/, "")).join(" "));
    lines.push(`## ${file.replace(/\.ts$/, "")}`, "");
    if (header) lines.push(header, "");
    for (const name of arrays) {
      const routes: Route[] = module[name];
      const at = source.search(new RegExp(`export const ${name}\\b[^=]*=\\s*\\[`));
      const written = at < 0 ? [] : entries(source, source.indexOf("[", source.indexOf("=", at)));
      if (written.length !== routes.length) throw new Error(`${file}: ${name} has ${routes.length} routes but ${written.length} entries written: write each route as an entry of its own.`);
      routes.forEach((route, index) => {
        const doc = plain(written[index].doc);
        lines.push(`- \`${route.route}\` · \`${route.feature}\`${doc ? `: ${doc}` : ""}`);
        lines.push(`  ${answersLine(route, namedAnswers(written[index].text))}`);
        total++;
      });
    }
    lines.push("");
  }
  lines.splice(4, 0, `${total} routes in ${files.length} files.`, "");
  return lines.join("\n").replace(/\n+$/, "\n");
}
