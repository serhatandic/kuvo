// SSH from this box to a computer it sets up (PROTOCOL.md, "Computers", Adding one): the system's
// own `ssh`, with your config, keys and agent, never a password; each script sent whole and run by
// `sh` there, with files and anything secret on its standard input, never in its arguments. A
// host's key is checked strictly, against your known hosts and this box's own (`ssh/known_hosts`
// in its folder); one not known yet is read without signing in, for you to look at. Used only
// while a computer is set up or uninstalled: boxes talk through the relay after.
// SSH connection sockets live under this account’s home.

import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import type net from "node:net";
import path from "node:path";
import { loginEnvironment } from "../../shared/platform";
import { IN_TEST_FOLDER, K, RUN, SSH_AGENTS, homeFolder } from "../home";
import { isSshKey } from "../vault/ssh";
import { type AgentKey, privateKeyOf, serveAgent } from "../vault/ssh-agent";
import { isLocked, readItems, valuesOf } from "../vault/store";

/** This box's own known hosts: the keys you trusted from Add a Machine. Yours are read, never written. */
export const KNOWN_HOSTS = path.join(K, "ssh", "known_hosts");

/** `ssh` as this box runs it: the system's; a test's stand-in in a test's folder alone. */
const program = (): string => {
  const standIn = IN_TEST_FOLDER ? process.env.KUVO_TEST_SSH : undefined;
  if (standIn && path.isAbsolute(standIn)) return standIn;
  return process.platform === "darwin" ? "/usr/bin/ssh" : "ssh";
};

// What every connection has: no questions on a terminal nobody's at, no command or terminal your
// config would add, a host that doesn't answer given up on, and none of your config's forwards.
const BASE = ["-o", "BatchMode=yes", "-o", "RemoteCommand=none", "-o", "RequestTTY=no", "-o", "ConnectTimeout=15",
  "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3", "-o", "ClearAllForwardings=yes", "-o", "ForwardAgent=no", "-o", "ForwardX11=no",
  // (Whatever your config says: a host's other keys are never written to your own known hosts from here.)
  "-o", "UpdateHostKeys=no"];

const quoted = (file: string): string => `"${file.replace(/(["\\])/g, "\\$1")}"`;

// The host's key must be one known: yours, or one you trusted here.
const strict = (): string[] => {
  const yours = path.join(homeFolder(), ".ssh");
  return ["-o", "StrictHostKeyChecking=yes", "-o", `UserKnownHostsFile=${[path.join(yours, "known_hosts"), path.join(yours, "known_hosts2"), KNOWN_HOSTS].map(quoted).join(" ")}`];
};

/** The folder shared connections' sockets (and the vault's agent's) go in, yours alone; null where
 * there's none to trust (someone else made it, or a path too long for a socket). */
function socketFolder(folder = path.join(RUN, "ssh")): string | null {
  try {
    fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "EEXIST") return null;
  }
  try {
    const seen = fs.lstatSync(folder);
    if (!seen.isDirectory() || seen.uid !== process.getuid?.() || (seen.mode & 0o077) !== 0) return null;
  } catch {
    return null;
  }
  return folder;
}

const sharing = (): string[] => {
  const folder = socketFolder();
  return folder && Buffer.byteLength(folder) <= 45 ? ["-o", "ControlMaster=auto", "-o", `ControlPath=${folder}/%C`, "-o", "ControlPersist=60"] : ["-o", "ControlMaster=no", "-o", "ControlPath=none"];
};

/** How a connection signs in besides your own keys: the vault's agent. */
export interface Way {
  agent?: string;
}

const signingIn = (way: Way | undefined): string[] => (way?.agent ? ["-o", `IdentityAgent=${quoted(way.agent)}`, "-o", "IdentitiesOnly=no"] : []);

