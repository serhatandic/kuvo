// The sign-ins an earlier Kuvo Code saved before there was a vault (Keychain internet passwords it
// marked as its own), taken into this Mac's vault once its server answers (#713 §3, `owed:
// passwords`; PROTOCOL.md, "Moving this Mac's box"). Only `kuvo-keys` can read them, and only for
// the process that holds the move's lock: it writes them to its standard output, a pipe this
// process reads, and each goes from memory to the box's own server on this Mac (the vault's
// routes, as a client adds an item). They're never written to a file, by either.
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

interface Login { host: string; account: string; password: string }

/** How it went: `done` once every sign-in is in the vault or said; `why` where it's to be tried
 * again; how many were added, and those the vault can't keep (`short`: under its four characters). */
export interface HandedOver { done: boolean; why?: string; added: number; short: number }

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
  const read = spawnSync(keys, ["passwords", "--home", home], { encoding: "utf8", timeout: 180_000, maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] });
  let said: unknown = null;
  try { said = JSON.parse((read.stdout ?? "").trim().split("\n").pop() ?? ""); } catch {}
  if (read.status !== 0 || !isObject(said) || !Array.isArray(said.logins)) {
    // (Only its one word for why: what it says besides may name a site or an account.)
    return { done: false, why: `kuvo-keys couldn't read them (${isObject(said) && typeof said.reason === "string" ? said.reason : read.status === null ? "it didn't answer" : `exit ${read.status}`})`, added: 0, short: 0 };
  }
  const logins = said.logins.filter((each): each is Login => isObject(each) && typeof each.host === "string" && each.host.length > 0 && typeof each.account === "string" && typeof each.password === "string");
  if (!logins.length) return { done: true, added: 0, short: 0 };
  const server = await serverOf(home, wait);
  if (!server) return { done: false, why: "Kuvo isn't answering on this Mac yet", added: 0, short: 0 };
  const ask = async (method: string, route: string, body?: unknown): Promise<{ status: number; value: Record<string, unknown> }> => {
    const answer = await fetch(`${server.url}/${route}`, { method, headers: { authorization: server.authorization, "content-type": "application/json", "x-kuvo-from": "app" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(60_000) });
    let value: unknown = null;
    try { value = await answer.json(); } catch {}
    return { status: answer.status, value: isObject(value) ? value : {} };
  };
  let added = 0, short = 0;
  try {
    const vault = await ask("GET", "kuvo/vault");
    if (vault.status !== 200) return { done: false, why: `the vault answered ${vault.status}`, added, short };
    const items = (Array.isArray(vault.value.items) ? vault.value.items : []).filter(isObject);
    const taken = new Set(items.map(item => String(item.name ?? "").toLowerCase()));
    const itemOf = (login: Login) => items.find(item => item.kind === "login" && Array.isArray(item.sites) && item.sites.includes(login.host) && (typeof item.username === "string" ? item.username : "") === login.account);
    const granted = new Set((Array.isArray(vault.value.grants) ? vault.value.grants : []).filter(isObject).filter(grant => isObject(grant.who) && grant.who.everyone === true).map(grant => grant.item));
    for (const login of logins) {
      let item = itemOf(login);
      if (!item) {
        // (The vault keeps no value under four characters: it couldn't hide one from agents.)
        if (login.password.length < 4) { short++; continue; }
        const name = freeName(login.host, login.account, taken);
        const made = await ask("POST", "kuvo/vault/items", { kind: "login", name, sites: [login.host], ...(login.account ? { username: login.account } : {}), signInOnly: true, values: { password: login.password } });
        item = isObject(made.value.item) ? made.value.item : undefined;
        if (made.status !== 200 || !item || typeof item.id !== "string") return { done: false, why: `the vault answered ${made.status} to a sign-in`, added, short };
        added++;
        taken.add(name.toLowerCase());
        items.push(item);
      }
      // Read the stored secret, including on retries: a matching site/account alone isn't proof
      // that this password is safe. Keep a different original in the Keychain.
      const checked = await ask("GET", `kuvo/vault/items/${encodeURIComponent(String(item.id))}/values`);
      if (checked.status !== 200 || !isObject(checked.value.values) || checked.value.values.password !== login.password) {
        return { done: false, why: "a saved sign-in couldn't be confirmed in the vault", added, short };
      }
      // A site agents signed in to by themselves, with that account, stays so: asked again where
      // a hand-over cut short left the sign-in without it.
      if (agentSites.includes(login.host) && agentAccounts[login.host] === login.account && !granted.has(item.id)) {
        const grant = await ask("POST", "kuvo/vault/grants", { item: item.id, who: { everyone: true }, how: "own" });
        if (grant.status !== 200) return { done: false, why: `the vault answered ${grant.status} to letting agents use a sign-in`, added, short };
        granted.add(item.id);
      }
      const removed = spawnSync(keys, ["delete-originals", "--home", home, "--password", login.host, "--account", login.account], { input: JSON.stringify({ password: login.password }), encoding: "utf8", timeout: 180_000, maxBuffer: 1024 * 1024, stdio: ["pipe", "pipe", "pipe"] });
      if (removed.status !== 0) return { done: false, why: "a saved sign-in's Keychain copy couldn't be removed", added, short };
    }
  } catch (error) {
    return { done: false, why: (error as Error).message, added, short };
  }
  return { done: true, added, short };
}
