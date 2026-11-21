import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { backupFolderVersion, FOLDER_VERSION_FILE, upgradeKuvoFolder } from "../../shared/folder-version";
import { writeDurablePrivate } from "../files";
import { artifactVersions } from "../artifacts/inventory";
import { MUTATION_EVIDENCE, readForwardEvidence } from "../ownership/forward-evidence";
import { ownedRecordFormat } from "../ownership/store";
import { kindOf, outputPath, storageInventory, tree } from "./locations";

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

export function stageRestore(stage: string, version: number, archived: FileEntry[], upgrade = upgradeKuvoFolder) {
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
  const queueProblems = new Map<string, { entry: string; reason: string }[]>();
  const artifactProblems: { entry: string; reason: string }[] = [];
  for (const { name, file, kind, expectedSize, expectedSHA } of storageInventory(layout)) {
    if (file) {
      const key = MUTATION_EVIDENCE.exec(name)?.[1];
      if (key) {
        const evidence = ownedRecordFormat.read(JSON.parse(fs.readFileSync(file, "utf8")));
        readForwardEvidence(evidence.forward, key);
      }
      if (kind === "draftSubmissionEvidence" || kind === "ownerMessageReceipts")
        ownedRecordFormat.read(JSON.parse(fs.readFileSync(file, "utf8")));
      entries.set(name, file);
    }
    const reason = !file ? "attachment missing from restore point" :
      expectedSize !== undefined && fs.lstatSync(file).size !== expectedSize ? "attachment size differs from its record" :
      expectedSHA !== undefined && crypto.createHash("sha256").update(fs.readFileSync(file)).digest("base64url") !== expectedSHA ? "attachment digest differs from its record" : null;
    if (reason) {
      if (kind === "managedArtifacts") {
        entries.delete(name);
        artifactProblems.push({ entry: name, reason: "retained artifact copy missing or differs from its record" });
        continue;
      }
      // Older points could omit a listed attachment and still be published.
      const id = name.split("/")[4]!;
      const problems = queueProblems.get(id) ?? [];
      problems.push({ entry: name, reason });
      queueProblems.set(id, problems);
    }
  }
  for (const [entry, version] of artifactVersions(layout.artifacts!)) {
    if (version.kept === false && version.status !== "deleted") {
      artifactProblems.push({ entry, reason: "artifact version bytes were not kept at the restore point" });
    }
  }
  for (const root of ["meta", "projects"]) {
    for (const entry of tree(layout[root]!, root, () => true)) entries.set(entry.name, entry.file);
  }
  return { entries, queueProblems, artifactProblems };
}