let loginSocket: Promise<string | undefined> | null = null;
// Your SSH agent, as your login shell has it, where this box wasn't started with one (a box its
// keeper started at boot): asked once.
async function environment(way: Way | undefined): Promise<Record<string, string | undefined>> {
  const env = { ...process.env };
  if (way?.agent) return { ...env, SSH_AUTH_SOCK: way.agent };
  if (!env.SSH_AUTH_SOCK && !IN_TEST_FOLDER) {
    loginSocket ??= loginEnvironment().then(found => found?.SSH_AUTH_SOCK, () => undefined);
    const socket = await loginSocket;
    if (socket) env.SSH_AUTH_SOCK = socket;
  }
  return env;
}

/** What a script came to there. */
export interface Ran {
  status: number | null;
  stdout: string;
  stderr: string;
}

export interface RunOptions {
  /** What its standard input gets: bytes, or a file's. */
  input?: Buffer | { file: string };
  prefix?: Buffer;
  /** Stream binary output to an already opened private file. */
  output?: number;
  /** Recheck permission after waits and before starting a command or sending its buffered input. */
  beforeInput?: () => void;
  /** Told as the input goes, in bytes. */
  sent?: (bytes: number, total: number) => void;
  /** Ends it (its own process, by its pid): only for what's safe to cut short. */
  signal?: AbortSignal;
  /** How long it may take, ms. */
  most?: number;
  way?: Way;
  /** More of `ssh`'s options, for this connection alone (reaching a computer at another of its
   * addresses: `Reached.options`). */
  options?: string[];
}

/** The command `ssh` runs there for `script`: in base64, so any login shell hands it to `sh` whole.
 * (`kuvo_script` in its command line is what a start's lock there knows a start by.) */
export const commandFor = (script: string): string =>
  `exec sh -c 'kuvo_script=$(printf %s ${Buffer.from(script).toString("base64")} | base64 -d) || exit 127; eval "$kuvo_script"'`;

function run(args: string[], options: RunOptions): Promise<Ran> {
  return new Promise<Ran>((resolve, reject) => {
    void environment(options.way).then(env => {
      if (options.signal?.aborted) return resolve({ status: null, stdout: "", stderr: "" });
      options.beforeInput?.();
      const child = spawn(program(), args, { env: env as NodeJS.ProcessEnv, stdio: ["pipe", options.output ?? "pipe", "pipe"] });
      const stdin = child.stdin!;
      const stderr = child.stderr!;
      const out: Buffer[] = [], err: Buffer[] = [];
      let kept = 0;
      child.stdout?.on("data", (chunk: Buffer) => { if ((kept += chunk.length) < 4_000_000) out.push(chunk); });
      stderr.on("data", (chunk: Buffer) => { if ((kept += chunk.length) < 4_000_000) err.push(chunk); });
      const stop = () => { try { child.kill("SIGTERM"); } catch {} };
      const timer = setTimeout(stop, options.most ?? 300_000);
      options.signal?.addEventListener("abort", stop, { once: true });
      const done = (status: number | null) => {
        clearTimeout(timer);
        options.signal?.removeEventListener("abort", stop);
        resolve({ status, stdout: Buffer.concat(out).toString("utf8"), stderr: Buffer.concat(err).toString("utf8") });
      };
      child.on("error", error => { err.push(Buffer.from(String(error.message))); done(127); });
      child.on("close", status => done(status));
      stdin.on("error", () => {});
      if (options.prefix) stdin.write(options.prefix);
      const input = options.input;
      if (!input) return void stdin.end();
      if (Buffer.isBuffer(input)) {
        // In pieces, so how far it's got can be said.
        let at = 0;
        const more = () => {
          while (at < input.length) {
            try { options.beforeInput?.(); } catch (error) { stop(); reject(error); return; }
            const piece = input.subarray(at, at + 262_144);
            at += piece.length;
            const room = stdin.write(piece);
            options.sent?.(at, input.length);
            if (!room) return void stdin.once("drain", more);
          }
          stdin.end();
        };
        return more();
      }
      let total = 0;
      try { total = fs.statSync(input.file).size; } catch {}
      let at = 0;
      const stream = fs.createReadStream(input.file);
      stream.on("data", chunk => { at += chunk.length; options.sent?.(at, total); });
      stream.on("error", stop);
      stream.pipe(stdin);
    }).catch(reject);
  });
}

