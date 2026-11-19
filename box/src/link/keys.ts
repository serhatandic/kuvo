// Where keys are kept (PROTOCOL.md, "Host interface"): integrations' keys by their variable's
// name (`KUVO_KEY_<ID>_<NAME>`). A machine keeps them in files only you can read (`box/keys`); this
// Mac's box asks its app, which keeps them in the Keychain and, for the box in its folder, writes
// the files its engine reads them from. A Mac's box in a home of its own writes those itself, where
// a machine keeps its keys, from what its host keeps (`writesKeyFiles`, `keepKeyFiles`).
//
// Bots once had keys of their own (`…__<BOT>`, a machine's in `box/bot-keys`): no longer, and the
// ones a box still keeps go as it starts (`dropBotKeys`). Until then they're no key of the box's:
// never listed, read or written.

import fs from "node:fs";
import path from "node:path";
import { lockedWhile } from "../shared/files";
import { textOf, listOf, locked, writeSecret } from "./files";
import { BOT_KEYS, BOX_KEYS, CONFIG_LOCK, K, KEYS_ON_HOST, RUN, writesKeyFiles } from "./home";

/** Where a Mac's start once left a machine's keys (`keys`, compat/adopt-settings.ts). */
const LEGACY_KEYS = path.join(K, "keys");
import { KeysUnavailable, NOT_ANSWERING, appHost, askHost } from "./host-interface";

/** The only names the box ever asks for. */
export const STORED_KEY = /^KUVO_KEY_[A-Z0-9_]+$/;

/** A key a bot had of its own, by its name, as an earlier box kept them: an integration's variable,
 * then the bot's ID in capitals (`<variable>__BOT_<ID>`). */
export const BOT_KEY = /^KUVO_KEY_[A-Z0-9_]+__BOT_[A-Z0-9]+$/;

/** The app that keeps this box's keys, or null for its files; this Mac's box without its app says so. */
function keyHost() {
  const host = appHost();
  if (KEYS_ON_HOST && !host) throw new KeysUnavailable(NOT_ANSWERING);
  return host;
}

// Key work, one change at a time: on this Mac each key is a round trip to the app, and two changes
// (a phone's and Settings', or a save as the box starts) would each go by the names they read
// before the other's.
let keyQueue: Promise<unknown> = Promise.resolve();
export function keyWork<T>(work: () => T | Promise<T>): Promise<T> {
  const run = keyQueue.then(work, work);
  keyQueue = run.catch(() => {});
  return run;
}

const checkedKeyName = (name: unknown): string => {
  if (typeof name !== "string" || !STORED_KEY.test(name) || BOT_KEY.test(name)) throw new KeysUnavailable("That isn't a key's name.");
  return name;
};

/** The file one is kept in on a machine (`box/keys`). */
const keyFile = (name: string) => path.join(BOX_KEYS, checkedKeyName(name));

/** The names of the keys kept, those that begin with `prefix` and end with `suffix`, sorted. */
export async function keyNames({ prefix = "", suffix = "" }: { prefix?: string; suffix?: string } = {}): Promise<string[]> {
  if (typeof prefix !== "string" || typeof suffix !== "string" || (prefix && !/^KUVO_KEY_[A-Z0-9_]*$/.test(prefix)) || (suffix && !/^[A-Z0-9_]+$/.test(suffix))) {
    throw new KeysUnavailable("That isn't a part of a key's name.");
  }
  const host = keyHost();
  let names: unknown[];
  if (host) {
    const answer = await askHost(host, "keys/names", { ...(prefix ? { prefix } : {}), ...(suffix ? { suffix } : {}) });
    // An answer without its list isn't "no keys".
    if (!Array.isArray(answer.names)) throw new KeysUnavailable("This Mac's keys couldn't be read.");
    names = answer.names;
  } else {
    names = listOf(BOX_KEYS);
  }
  return [...new Set(names.filter((name): name is string => typeof name === "string" && STORED_KEY.test(name) && !BOT_KEY.test(name) && name.startsWith(prefix) && name.endsWith(suffix)))].sort();
}

