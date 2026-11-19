// The sign-ins an earlier Kuvo Code saved before there was a vault (Keychain internet passwords it
// marked as its own), taken into this Mac's vault when you ask (#713 §3, `owed: passwords`;
// PROTOCOL.md, "Moving this Mac's box"). Only `kuvo-keys` can read them, and only for the process
// that holds the move's lock: it writes them to its standard output, a pipe this process reads, and
// each goes from memory to the box's own server on this Mac (the vault's routes, as a client adds
// an item). They're never written to a file, by either.
//
// Reading them may have macOS ask you about each one (Always Allow), so it's never part of the
// move, nor of opening or updating Kuvo: a task of its own (`rehome --sign-ins`), which Kuvo on
// this Mac starts when you ask it to, having said that macOS will ask (`POST kuvo/box/rehome
// {signIns: true}`). It holds the move's lock while it runs, which nothing that opens or updates
// Kuvo waits for (`leftoverHolder`).
//
// As the app's own hand-over went (`Passwords.swift`, read only): each a sign-in of its site, to
// sign in with only; one whose site and account the vault has already is left as it is (so a
// hand-over cut short adds nothing twice); a site you let agents sign in to by themselves, with
// that account, stays so. Each Keychain original goes only after its vault value is read back.
// Platform: a Mac's alone.
// COMPAT(0.7 move): takes the sign-ins Kuvo Code 0.5 to 0.7.2 and 0.8.0-beta.1 saved in the Keychain into the vault, from kuvo-keys. Goes on the user's word, once no 0.7 Mac is expected to update.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { isObject } from "../../shared/json";
import { sleep } from "../../shared/wait";
import { launchTools } from "../uninstall";
import { locked, NOTHING_TO_MOVE, type RehomeOutcome } from "./move";
import { keysOf, movedHere } from "./run";
import { noteRecord, readRecord, rehomeStatus, SIGN_INS_PID } from "./state";

interface Login { host: string; account: string; password: string }

/** How it went: `done` once every sign-in is in the vault or said; `why` where it's to be tried
 * again; how many were added, those the vault can't keep (`short`: under its four characters), and
 * those left in the Keychain as they are (`kept`: the vault has another password for that site and
 * account or refuses it, two Keychain sign-ins share them, one's password couldn't be read
 * exactly, or it changed meanwhile). */
export interface HandedOver { done: boolean; why?: string; reason?: Reason; added: number; short: number; kept: number }
/** Why it wasn't done, for the words you're shown: the Keychain locked; macOS kept them from
 * kuvo-keys (you clicked Deny, or it wasn't allowed to ask); its question went unanswered; or
 * anything else. */
type Reason = "locked" | "not-allowed" | "unanswered" | "failed";

/** Where the box's server answers on this Mac (`run/serve.json`), waited for a while: it starts
 * a moment after its helper does. */
async function serverOf(home: string, wait: number): Promise<{ url: string; authorization: string } | null> {
  for (const began = Date.now(); ;) {
    try {
      const said: unknown = JSON.parse(fs.readFileSync(path.join(home, "run", "serve.json"), "utf8"));
      if (isObject(said) && Number.isInteger(said.port) && typeof said.password === "string") {
        return { url: `http://127.0.0.1:${said.port}`, authorization: "Basic " + Buffer.from(`opencode:${said.password}`).toString("base64") };
      }
    } catch {}
    if (Date.now() - began >= wait) return null;
    await sleep(500);
  }
}

/** The one word kuvo-keys gives for why it failed (`{"reason": …}`, its last line). */
const reasonOf = (said: string | null | undefined): string => {
  try { const answer: unknown = JSON.parse((said ?? "").trim().split("\n").pop() ?? ""); return isObject(answer) && typeof answer.reason === "string" ? answer.reason : ""; } catch { return ""; }
};

/** A name no item has: the site's, then with its account, then numbered. */
function freeName(host: string, account: string, taken: Set<string>): string {
  const free = (name: string) => !taken.has(name.toLowerCase());
  if (free(host)) return host;
  if (account && free(`${host} (${account})`)) return `${host} (${account})`;
  for (let n = 2; ; n++) if (free(`${host} ${n}`)) return `${host} ${n}`;
}

