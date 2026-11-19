import fs from "node:fs";
import path from "node:path";
import type { DataReference } from "../ownership/resolve";
import { DATA_DECLARATIONS, type DataDeclaration, type DataKind } from "../../shared/data-declarations";
import { APP_FOLDER, BOTS, ENGINE_DATA, K, MAC_ATTACHMENTS, SESSION_WAKES } from "../home";

export const roots: Record<string, string | null> = {
  home: K,
  bots: BOTS,
  wakes: SESSION_WAKES,
  engine: process.env.KUVO_ENGINE_DB_FOLDER || ENGINE_DATA,
  agents: process.env.KUVO_AGENTS_HOME || (APP_FOLDER ? path.join(APP_FOLDER, "agents") : path.join(K, "agents")),
  groups: process.env.KUVO_GROUPS || path.join(APP_FOLDER ?? K, "groups"),
  artifacts: process.env.KUVO_ARTIFACTS || path.join(K, "artifacts"),
  attachments: path.join(K, "attachments"),
  macAttachments: MAC_ATTACHMENTS,
};

const component = (pattern: string) => new RegExp("^" + pattern.split("*")
  .map(part => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("[^/]*") + "$");

const selectors = Object.entries(DATA_DECLARATIONS).flatMap(([key, policy]: [string, DataDeclaration]) =>
  !policy.backup ? [] : (policy.storage ?? []).map(location => {
    const pattern = typeof location === "string" ? location : location.pattern;
    const restore = typeof location === "string" ? policy.restore : location.restore;
    const [root, ...parts] = pattern.replace(/\/$/, "").split("/");
    return { kind: key as DataKind, restore, root, parts: parts.map(component), tree: pattern.endsWith("/") };
  }));

const temporary = (name: string) => name.startsWith(".") || /\.(?:tmp|new|lock)$/.test(name);
export function storageOf(name: string) {
  const [root, ...parts] = name.split("/");
  if (parts.some(temporary)) return undefined;
  return selectors.find(rule => rule.root === root && (rule.tree ? parts.length >= rule.parts.length : parts.length === rule.parts.length)
    && rule.parts.every((match, i) => match.test(parts[i]!)));
}

export const kindOf = (name: string): DataKind | undefined => storageOf(name)?.kind;

/** Map individual files to the same record address used by writes. Aggregate
 * stores (for example SQLite) resolve their record IDs with their own reader. */
export function recordReferences(name: string, sessionFor: (root: string, id: string) => string): DataReference[] {
  const [root, first, second] = name.split("/");
  if (root === "bots" && second) return [
    { kind: "botSettings", id: first }, { kind: "conversations", id: sessionFor(root, first) },
  ];
  if (root === "groups" && second) return [{ kind: "groupSettings", id: first }];
  if (root === "agents" && first === "sessions" && second) return [{ kind: "conversations", id: second }];
  if (root === "wakes") return [{ kind: "conversations", id: sessionFor(root, first) }];
  if ((root === "attachments" || root === "macAttachments") && first.startsWith("ses_"))
    return [{ kind: "sessionFiles", id: first }];
  return [];

}

export function stat(file: string): fs.Stats | undefined {
  return fs.lstatSync(file, { throwIfNoEntry: false });
}

export function* tree(file: string, name: string, include: (name: string) => boolean): Generator<{ file: string; name: string }> {
  const info = stat(file);
  if (info?.isFile()) yield { file, name };
  if (info?.isDirectory()) for (const child of fs.readdirSync(file).sort()) {
    if (include(child)) yield* tree(path.join(file, child), `${name}/${child}`, include);
  }
}

export function* durableFiles(layout: Readonly<Record<string, string | null>> = roots): Generator<{ name: string; file: string; kind: DataKind }> {
  const seen = new Set<string>();
  for (const { root, parts, tree: recursive } of selectors) {
    const base = layout[root];
    if (!base) continue;
    function* walk(file: string, name: string, at: number): Generator<{ file: string; name: string }> {
      if (at === parts.length) {
        if (recursive) yield* tree(file, name, child => !temporary(child));
        else if (stat(file)?.isFile()) yield { file, name };
        return;
      }
      if (!stat(file)?.isDirectory()) return;
      for (const child of fs.readdirSync(file).sort()) {
        if (!temporary(child) && parts[at]!.test(child)) yield* walk(path.join(file, child), `${name}/${child}`, at + 1);
      }
    }
    for (const item of walk(base, root, 0)) {
      if (seen.has(item.name)) continue;
      seen.add(item.name);
      yield { ...item, kind: kindOf(item.name)! };
    }
  }
}

export function inside(parent: string, child: string): boolean {
  const relative = path.relative(parent, child);
  return !relative || (!relative.startsWith(".." + path.sep) && relative !== ".." && !path.isAbsolute(relative));
}

/** Walk before writing so a link elsewhere cannot turn a restored file into an overwrite. */
export function outputPath(base: string, parts: string[]): string {
  const root = path.resolve(base);
  const target = path.resolve(root, ...parts);
  if (target === root || !inside(root, target)) throw new Error("Invalid recovery path.");
  for (let at = path.dirname(target); inside(path.resolve(base), at); at = path.dirname(at)) {
    const info = stat(at);
    if (info && !info.isDirectory()) throw new Error("The recovery path contains a link or non-directory.");
    if (at === root) break;
  }
  return target;
}