/** Runs `script` on `destination` with `sh`, over a connection shared with this try's other
 * scripts. Its status is the script's; 255 is ssh's own (it couldn't connect, or sign in). */
export const remote = (destination: string, script: string, options: RunOptions = {}): Promise<Ran> =>
  remoteCommand(destination, commandFor(script), options);

export const remoteCommand = (destination: string, command: string, options: RunOptions = {}): Promise<Ran> =>
  run([...BASE, ...strict(), ...sharing(), ...signingIn(options.way), ...(options.options ?? []), "--", destination, command], options);

/** Lets go of the connection shared with `destination` (as a try ends): nothing of it stays. */
export async function hangUp(destination: string, way?: Way, options: string[] = []): Promise<void> {
  const shared = sharing();
  if (shared.includes("ControlPath=none")) return;
  await run([...BASE, ...shared, ...signingIn(way), ...options, "-O", "exit", "--", destination], { most: 10_000, way });
}

/** How `ssh` reaches `destination` as your config has it (`ssh -G`): the host it connects to, as
 * signing in, on which port, with which of your keys. Where that can't be read, what the
 * destination itself says (`user@host`). */
export async function sshConfigOf(destination: string): Promise<{ host: string; user: string | null; port: string | null; keys: string[]; alias: string | null }> {
  const at = destination.lastIndexOf("@");
  const plain = { host: at >= 0 ? destination.slice(at + 1) : destination, user: at >= 0 ? destination.slice(0, at) : null, port: null, keys: [] as string[], alias: null };
  const ran = await run(["-G", "--", destination], { most: 10_000 }).catch(() => null);
  if (!ran || ran.status !== 0) return plain;
  const said = (name: string) => ran.stdout.split("\n").filter(line => line.startsWith(`${name} `)).map(line => line.slice(name.length + 1).trim()).filter(Boolean);
  const home = homeFolder();
  const keys = said("identityfile").map(file => (file.startsWith("~/") ? path.join(home, file.slice(2)) : file)).filter(file => path.isAbsolute(file) && fs.existsSync(file));
  return { host: said("hostname")[0] ?? plain.host, user: said("user")[0] ?? plain.user, port: said("port")[0] ?? null, keys, alias: said("hostkeyalias")[0] ?? null };
}

/** The options that reach a computer at `other`, one of its addresses, as `destination` is reached:
 * as the same user, on the same port, with the same keys, and its host key checked against the one
 * known for `destination`'s host (`HostKeyAlias`), so no key is taken on trust for an address of its own. */
export async function reachedAs(destination: string): Promise<string[]> {
  const config = await sshConfigOf(destination);
  // (Its key is kept under the name ssh kept it by: your config's own alias, else its host, with
  // its port where that isn't 22, as known_hosts writes it; an alias is looked up as it is.)
  const alias = config.alias ?? (config.port && config.port !== "22" ? `[${config.host}]:${config.port}` : config.host);
  return ["-o", `HostKeyAlias=${alias}`, ...(config.port ? ["-p", config.port] : []), ...(config.user ? ["-l", config.user] : []), ...config.keys.flatMap(key => ["-i", key])];
}

/** A host's key as it was read: its line for a known hosts file, its kind and its fingerprint as
 * `ssh` shows it (`SHA256:…`). */
export interface HostKey {
  line: string;
  kind: string;
  fingerprint: string;
}

/** The fingerprint of a public key's blob (base64), as OpenSSH writes it. */
export const fingerprintOf = (blob: string): string => `SHA256:${crypto.createHash("sha256").update(Buffer.from(blob, "base64")).digest("base64").replace(/=+$/, "")}`;

