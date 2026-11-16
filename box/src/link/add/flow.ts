// Adding a computer over SSH, setting one up again, and uninstalling Kuvo Code from one (PROTOCOL.md,
// "Computers", Adding one), done by this box for whichever client asks: the address checked, the
// host's key looked at, what's on the computer judged, this box's own files and engine put there,
// its box started to keep itself, and the computer signed into your setup, as a Mac's app did all
// of it before (#636). Each step is told as it goes (`tries.ts`); every word a client shows is here.
// Platform: runs on macOS and Linux alike. Which box may add (`canAdd`) is decided in one place: a
// Mac's box that isn't a member of another's setup, since only a Mac's box writes the record of
// your machines (its peers document, `mac/routes.ts`); a Linux box says so in words.

import crypto from "node:crypto";
import fs from "node:fs";
import type { AddTry, ComputerAction, ListedComputer } from "../../shared/routes/box";
import { isObject } from "../../shared/json";
import { plainName } from "../../shared/text";
import { macDocsChanged, receiveDocs } from "../documents/exchange";
import { SIGN } from "../documents/format";
import { writePrivate } from "../files";
import { IS_MAC, K } from "../home";
import { ownName } from "../letters/start";
import { macLinkCheck } from "../mac/link";
import { LIST_FILE, PHONE_LINK_FILE, savedMachines } from "../mac/machines";
import { setMacPeers, setupGift } from "../mac/routes";
import { ADDRESS, KEY, ownKey, peersOf } from "../peers/known";
import { vouchedWords } from "../phones";
import { oldAppAttached, removeHere } from "../peers/manage";
import { appsOf, foundOf, judgeComputer, oursNow } from "../setup/judge";
import { addElsewhere, mainElsewhere } from "../setup/main-only";
import { macLink, readLink, updateLink } from "../state";
import { boxVersion, newerVersion } from "../updates/update";
import { addressProblem } from "./address";
import { ENGINE_VERSION, RELEASE_FILES, engineFor, ownFiles, tarOf, type Kind } from "./files";
import * as scripts from "./scripts";
import { gatherSeed } from "./seed";
import { type HostKey, type Ran, type Way, hangUp, lastLine, readHostKey, remote, troubleOf, trustHostKeys, vaultAgent } from "./ssh";
import { Failed, type Running, Stopped, type Telling, startTry, stopTry, underWay } from "./tries";

/** Every address of the computers this box set up. */
export const takenAddresses = (): string[] => (savedMachines() ?? []).flatMap(each => [each.destination, ...(Array.isArray(each.otherAddresses) ? each.otherAddresses : [])]);

// What this box keeps of its machines' links for your phones (`kuvo/hosts`), as the Mac app kept it.
function keepHost(id: string, host: { name: string; address: string; key: string; sign: string }): void {
  let kept: Record<string, unknown> = {};
  try {
    const read: unknown = JSON.parse(fs.readFileSync(PHONE_LINK_FILE, "utf8"));
    if (!isObject(read)) throw new Failed(`${ownName()}'s record of its machines can't be read right now.`);
    kept = read;
  } catch (error) {
    if (error instanceof Failed) throw error;
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw new Failed(`${ownName()}'s record of its machines can't be read right now.`);
  }
  const machines = isObject(kept.machines) ? kept.machines : {};
  writePrivate(PHONE_LINK_FILE, JSON.stringify({ ...kept, machines: { ...machines, [id]: { ...host, anotherMac: false, peers: true } } }, null, 2));
}

/** Whether this box adds computers now, and why not, in words a client shows as they are. */
export function canAdd(): { can: true } | { can: false; status: 409 | 503; message: string; primary?: string } {
  const link = readLink();
  const main = mainElsewhere(link);
  if (main) return { can: false, status: 409, ...addElsewhere(main) };
  // BOX-NEEDED (#183): a Linux box writing the record of a setup's computers. Until then only a Mac's does.
  if (!IS_MAC) return { can: false, status: 409, message: "Add computers from a Mac for now: on Linux, Kuvo Code can’t yet keep the list of your computers." };
  const here = ownName();
  if (oldAppAttached()) return { can: false, status: 409, message: `Quit the old Kuvo Code app on ${here} first: it keeps its own list of machines, and would write over this one.` };
  if (!macLink.config?.relay) return { can: false, status: 503, message: `${here} isn’t linked to your other computers yet, so it can’t add one.` };
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
  return { kind, version: /^\d+\.\d+\.\d+/.test(version) ? version : null, updating: lines.includes("kuvo updating"), engine: lines.includes("kuvo engine"), id: /^[0-9a-f]{32}$/.test(id) ? id : null, lines };
}

