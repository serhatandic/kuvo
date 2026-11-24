// Adding a computer over SSH, setting one up again, and uninstalling Kuvo Code from one (PROTOCOL.md,
// "Computers", Adding one), done by this box for whichever client asks: the address checked, the
// host's key looked at, what's on the computer judged, this box's own files and engine put there,
// its box started to keep itself, and the computer signed into your setup, as a Mac's app did all
// of it before (#636). Each step is told as it goes (`tries.ts`); every word a client shows is here.
// The setup's primary enrolls members and signs the resulting membership list.

import { addComputersLayoutReady, computerHistoryLayoutReady, credentialSelectionLayoutReady, setupLayoutDeferred } from "../../shared/folder-version";
import { updateAccess, type MembershipCompanions } from "../setup/access";
import { type Receipt } from "../setup/shared";
import { holdsRelayLease, LEADERSHIP_WAIT, primaryFenced } from "../setup/lease";
import crypto from "node:crypto";
import fs from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import type { AddTry, ComputerAction, ListedComputer } from "../../shared/routes/box";
import { isObject, parsed } from "../../shared/json";
import { plainName } from "../../shared/text";
import { setupSaveReceipt } from "../../shared/setup-local";
import { macDocsChanged, receiveDocs } from "../documents/exchange";
import { ownSign, SIGN } from "../documents/format";
import { writeDurablePrivate, writePrivate } from "../files";
import { IS_MAC, K } from "../home";
import { ownName } from "../letters/start";
import { macLinkCheck, tellGone } from "../mac/link";
import { changeKeptHosts, keptHosts, LIST_FILE, localDestination, peerAtDestination, savedMachines } from "../mac/machines";
import { setMacPeers, setupGift } from "../mac/routes";
import { ADDRESS, KEY, MOST_PEERS, ownKey, peersOf, type Peer } from "../peers/known";
import { readPeerAgents } from "../peers/directory";
import { pairedPhone } from "../phones";
import { machineBoxIn } from "../install/verdict";
import { takeWipeReceipt } from "../setup/removal";
import { localRemovalPending, removalsOf, wipeOutstanding } from "../setup/removal-record";
import { oldAppAttached, removeComputerHere } from "../peers/manage";
import { appsOf, foundOf, judgeComputer, oursNow } from "../setup/judge";
import { addElsewhere, mainElsewhere } from "../setup/main-only";
import { liveLink, macLink, readLink, updateLink } from "../state";
import type { LinkState } from "../state";
import type { Asker } from "../server/context";
import { boxVersion, newerVersion } from "../updates/update";
import { addressProblem } from "./address";
import { ENGINE_VERSION, RELEASE_FILES, engineFor, machineHelperPack, ownFiles, tarOf, type Kind } from "./files";
import * as scripts from "./scripts";
import { credentialSeedFormat, isSetupReservation, RESERVATION_PREFIX, settingsSeedFormat, setupInputFormat, UNSUPPORTED_RESERVATION } from "./command-formats";
import { gatherSeed, sshCopyScope, sshCopySelection } from "./seed";
import { type HostKey, type Ran, type Way, hangUp, lastLine, reachedAs, readHostKey, remote, remoteCommand, savedHostLine, troubleOf, trustHostKeys, vaultAgent } from "./ssh";
import { WINDOWS_PROBE, WSL_LIST, scriptHeader, selectDistro, windowsHost, wslBootstrap } from "./windows";
import { distroName } from "../../shared/wsl";
import { Failed, type Running, Stopped, type Telling, startTry, stopTry, underWay } from "./tries";
import { noteKeyNow } from "../setup/lead";
import { acceptedMembership, membershipOf } from "../setup/membership";
import { followedSetup } from "../setup/record";
import { computerHistory, enrollmentPredecessor, fingerprintOf, fingerprintSourceOf, requiresReplacementConfirmation, type FingerprintSource, historyFor, orderComputers, rememberEnrollment } from "../setup/computer-history";
import { SETUP_LOCAL_PAIRINGS, SETUP_OCCUPIED } from "../marks";
import { MergeRefusal } from "../setup/merge-data";

class AccountOccupied extends Failed {
  constructor(name: string) {
    super(`Kuvo Code on ${name} already belongs to another setup. To add this computer, connect over SSH to your own OS account.`, false);
  }
}

/** Every address of the computers this box set up. */
export const takenAddresses = (): string[] => (savedMachines() ?? []).flatMap(each => [each.destination, ...(Array.isArray(each.otherAddresses) ? each.otherAddresses : [])]);

// What this box keeps of its machines' links for your phones (`kuvo/hosts`), as the Mac app kept it.
function keepHost(id: string, host: { name: string; address: string; key: string; sign: string }): void {
  if (!changeKeptHosts(machines => ({ ...machines, [id]: { ...host, anotherMac: false, peers: true } }))) throw new Failed(`${ownName()}'s record of its machines can't be read right now.`);
}

/** Whether this box adds computers now, and why not, in words a client shows as they are. */
export function canAdd(): { can: true } | { can: false; status: 409 | 503; message: string; primary?: string } {
  const link = readLink();
  if (primaryFenced(link)) return { can: false, status: 503, message: LEADERSHIP_WAIT };
  const main = mainElsewhere(link);
  if (main) return { can: false, status: 409, ...addElsewhere(main) };
  if (!IS_MAC && followedSetup(link)?.role !== "primary") return { can: false, status: 409, message: "This computer can add members once its setup is ready." };
  const here = ownName();
  if (oldAppAttached()) return { can: false, status: 409, message: `Quit the old Kuvo Code app on ${here} first: it keeps its own list of machines, and would write over this one.` };
  if (!(IS_MAC ? macLink.config?.relay : link?.relay)) return { can: false, status: 503, message: `${here} isn’t linked to your other computers yet, so it can’t add one.` };
  if (!RELEASE_FILES.every(([name]) => fs.existsSync(`${K}/${name}`))) return { can: false, status: 503, message: `Kuvo Code on ${here} has no files of its own to give another computer yet. Update Kuvo Code here, then try again.` };
  if (!savedMachines()) return { can: false, status: 503, message: `${here}’s list of machines can’t be read right now.` };
  return { can: true };
}

/** What the probe said of a computer. */
interface Probed {
  kind: Kind;
  version: string | null;
  updating: boolean;
  engine: boolean;
  id: string | null;
  fingerprint?: string;
  freshAdd?: string;
  fingerprintSource?: FingerprintSource;
  lines: string[];
}

// Reads the probe's answer, or says why the computer can't run Kuvo Code.
function probed(answer: string): Probed {
  const lines = answer.split("\n").map(line => line.trim());
  const [system, machine] = (lines[0] ?? "").split(/\s+/);
  const kind: Kind | null = (system === "Linux" || system === "Darwin") && (machine === "x86_64" || machine === "aarch64" || machine === "arm64")
    ? { system: system === "Linux" ? "linux" : "darwin", machine: machine === "x86_64" ? "x86_64" : "aarch64" } : null;
  const unsupported = (what: string) => new Failed(`Kuvo Code runs on macOS, and on Linux with glibc, on x86-64 or ARM64; this computer is ${plainName(what).slice(0, 60) || "something else"}.`, false);
  if (!kind) throw unsupported(lines[0] ?? "");
  if (kind.system === "linux" && lines[1] !== "glibc") throw unsupported(`${lines[0]} (musl)`);
  if (kind.system === "darwin") {
    const macos = lines.find(line => line.startsWith("macos "))?.slice(6) ?? "";
    if (Number(macos.split(".")[0]) < 13) throw new Failed(`Kuvo Code runs on macOS 13 or later; that Mac has macOS ${plainName(macos).slice(0, 20) || "an older one"}.`, false);
    if (lines[1] === "no-tools") throw new Failed("That Mac needs a working git first. Run xcode-select --install on it (with Xcode, accept its license), then try again.");
  }
  const said = (name: string) => lines.find(line => line.startsWith(`kuvo ${name} `))?.slice(name.length + 6) ?? "";
  const version = said("version");
  const id = said("id");
  return { kind, version: /^\d+\.\d+\.\d+/.test(version) ? version : null, updating: lines.includes("kuvo updating"), engine: lines.includes("kuvo engine"), id: /^[0-9a-f]{32}$/.test(id) ? id : null, freshAdd: /^v1 [a-f0-9]{64} [a-f0-9]{64} (?:prepared|copying)$/.test(said("fresh-add")) ? said("fresh-add") : undefined, fingerprint: fingerprintOf(said("fingerprint")), fingerprintSource: fingerprintSourceOf(said("fingerprint-source")), lines };
}