/** Unlink must clear both the current keys and any legacy per-bot Keychain entries. */
export async function clearHostKeys(): Promise<void> {
  await keyWork(async () => {
    const host = keyHost();
    if (!host) return;
    const names = async () => {
      const answer = await askHost(host, "keys/names", {});
      if (!Array.isArray(answer.names) || !answer.names.every(name => typeof name === "string" && STORED_KEY.test(name))) {
        throw new KeysUnavailable("This Mac's keys couldn't be read.");
      }
      return answer.names as string[];
    };
    for (const name of new Set(await names())) await askHost(host, "keys/delete", { name });
    if ((await names()).length) throw new KeysUnavailable("The host still holds Kuvo integration keys.");
  });
}

/** One key's value as it's kept; null for one there isn't. */
export async function readKey(name: string): Promise<string | null> {
  checkedKeyName(name);
  const host = keyHost();
  if (!host) return textOf(keyFile(name));
  const answer = await askHost(host, "keys/get", { name }, true);
  return typeof answer?.value === "string" ? answer.value : null;
}

/** One key kept, in place of one there was. */
export async function writeKey(name: string, value: unknown): Promise<void> {
  checkedKeyName(name);
  if (typeof value !== "string") throw new KeysUnavailable("A key is text.");
  const host = keyHost();
  if (!host) return writeKeyFile(name, value);
  if (!writesKeyFiles()) return void await askHost(host, "keys/set", { name, value });
  // Its file for the engine once its host kept it, both while no other process changes it
  // (`KEY_FILES_LOCK`).
  await keyFileWork(async () => {
    await askHost(host, "keys/set", { name, value });
    writeKeyFile(name, value);
  });
}

/** One key gone (nothing for one there wasn't). */
export async function deleteKey(name: string): Promise<void> {
  checkedKeyName(name);
  const host = keyHost();
  if (!host) return removeKeyFile(name);
  if (!writesKeyFiles()) return void await askHost(host, "keys/delete", { name });
  // Its file once its host let it go: one its host still keeps (it couldn't be reached) stays.
  await keyFileWork(async () => {
    await askHost(host, "keys/delete", { name });
    removeKeyFile(name);
  });
}

/** One key's file, where a machine keeps it (only you can read it), under the settings' lock, as
 * a Mac making the machine's settings its own copies them. */
function writeKeyFile(name: string, value: string): void {
  locked(CONFIG_LOCK, () => writeSecret(keyFile(name), value));
}

function removeKeyFile(name: string): void {
  locked(CONFIG_LOCK, () => fs.rmSync(keyFile(name), { force: true }));
}

/** The keys bots had of their own (#726: bots use this box's integrations, as every session does),
 * gone from where an earlier box kept them, every one, whichever bot's (one here, one deleted, one
 * that moved away, a machine's an earlier Kuvo Code kept in this Mac's Keychain): a machine's files
 * (`box/bot-keys`, and the folder), this Mac's Keychain items (through its app or its helper,
 * `keys/delete`, which takes the app's own file of each away too), and the files a Mac's box in a
 * home of its own wrote them to (`box/bot-keys`). Nothing else is touched: no key of the box's own
 * has such a name (`KUVO_KEY_SLACK_BOT_TOKEN` isn't one: a bot's has `__BOT_`, and the box refuses
 * `__` in a key of its own). Also one so named among the box's own (`box/keys`) or where an earlier
 * Mac's start left its keys (`keys`), which nothing reads. But for `keep`: those of the bots whose
 * folders' settings couldn't be put right, by how their names end. Within `keyWork`. Throws `KeysUnavailable` where its host
 * can't be asked now (what could go has gone, and the rest goes the next time). */
