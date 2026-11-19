import fs from "node:fs";
import path from "node:path";
import { backupFolderVersion, FOLDER_VERSION_FILE, upgradeKuvoFolder } from "../../shared/folder-version";
import { writeDurablePrivate } from "../files";
import { durableFiles, kindOf, outputPath, tree } from "./locations";

interface FileEntry { name: string; file: string }

// Use the computer folder's canonical addresses, including data that a Mac may
// keep beside its Box. The metadata and opted-in projects have no live address.
const paths: Record<string, string> = {
  home: ".",
  agents: "agents",
  artifacts: "artifacts",
  attachments: "attachments",
  bots: "bots",
  engine: "data/opencode",
  groups: "groups",
  macAttachments: ".restore/macAttachments",
  wakes: "wakes",
  meta: ".restore/meta",
  projects: ".restore/projects",
};

export function stageRestore(stage: string, version: number, archived: FileEntry[], upgrade = upgradeKuvoFolder): Map<string, string> {
  fs.mkdirSync(stage, { mode: 0o700 });
  writeDurablePrivate(path.join(stage, FOLDER_VERSION_FILE), JSON.stringify({ version }));
  backupFolderVersion(stage);
  const layout = Object.fromEntries(Object.entries(paths).map(([root, relative]) => [root, path.join(stage, relative)]));
  for (const { name, file } of archived) {
    const [root, ...parts] = name.split("/");
    if (!Object.hasOwn(layout, root) || (root !== "projects" && root !== "meta" && !kindOf(name)))
      throw new Error("The archive contains an undeclared record.");
    const at = outputPath(layout[root]!, parts);
    fs.mkdirSync(path.dirname(at), { recursive: true, mode: 0o700 });
    fs.linkSync(file, at);
  }
  upgrade(stage);

  const entries = new Map<string, string>();
  for (const entry of durableFiles(layout)) entries.set(entry.name, entry.file);
  for (const root of ["meta", "projects"]) {
    for (const entry of tree(layout[root]!, root, () => true)) entries.set(entry.name, entry.file);
  }
  return entries;
}