/** Whether the computer has a machine's box, by the installer's rule for it (`machineBoxIn`,
 * from the probe's lines): then its installer leaves an earlier Kuvo Code's folder as it is
 * (`machine`, `two`), so that folder is no reason to refuse. (A Mac's own box the probe tells
 * itself: it doesn't say that folder there at all, as before.) */
function machineBoxProbed(probe: Probed): boolean {
  const has = (word: string) => () => probe.lines.includes(`kuvo ${word}`);
  const kind = probe.lines.find(line => line.startsWith("kuvo kind"))?.slice(9).trim();
  return machineBoxIn({
    kind: kind === "mac" || kind === "machine" ? kind : null, linux: probe.kind.system === "linux",
    gone: has("gone"), making: has("making"), running: has("running"), keys: has("keys"),
    found: () => foundOf(probe.lines).some(box => !box.app && (box.card !== undefined || box.link !== undefined)),
  });
}

const megabytes = (bytes: number): string => `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;

/** Why a computer couldn't be reached, in words: what ssh said goes to this box's log alone. */
const unreached = (name: string, said?: string): Failed => {
  if (said) console.error(`Kuvo couldn't reach ${name} over SSH: ${said}`);
  return new Failed(`Couldn’t reach ${name}. Make sure it’s on and connected, then Try Again.`);
};

/** Another address of a computer this box set up: `own`, one you added it under (reached as itself,
 * its host key trusted then); else one it tells its Macs (reached as the first, `reachedAs`). */
export interface OtherAddress {
  address: string;
  own: boolean;
}

/** Where else the computer added under `destination` may be reached, in the order they're tried:
 * the other addresses you added it under, then those it tells its Macs (its tailnet's first). */
export function otherAddressesOf(destination: string): OtherAddress[] {
  const record = (savedMachines() ?? []).find(each => each.destination === destination);
  const peer = peerAtDestination(readLink(), destination);
  const told = peer ? readPeerAgents()[peer.key]?.computer?.addresses ?? [] : [];
  const host = (address: string) => address.slice(address.lastIndexOf("@") + 1).toLowerCase();
  const seen = new Set([host(destination)]);
  const others: OtherAddress[] = [];
  for (const address of Array.isArray(record?.otherAddresses) ? record.otherAddresses : []) {
    if (typeof address !== "string" || addressProblem(address) || seen.has(host(address))) continue;
    seen.add(host(address));
    others.push({ address, own: true });
  }
  for (const address of told) {
    if (seen.has(host(address)) || addressProblem(address)) continue;
    seen.add(host(address));
    others.push({ address, own: false });
  }
  return others.slice(0, 8);
}

/** A connection for one try: every script over it, signed in with your keys, or the vault's once
 * yours were refused; a host key not known asked about, and kept on your word. A computer this box
 * set up has more addresses than the one it was added under (`others`: another it was added at, and
 * those it tells its Macs, as its tailnet's): where that one can't be reached (a home network's
 * address while you're away), each is tried in turn before the try fails, reached as the first is
 * and checked against its host key, never asked about. */
class Reach {
  private way: Way | undefined;
  private closeAgent: (() => void) | null = null;
  /** Where it's reached now, with the options that reach it there as at `destination`. */
  private target: string;
  private options: string[] = [];
  /** It's been reached: no other address is tried from then on. */
  private reached = false;
  windows?: { host: string; distro: string };
  /** What ssh said at the address it was added under, where that didn't answer. */
  private firstSaid: string | null = null;
  private readonly others: OtherAddress[];
  private readonly savedDistro: string | undefined;
  constructor(private readonly destination: string, private readonly name: string, private readonly tell: Telling, others: OtherAddress[] = []) {
    this.target = destination;
    this.others = [...others];
    this.savedDistro = distroName(savedMachines()?.find(each => each.destination === destination)?.wslDistro);
  }

  /** Moves on to the next of its other addresses, where there's one left and none was reached yet. */
  private async next(said: string): Promise<boolean> {
    if (this.reached || !this.others.length) return false;
    this.firstSaid ??= said;
    const other = this.others.shift()!;
    console.error(`Kuvo couldn't reach ${this.name} at ${this.target} over SSH (${said || "no answer"}); trying ${other.address}.`);
    await hangUp(this.target, this.way, this.options).catch(() => {});
    this.target = other.address;
    this.options = other.own ? [] : await reachedAs(this.destination).catch(() => []);
    return true;
  }

  /** Runs `script` there; its answer when it ran (whatever its status), or a `Failed` for a
   * connection that didn't come about. `cut`: ended with the try when it's stopped (only what's
   * safe cut short); `anyway`: run though the try was stopped (its own tidying up).
   * `once`: never resend a command whose completion might be unknown. */
  async run(script: string, options: { input?: Buffer | { file: string }; beforeInput?: () => void; sent?: (bytes: number, total: number) => void; cut?: boolean; most?: number; anyway?: boolean; once?: boolean } = {}): Promise<Ran> {
    for (let tries = 0; ; tries++) {
      if (!options.anyway) this.tell.check();
      const elsewhere = this.target !== this.destination;
      const settings = { input: options.input, beforeInput: options.beforeInput, sent: options.sent, most: options.most, way: this.way, options: this.options, ...(options.cut ? { signal: this.tell.signal } : {}) };
      const hop = (distro: string) => remoteCommand(this.target, wslBootstrap(distro), { ...settings, prefix: scriptHeader(script) });
      let ran = this.windows ? await hop(this.windows.distro) : await remote(this.target, script, settings);
      // The first command is a POSIX probe or account reservation, with no secret input.
      // Only retry it when a Windows command positively identifies the remote shell.
      if (!this.reached && !this.windows && (ran.status === 1 || ran.status === 127 || (ran.status === 255 && /\bexec\b|kuvo_script/.test(ran.stderr)))) {
        const query = { way: this.way, options: this.options, signal: this.tell.signal, most: 30_000 };
        const windows = await remoteCommand(this.target, WINDOWS_PROBE, query);
        const host = windows.status === 0 && windowsHost(windows.stdout);
        if (host) {
          const list = await remoteCommand(this.target, WSL_LIST, query);
          const distro = list.status === 0 && selectDistro(list.stdout, this.savedDistro);
          if (!distro && this.savedDistro) throw new Failed(`Open ${this.savedDistro} on ${host} and check that it runs on WSL 2, then try again.`);
          if (!distro) throw new Failed(`Install Ubuntu with WSL 2 on ${host}, then open Ubuntu once and try again.`);
          this.windows = { host, distro };
          ran = await hop(distro);
        }
      }
      const trouble = troubleOf(ran);
      if (!trouble) {
        this.reached = true;
        return ran;
      }
      if (options.anyway || options.once) throw unreached(this.name, lastLine(ran.stderr));
      this.tell.check();
      // (Not there, or not the key known for it at another address: the next address, as a try of its own.)
      if ((trouble.kind === "unreachable" || (elsewhere && trouble.kind !== "refused")) && await this.next(lastLine(ran.stderr))) {
        tries--;
        continue;
      }
      // (None of its other addresses answered: it couldn't be reached, as at the first. What one of
      // them said, someone else's computer at a home network's address say, isn't what's true of it.)
      if (elsewhere && trouble.kind !== "refused") throw unreached(this.name, `${this.firstSaid ?? ""} (and at its other addresses: ${lastLine(ran.stderr)})`);
      if (tries >= 3) throw unreached(this.name, lastLine(ran.stderr));
      if (trouble.kind === "unknownHost") {
        await this.askKey("hostKey", `Is this ${this.name}?`,
          `${ownName()} hasn’t connected to ${this.name} before. Check that the fingerprint is its own (on that computer: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub), then trust it.`, "Trust");
      } else if (trouble.kind === "changedHost") {
        if (!trouble.own) {
          throw new Failed(`${this.name}’s host key has changed since ${ownName()} last connected. If you expect that (it was reinstalled, say), remove the old key in a terminal on ${ownName()} with ssh-keygen -R ${trouble.host ? `'${trouble.host}'` : "and its address"}, then try again. If not, someone may be intercepting the connection.`, true, `${this.name}’s host key changed`);
        }
        await this.askKey("hostKeyChanged", `${this.name}’s host key changed`,
          `It isn’t the key you trusted when you added it. If you expect that (it was reinstalled, say), trust the new one. If not, someone may be intercepting the connection: cancel.`, "Trust New Key", trouble.line);
      } else if (trouble.kind === "refused") {
        const vault = this.way ? null : await vaultAgent();
        if (!vault) {
          throw new Failed(`${this.name} didn’t accept ${ownName()}’s SSH keys. Kuvo Code connects with the keys and SSH agent of ${ownName()}, and the SSH keys in its vault, without a password. If your key is in an app like 1Password or Secretive, or on a security key, unlock it, then try again.`, true, `${this.name} refused the connection`);
        }
        this.way = vault.way;
        this.closeAgent = vault.close;
      } else {
        throw unreached(this.name, trouble.kind === "unreachable" ? trouble.said : lastLine(ran.stderr));
      }
    }
  }