/** The keys in a known hosts file's text, each line as it is. */
export function keysIn(text: string): HostKey[] {
  const keys: HostKey[] = [];
  for (const line of text.split("\n")) {
    const words = line.trim().split(/\s+/);
    // (`host kind blob`; a marker first, `@cert-authority` or `@revoked`, is never one to trust here.)
    if (words.length < 3 || words[0]!.startsWith("#") || words[0]!.startsWith("@")) continue;
    if (!/^(ssh-|ecdsa-|sk-)[A-Za-z0-9@.-]+$/.test(words[1]!) || !/^[A-Za-z0-9+/]+=*$/.test(words[2]!)) continue;
    keys.push({ line: words.slice(0, 3).join(" "), kind: words[1]!, fingerprint: fingerprintOf(words[2]!) });
  }
  return keys;
}

/** Reads `destination`'s host key as `ssh` reaches it (through your config: its port, its jump
 * host), without signing in: nothing is offered to a host not trusted yet, no key and no agent.
 * The keys it showed, or why it couldn't be reached (ssh's own words). */
export async function readHostKey(destination: string, signal?: AbortSignal): Promise<{ keys: HostKey[]; said: string }> {
  const folder = path.join(RUN, "ssh-scan");
  fs.mkdirSync(folder, { recursive: true, mode: 0o700 });
  const file = path.join(folder, crypto.randomBytes(9).toString("base64url"));
  try {
    const ran = await run([
      "-o", "BatchMode=yes", "-o", "RemoteCommand=none", "-o", "RequestTTY=no", "-o", "ConnectTimeout=15", "-o", "ClearAllForwardings=yes", "-o", "ForwardAgent=no", "-o", "ForwardX11=no",
      "-o", "ControlMaster=no", "-o", "ControlPath=none",
      "-o", "StrictHostKeyChecking=accept-new", "-o", `UserKnownHostsFile=${quoted(file)}`, "-o", "GlobalKnownHostsFile=/dev/null", "-o", "UpdateHostKeys=no",
      // (Its name as it is, and no line for its address beside it: what's kept is what's read here.)
      "-o", "HashKnownHosts=no", "-o", "CheckHostIP=no",
      "-o", "PubkeyAuthentication=no", "-o", "PasswordAuthentication=no", "-o", "KbdInteractiveAuthentication=no", "-o", "GSSAPIAuthentication=no", "-o", "HostbasedAuthentication=no",
      "-o", "IdentityAgent=none",
      "--", destination, "true",
    ], { signal, most: 60_000 });
    let text = "";
    try { text = fs.readFileSync(file, "utf8"); } catch {}
    return { keys: keysIn(text).slice(0, 8), said: ran.stderr };
  } finally {
    fs.rmSync(file, { force: true });
    try { fs.rmdirSync(folder); } catch {}
  }
}

export function savedHostLine(line: number): string | undefined {
  try { return fs.readFileSync(KNOWN_HOSTS, "utf8").split("\n")[line - 1]; } catch { return undefined; }
}

/** Keeps `keys` as trusted, in this box's own file (yours alone to read). `replacing`: the line
 * there (counted from 1, as `ssh` names it) that holds the key the host had before, which you saw
 * change and took the new one for: that line goes, whichever way its host is written (a name, or
 * hashed, as `HashKnownHosts` writes them). */
export function trustHostKeys(keys: HostKey[], replacing?: { line: number; previous: string }): boolean {
  fs.mkdirSync(path.dirname(KNOWN_HOSTS), { recursive: true, mode: 0o700 });
  let had = "";
  try { had = fs.readFileSync(KNOWN_HOSTS, "utf8"); } catch {}
  const lines = had.split("\n");
  if (replacing && lines[replacing.line - 1] !== replacing.previous) return false;
  // (With it, every other key kept under that host's name: only the one you just saw stays.)
  const hosts = new Set(replacing ? keys.map(key => key.line.split(" ")[0]!) : []);
  const kept = lines.filter((line, index) => line.trim() && index + 1 !== replacing?.line && !hosts.has(line.trim().split(/\s+/)[0]!) && !keys.some(key => key.line === line.trim()));
  const text = [...kept, ...keys.map(key => key.line)].join("\n") + "\n";
  const next = `${KNOWN_HOSTS}.${process.pid}.new`;
  fs.writeFileSync(next, text, { mode: 0o600 });
  fs.renameSync(next, KNOWN_HOSTS);
  return true;
}

