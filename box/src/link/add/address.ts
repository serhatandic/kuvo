// Where a computer to add is (PROTOCOL.md, "Computers", Adding one): what you type, as `ssh` takes
// it (a Host from this computer's `~/.ssh/config`, a host's name, or `user@host`), and the hosts to
// suggest, from that file. Nothing here reaches the network.
// Platform: the same on macOS and Linux (OpenSSH's own file, in your home on both).

import fs from "node:fs";
import path from "node:path";
import { homeFolder } from "../home";

/** The longest address taken (a host's name is 253 characters at most, and a user's a few more). */
const LONGEST = 320;

/** The most hosts suggested. */
export const MOST_HOSTS = 50;

/** What a Mac's own box is named among its machines by Kuvo Code before 1.0: never an address. */
const THIS_COMPUTER = "this-mac.localhost";

/** The address as it's kept and given to `ssh`: what was typed, without the space around it. */
export const cleanAddress = (given: unknown): string => (typeof given === "string" ? given.replace(/^[ \t]+|[ \t]+$/g, "") : "");

/** Why `address` can't name a computer, in words a client shows as they are; null when it can.
 * A port, a path or an IPv6 address goes in `~/.ssh/config` as a Host, and its name here: `:` and
 * `/` never reach `ssh`, nor anything it would read as an option. */
export function addressProblem(address: string): string | null {
  if (!address) return "Give a host name, or user@host.";
  if (address.length > LONGEST) return "That's too long for a host's name.";
  // (Only what a host, a user or a Host's name is written with: nothing a shell or ssh reads otherwise.)
  if (address.startsWith("-") || /[\s/:]/.test(address) || !/^[A-Za-z0-9._@%+=,~-]+$/.test(address) || (address.match(/@/g) ?? []).length > 1 || address.startsWith("@") || address.endsWith("@")) {
    return "Give a host name, or user@host. For a port or an IPv6 address, add a Host for the machine to ~/.ssh/config and give its name.";
  }
  if (address.toLowerCase() === THIS_COMPUTER) return "That’s this computer.";
  return null;
}

/** The `Host`s of an OpenSSH config file and those it includes, in their order, each once: names
 * only, never a pattern (`*`, `?`, `!`). `Include` is read as ssh reads it (from `~/.ssh` unless
 * it's absolute or starts with `~/`, with `*` and `?` in its last part), four files deep at most. */
export function configHosts(file = path.join(homeFolder(), ".ssh", "config"), home = homeFolder()): string[] {
  const hosts: string[] = [];
  const seen = new Set<string>();
  const read = (at: string, depth: number): void => {
    if (depth >= 4 || hosts.length >= 500) return;
    let text: string;
    try {
      if (fs.statSync(at).size > 1_000_000) return;
      text = fs.readFileSync(at, "utf8");
    } catch {
      return;
    }
    for (const line of text.split("\n")) {
      const words = line.trim().split(/[ \t=]+/).filter(Boolean);
      const keyword = words[0]?.toLowerCase();
      if (!keyword || keyword.startsWith("#")) continue;
      const rest: string[] = [];
      for (const word of words.slice(1)) {
        if (word.startsWith("#")) break;
        rest.push(word.replace(/^"(.*)"$/, "$1"));
      }
      if (keyword === "host") {
        for (const host of rest) {
          if (/[*?!]/.test(host) || seen.has(host) || addressProblem(host)) continue;
          seen.add(host);
          hosts.push(host);
        }
      } else if (keyword === "include") {
        for (const named of rest) for (const included of includedFiles(named, home)) read(included, depth + 1);
      }
    }
  };
  read(file, 0);
  return hosts;
}

// The files an `Include` names: one, or those its last part's pattern matches, by name.
function includedFiles(named: string, home: string): string[] {
  const whole = named.startsWith("~/") ? path.join(home, named.slice(2)) : path.isAbsolute(named) ? named : path.join(home, ".ssh", named);
  const folder = path.dirname(whole), last = path.basename(whole);
  if (!/[*?]/.test(last)) return [whole];
  // (A pattern in a folder's name isn't followed: ssh's own files rarely have one, and it's only a suggestion.)
  if (/[*?]/.test(folder)) return [];
  const pattern = new RegExp(`^${last.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".")}$`);
  try {
    return fs.readdirSync(folder).filter(name => pattern.test(name)).sort().slice(0, 100).map(name => path.join(folder, name));
  } catch {
    return [];
  }
}

/** The hosts to suggest for Add a Machine: this computer's `~/.ssh/config` Hosts that aren't an
 * address of a computer already added (`taken`), in the file's order. */
export function suggestedHosts(taken: Iterable<string>, hosts: string[] = configHosts()): string[] {
  const has = new Set([...taken].map(each => each.toLowerCase()));
  return hosts.filter(host => !has.has(host.toLowerCase())).slice(0, MOST_HOSTS);
}