  // Reads the host's key without signing in, shows it, and keeps it once you say so.
  private async askKey(id: string, title: string, message: string, label: string, replacing?: number): Promise<void> {
    const previous = replacing === undefined ? undefined : savedHostLine(replacing);
    if (replacing !== undefined && previous === undefined) throw new Failed(`Kuvo couldn’t save the SSH host key for ${this.name}.`);
    const read = await readHostKey(this.destination, this.tell.signal);
    this.tell.check();
    if (!read.keys.length) {
      const said = lastLine(read.said);
      throw said && !/permission denied/i.test(said) ? unreached(this.name, said) : new Failed(`Couldn’t read ${this.name}’s host key.`);
    }
    // (The one ssh checks first, as it orders them: one to compare, not three.)
    const shown: HostKey = read.keys.find(key => key.kind === "ssh-ed25519") ?? read.keys[0]!;
    const answer = await this.tell.ask({
      id, title, message,
      facts: [{ label: "Address", value: this.destination }, { label: `Fingerprint (${shown.kind.replace(/^ssh-/, "").toUpperCase()})`, value: shown.fingerprint, code: true }],
      answers: [{ id: "cancel", label: "Cancel", kind: "plain" }, { id: "trust", label, kind: replacing ? "danger" : "primary" }],
    });
    // (Anything but Trust stops the try: nothing is kept.)
    if (answer !== "trust") {
      stopFrom(this.tell);
      throw new Stopped();
    }
    this.tell.check();
    if (!trustHostKeys(read.keys, replacing === undefined ? undefined : { line: replacing, previous: previous! })) {
      throw new Failed(`Kuvo couldn’t save the SSH host key for ${this.name}.`);
    }
  }

  async close(): Promise<void> {
    await hangUp(this.target, this.way, this.options).catch(() => {});
    this.closeAgent?.();
  }
}

// A try you answered Cancel to stops as one you stopped.
const stoppers = new WeakMap<Telling, () => void>();
function stopFrom(tell: Telling): void {
  stoppers.get(tell)?.();
}

const SETUP_STEPS = [["checking", "Checking it"], ["files", "Copying Kuvo Code"], ["engine", "Copying the engine"], ["starting", "Starting Kuvo Code there"], ["seeding", "Copying your keys and sign-ins"], ["joining", "Adding it to your computers"]] as const;
const AGAIN_STEPS = [["checking", "Checking it"], ["files", "Copying Kuvo Code"], ["engine", "Copying the engine"], ["starting", "Restarting Kuvo Code there"], ["seeding", "Copying your keys and sign-ins"], ["joining", "Adding it to your computers"]] as const;
const UPDATE_STEPS = [["checking", "Checking it"], ["files", "Copying the update"], ["engine", "Copying the engine"], ["starting", "Restarting Kuvo there"], ["joining", "Reconnecting it"]] as const;
const UNINSTALL_STEPS = [["checking", "Reaching it"], ["removing", "Stopping and removing Kuvo Code"], ["forgetting", "Taking it off your computers"]] as const;

/** The key this box is known by on the computers it starts (its engine launches' notes there):
 * from its link's key, never the key itself. */
const launcherID = (key: string): string => crypto.createHash("sha256").update(`kuvo-launcher\n${key}`).digest("hex").slice(0, 32);

// Attempts from this box are still active between remote commands, even through a different address.
const activeReservations = new Set<string>();

class AccountReservation {
  readonly token = RESERVATION_PREFIX + crypto.randomBytes(24).toString("hex");
  private finished = true;
  clearFresh = false;
  get canRollBack(): boolean { return this.finished; }

  constructor(private reach: Reach, private name: string, private tell: Telling) {}

  private busy(): Failed {
    return new Failed(`Another Kuvo Code change is in progress on ${this.name}. Try again once it finishes.`);
  }

  private cannotPrepare(ran: Ran): Failed {
    console.error(`Kuvo couldn't prepare the SSH account on ${this.name} (status ${ran.status}): ${lastLine(ran.stderr)}`);
    return new Failed(`Kuvo Code couldn’t prepare the OS account on ${this.name}. Check that you can write to your home folder there, then try again.`);
  }

  private async acquire(): Promise<void> {
    const claim = await this.reach.run(scripts.reserve(this.token), { most: 30_000 });
    if (claim.status === 0) return;
    if (claim.status === scripts.ACCOUNT_BUSY) throw this.busy();
    if (claim.status === scripts.ACCOUNT_OLD) {
      throw new Failed(`An older version of Kuvo Code left an unfinished change on ${this.name}. Restart ${this.name} to end that change, then sign in to the same OS account and run rm -f ~/.kuvo-code-adding before trying again.`);
    }
    const prior = claim.stdout.trim();
    if (claim.status !== scripts.ACCOUNT_RECOVER) throw this.cannotPrepare(claim);
    if (!isSetupReservation(prior)) throw new Failed(UNSUPPORTED_RESERVATION);
    if (activeReservations.has(prior)) throw this.busy();
    const answer = await this.tell.ask({
      id: "recoverAccount", title: `Continue adding ${this.name}?`,
      message: `An earlier attempt to add ${this.name} didn't finish, and it's no longer running. Kuvo can clear it and start again.`,
      facts: [],
      answers: [{ id: "cancel", label: "Cancel", kind: "plain" }, { id: "recover", label: "Clear and Continue", kind: "primary" }],
    });
    if (answer !== "recover") {
      stopFrom(this.tell);
      throw new Stopped();
    }
    const retried = await this.reach.run(scripts.reserve(this.token, prior), { most: 30_000 });
    if (retried.status === scripts.ACCOUNT_BUSY) throw this.busy();
    if (retried.status === scripts.ACCOUNT_LOST) throw new Failed(`The account on ${this.name} changed while you were deciding. Try again.`);
    if (retried.status !== 0) throw this.cannotPrepare(retried);
  }

  private interrupted(): Failed {
    return new Failed(`Kuvo Code lost contact with ${this.name} during a change. Try again to check whether it finished.`);
  }

  readonly run: Reach["run"] = async (script, options) => {
    if (!options?.anyway) this.tell.check();
    if (!this.finished) throw this.interrupted();
    this.finished = false;
    let ran: Ran;
    let authorized = false;
    let denied = false;
    const check = options?.beforeInput;
    const beforeInput = check && (() => {
      try {
        check();
        authorized = true;
      } catch (error) {
        denied = true;
        throw error;
      }
    });
    try {
      ran = await this.reach.run(scripts.reserved(this.token, script), { ...options, beforeInput, once: true });
    } catch (error) {
      if (denied) {
        // A rejection before the first check passed means SSH never started.
        this.finished = !authorized;
        throw error;
      }
      throw this.interrupted();
    }
    if (ran.status === null) throw this.interrupted();
    this.finished = true;
    if (ran.status === scripts.ACCOUNT_BUSY) throw this.busy();
    if (ran.status === scripts.ACCOUNT_LOST) throw new Failed(`The account on ${this.name} was cleared for another attempt. Try again.`);
    if (ran.status === scripts.ACCOUNT_ERROR) throw this.cannotPrepare(ran);
    return ran;
  };

  async use(work: () => Promise<void>): Promise<void> {
    let acquired = false;
    let failure: unknown;
    activeReservations.add(this.token);
    try {
      await this.acquire();
      acquired = true;
      await work();
    } catch (error) {
      failure = error;
      throw error;
    } finally {
      try {
        if (acquired && this.finished) await this.release(failure);
      } finally {
        activeReservations.delete(this.token);
        await this.reach.close();
      }
    }
  }

  private async release(failure: unknown): Promise<void> {
    const released = await this.reach.run(scripts.release(this.token, this.clearFresh), { anyway: true, most: 15_000 }).catch(() => null);
    if (released?.status === 0) return;
    console.error(`Kuvo couldn't clear the SSH reservation on ${this.name} (status ${released?.status}): ${lastLine(released?.stderr ?? "")}`);
    const message = failure instanceof Error && !(failure instanceof Stopped) ? `${failure.message} ` : "";
    const clearing = `couldn’t clear the account on ${this.name} for the next change. The next time you connect, Kuvo Code can check and offer to clear it.`;
    throw new Failed(failure ? `${message}Kuvo Code ${clearing}` : `Kuvo Code finished the change but ${clearing}`);
  }
}

