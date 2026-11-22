// git as the box runs it in your folders: never asking anything, taking no locks it needn't,
// starting nothing the repository's own settings name (`../shared/git`), and what takes a
// while without holding up the bridge.

import { execFile, spawn } from "node:child_process";
import { askedArgsLater, ownArgsLater } from "../shared/git";

const GIT_ENV = { ...process.env, GIT_OPTIONAL_LOCKS: "0", GIT_TERMINAL_PROMPT: "0" };

/** Who the box's own commits are by, where they aren't anyone's (a worktree's leftovers, a new
 * project's first commit with no one set to commit). */
export const BY_KUVO = { GIT_AUTHOR_NAME: "Kuvo Code", GIT_AUTHOR_EMAIL: "kuvo-code@localhost", GIT_COMMITTER_NAME: "Kuvo Code", GIT_COMMITTER_EMAIL: "kuvo-code@localhost" };

function ranIn(folder: string, args: string[] | undefined, env: Record<string, string>): Promise<string | null> {
  if (!args) return Promise.resolve(null);
  return new Promise(resolve => {
    const child = execFile("git", args, { cwd: folder, encoding: "utf8", env: { ...GIT_ENV, ...env }, timeout: 60_000, maxBuffer: 64 * 1024 * 1024 },
      (error, stdout) => resolve(error ? null : stdout.trim()));
    child.stdin?.on("error", () => {});
    child.stdin?.end();
  });
}

/** A repository with no commits yet (a new one, an empty one cloned) given an empty first one, so
 * its sessions can each start in a worktree at once; what's staged stays staged. By whoever commits
 * there (your git settings, or the environment's), else by Kuvo Code, as git won't guess a name.
 * Only one with nothing in it at all: a branch you started with nothing on it (`checkout
 * --orphan`) in one with history is yours to begin. Whether it has a commit now. */
export async function firstCommit(folder: string): Promise<boolean> {
  const has = async () => await gitLater(folder, ["rev-parse", "--verify", "--quiet", "HEAD"]) !== null;
  if (await has()) return true;
  if (await gitLater(folder, ["for-each-ref", "--count=1", "--format=x"]) !== "") return false;
  const empty = await gitLater(folder, ["mktree"]);
  if (!empty) return false;
  const made = (env: Record<string, string>) => gitLater(folder, ["-c", "user.useConfigOnly=true", "commit-tree", empty, "-m", "Initial commit"], env);
  const commit = await made({}) ?? await made(BY_KUVO);
  // (Another may have made one meanwhile: then it has one.)
  return (commit !== null && await gitLater(folder, ["update-ref", "-m", "commit (initial): Initial commit", "HEAD", commit, ""]) !== null) || await has();
}

/** Git in `folder` doing what you asked for, with the repository's hooks and filters. */
export async function gitAsked(folder: string, args: string[], env: Record<string, string> = {}): Promise<string | null> {
  return ranIn(folder, await askedArgsLater(args, folder), env);
}

/** Git in `folder` by the box itself. Its trimmed output, or null if it failed. */
export async function gitLater(folder: string, args: string[], env: Record<string, string> = {}): Promise<string | null> {
  return ranIn(folder, await ownArgsLater(args, folder), env);
}

/** The names a git command lists in `folder`, each ended by a NUL (`-z`), each once, as many as
 * `limit`: read as git writes them, and git stopped once there are enough, so a repository of a
 * million files is never held whole (`complete` false then). null when git failed (outside a
 * repository) or `signal` said no one waits any longer, which stops it. Stopped after `timeout`
 * (ten minutes: stuck on a lock or a hung disk), with what it gave by then, `complete` false. */
export async function gitNamesLater(folder: string, args: string[], limit: number, signal?: AbortSignal, timeout = 600_000): Promise<{ names: string[]; complete: boolean } | null> {
  const own = await ownArgsLater(args, folder);
  if (!own || signal?.aborted) return null;
  return new Promise(resolve => {
    const names = new Set<string>();
    let rest = "";
    let enough = false;
    const child = spawn("git", own, { cwd: folder, env: GIT_ENV, stdio: ["ignore", "pipe", "ignore"], timeout, ...(signal ? { signal } : {}) });
    child.stdout.setEncoding("utf8");
    child.stdout.on("data", (piece: string) => {
      if (enough) return;
      const parts = (rest + piece).split("\0");
      rest = parts.pop() ?? "";
      for (const name of parts) {
        if (name) names.add(name);
        if (names.size >= limit) {
          enough = true;
          child.kill();
          return;
        }
      }
    });
    child.on("error", () => resolve(null));
    // Stopped (`code` null) by its time running out: what it gave, never a walk that lists what git
    // ignores (so too one killed otherwise, a crash or the system short of memory); by `signal`, nothing.
    child.on("close", code => resolve(signal?.aborted ? null : enough || code === 0 || code === null ? { names: [...names], complete: code === 0 && !enough } : null));
  });
}

/** The room `folder` takes, in bytes; undefined when `du` can't say. */
export function sizeLater(folder: string): Promise<number | undefined> {
  return new Promise(resolve => {
    execFile("du", ["-sk", folder], { encoding: "utf8", timeout: 60_000 }, (error, stdout) => {
      const kilobytes = Number(String(stdout ?? "").split(/\s/)[0]);
      resolve(!error && Number.isFinite(kilobytes) ? kilobytes * 1024 : undefined);
    });
  });
}