export async function dropBotKeys(keep: string[] = []): Promise<void> {
  const kept = (name: string): boolean => keep.some(end => name.endsWith(end));
  const host = keyHost();
  if (host) {
    const answer = await askHost(host, "keys/names", {});
    if (!Array.isArray(answer.names)) throw new KeysUnavailable("This Mac's keys couldn't be read.");
    for (const name of answer.names) if (typeof name === "string" && BOT_KEY.test(name) && !kept(name)) await askHost(host, "keys/delete", { name });
  }
  // (The files only once its host has let them go: one it still keeps says so by its file.)
  for (const folder of [BOT_KEYS, BOX_KEYS, LEGACY_KEYS]) {
    for (const name of listOf(folder)) if (BOT_KEY.test(name) && !kept(name)) fs.rmSync(path.join(folder, name), { force: true });
  }
  // (Their folder too, unless one's kept in it.)
  if (!listOf(BOT_KEYS).some(kept)) fs.rmSync(BOT_KEYS, { recursive: true, force: true });
}

// A key changed on the host and its file, one at a time across this box's processes (`serve`, a
// client's `box route`): its host may wait on you for a while (a Keychain prompt, two minutes), so
// the lock is held that long.
const KEY_FILES_LOCK = path.join(RUN, ".key-files.lock");
const keyFileWork = <T>(work: () => Promise<T>): Promise<T> => lockedWhile(KEY_FILES_LOCK, work, { stale: 150_000, wait: 150_000 });

/** The engine's keys' files as its host keeps them now (a Mac's box in a home of its own,
 * `writesKeyFiles`): each key it keeps written, and none it doesn't left. Each file is decided on
 * what its host says of it then, under the lock its changes take, so a key kept or removed meanwhile
 * (from another process too) is never undone. False, and the files as they were, where its host
 * can't list them (this Mac's app or its helper not answering, its Keychain locked): a list that
 * can't be read isn't "no keys". False too where a key it lists can't be read now, whose file stays
 * as it is; only one it no longer lists, and says it doesn't keep, goes. */
export async function syncKeyFiles(): Promise<boolean> {
  if (!writesKeyFiles()) return false;
  let names: string[];
  try { names = await keyNames(); } catch { return false; }
  const files = listOf(BOX_KEYS).filter(name => STORED_KEY.test(name) && !BOT_KEY.test(name));
  let all = true;
  for (const name of [...new Set([...names, ...files])]) {
    await keyFileWork(async () => {
      let value: string | null;
      try { value = await readKey(name); } catch { all = false; return; }
      if (value !== null) {
        if (textOf(keyFile(name)) !== value) writeKeyFile(name, value);
      } else if (names.includes(name)) {
        // Listed, but its value can't be had now (the Keychain locked, its prompt turned down,
        // which the host answers as none): its file stays, and it's tried again soon.
        all = false;
      } else {
        removeKeyFile(name);
      }
    });
  }
  return all;
}

let keeping: ReturnType<typeof setTimeout> | null = null;
/** How long until the next look at the keys' files after `failed` looks in a row that couldn't be
 * finished: a minute after the first, twice as long after each one more (a Keychain prompt turned
 * down isn't asked again every minute), twice a day at most, and twice a day once one is. */
export const keyFilesWait = (failed: number, every = 12 * 3_600_000, retry = 60_000): number => failed ? Math.min(every, retry * 2 ** (failed - 1)) : every;

/** The keys' files kept as its host keeps them, while this box runs (`serve`): now, again soon while
 * its host can't list them or give one it lists (`keyFilesWait`), and twice a day after, for a key
 * changed in the Keychain without the box (an older client's Settings), as this Mac's app kept its
 * own files. */
export function keepKeyFiles(every = 12 * 3_600_000, retry = 60_000): void {
  if (!writesKeyFiles() || keeping) return;
  let failed = 0;
  const round = async () => {
    failed = await syncKeyFiles().catch(() => false) ? 0 : failed + 1;
    keeping = setTimeout(() => void round(), keyFilesWait(failed, every, retry));
    keeping.unref?.();
  };
  keeping = setTimeout(() => void round(), 0);
  keeping.unref?.();
}
