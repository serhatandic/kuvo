// Windows path spelling for the protocol. This is lexical conversion only. Access checks must
// resolve the actual object via the filesystem, including junctions and case-sensitive directories.

import { win32 } from "node:path";

/** Absolute drive and UNC paths, spelled with forward slashes on the wire. */
export function windowsWirePath(native: string): string | undefined {
  const wire = native.replaceAll("\\", "/");
  const root = /^(?:[A-Za-z]:\/|\/\/[^/]+\/[^/]+(?=\/|$))/.exec(wire)?.[0];
  if (!root || /[:<>"|?*\x00-\x1F]/.test(wire.slice(root[1] === ":" ? 2 : 0))) return undefined;
  // A UNC share root may have no trailing separator. Both root spellings round-trip.
  if (root.startsWith("//") && root.slice(2).split("/").some(part => /[. ]$/.test(part))) return undefined;
  const tail = wire.slice(root.length).replace(/^\//, "").replace(/\/$/, "");
  if (tail && tail.split("/").some(part => !part || /[. ]$/.test(part))) return undefined;
  if (wire.includes("//", 2)) return undefined;
  return wire;
}

/** The native spelling of a canonical wire path, without resolving it against a local cwd. */
export function windowsNativePath(wire: string): string | undefined {
  return windowsWirePath(wire) === wire ? wire.replaceAll("/", "\\") : undefined;
}

/** Environment names on Windows stay case-insensitive even in plain copies of process.env. */
export function windowsEnvValue(env: NodeJS.ProcessEnv, key: string): string | undefined {
  return env[key] ?? Object.entries(env).find(([name]) => name.toUpperCase() === key)?.[1];
}

/** Search only fully qualified folders. Keep case for NTFS directories that distinguish it. */
export function windowsPathFolders(list: string | undefined): string[] {
  const folders: string[] = [];
  for (const part of (list ?? "").matchAll(/(?:^|;)(?:"([^"]*)"|([^;"]*))(?=;|$)/g)) {
    const wire = windowsWirePath(part[1] ?? part[2].trim());
    if (wire !== undefined) folders.push(wire.replaceAll("/", "\\"));
  }
  return folders;
}

/** Join lists by semicolon, quoting a folder that contains one. Drive colons are ordinary text. */
export function joinWindowsPath(...lists: (string | undefined)[]): string {
  return [...new Set(lists.flatMap(windowsPathFolders))]
    .map(folder => folder.includes(win32.delimiter) ? `"${folder}"` : folder).join(win32.delimiter);
}