/** How a connection that didn't come about went wrong, from what `ssh` said (status 255). */
export type Trouble =
  | { kind: "unknownHost" }
  /** `own`: the key it had before is one this box kept (not one in your own known hosts), on that
   * line of its file. */
  | { kind: "changedHost"; own: boolean; line: number; host: string | null }
  | { kind: "refused" }
  | { kind: "unreachable"; said: string };

/** The last thing `ssh` said, as one plain line for showing: never more of what a host sent than that. */
export const lastLine = (stderr: string): string => {
  const lines = stderr.split("\n").map(line => line.replace(/[\u0000-\u001f\u007f\u200e\u200f\u202a-\u202e\u2066-\u2069]/g, " ").trim()).filter(line => line && !/^Warning: Permanently added/.test(line));
  const last = lines[lines.length - 1] ?? "";
  return last.length > 300 ? `${last.slice(0, 299)}…` : last;
};

export function troubleOf(ran: Ran): Trouble | null {
  if (ran.status !== 255) return null;
  const said = ran.stderr;
  if (said.includes("REMOTE HOST IDENTIFICATION HAS CHANGED")) {
    // (Only a whole line of ssh's own names the host: nothing a host could have written there.)
    const host = /^Host key for ([A-Za-z0-9._:\-[\]]+) has changed and you have requested strict checking\.\s*$/m.exec(said)?.[1] ?? null;
    const offending = /^Offending (?:\S+ )?key in (.+):(\d+)\s*$/m.exec(said);
    return { kind: "changedHost", own: Boolean(offending) && path.resolve(offending![1]!) === path.resolve(KNOWN_HOSTS), line: Number(offending?.[2] ?? 0), host };
  }
  if (said.includes("Host key verification failed")) return { kind: "unknownHost" };
  if (/permission denied|too many authentication failures/i.test(said)) return { kind: "refused" };
  return { kind: "unreachable", said: lastLine(said) };
}

/** An agent of this box's own that offers the SSH keys you keep in the vault, for a connection of
 * your own asking (adding a computer): its socket, and what closes it. Null while the vault is
 * locked, has no SSH key, or there's nowhere safe for its socket. */
export async function vaultAgent(): Promise<{ way: Way; close: () => void } | null> {
  fs.mkdirSync(RUN, { recursive: true, mode: 0o700 });
  const parentPath = path.dirname(SSH_AGENTS);
  // Create the uid-scoped directory itself before trusting it, without following a symlink.
  try { fs.mkdirSync(parentPath, { mode: 0o700 }); }
  catch (error) { if ((error as NodeJS.ErrnoException).code !== "EEXIST") return null; }
  const parent = socketFolder(parentPath);
  if (!parent) return null;
  const folder = socketFolder(path.join(parent, `a-${crypto.createHash("sha256").update(path.resolve(K)).digest("hex").slice(0, 12)}`));
  if (!folder) return null;
  let keys: AgentKey[] = [];
  try {
    if (isLocked()) return null;
    // (A few: a server gives up after half a dozen keys tried, yours included.)
    for (const item of readItems().filter(isSshKey).slice(0, 4)) {
      try { keys.push({ id: item.id, name: item.name, ...privateKeyOf((await valuesOf(item.id)).private_key ?? "") }); } catch {}
    }
  } catch {
    keys = [];
  }
  if (!keys.length) return null;
  const socket = path.join(folder, `add-${crypto.randomBytes(6).toString("hex")}.sock`);
  const server: net.Server = serveAgent(socket, { keys: async () => keys, allow: async () => true });
  const opened = await new Promise<boolean>(resolve => {
    server.once("listening", () => resolve(true));
    server.once("error", () => resolve(false));
  });
  if (!opened) return null;
  return { way: { agent: socket }, close: () => { server.close(); fs.rmSync(socket, { force: true }); } };
}