interface AddGrant {
  caller?: { key: string; pairing: string };
  replace?: string;
  discardLocalPairings?: boolean;
  credentials?: string[];
  revisions: Record<string, number>;
}

const ADD_REVOKED = "The device that started this add is no longer paired. Start a new add from one of your devices.";
const removedDuringAdd = (name: string) => new Failed(`${name} was removed from your computers. Add it again to restore access.`);

function checkAdd(link: LinkState | null, grant: AddGrant | undefined, name: string, key?: string): void {
  const removal = key ? (removalsOf(link)[key] ?? link?.memberRemovals?.[key]) : undefined;
  if (wipeOutstanding(removal)) throw new Failed(`Wait for the pending wipe on ${name} before adding it again.`);
  if (localRemovalPending(link)) throw new Failed("Finish or cancel this computer’s removal before adding a computer.");
  if (setupLayoutDeferred(K)) throw new Failed("Wait for the Kuvo Code update to finish before adding a computer.");
  if (primaryFenced(link)) throw new Failed(LEADERSHIP_WAIT, false);
  const main = mainElsewhere(link);
  if (main) throw new Failed(addElsewhere(main).message, false);
  if (!IS_MAC && followedSetup(link)?.role !== "primary") throw new Failed("This computer no longer leads a setup.", false);
  if (!grant) return;
  if (grant.caller && !pairedPhone(link, grant.caller.key, grant.caller.pairing)) throw new Failed(ADD_REVOKED);
  if (key && (grant.revisions[key] ?? 0) !== (link?.removalRevisions?.[key] ?? 0)) throw removedDuringAdd(name);
}

function revokedKeys(link: LinkState | null): Set<string> {
  return new Set([...(link?.left ?? []), ...(link?.removedHere ?? []), ...(acceptedMembership(link)?.left ?? [])]);
}

function checkPredecessor(link: LinkState | null, key: string | undefined, name: string): void {
  if (!key) return;
  const prior = computerHistory(link)?.slots.find(slot => slot.key === key)?.previous ?? [];
  const removals = link?.memberRemovals ?? removalsOf(link);
  if (prior.some(each => {
    const removal = removals[each];
    return wipeOutstanding(removal);
  })) throw new Failed(`Wait for the pending wipe on ${name} before adding it again.`);
  if (key === link?.setup?.primary) throw new Failed("The main computer cannot be replaced through Add.");
  if (link?.wipePreparations?.[key]) {
    throw new Failed("Finish this computer's removal before adding it again.");
  }
  if (!peersOf(link).some(peer => peer.key === key) && !revokedKeys(link).has(key)) throw removedDuringAdd(name);
}

function checkFreshKeys(link: LinkState | null, peer: { key: string; sign: string }, name: string): void {
  const { sign } = peer;
  if (revokedKeys(link).has(peer.key) || peer.key === ownKey(link ?? {}) || sign === ownSign(link)
    || computerHistory(link)?.retiredSigns.includes(sign)
    || Object.values(removalsOf(link)).some(record => record.peer.sign === sign)
    || link?.untold?.some(each => each.sign === sign)
    || peersOf(link).some(each => (each.key === peer.key || each.sign === sign) && (each.key !== peer.key || each.sign !== sign))) {
    throw new Failed(`${name} presented membership keys that cannot be used. Update Kuvo Code there and add it again.`);
  }
}