const megabytes = (bytes: number): string => `${Math.max(1, Math.round(bytes / 1_000_000))} MB`;

/** A connection for one try: every script over it, signed in with your keys, or the vault's once
 * yours were refused; a host key not known asked about, and kept on your word. */
class Reach {
  private way: Way | undefined;
  private closeAgent: (() => void) | null = null;
  constructor(private readonly destination: string, private readonly name: string, private readonly tell: Telling) {}

  /** Runs `script` there; its answer when it ran (whatever its status), or a `Failed` for a
   * connection that didn't come about. `cut`: ended with the try when it's stopped (only what's
   * safe cut short); `anyway`: run though the try was stopped (its own tidying up). */
  async run(script: string, options: { input?: Buffer | { file: string }; sent?: (bytes: number, total: number) => void; cut?: boolean; most?: number; anyway?: boolean } = {}): Promise<Ran> {
    for (let tries = 0; ; tries++) {
      if (!options.anyway) this.tell.check();
      const ran = await remote(this.destination, script, { input: options.input, sent: options.sent, most: options.most, way: this.way, ...(options.cut ? { signal: this.tell.signal } : {}) });
      const trouble = troubleOf(ran);
      if (!trouble) return ran;
      if (options.anyway) throw new Failed(`Couldn’t reach ${this.name}.`);
      this.tell.check();
      if (tries >= 3) throw new Failed(`Couldn’t reach ${this.name}.`);
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
        throw new Failed(trouble.said ? `Couldn’t reach ${this.name}. ${trouble.said}${/[.!?…]$/.test(trouble.said) ? "" : "."}` : `Couldn’t reach ${this.name}.`);
      }
    }
  }

  // Reads the host's key without signing in, shows it, and keeps it once you say so.
  private async askKey(id: string, title: string, message: string, label: string, replacing?: number): Promise<void> {
    const read = await readHostKey(this.destination, this.tell.signal);
    this.tell.check();
    if (!read.keys.length) {
      const said = lastLine(read.said);
      throw new Failed(said && !/permission denied/i.test(said) ? `Couldn’t reach ${this.name}. ${said}${/[.!?…]$/.test(said) ? "" : "."}` : `Couldn’t read ${this.name}’s host key.`);
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
    trustHostKeys(read.keys, replacing);
  }

  async close(): Promise<void> {
    await hangUp(this.destination, this.way).catch(() => {});
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
const UNINSTALL_STEPS = [["checking", "Reaching it"], ["removing", "Stopping and removing Kuvo Code"], ["forgetting", "Taking it off your computers"]] as const;

/** The key this box is known by on the computers it starts (its engine launches' notes there):
 * from its link's key, never the key itself. */
const launcherID = (key: string): string => crypto.createHash("sha256").update(`kuvo-launcher\n${key}`).digest("hex").slice(0, 32);

// Sets a computer up at `destination`, or again: everything to its box running there as one of yours.
async function setUp(tell: Telling, destination: string, name: string, kind: "add" | "setup"): Promise<void> {
  const reach = new Reach(destination, name, tell);
  try {
    tell.step("checking", `Connecting to ${name}`);
    const mine = boxVersion();
    const files = ownFiles();
    if (!files || !mine) throw new Failed(`Kuvo Code on ${ownName()} has no files of its own to give another computer yet. Update Kuvo Code here, then try again.`);
    const answer = await reach.run(scripts.probe(ENGINE_VERSION), { cut: true, most: 120_000 });
    if (answer.status !== 0) throw new Failed(`${name} couldn’t be checked. ${lastLine(answer.stderr)}`.trim());
    const probe = probed(answer.stdout);
    if (probe.updating) throw new Failed(`${name} is updating Kuvo Code itself right now. Try again in a few minutes.`);
    const link = readLink();
    const judged = judgeComputer(foundOf(probe.lines), oursNow(link), appsOf(probe.lines));
    if (judged.verdict === "self") throw new Failed(`That’s ${ownName()} itself. Add another computer.`, false);
    // The same computer, here already under another address: that address is kept as one of its own.
    const saved = savedMachines();
    if (!saved) throw new Failed(`${ownName()}’s list of machines can’t be read right now.`);
    // (By its id there, and its box's own key where its card says it: a copy of another machine's
    // disk has that machine's id, and is a computer of its own.)
    const found = foundOf(probe.lines).find(box => !box.app);
    const sameID = kind === "add" && probe.id ? saved.find(each => each.identity === probe.id && each.destination !== destination) : undefined;
    const itsPeer = sameID ? peersOf(link).find(each => each.id === sameID.destination) : undefined;
    const same = sameID && (!found?.card?.self.key || !itsPeer || found.card.self.key === itsPeer.key) ? sameID : undefined;
    if (same) {
      const peer = itsPeer;
      writePrivate(LIST_FILE, JSON.stringify(saved.map(each => (each === same ? { ...each, otherAddresses: [...new Set([...(each.otherAddresses ?? []), destination])] } : each)), null, 2));
      for (const id of ["files", "engine", "starting", "seeding", "joining"]) tell.skip(id, `It’s ${peer?.name ?? same.destination}, already one of your computers`);
      tell.rename(peer?.name ?? same.destination);
      if (peer) tell.came({ key: peer.key, name: peer.name }, true);
      return;
    }
    if (judged.verdict !== "free" && judged.verdict !== "ours" && !judged.joins) {
      const theirs = judged.boxes.find(box => box.verdict !== "free" && box.verdict !== "ours");
      const whose = plainName(theirs?.primary?.name) || plainName(theirs?.name);
      const answer = await tell.ask({
        id: "inAnotherSetup",
        title: `${name} already runs Kuvo Code`,
        message: judged.verdict === "older"
          ? `It was set up by ${whose || "another Mac"}, with an older Kuvo Code. Adding it here updates it, and both use it from then on: its sessions and bots stay.`
          : `It’s one of ${whose ? `${whose}’s` : "another setup’s"} computers. Adding it here shares it between the two: its sessions and bots stay, and it keeps its own settings and sign-ins.`,
        facts: [{ label: "Address", value: destination }, ...(whose ? [{ label: "Set up by", value: whose }] : [])],
        answers: [{ id: "cancel", label: "Cancel", kind: "plain" }, { id: "add", label: "Add Anyway", kind: "primary" }],
      });
      if (answer !== "add") {
        stopFrom(tell);
        throw new Stopped();
      }
      tell.check();
    }

    // Its files, unless it has a newer Kuvo Code (which keeps its own) or has these already.
    const newer = newerVersion(probe.version, mine);
    if (newer) {
      tell.skip("files", `It has a newer Kuvo Code (${probe.version}), left as it is`);
      tell.skip("engine", "Its own");
    } else {
      tell.step("files");
      const has = await reach.run(scripts.filesCheck(files), { cut: true });
      if (has.status === 0 && has.stdout.trim() === "same") tell.skip("files", "It has them already");
      else {
        const placed = await reach.run(scripts.filesInstall(files), { input: tarOf(files), sent: tell.sent, cut: true, most: 900_000 });
        if (placed.status !== 0) throw new Failed(`Kuvo Code’s files couldn’t be put on ${name}. ${lastLine(placed.stderr)}`.trim());
      }
      if (probe.engine) tell.skip("engine", "It has it already");
      else {
        tell.step("engine");
        const engine = await engineFor(probe.kind, () => tell.step("engine", `Downloading it to ${ownName()} first`)).catch(error => { throw new Failed(error instanceof Error ? error.message : String(error)); });
        tell.step("engine", megabytes(engine.size));
        const placed = await reach.run(scripts.engineInstall(ENGINE_VERSION, engine.sum, engine.packed), { input: { file: engine.file }, sent: tell.sent, cut: true, most: 3_600_000 });
        if (placed.status !== 0) throw new Failed(`The engine couldn’t be put on ${name}. ${lastLine(placed.stderr)}`.trim());
      }
    }

    // Its start, by its own keeper: never cut short once it's begun.
    tell.step("starting");
    const own = link ? ownKey(link) : null;
    const notes = JSON.stringify({ mac: own ? launcherID(own) : "", macName: ownName().replace(/[{}"\\]/g, ""), box: mine });
    // A computer with nothing of Kuvo Code on it: no box found there, no Kuvo Code app's folder
    // either, judged free, and being added (never set up again). Only such a one is given a copy
    // of this box's keys and sign-ins, and only such a one is taken off again if this doesn't end
    // with it one of your computers: a Mac with Kuvo Code's app, or any box, is someone's already.
    // (Nor one that ever had Kuvo Code: its id or its version there says so, though its box is off
    // or was uninstalled there, which keeps its sign-ins and keys. Those are never ours to delete.)
    const fresh = kind === "add" && judged.verdict === "free" && foundOf(probe.lines).length === 0 && appsOf(probe.lines) === 0 && !probe.id && !probe.version;
    // (What it starts with goes with its start, on its standard input: this box's settings and
    // integrations' keys.)
    const given = fresh ? await gatherSeed() : null;
    // Started there, on a computer that had nothing, and not one of yours yet: whatever ends this
    // before it is (stopped, or failed), it's stopped there and what was copied is deleted.
    let unjoined = false;
    try {
      unjoined = fresh;
      const started = await reach.run(scripts.start(ENGINE_VERSION, name, notes), { most: 600_000, ...(given ? { input: Buffer.from(JSON.stringify({ settings: given.settings, keys: given.keys })) } : {}) });
      if (started.status === 7) throw new Failed(`${name} was removed from Kuvo Code while it was starting, so it wasn’t started. Add it again to use it.`);
      if (started.status === 6) throw new Failed(`${name}’s settings couldn’t be made ready. ${lastLine(started.stderr)}`.trim());
      if (started.status !== 0) throw new Failed(`Kuvo Code didn’t start on ${name}. ${lastLine(started.stderr)}`.trim(), true, `Kuvo Code didn’t start on ${name}`);
      // Its accounts, once: the keys and the sign-ins that stay the same. Never what renews itself.
      const renewing = given?.renewing ? ` ${given.renewing === 1 ? "One sign-in renews itself" : `${given.renewing} sign-ins renew themselves`}, so sign in to ${given.renewing === 1 ? "it" : "those"} on ${name}.` : "";
      if (!given) tell.skip("seeding", "It keeps its own");
      else {
        tell.step("seeding");
        const settings = /^seeded$/m.test(started.stdout);
        let taken = 0;
        if (given.credentials.length) {
          const ran = await reach.run(scripts.seed(ENGINE_VERSION), { input: Buffer.from(JSON.stringify({ credentials: given.credentials })), most: 300_000 });
          let said: unknown = null;
          try { said = JSON.parse(ran.stdout.trim().split("\n").pop() ?? ""); } catch {}
          taken = isObject(said) && typeof said.taken === "number" ? said.taken : 0;
        }
        const keys = settings ? Object.keys(given.keys).length : 0;
        const copied = [keys ? `${keys} ${keys === 1 ? "key" : "keys"}` : "", taken ? `${taken} ${taken === 1 ? "account" : "accounts"}` : ""].filter(Boolean).join(" and ");
        tell.skip("seeding", `${copied ? `Copied ${copied}.` : settings ? "Copied your settings." : "Nothing to copy."}${renewing}`);
      }
      tell.step("joining");
      const identity = /^id ([0-9a-f]{32})$/m.exec(started.stdout)?.[1] ?? probe.id;
      const computer = await join(reach, destination, name, identity);
      unjoined = false;
      tell.came(computer);
    } catch (error) {
      if (!unjoined) throw error;
      // (Run though the try was stopped: this is its own tidying up.)
      // (With no key of this box's to let go there, as when its link has none: undone all the same.)
      const undone = await reach.run(scripts.uninstall(ENGINE_VERSION, own ?? "", true), { most: 600_000, anyway: true }).catch(() => null);
      const still = `Kuvo Code may still be running on ${name}, with the keys and sign-ins copied there. Add ${name} again, then uninstall it from its page.`;
      if (error instanceof Stopped || tell.signal.aborted) {
        if (undone?.status !== 0) throw new Failed(`This was stopped after Kuvo Code had started on ${name}, and it couldn’t be stopped there again. ${still}`, true, `${name} is still running Kuvo Code`);
        throw error;
      }
      const failure = error instanceof Failed ? error : new Failed(error instanceof Error && error.message ? error.message : "Something went wrong.");
      throw new Failed(`${failure.message} ${undone?.status === 0 ? `Kuvo Code was stopped on ${name} again, and the keys and sign-ins copied there were deleted.` : still}`, failure.again, failure.title);
    }
  } finally {
    await reach.close();
  }
}

// Signs the computer into your setup: this box's keys and documents given over SSH (the trust
// between them), its own taken back, and its record written here, for your other computers and
// phones to take from this box.
async function join(reach: Reach, destination: string, name: string, identity: string | null): Promise<{ key: string; name: string }> {
  const relay = macLink.config?.relay;
  const before = peersOf(readLink()).find(peer => peer.id === destination);
  const gift = await setupGift(before?.key ?? null);
  if (!relay || !gift?.mac) throw new Failed(`${ownName()} isn’t linked to your other computers yet, so it couldn’t add ${name}.`);
  const input = { relay, name, mac: gift.mac, macSign: gift.macSign, docs: gift.docs, id: destination, tolerates: 1, macID: launcherID(gift.mac), macName: ownName(), replaces: gift.replaces };
  const ran = await reach.run(scripts.join(ENGINE_VERSION), { input: Buffer.from(JSON.stringify(input)), most: 300_000 });
  let said: unknown = null;
  try { said = JSON.parse(ran.stdout.trim().split("\n").pop() ?? ""); } catch {}
  if (ran.status !== 0 || !isObject(said) || typeof said.address !== "string" || !ADDRESS.test(said.address) || typeof said.key !== "string" || !KEY.test(said.key) || typeof said.sign !== "string" || !SIGN.test(said.sign)) {
    throw new Failed(`Kuvo Code on ${name} couldn’t join your computers. ${lastLine(ran.stderr)}`.trim());
  }
  const peer = { key: said.key, sign: said.sign, address: said.address, name, id: destination };
  // Its record: on this box's list of machines first (which a peer it removed before is taken back by).
  const saved = savedMachines();
  if (!saved) throw new Failed(`${ownName()}’s list of machines can’t be read right now.`);
  const had = saved.find(each => each.destination === destination);
  writePrivate(LIST_FILE, JSON.stringify(had ? saved.map(each => (each === had && identity ? { ...each, identity } : each)) : [...saved, { destination, ...(identity ? { identity } : {}) }], null, 2));
  keepHost(destination, { name, address: peer.address, key: peer.key, sign: peer.sign });
  let named = name;
  const changed = updateLink(link => {
    const state = link ?? { phones: [] };
    // (Under its id here, or its key, once: set up again with keys made anew, it's the same machine.)
    const others = (Array.isArray(state.macPeers) ? state.macPeers : []).filter(each => isObject(each) && each.key !== peer.key && each.id !== destination);
    return setMacPeers(state, { peers: [...others, peer], moved: state.macMoved }).link;
  });
  if (!changed || !peersOf(changed).some(each => each.key === peer.key)) throw new Failed(`${ownName()} couldn’t keep ${name}’s record.`);
  named = peersOf(changed).find(each => each.key === peer.key)?.name ?? name;
  // This box meets its machines on the relay from now on.
  if (macLink.config) macLink.config = { ...macLink.config, machines: true };
  macLinkCheck();
  macLink.wake?.();
  macDocsChanged();
  // What it has of documents (its own, signed with the key it just gave), taken as if by mail.
  if (Array.isArray(said.docs) && said.docs.length) await receiveDocs(peer, { give: said.docs }, macLink.host).catch(() => {});
  return { key: peer.key, name: named };
}

function begin(kind: AddTry["kind"], destination: string, name: string, steps: readonly (readonly [string, string])[], work: (tell: Telling) => Promise<void>): Running {
  let it: Running | null = null;
  const made = startTry(kind, destination, name, steps, tell => {
    stoppers.set(tell, () => { if (it) stopTry(it); });
    return work(tell);
  });
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
export function beginAdd(destination: string): Running | Busy {
  const now = underWay(destination);
  return uninstalling(now) ?? now ?? begin("add", destination, destination, SETUP_STEPS, tell => setUp(tell, destination, destination, "add"));
}

/** The address this box set the computer `key` up at, and its name: null for one it didn't. */
export function sshAddressOf(key: string): { destination: string; name: string } | null {
  const peer = peersOf(readLink()).find(each => each.key === key);
  if (!peer?.id || addressProblem(peer.id) || !(savedMachines() ?? []).some(each => each.destination === peer.id)) return null;
  return { destination: peer.id, name: peer.name };
}

/** Begins setting the computer `key` up again, at the address it was added under: the same steps,
 * which bring an older Kuvo Code there up to this one's. */
export function beginSetup(key: string): Running | Busy | null {
  const at = sshAddressOf(key);
  if (!at) return null;
  const now = underWay(at.destination);
  return uninstalling(now) ?? now ?? begin("setup", at.destination, at.name, AGAIN_STEPS, tell => setUp(tell, at.destination, at.name, "setup"));
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
    const reach = new Reach(at.destination, at.name, tell);
    try {
      tell.step("checking", `Connecting to ${at.name}`);
      const answer = await reach.run(scripts.probe(ENGINE_VERSION), { cut: true, most: 120_000 });
      if (answer.status !== 0) throw new Failed(`${at.name} couldn’t be checked. ${lastLine(answer.stderr)}`.trim());
      const link = readLink();
      const own = link ? ownKey(link) : null;
      if (!own) throw new Failed(`${ownName()}’s link has no keys yet.`);
      // Another Mac that set it up still uses it: Kuvo Code stays there for that one.
      const box = foundOf(answer.stdout.split("\n").map(line => line.trim())).find(each => !each.app);
      const macs = new Set([...(box?.card?.macs ?? []).map(mac => mac.key), ...(box?.link?.macs ?? [])]);
      macs.delete(own);
      const only = macs.size === 0;
      tell.step("removing", only ? undefined : "Another Mac uses it too, so Kuvo Code stays there");
      // (Never cut short: a removal half done would leave it running with nothing to start it.)
      const ran = await reach.run(scripts.uninstall(ENGINE_VERSION, own, only), { most: 600_000 });
      if (ran.status !== 0) {
        let said: unknown = null;
        try { said = JSON.parse(ran.stdout.trim().split("\n").pop() ?? ""); } catch {}
        throw new Failed(isObject(said) && typeof said.message === "string" ? plainName(said.message).slice(0, 400) : `Kuvo Code couldn’t be stopped on ${at.name}. ${lastLine(ran.stderr)}`.trim());
      }
      tell.step("forgetting");
      const removed = removeHere(key);
      // (Gone from the list already, as when it was asked twice: done all the same.)
      if (removed.status !== 200 && removed.status !== 404) throw new Failed(isObject(removed.value) && typeof removed.value.message === "string" ? removed.value.message : `${ownName()} couldn’t take ${at.name} off its list.`);
    } finally {
      await reach.close();
    }
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
  const why = busy ? (busy.seen.kind === "uninstall" ? "Kuvo Code is being uninstalled from it now." : "It’s being set up now.") : null;
  return [
    { id: "setup", label: "Set Up Again", ...(why ? { why } : {}) },
    { id: "uninstall", label: "Uninstall", ...(busy?.seen.kind === "uninstall" ? { why: why! } : { confirm: { title: `Uninstall Kuvo Code from ${name}?`, message: `${here} connects to ${name} over SSH and stops Kuvo Code there: its engine and agents, and what starts them after a restart. The sign-ins and keys it keeps are deleted.${vouchedWords(readLink(), computer.key, name)} Its sessions stay in ~/.kuvo-code on ${name}.`, label: "Uninstall" } }) },
  ];
}
