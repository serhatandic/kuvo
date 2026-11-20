import { fileURLToPath, pathToFileURL } from "node:url";

/** Chat records may carry paths on their own, in attachment notes, or as file URLs. */
export function mapAttachmentPaths<T>(value: T, paths: ReadonlyMap<string, string>): T {
  if (!paths.size) return value;
  const replacements = new Map(paths);
  for (const [from, to] of paths) replacements.set(pathToFileURL(from).href, pathToFileURL(to).href);
  const pattern = new RegExp([...replacements.keys()].sort((a, b) => b.length - a.length)
    .map(key => key.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|") + "(?![\\w/%-]|\\.[\\w.])", "g");
  const visit = (part: unknown): unknown => {
    if (Array.isArray(part)) return part.map(visit);
    if (part && typeof part === "object") return Object.fromEntries(Object.entries(part).map(([key, value]) => [key, visit(value)]));
    if (typeof part !== "string") return part;
    if (part.startsWith("file:")) {
      try {
        const mapped = paths.get(fileURLToPath(part));
        if (mapped) return pathToFileURL(mapped).href;
      } catch {}
    }
    return part.replace(pattern, match => replacements.get(match)!);
  };
  return visit(value) as T;
}