// Sets a computer up at `destination`, or again: everything to its box running there as one of yours.
async function setUp(tell: Telling, destination: string, name: string, kind: "add" | "setup" | "update", expectedKey?: string, grant?: AddGrant): Promise<void> {
  tell.step("checking", `Connecting to ${name}`);
  const beforeEnrollment = readLink();
  const identitySetup = computerHistoryLayoutReady(K) ? followedSetup(beforeEnrollment)?.id : undefined;
  if (grant?.replace && !identitySetup) throw new Failed("Finish updating the setup before choosing a computer to replace.");
  const mine = boxVersion();
  const files = ownFiles();
  if (!files || !mine) throw new Failed(`Kuvo Code on ${ownName()} has no files of its own to give another computer yet. Update Kuvo Code here, then try again.`);
  const reach = new Reach(destination, name, tell, kind === "add" ? [] : otherAddressesOf(destination));
  const checkGrant = () => checkAdd(readLink(), grant, name, expectedKey);
  const account = new AccountReservation(reach, name, tell);
  const reserved = account.run;
  await account.use(async () => {
    if (reach.windows && kind === "add") {
      name = reach.windows.host;
      tell.rename(name);
    }
    const answer = await reach.run(scripts.probe(ENGINE_VERSION, identitySetup), { cut: true, most: 120_000 });
    if (answer.status !== 0) throw new Failed(`${name} couldn’t be checked. ${lastLine(answer.stderr)}`.trim());
    const probe = probed(answer.stdout);
    if (probe.updating) throw new Failed(`${name} is updating Kuvo Code itself right now. Try again in a few minutes.`);
    // Brought up to date: nothing to do where it's as new already (it updated itself, or another
    // computer brought it up meanwhile).
    if (kind === "update" && probe.version && !newerVersion(mine, probe.version)) {
      for (const id of ["files", "engine", "starting", "seeding", "joining"]) tell.skip(id, "It’s up to date already");
      return;
    }
    if (/^app-(?:07|box)$/m.test(answer.stdout) && !machineBoxProbed(probe)) throw new Failed(`${name} still has Kuvo Code 0.7’s sessions. Update it to Kuvo first — its sessions move over then — and add it again.`, false);
    if (kind === "add" && identitySetup) {
      const prefix = "kuvo removal-ack ";
      for (const line of answer.stdout.split("\n").filter(line => line.startsWith(prefix)).slice(0, 16)) {
        let envelope: unknown;
        try { envelope = JSON.parse(line.slice(prefix.length)); } catch { continue; }
        await takeWipeReceipt(envelope);
      }
    }
    const link = readLink();
    if (identitySetup && followedSetup(link)?.id !== identitySetup) throw new Failed("The setup changed. Start a new add.");
    const history = computerHistory(link);
    const selected = enrollmentPredecessor(history, probe.fingerprint, grant?.replace);
    const predecessor = selected ?? (kind === "add" ? undefined : expectedKey);
    const foundKey = foundOf(probe.lines).find(box => !box.app)?.card?.self.key;
    const oldFingerprint = history?.slots.find(slot => slot.key === foundKey)?.fingerprint;
    if (oldFingerprint && probe.fingerprint && oldFingerprint !== probe.fingerprint && grant?.replace !== foundKey) {
      throw new Failed("This computer presents another computer’s membership. Clear the copied Kuvo link before adding it.");
    }
    checkFingerprintReplacement(link, selected, probe.fingerprintSource, foundKey === expectedKey ? expectedKey : undefined, grant?.replace);
    checkAdd(link, grant, name, predecessor);
    if (predecessor !== foundKey) checkPredecessor(link, predecessor, name);
    if (link) noteKeyNow(link);
    const foundBoxes = foundOf(probe.lines);
    // The same computer, here already under another address: that address is kept as one of its own.
    const saved = savedMachines();
    if (!saved) throw new Failed(`${ownName()}’s list of machines can’t be read right now.`);
    // (By its id there, and its box's own key where its card says it: a copy of another machine's
    // disk has that machine's id, and is a computer of its own.)
    const found = foundBoxes.find(box => !box.app);
    const savedHere = saved.find(each => each.destination === destination && each.identity && each.identity === probe.id);
    const known = savedHere && peerAtDestination(link, savedHere.destination);
    const ours = { ...oursNow(link), ...(known ? { rotated: { machine: known.key, keys: new Set(link?.keysBefore ?? []) } } : {}) };
    const judged = judgeComputer(foundBoxes, ours, appsOf(probe.lines));
    if (judged.verdict === "self") throw new Failed(`That’s ${ownName()} itself. Add another computer.`, false);
    const sameID = kind === "add" && probe.id ? saved.find(each => each.identity === probe.id && each.destination !== destination) : undefined;
    const itsPeer = sameID ? peerAtDestination(link, sameID.destination) : undefined;
    const same = sameID && itsPeer && (!found?.card?.self.key || found.card.self.key === itsPeer.key) ? sameID : undefined;
    if (same && itsPeer && !grant?.replace) {
      const peer = itsPeer;
      updateLink(current => {
        checkAdd(current, grant, name, peer.key);
        const latest = savedMachines();
        const record = latest?.find(each => each.destination === same.destination);
        if (!latest || !record || peerAtDestination(current, record.destination)?.key !== peer.key) throw removedDuringAdd(name);
        writePrivate(LIST_FILE, JSON.stringify(latest.map(each => (each === record ? { ...each, otherAddresses: [...new Set([...(each.otherAddresses ?? []), destination])] } : each)), null, 2));
        return null;
      });
      for (const id of ["files", "engine", "starting", "seeding", "joining"]) tell.skip(id, `It’s ${peer?.name ?? same.destination}, already one of your computers`);
      tell.rename(peer?.name ?? same.destination);
      if (peer) tell.came({ key: peer.key, name: peer.name }, true);
      return;
    }
    const returning = foundKey && revokedKeys(link).has(foundKey) && foundBoxes.length === 1
      && found?.card?.role === "primary" && found.card.computers === 1 && found.card.used !== true && !found.card.app && !appsOf(probe.lines);
    if (foundKey && revokedKeys(link).has(foundKey)) checkAdd(link, grant, name, foundKey);
    if (!returning && judged.verdict !== "free" && judged.verdict !== "ours" && !judged.joins) {
      throw new AccountOccupied(name);
    }

    // Only a fresh account can take selected credentials. Resolve the choice before writing
    // anything to it, so an unavailable selection can be retried against the same fresh account.
    const selection = grant?.credentials ?? [];
    const issuer = link && ownKey(link);
    const recoveryScope = issuer && credentialSelectionLayoutReady(K) ? sshCopyScope(identitySetup, issuer) : undefined;
    const record = recoveryScope && recoveryScope + sshCopySelection(selection);
    const knownMember = foundKey && peersOf(link).some(peer => peer.key === foundKey);
    const recoveringFresh = kind === "add" && !knownMember && recoveryScope && probe.freshAdd?.startsWith(recoveryScope)
      && appsOf(probe.lines) === 0 && (judged.verdict === "ours" || judged.verdict === "free");
    if (kind === "add" && !knownMember && probe.freshAdd && !recoveringFresh) {
      throw new Failed("This OS account still has an interrupted add. Finish that attempt or remove the interrupted installation first.");
    }
    if (recoveringFresh && probe.freshAdd?.endsWith(" copying") && `${record} copying` !== probe.freshAdd) {
      throw new Failed("The interrupted add had a different choice of sign-ins and keys. Resume with that choice or remove the interrupted installation first.");
    }
    const fresh = Boolean(recoveringFresh) || (kind === "add" && judged.verdict === "free" && foundBoxes.length === 0 && appsOf(probe.lines) === 0 && !probe.id && !probe.version);
    checkGrant();
    const given = fresh ? await gatherSeed(selection) : null;
    checkGrant();
    if (given && Object.keys(given.keys).length + given.credentials.length < selection.length) {
      throw new Failed("Some selected sign-ins or keys are no longer available. Open the checklist again and retry.");
    }
    if (fresh && record) tell.choseCredentials(selection);

    if (fresh && record && !recoveringFresh) {
      const saved = await reserved(scripts.rememberFresh(`${record} prepared`), { beforeInput: checkGrant });
      if (saved.status !== 0) throw new Failed(`Kuvo couldn’t save the installation choice on ${name}.`);
    }

    // Its files, unless it has a newer Kuvo Code (which keeps its own) or has these already.
    const newer = newerVersion(probe.version, mine);
    if (newer) {
      tell.skip("files", `It has a newer Kuvo Code (${probe.version}), left as it is`);
      tell.skip("engine", "Its own");
    } else {
      const helper = probe.kind.system === "darwin" ? machineHelperPack() : null;
      if (probe.kind.system === "darwin" && !helper) throw new Failed(`Kuvo Code on ${ownName()} has no Kuvo Helper to give ${name}. Update Kuvo Code here, then try again.`);
      tell.step("files");
      const has = await reach.run(scripts.filesCheck(files), { cut: true });
      if (has.status === 0 && has.stdout.trim() === "same") tell.skip("files", "It has them already");
      else {
        const placed = await reserved(scripts.filesInstall(files), { input: tarOf(files), sent: tell.sent, most: 900_000 });
        if (placed.status !== 0) throw new Failed(`Kuvo Code’s files couldn’t be put on ${name}. ${lastLine(placed.stderr)}`.trim());
      }
      if (helper) {
        const placed = await reserved(scripts.helperInstall(helper.sum), { input: helper.bytes, sent: tell.sent, cut: true, most: 120_000 });
        if (placed.status !== 0) throw new Failed(`Kuvo Helper couldn’t be put on ${name}. ${lastLine(placed.stderr)}`.trim());
      }
      if (probe.engine) tell.skip("engine", "It has it already");
      else {
        tell.step("engine");
        const engine = await engineFor(probe.kind, () => tell.step("engine", `Downloading it to ${ownName()} first`)).catch(error => { throw new Failed(error instanceof Error ? error.message : String(error)); });
        tell.step("engine", megabytes(engine.size));
        const placed = await reserved(scripts.engineInstall(ENGINE_VERSION, engine.sum, engine.packed), { input: { file: engine.file }, sent: tell.sent, most: 3_600_000 });
        if (placed.status !== 0) throw new Failed(`The engine couldn’t be put on ${name}. ${lastLine(placed.stderr)}`.trim());
      }
    }

    // Its start, by its own keeper: never cut short once it's begun.
    tell.step("starting");
    const own = link ? ownKey(link) : null;
    const notes = JSON.stringify({ mac: own ? launcherID(own) : "", macName: ownName().replace(/[{}"\\]/g, ""), box: mine });
    // Started there, on a computer that had nothing, and not one of yours yet: whatever ends this
    // before it is (stopped, or failed), it's stopped there and what was copied is deleted.
    let unjoined = false;
    try {
      const started = await reserved(scripts.start(ENGINE_VERSION, name, notes, fresh && record && selection.length ? `${record} copying` : undefined, reach.windows?.distro), {
        most: 600_000,
        beforeInput: () => { checkGrant(); unjoined = fresh; },
        ...(given ? { input: Buffer.from(JSON.stringify(settingsSeedFormat.write({ settings: given.settings, keys: given.keys }))) } : {}),
      });
      if (started.status === 7) throw new Failed(`${name} was removed from Kuvo Code while it was starting, so it wasn’t started. Add it again to use it.`);
      if (started.status === 6) throw new Failed(`${name}’s settings couldn’t be made ready. ${lastLine(started.stderr)}`.trim());
      if (started.status !== 0) throw new Failed(`Kuvo Code didn’t start on ${name}. ${lastLine(started.stderr)}`.trim(), true, `Kuvo Code didn’t start on ${name}`);
      checkGrant();
      // Its accounts, once: the keys and the sign-ins that stay the same. Never what renews itself.
      const renewing = given?.renewing ? ` ${given.renewing === 1 ? "One sign-in renews itself" : `${given.renewing} sign-ins renew themselves`}, so sign in to ${given.renewing === 1 ? "it" : "those"} on ${name}.` : "";
      if (!given) tell.skip("seeding", "It keeps its own");
      else {
        tell.step("seeding");
        const settings = /^seeded$/m.test(started.stdout);
        const pendingKeys = !settings && recoveringFresh ? given.keys : {};
        if (!settings && !recoveringFresh && Object.keys(given.keys).length) throw new Failed(`The selected keys couldn’t be saved on ${name}.`);
        let taken = 0;
        let keys = settings ? Object.keys(given.keys).length : 0;
        if (given.credentials.length || Object.keys(pendingKeys).length) {
          const ran = await reserved(scripts.seed(ENGINE_VERSION), { input: Buffer.from(JSON.stringify(credentialSeedFormat.write({
            credentials: given.credentials, ...(Object.keys(pendingKeys).length ? { keys: pendingKeys, integrations: given.integrations } : {}),
          }))), beforeInput: checkGrant, most: 300_000 });
          checkGrant();
          let said: unknown = null;
          try { said = JSON.parse(ran.stdout.trim().split("\n").pop() ?? ""); } catch {}
          if (ran.status !== 0 || !isObject(said) || said.status !== 200 || typeof said.taken !== "number") {
            throw new Failed(`The selected sign-ins couldn’t be saved on ${name}.`);
          }
          taken = said.taken;
          if (typeof said.keys === "number") keys = said.keys;
        }
        const copied = [keys ? `${keys} ${keys === 1 ? "key" : "keys"}` : "", taken ? `${taken} ${taken === 1 ? "account" : "accounts"}` : ""].filter(Boolean).join(" and ");
        tell.skip("seeding", `${copied ? `Copied ${copied}.` : settings ? "Copied your settings." : "Nothing to copy."}${renewing}`);
      }
      tell.step("joining");
      const identity = /^id ([0-9a-f]{32})$/m.exec(started.stdout)?.[1] ?? probe.id;
      const joined = await join(tell, reserved, destination, name, identity, reach.windows?.distro, account.token, (peer, enrollment) => {
        tell.saving(enrollment, peer);
        tell.commit?.();
        unjoined = false;
      }, expectedKey, grant, { setup: identitySetup, fingerprint: probe.fingerprint, fingerprintSource: probe.fingerprintSource, predecessor, selected });
      unjoined = false;
      tell.came(joined);
      account.clearFresh = fresh || Boolean(knownMember && recoveryScope && probe.freshAdd?.startsWith(recoveryScope));
    } catch (error) {
      // A competing setup enrolled this account after the probe. Its data belongs to that setup.
      if (!unjoined || !account.canRollBack || error instanceof AccountOccupied) throw error;
      // (Run though the try was stopped: this is its own tidying up.)
      // (With no key of this box's to let go there, as when its link has none: undone all the same.)
      const undone = await reserved(scripts.uninstall(ENGINE_VERSION, own ?? "", true), { most: 600_000, anyway: true }).catch(() => null);
      if (undone?.status === 0) account.clearFresh = true;
      const still = `Kuvo Code may still be running on ${name}, with the keys and sign-ins copied there. Add ${name} again, then uninstall it from its page.`;
      if (error instanceof Stopped || tell.signal.aborted) {
        if (undone?.status !== 0) throw new Failed(`This was stopped after Kuvo Code had started on ${name}, and it couldn’t be stopped there again. ${still}`, true, `${name} is still running Kuvo Code`);
        throw error;
      }
      const failure = error instanceof Failed ? error : new Failed(error instanceof Error && error.message ? error.message : "Something went wrong.");
      throw new Failed(`${failure.message} ${undone?.status === 0 ? `Kuvo Code was stopped on ${name} again, and the keys and sign-ins copied there were deleted.` : still}`, failure.again, failure.title);
    }
  });
}

function checkFingerprintReplacement(link: LinkState | null, predecessor: string | undefined, source: FingerprintSource | undefined, presented: string | null | undefined, approved: string | undefined) {
  if (predecessor && predecessor !== presented && predecessor !== approved
    && peersOf(link).some(peer => peer.key === predecessor)
    && requiresReplacementConfirmation(computerHistory(link), predecessor, source)) {
    throw new Failed("This computer has the same system image identity as a setup member. Confirm which computer to replace.", false);
  }
}

// Signs the computer into your setup: this box's keys and documents given over SSH (the trust
// between them), its own taken back, and its record written here, for your other computers and
// phones to take from this box.
async function join(tell: Telling, run: Reach["run"], destination: string, name: string, identity: string | null, wslDistro: string | undefined, token: string, staged: (peer: { key: string; name: string }, enrollment: Receipt) => void, expectedKey: string | undefined, grant: AddGrant | undefined, match: { setup?: string; fingerprint?: string; fingerprintSource?: FingerprintSource; predecessor?: string; selected?: string }): Promise<{ key: string; name: string }> {
  const beforeLink = readLink();
  const relay = IS_MAC ? macLink.config?.relay : beforeLink?.relay;
  const removedWhileAdding = (link: LinkState | null) => expectedKey && link?.removedHere?.includes(expectedKey);
  checkAdd(beforeLink, grant, name, expectedKey);
  if (removedWhileAdding(beforeLink)) throw removedDuringAdd(name);
  const gift = await setupGift();
  if (!relay || !gift?.mac) throw new Failed(`${ownName()} isn’t linked to your other computers yet, so it couldn’t add ${name}.`);
  const keepMemberships = peersOf(beforeLink).flatMap(({ key, sign }) => sign && key !== grant?.replace ? [{ key, sign }] : []);
  const blockedMemberships = Object.values(removalsOf(beforeLink)).filter(record => record.wipe && !record.delivered).map(record => record.peer.key);
  const input = setupInputFormat.write({ relay, name, keepMemberships, blockedMemberships, discardLocalPairings: grant?.discardLocalPairings, mac: gift.mac, macSign: gift.macSign, docs: gift.docs, id: destination, tolerates: 1, macID: launcherID(gift.mac), macName: ownName(), replaces: gift.replaces });
  const ran = await run(scripts.join(ENGINE_VERSION, token), { input: Buffer.from(JSON.stringify(input)), beforeInput: () => checkAdd(readLink(), grant, name, expectedKey), most: 300_000 });
  if (ran.status === SETUP_LOCAL_PAIRINGS) throw new Failed(`${name} has locally paired devices that are missing from this setup. Confirm discarding those pairings before adding it.`, false);
  if (ran.status === SETUP_OCCUPIED) throw new AccountOccupied(name);
  let said: unknown = null;
  try { said = JSON.parse(ran.stdout.trim().split("\n").pop() ?? ""); } catch {}
  if (ran.status !== 0 || !isObject(said) || typeof said.address !== "string" || !ADDRESS.test(said.address) || typeof said.key !== "string" || !KEY.test(said.key) || typeof said.sign !== "string" || !SIGN.test(said.sign)) {
    throw new Failed(`Kuvo Code on ${name} couldn’t join your computers. ${lastLine(ran.stderr)}`.trim());
  }
  const peer = { key: said.key, sign: said.sign, address: said.address, name, id: destination, kind: "machine" as const };
  const checkEnrollment = (link: LinkState | null) => {
    checkAdd(link, grant, name, expectedKey);
    checkAdd(link, grant, name, peer.key);
    checkAdd(link, grant, name, match.predecessor);
    if (removedWhileAdding(link)) throw removedDuringAdd(name);
    checkFreshKeys(link, peer, name);
    if (grant?.replace === peer.key) throw new Failed("The replacement must have its own membership keys.");
    if (match.setup && (followedSetup(link)?.id !== match.setup
      || enrollmentPredecessor(computerHistory(link), match.fingerprint, grant?.replace) !== match.selected)) {
      throw new Failed("The computer’s record changed. Start a new add.");
    }
    checkFingerprintReplacement(link, match.selected, match.fingerprintSource, peer.key === expectedKey ? expectedKey : undefined, grant?.replace);
    if (match.predecessor && match.predecessor !== peer.key) checkPredecessor(link, match.predecessor, name);
    if (match.predecessor && match.predecessor !== peer.key && peersOf(link).some(each => each.key === peer.key)) {
      throw new Failed("This membership already belongs to another computer in the setup.");
    }
  };
  const keepRecord = () => {
    const saved = savedMachines();
    if (!saved) throw new Failed(`${ownName()}’s list of machines can’t be read right now.`);
    const had = saved.find(each => each.destination === destination);
    writeDurablePrivate(LIST_FILE, JSON.stringify(had ? saved.map(each => (each === had ? { ...each, ...(identity ? { identity } : {}), wslDistro } : each)) : [...saved, { destination, ...(identity ? { identity } : {}), wslDistro }], null, 2));
  };
  checkEnrollment(readLink());
  const { gone, save } = stageEnrollment(peer, checkEnrollment, match, () => {
    keepRecord();
    keepHost(destination, peer);
  });
  if (save) staged(peer, save);
  const docs = Array.isArray(said.docs) ? said.docs : [];
  const computer = await finishEnrollment(tell, peer, save);
  if (gone.length) {
    macDocsChanged({ closeRemoved: readLink() });
    tellGone(gone);
  }
  // Its own documents, signed with the key it just gave, taken as if by mail.
  if (docs.length) await receiveDocs(peer, { give: docs }, liveLink()).catch(() => {});
  return computer;
}

export function stageInstallerEnrollment(peer: Peer & { sign: string }, expectedSetup: string, operation: string, checkInvite: (link: LinkState | null) => void,
  merge?: MembershipCompanions, markEnrollment?: (link: LinkState) => void,
  recovery?: NonNullable<LinkState["installerRecoveries"]>[string]) {
  return stageEnrollment(peer, link => {
    checkInvite(link);
    checkAdd(link, undefined, peer.name, peer.key);
    try { checkFreshKeys(link, peer, peer.name); }
    catch (error) {
      if (merge && error instanceof Failed) throw new MergeRefusal(error.message);
      throw error;
    }
    if (followedSetup(link)?.id !== expectedSetup) throw new Failed("The setup changed. Start a new add.");
  }, { setup: expectedSetup }, markEnrollment, operation, merge, recovery);
}

/** Save a member through the primary's roster and accepted membership operation. */
function stageEnrollment(
  peer: Peer & { sign: string },
  checkEnrollment: (link: LinkState | null) => void,
  match: { setup?: string; fingerprint?: string; fingerprintSource?: FingerprintSource; predecessor?: string },
  keep?: (link: LinkState) => void,
  operation?: string,
  merge?: MembershipCompanions,
  recovery?: NonNullable<LinkState["installerRecoveries"]>[string],
): { gone: Peer[]; save: Receipt | null } {
  let gone: Peer[] = [];
  const saved = updateAccess(link => {
    checkEnrollment(link);
    const state = link ?? { phones: [] };
    const peers = peersOf(state);
    const predecessor = match.predecessor;
    const replaced = predecessor && predecessor !== peer.key ? peers.find(each => each.key === predecessor) : undefined;
    const already = peers.some(each => each.key === peer.key);
    if (!already && !replaced && peers.length >= MOST_PEERS) {
      const message = "The setup already has the maximum number of computers.";
      throw merge ? new MergeRefusal(message) : new Failed(message);
    }
    const index = peers.findIndex(each => each.key === (replaced?.key ?? peer.key));
    const roster = [...peers];
    if (index < 0) roster.push(peer);
    else roster[index] = { ...peer, kind: roster[index]!.kind };
    const history = match.setup ? historyFor(state, peers) : undefined;
    if (match.setup && state.setup && addComputersLayoutReady(K)) state.setup.managed = true;
    const made = setMacPeers(state, { peers: orderComputers(history, roster), moved: state.macMoved, removed: replaced ? [replaced.key] : [] });
    if (!made.link) return null;
    if (recovery) state.installerRecoveries = { ...state.installerRecoveries, [peer.key]: recovery };
    else if (state.installerRecoveries) delete state.installerRecoveries[peer.key];
    if (history) {
      const retired = computerHistory(state);
      if (retired) {
        history.retiredSigns = [...new Set([...history.retiredSigns, ...retired.retiredSigns])];
        if (retired.retiredNames) history.retiredNames = retired.retiredNames;
      }
      rememberEnrollment(history, peer.key, match.fingerprint, predecessor, match.fingerprintSource);
      state.computerHistory = history;
      state.macPeers = orderComputers(history, peersOf(state));
    }
    gone = replaced ? [replaced] : [];
    return state;
  }, { operation, checkpoint: Boolean(match.setup && addComputersLayoutReady(K)), together: merge, beforeWrite: keep });
  const { changed, save, busy } = saved;
  if (busy) throw new Failed("The main computer could not save the access change.");
  if (!save && !changed) throw new Failed(`${ownName()} couldn’t keep ${peer.name}’s record.`);
  return { gone, save };
}

async function finishEnrollment(tell: Telling, peer: { key: string; name: string }, save: Receipt | null) {
  if (save) {
    macDocsChanged();
    tell.saved(save);
  }
  const enrolled = peersOf(readLink()).find(each => each.key === peer.key);
  if (!enrolled) throw new Failed(`${ownName()} couldn’t keep ${peer.name}’s record.`);
  // This box meets its machines on the relay from now on.
  if (IS_MAC && macLink.config) {
    macLink.config = { ...macLink.config, machines: true };
    macLinkCheck();
    macLink.wake?.();
  }
  macDocsChanged();
  return { key: enrolled.key, name: enrolled.name };
}

function begin(kind: AddTry["kind"], destination: string, name: string, steps: readonly (readonly [string, string])[], work: (tell: Telling) => Promise<void>, credentials?: string[]): Running {
  let it: Running | null = null;
  const made = startTry(kind, destination, name, steps, tell => {
    stoppers.set(tell, () => { if (it) stopTry(it); });
    return work(tell);
  }, credentials);
  it = made;
  return made;
}

/** What `begin…` answers where something else is under way for that computer, in words. */
export interface Busy {
  busy: string;
}

// Why a computer can't be set up now: it's being uninstalled.
const uninstalling = (now: Running | undefined): Busy | null => (now?.seen.kind === "uninstall" ? { busy: `Kuvo Code is being uninstalled from ${now.seen.name} now. Add it again once that’s done.` } : null);

/** Begins adding the computer at `destination` (checked first: `addressProblem`, `canAdd`). One
 * being added or set up there already is answered as it is; one being stopped is waited for. */
export function beginAdd(destination: string, asker?: Asker, replace?: string, discardLocalPairings?: boolean, credentials?: string[]): Running | Busy {
  const now = underWay(destination);
  const link = readLink();
  const key = asker?.key?.toString("base64url");
  const caller = key && pairedPhone(link, key, asker?.pairing);
  if (key && !caller) return { busy: ADD_REVOKED };
  if (now?.seen.kind === "add" && sshCopySelection(now.selectedCredentials ?? []) !== sshCopySelection(credentials ?? [])) {
    return { busy: "An add with a different choice of sign-ins and keys is already in progress for this computer." };
  }
  const grant: AddGrant = { replace, discardLocalPairings, credentials, caller: caller ? { key: caller.key, pairing: caller.pairing ?? "" } : undefined, revisions: link?.removalRevisions ?? {} };
  const expected = peerAtDestination(link, destination)?.key;
  return uninstalling(now) ?? now ?? begin("add", destination, destination, SETUP_STEPS, tell => setUp(tell, destination, destination, "add", expected, grant), credentials);
}

/** The address this box set the computer `key` up at, and its name: null for one it didn't. */
export function sshAddressOf(key: string): { destination: string; name: string; wslDistro?: string } | null {
  const link = readLink();
  const peer = peersOf(link).find(each => each.key === key);
  const destination = peer && localDestination(link, key);
  if (!peer || !destination || addressProblem(destination)) return null;
  const wslDistro = distroName(savedMachines()?.find(each => each.destination === destination)?.wslDistro);
  return { destination, name: peer.name, wslDistro };
}

/** Begins setting the computer `key` up again, at the address it was added under: the same steps,
 * which bring an older Kuvo Code there up to this one's. */
export function beginSetup(key: string): Running | Busy | null {
  if (readPeerAgents()[key]?.computer?.kuvo) return { busy: "This computer updates with its setup." };
  const at = sshAddressOf(key);
  if (!at) return null;
  const now = underWay(at.destination);
  return uninstalling(now) ?? now ?? begin("setup", at.destination, at.name, AGAIN_STEPS, tell => setUp(tell, at.destination, at.name, "setup", key));
}

/** What Kuvo the computer `key` has, and how many conversations are at work there now (null: that
 * couldn't be told), asked over SSH at the address this box set it up at, changing nothing: for
 * bringing it up to date only while nothing is (`keep-up.ts`). Nobody is asked anything (a host key
 * not known fails it). Null for a computer this box didn't set up; a `Failed` for one it couldn't reach. */
export async function lookAtWork(key: string): Promise<{ version: string | null; working: number | null } | null> {
  const at = sshAddressOf(key);
  if (!at) return null;
  const stop = new AbortController();
  const quiet: Telling = {
    signal: stop.signal,
    check() { if (stop.signal.aborted) throw new Stopped(); },
    step() {}, sent() {}, skip() {}, came() {}, rename() {}, saving() {}, saved() {}, choseCredentials() {},
    ask: async () => { throw new Failed(`${at.name}’s host key isn’t one ${ownName()} knows.`); },
  };
  const reach = new Reach(at.destination, at.name, quiet, otherAddressesOf(at.destination));
  try {
    const answer = await reach.run(scripts.atWork(ENGINE_VERSION), { most: 60_000 });
    if (answer.status !== 0) throw new Failed(`${at.name} couldn’t be checked. ${lastLine(answer.stderr)}`.trim());
    const said = (name: string) => answer.stdout.split("\n").map(line => line.trim()).find(line => line.startsWith(`${name} `))?.slice(name.length + 1) ?? "";
    const version = said("kuvo version");
    const working = said("working");
    return { version: /^\d+\.\d+\.\d+/.test(version) ? version : null, working: /^\d+$/.test(working) ? Number(working) : null };
  } finally {
    await reach.close();
  }
}

/** Begins bringing the computer `key`'s older Kuvo up to date with this box's (`computers.update`,
 * `keep-up.ts`): Set Up Again's steps, which leave its running engine alone and its coding agents'
 * bridge to make way when nothing runs. `auto`: begun by this box itself, with nobody there to ask,
 * so what it would ask you (a host key, another setup's box) fails it instead, in words that say
 * to update it from its page. */
export function beginUpdate(key: string, auto = false): Running | Busy | null {
  if (readPeerAgents()[key]?.computer?.kuvo) return { busy: "This computer updates with its setup." };
  const at = sshAddressOf(key);
  if (!at) return null;
  const now = underWay(at.destination);
  if (now) return uninstalling(now) ?? now;
  return begin("update", at.destination, at.name, UPDATE_STEPS, tell => setUp(auto ? {
    ...tell,
    ask: async () => { throw new Failed(`${at.name} needs you to answer something before it’s updated. Choose Update Now on its page in Computers.`); },
  } : tell, at.destination, at.name, "update", key));
}

/** Begins uninstalling Kuvo Code from the computer `key` (one being uninstalled already is
 * answered as it is): what's under way there (it's being set up) is stopped first and waited for, so nothing starts it again behind the removal; then it lets
 * this computer go, stops and removes what starts it, and this box takes it off its list. */
export function beginUninstall(key: string): Running | null {
  const at = sshAddressOf(key);
  if (!at) return null;
  const now = underWay(at.destination);
  if (now?.seen.kind === "uninstall") return now;
  if (now) stopTry(now);
  return begin("uninstall", at.destination, at.name, UNINSTALL_STEPS, async tell => {
    if (followedSetup(readLink())?.checkpointed) {
      tell.step("removing");
      const result = await removeComputerHere(key, false, { wipe: true });
      const receipt = setupSaveReceipt(result.value);
      if (receipt) tell.saved(receipt);
      if (result.status !== 200) throw new Failed(
        isObject(result.value) && typeof result.value.message === "string" ? result.value.message : "The removal couldn’t be saved.");
      tell.commit?.();
      tell.step("removing", "Access was removed. Waiting for confirmation that the data was erased.");
      const deadline = Date.now() + 15_000;
      while (true) {
        const wipe = removalsOf(readLink())[key];
        if (wipe?.wipe && wipe.delivered && !wipe.resolution) break;
        if (wipe?.error) throw new Failed(wipe.error, false);
        if (wipe?.resolution) throw new Failed("The removal was closed without confirmation that the data was erased.", false);
        if (Date.now() >= deadline) throw new Failed("Access was removed. This computer’s wipe is still pending in Computers.", false);
        await delay(250);
      }
      tell.step("forgetting");
      return;
    }
    const reach = new Reach(at.destination, at.name, tell, otherAddressesOf(at.destination));
    const account = new AccountReservation(reach, at.name, tell);
    tell.step("checking", `Connecting to ${at.name}`);
    await account.use(async () => {
      let answer: Ran;
      try {
        answer = await reach.run(scripts.probe(ENGINE_VERSION), { cut: true, most: 120_000 });
        if (answer.status !== 0) throw new Error(answer.stderr);
      } catch (error) {
        if (error instanceof Stopped) throw error;
        console.error("Uninstall SSH probe failed:", error);
        throw new Failed(`Kuvo Code couldn’t reach ${at.name} to start uninstalling. Try again when it’s online.`);
      }
      const link = readLink();
      const own = link ? ownKey(link) : null;
      if (!own) throw new Failed(`${ownName()}’s link has no keys yet.`);
      // Another Mac that set it up still uses it: Kuvo Code stays there for that one.
      const box = foundOf(answer.stdout.split("\n").map(line => line.trim())).find(each => !each.app);
      const macs = new Set([...(box?.card?.macs ?? []).map(mac => mac.key), ...(box?.link?.macs ?? [])]);
      macs.delete(own);
      const only = macs.size === 0;
      tell.step("removing", only ? undefined : "Another Mac uses it too, so Kuvo Code stays there");
      if (only) {
        const prepared = await account.run(scripts.prepareUninstall(ENGINE_VERSION, own), { most: 600_000 });
        const reply = parsed(prepared.stdout.trim().split("\n").at(-1));
        if (prepared.status !== 0 || !isObject(reply) || reply.result !== "prepared")
          throw new Failed(isObject(reply) && typeof reply.message === "string" ? reply.message : `Kuvo Code couldn’t verify the pre-wipe backup on ${at.name}.`);
      }
      // Persist the revocation before the first destructive command. SSH may lose the reply.
      let removed: Awaited<ReturnType<typeof removeComputerHere>>;
      try {
        removed = await removeComputerHere(key);
      } catch (error) {
        console.error("Uninstall local revocation failed:", error);
        throw new Failed("Kuvo Code couldn’t confirm that access was revoked. Try uninstalling again.");
      }
      const receipt = setupSaveReceipt(removed.value);
      if (receipt) tell.saved(receipt);
      const current = readLink();
      const cleanupOnly = removed.status === 503 && current?.removedHere?.includes(key)
        && !peersOf(current).some(peer => peer.key === key);
      if (removed.status !== 200 && !cleanupOnly) {
        console.error("Uninstall local revocation returned", removed);
        throw new Failed("Kuvo Code couldn’t confirm that access was revoked. Try uninstalling again.");
      }
      const listCleanup = cleanupOnly ? " The saved machines list still needs to be updated." : "";
      tell.commit?.();
      let cleanup = "";
      // Never cut short: let the remote removal finish even if the client stops waiting.
      try {
        const ran = await account.run(scripts.uninstall(ENGINE_VERSION, own, only, only), { most: 600_000 });
        const last = ran.stdout.trim().split("\n").pop();
        let said: unknown;
        try { said = last && JSON.parse(last); } catch {}
        if (ran.status !== 0 || (last !== "kept" && (!isObject(said) || (said.result !== "none" && said.result !== "uninstalled")))) {
          console.error("Uninstall remote cleanup response:", ran.status, ran.stdout, ran.stderr);
          cleanup = isObject(said) && said.result === "incomplete" ? "The desktop identity folder couldn’t be deleted." :
            isObject(said) && said.result === "unfinished" ? "Kuvo Code couldn’t finish cleanup on that computer." :
            "Kuvo Code couldn’t confirm whether cleanup finished on that computer.";
        }
      } catch (error) {
        console.error("Uninstall remote cleanup unconfirmed:", error);
        cleanup = "Kuvo Code couldn’t confirm whether cleanup finished on that computer.";
      }
      tell.step("forgetting");
      if (cleanup || listCleanup) {
        const local = only
          ? "Run the local uninstall command (kuvo-link.js uninstall) on that computer to finish cleanup."
          : `Run kuvo-link.js leave ${own} on that computer to disconnect it from this setup.`;
        throw new Failed(`${at.name}’s access to your other computers was revoked.${listCleanup}${cleanup ? ` ${cleanup} ${local}` : ""}`, false);
      }
    });
  });
}

/** What this box offers for a computer it set up over SSH, beside the rest (`kuvo/computers`'
 * `actions`): Set Up Again, and Uninstall with its confirm; `why` while one is under way. */
export function sshActions(computer: Pick<ListedComputer, "key" | "name" | "kind" | "here">, here: string): ComputerAction[] {
  if (computer.here || computer.kind !== "machine" || !computer.key || !canAdd().can) return [];
  const at = sshAddressOf(computer.key);
  if (!at) return [];
  const name = computer.name || "this machine";
  const busy = underWay(at.destination);
  const why = busy ? (busy.seen.kind === "uninstall" ? "Kuvo Code is being uninstalled from it now." : busy.seen.kind === "update" ? "It’s being updated now." : "It’s being set up now.") : null;
  const link = readLink();
  const wipe = followedSetup(link)?.checkpointed;
  const label = wipe ? "Unlink and wipe" : "Uninstall";
  const message = wipe
    ? `${name} leaves this setup immediately. Kuvo Code, its sessions and its bots are deleted when that computer is online and its pre-wipe checks pass.`
    : `${here} connects to ${name} over SSH and stops Kuvo Code there: its engine and agents, and what starts them after a restart. The sign-ins and keys it keeps are deleted. Its sessions stay in ~/.kuvo-code on ${name}.`;
  return [
    ...(readPeerAgents()[computer.key]?.computer?.kuvo ? [] : [{ id: "setup" as const, label: "Set Up Again", ...(why ? { why } : {}) }]),
    { id: "uninstall", label, ...(busy?.seen.kind === "uninstall" ? { why: why! } : { confirm: { title: wipe ? `Unlink and wipe ${name}?` : `Uninstall Kuvo Code from ${name}?`, message, label } }) },
  ];
}
