import path from "node:path";
import { mapAttachmentPaths } from "../../shared/attachment-paths";
import { isObject } from "../../shared/json";
import { transcriptAttachmentPaths } from "../moves/session-files";
import { recordKey } from "../ownership/store";
import { inside, outputPath, stat } from "./locations";

export function attachmentRecovery(saved: unknown, recovery: string, unavailable: { entry: string; reason: string }[]) {
  const names = ["attachments", "macAttachments"] as const;
  const sources = new Map(names.flatMap(name => {
    const folder = isObject(saved) ? saved[name] : null;
    return typeof folder === "string" && path.isAbsolute(folder) ? [[name, path.resolve(folder)] as const] : [];
  }));
  const placed = new Map<string, string>();
  const reported = new Set<string>();
  return {
    add(root: string, parts: string[], target: string) {
      const source = sources.get(root as typeof names[number]);
      if (source) placed.set(path.join(source, ...parts), target);
    },
    rewrite(record: unknown): unknown {
      if (!sources.size) return record;
      const paths = new Map<string, string>();
      for (const file of transcriptAttachmentPaths(record)) {
        if (file.includes("\0")) continue;
        const target = placed.get(file);
        if (target) {
          if (target !== file) paths.set(file, target);
        } else {
          for (const [root, source] of sources) {
            if (file === source || !inside(source, file)) continue;
            const entry = root + "/" + path.relative(source, file);
            if (!reported.has(entry)) {
              reported.add(entry);
              unavailable.push({ entry, reason: "referenced session attachment has no restored copy (missing, expired or moved)" });
            }
            // The old absolute address can belong to unrelated replacement data.
            const missing = outputPath(recovery, ["unavailable-attachments", recordKey(entry), path.basename(file)]);
            if (stat(missing)) throw new Error("An unavailable attachment address is occupied.");
            paths.set(file, missing);
            break;
          }
        }
      }
      return paths.size ? mapAttachmentPaths(record, paths) : record;
    },
  };
}