/** The earlier Kuvo Code's saved sign-ins, into the vault of the box in `home`. `keys`: the app's
 * `kuvo-keys`; `agentSites` and `agentAccounts`: the sites it let agents sign in to alone, and
 * with which account (its preferences). */
export async function handOverPasswords({ home, keys, agentSites, agentAccounts, wait = 30_000 }: { home: string; keys: string; agentSites: string[]; agentAccounts: Record<string, string>; wait?: number }): Promise<HandedOver> {
  // (macOS may ask you about each sign-in it reads: room for your answers.)
  const read = spawnSync(keys, ["passwords", "--home", home], { encoding: "utf8", timeout: 1_800_000, maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] });
  let said: unknown = null;
  try { said = JSON.parse((read.stdout ?? "").trim().split("\n").pop() ?? ""); } catch {}
  if (read.status !== 0 || !isObject(said) || !Array.isArray(said.logins)) {
    // (Only its one word for why: what it says besides may name a site or an account.)
    const status = isObject(said) && typeof said.status === "number" ? ` OSStatus ${said.status}` : "";
    const word = isObject(said) && typeof said.reason === "string" ? said.reason : null;
    const reason: Reason = word === "locked" || word === "not-allowed" ? word : read.status === null && !word ? "unanswered" : "failed";
    return { done: false, why: `kuvo-keys couldn't read them (${word ?? (read.status === null ? "it didn't answer" : `exit ${read.status}`)}${status})`, reason, added: 0, short: 0, kept: 0 };
  }
  const logins = said.logins.filter((each): each is Login => isObject(each) && typeof each.host === "string" && each.host.length > 0 && typeof each.account === "string" && typeof each.password === "string");
  if (!logins.length) return { done: true, added: 0, short: 0, kept: 0 };
  const server = await serverOf(home, wait);
  if (!server) return { done: false, why: "Kuvo isn't answering on this Mac yet", added: 0, short: 0, kept: 0 };
  const ask = async (method: string, route: string, body?: unknown): Promise<{ status: number; value: Record<string, unknown> }> => {
    const answer = await fetch(`${server.url}/${route}`, { method, headers: { authorization: server.authorization, "content-type": "application/json", "x-kuvo-from": "app", "x-kuvo-internal": "1" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(60_000) });
    let value: unknown = null;
    try { value = await answer.json(); } catch {}
    return { status: answer.status, value: isObject(value) ? value : {} };
  };
  let added = 0, short = 0, kept = 0;
  const keyOf = (login: Login) => JSON.stringify([login.host, login.account]);
  const seen = new Set<string>(), unsure = new Set<string>();
  for (const login of logins) (seen.has(keyOf(login)) ? unsure : seen).add(keyOf(login));
  try {
    const vault = await ask("GET", "kuvo/vault");
    if (vault.status !== 200) return { done: false, why: `the vault answered ${vault.status}`, added, short, kept };
    const items = (Array.isArray(vault.value.items) ? vault.value.items : []).filter(isObject);
    const taken = new Set(items.map(item => String(item.name ?? "").toLowerCase()));
    const itemOf = (login: Login) => items.find(item => item.kind === "login" && Array.isArray(item.sites) && item.sites.includes(login.host) && (typeof item.username === "string" ? item.username : "") === login.account);
    const granted = new Set((Array.isArray(vault.value.grants) ? vault.value.grants : []).filter(isObject).filter(grant => isObject(grant.who) && grant.who.everyone === true).map(grant => grant.item));
    for (const login of logins) {
      // Kept in the Keychain, never given to the vault: a password kuvo-keys couldn't say exactly
      // (not valid UTF-8: read with U+FFFD in its place), and sign-ins that share a site and
      // account (the Keychain finds one by them, so which is which can't be told).
      if (login.password.includes("\uFFFD") || unsure.has(keyOf(login))) { kept++; continue; }
      let item = itemOf(login);
      if (!item) {
        // (The vault keeps no value under four characters: it couldn't hide one from agents.)
        if (login.password.length < 4) { short++; continue; }
        const name = freeName(login.host, login.account, taken);
        const made = await ask("POST", "kuvo/vault/items", { kind: "login", name, sites: [login.host], ...(login.account ? { username: login.account } : {}), signInOnly: true, values: { password: login.password } });
        // (One the vault refuses, a site it can't keep or a value too long, stays in the Keychain.)
        if (made.status === 400) { kept++; continue; }
        item = isObject(made.value.item) ? made.value.item : undefined;
        if (made.status !== 200 || !item || typeof item.id !== "string") return { done: false, why: `the vault answered ${made.status} to a sign-in`, added, short, kept };
        added++;
        taken.add(name.toLowerCase());
        items.push(item);
      }
      // The vault's value checked against this one, including on retries: a matching site and
      // account alone isn't proof it's this password. Asked without its being shown (`same` notes
      // nothing in the vault's activity). One the vault keeps differently (it had another for that
      // site and account already) keeps its original in the Keychain, and the rest go on.
      const checked = await ask("POST", `kuvo/vault/items/${encodeURIComponent(String(item.id))}/same`, { values: { password: login.password } });
      if (checked.status !== 200 || typeof checked.value.same !== "boolean") return { done: false, why: `the vault answered ${checked.status} to checking a sign-in`, added, short, kept };
      if (!checked.value.same) { kept++; continue; }
      // A site agents signed in to by themselves, with that account, stays so: asked again where
      // a hand-over cut short left the sign-in without it.
      if (agentSites.includes(login.host) && agentAccounts[login.host] === login.account && !granted.has(item.id)) {
        const grant = await ask("POST", "kuvo/vault/grants", { item: item.id, who: { everyone: true }, how: "own" });
        if (grant.status !== 200) return { done: false, why: `the vault answered ${grant.status} to letting agents use a sign-in`, added, short, kept };
        granted.add(item.id);
      }
      const removed = spawnSync(keys, ["delete-originals", "--home", home, "--password", login.host, "--account", login.account], { input: JSON.stringify({ password: login.password }), encoding: "utf8", timeout: 180_000, maxBuffer: 1024 * 1024, stdio: ["pipe", "pipe", "pipe"] });
      // (One that's another password in the Keychain now, `copy-missing`, is kept as it is.)
      if (removed.status !== 0 && reasonOf(removed.stdout) === "copy-missing") { kept++; continue; }
      if (removed.status !== 0) return { done: false, why: "a saved sign-in's Keychain copy couldn't be removed", added, short, kept };
    }
  } catch (error) {
    return { done: false, why: (error as Error).message, added, short, kept };
  }
  return { done: true, added, short, kept };
}

/** What you're shown when bringing them over didn't finish, by why (nothing that names a site,
 * an account or a code: those go to the log, for support). */
const NOT_BROUGHT: Record<Reason | "no-app", string> = {
  locked: "Your Mac's Keychain is locked. Unlock it, then try again.",
  "not-allowed": "macOS didn't let Kuvo read them. Try again: when macOS asks whether kuvo-keys may use each one, enter your Mac's password and click Always Allow.",
  unanswered: "macOS's question wasn't answered, so nothing more was brought over. Try again when you're at this Mac.",
  failed: "Kuvo couldn't bring them all over this time. Try again.",
  "no-app": "Kuvo couldn't find its app on this Mac. Open Kuvo, then try again.",
};

/** `rehome --sign-ins`: an earlier Kuvo Code's saved sign-ins brought into the vault of the box a
 * move took them to, now, as you asked; macOS may ask you about each. Each that's in the vault
 * and read back there leaves the Keychain (you decided so, 2026-11-16); one the vault can't keep
 * exactly stays, and is said. How it went is kept for Kuvo to show (`signInsLast`), and the hand-over
 * is no longer owed once it's done. Nothing where nothing's owed. */
export async function rehomeSignIns(home: string): Promise<RehomeOutcome> {
  if (process.platform !== "darwin") return { result: "refused", message: NOTHING_TO_MOVE, status: rehomeStatus(home) };
  const owed = () => movedHere(home) && readRecord(home).owed?.includes("passwords") === true;
  let handed: HandedOver | null = null;
  let reason: Reason | "no-app" = "failed";
  try {
    if (!owed()) return { result: "nothing", status: rehomeStatus(home) };
    let outcome: RehomeOutcome;
    try {
      outcome = await locked(home, () => console.error("Another move is under way: this one waits for it."), async () => {
      if (!owed()) return { result: "nothing", status: rehomeStatus(home) };
      const keys = keysOf(home);
      if (!keys) {
        reason = "no-app";
        handed = { done: false, why: "kuvo-keys isn't in the app on this Mac", added: 0, short: 0, kept: 0 };
        return { result: "failed", status: rehomeStatus(home) };
      }
      const record = readRecord(home);
      const prefs = typeof record.from === "string" ? (await import("./upgrades")).appPreferences(record.from) : {};
      const sites = Array.isArray(prefs["passwords.agentSignIn"]) ? prefs["passwords.agentSignIn"].filter((each): each is string => typeof each === "string") : [];
      const accounts = isObject(prefs["passwords.agentSignInAccounts"]) ? Object.fromEntries(Object.entries(prefs["passwords.agentSignInAccounts"]).filter((each): each is [string, string] => typeof each[1] === "string")) : {};
      // (Where nothing starts the box, a copy's home, its server is waited for no longer than it takes to look.)
      handed = await handOverPasswords({ home, keys, agentSites: sites, agentAccounts: accounts, ...(launchTools() ? {} : { wait: 0 }) });
      if (!handed.done) reason = handed.reason ?? "failed";
      return { result: handed.done ? "done" : "failed", status: rehomeStatus(home) };
    });
    } catch (error) {
      // (Cut short by something unforeseen: said as a try that didn't finish, never left unsaid.)
      outcome = { result: "failed", status: rehomeStatus(home) };
      handed = { done: false, why: (error as Error).message, added: 0, short: 0, kept: 0 };
    }
    // (One that never got to ask, the lock held by another past its wait: said too.)
    if (!handed && outcome.result === "waits") handed = { done: false, why: "another move held the lock", added: 0, short: 0, kept: 0 };
    if (!handed) return outcome;
  } finally {
    // (The box's note that this runs, gone with it: before how it went is kept, so the two are
    // never said at once.)
    try { if (Number(fs.readFileSync(SIGN_INS_PID(home), "utf8")) === process.pid) fs.rmSync(SIGN_INS_PID(home), { force: true }); } catch {}
  }
  const result = handed as HandedOver;
  if (!result.done) {
    // (Why goes to the move's own log, for support: it may name a site or an account.)
    console.error(`Your saved sign-ins couldn't be brought into the vault: ${result.why ?? "it didn't say why"}`);
    noteRecord(home, { signInsLast: { state: "failed", at: Date.now(), message: NOT_BROUGHT[reason] } });
    return { result: "failed", message: NOT_BROUGHT[reason], status: rehomeStatus(home) };
  }
  const said: string[] = [];
  if (result.short) said.push(`${result.short === 1 ? "A saved password" : `${result.short} saved passwords`} shorter than four characters ${result.short === 1 ? "wasn't" : "weren't"} brought into the vault, which keeps none that short.`);
  if (result.kept) said.push(`${result.kept === 1 ? "A saved password stays" : `${result.kept} saved passwords stay`} in your Keychain: Kuvo couldn't be sure the vault would keep ${result.kept === 1 ? "it" : "them"} exactly. Add any you use to the vault yourself.`);
  // (How it went and that they're owed no more, in one: never one said without the other.)
  noteRecord(home, now => ({
    dropped: [...(now.dropped ?? []), ...said], owed: (now.owed ?? []).filter(what => what !== "passwords"),
    signInsLast: { state: "done", at: Date.now(), added: result.added, kept: result.kept + result.short },
  }));
  return { result: "done", status: rehomeStatus(home) };
}
