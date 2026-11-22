import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import { Readable } from "node:stream";
import path from "node:path";
import { auth, extractWWWAuthenticateParams, type OAuthClientProvider, type OAuthDiscoveryState } from "@modelcontextprotocol/sdk/client/auth.js";
import type { OAuthClientInformationMixed, OAuthTokens } from "@modelcontextprotocol/sdk/shared/auth.js";
import { isObject } from "../../shared/json";
import { locked, placeWhole } from "../../shared/files";
import { addItem, changeItem, isLocked, isMade, itemNamed, pausedFor, readItems, removeItem, valuesOf, VaultRefusal } from "../vault/store";
import { changeSettings, ownsSettings, readSettings, SERVERS, sortedKeys, valueAt } from "../areas/config";
import { installations, no, type InstalledApp } from "./installations";
import { marketplaceFolder, sha256 } from "./catalog";

import { MARKETPLACE_SERVER_PREFIX as prefix } from "../../shared/marketplace";
export const serverName = (id: string) => prefix + id;
const credentialsName = (app: InstalledApp) => `Marketplace · ${app.id} · ${sha256(Buffer.from(app.url)).slice(0, 16)}`;
const callbackPath = "/marketplace/oauth/callback";
const endpointFile = path.join(marketplaceFolder, "mcp-relay-v1.json");
const appIndex = path.join(marketplaceFolder, "mcp-projection-v1.json");
const newKey = () => crypto.randomBytes(24).toString("base64url");
interface Relay { port: number; key: string }
let relay: Relay | undefined;

function storedRelay(): Relay | undefined {
  try {
    const value = JSON.parse(fs.readFileSync(endpointFile, "utf8"));
    if (Number.isSafeInteger(value?.port) && value.port > 0 && value.port <= 65535 && /^[A-Za-z0-9_-]{32}$/.test(value.key))
      return { port: value.port, key: value.key };
  } catch {}
  return undefined;
}
const localhost = (port: number) => `http://127.0.0.1:${port}`;
const appKey = (key: string, app: InstalledApp) => crypto.createHmac("sha256", key).update(app.id + "\0" + app.url).digest("base64url");

export function connectionState(app: InstalledApp): "ready" | "sign-in" | "locked" {
  if (app.auth === "open") return "ready";
  if (!isMade() || isLocked()) return "locked";
  try { return itemNamed(credentialsName(app)) ? "ready" : "sign-in"; }
  catch { return "locked"; }
}
async function keepCredential(app: InstalledApp, record: Record<string, string>) {
  const given = { kind: "key", name: credentialsName(app), sites: [new URL(app.url).hostname], values: record };
  const before = itemNamed(given.name);
  if (before) await changeItem(before.id, given);
  else await addItem(given);
}
export async function disconnect(app: InstalledApp) {
  if (app.auth === "open") return;
  if (isLocked()) throw new VaultRefusal(423, "Unlock this computer’s vault to disconnect the app.");
  const item = itemNamed(credentialsName(app));
  if (item) await removeItem(item.id);
}
export async function connectToken(app: InstalledApp, token: unknown) {
  if (typeof token !== "string" || token.length < 4 || token.length > 8192 || /[^!-~]/.test(token))
    throw new VaultRefusal(400, "Enter the personal access token issued by the service.");
  await keepCredential(app, { token });
}

interface Flow {
  app: InstalledApp;
  redirect: string;
  expires: number;
  state: string;
  scope?: string;
  wait?: Promise<void>;
  done?: true;
  authorization?: URL;
  client?: OAuthClientInformationMixed;
  discovery?: OAuthDiscoveryState;
  verifier?: string;
  tokens?: OAuthTokens;
  expiry?: number;
}
const flows = new Map<string, Flow>();
const renewable = new Map<string, Promise<string>>();
const network: typeof fetch = ((input: URL | string | Request, init?: RequestInit) => {
  const url = new URL(input instanceof Request ? input.url : String(input));
  if (url.protocol !== "https:" || url.username || url.password) throw new Error("The service must use HTTPS for sign-in.");
  return fetch(input, { ...init, redirect: "error", signal: AbortSignal.timeout(15_000) });
}) as typeof fetch;

function provider(flow: Flow): OAuthClientProvider {
  return {
    redirectUrl: flow.redirect,
    clientMetadata: { client_name: "Kuvo Code", redirect_uris: [flow.redirect], grant_types: ["authorization_code", "refresh_token"],
      response_types: ["code"], token_endpoint_auth_method: "none" },
    state: () => flow.state,
    clientInformation: () => flow.client,
    saveClientInformation: client => { flow.client = client; },
    discoveryState: () => flow.discovery,
    saveDiscoveryState: discovery => { flow.discovery = discovery; },
    tokens: () => flow.tokens,
    saveTokens: tokens => {
      flow.tokens = tokens;
      flow.expiry = tokens.expires_in ? Date.now() + tokens.expires_in * 1000 : undefined;
    },
    redirectToAuthorization: url => {
      if (url.protocol !== "https:" || url.username || url.password) throw new Error("The sign-in page must use HTTPS.");
      flow.authorization = url;
    },
    saveCodeVerifier: verifier => { flow.verifier = verifier; },
    codeVerifier: () => {
      if (!flow.verifier) throw new Error("Start sign-in again.");
      return flow.verifier;
    },
  };
}
const saveFlow = (flow: Flow) => keepCredential(flow.app, { oauth: JSON.stringify({
  tokens: flow.tokens, expiry: flow.expiry, client: flow.client, discovery: flow.discovery, redirect: flow.redirect, scope: flow.scope,
}) });

export async function startSignIn(app: InstalledApp) {
  // Check the vault before opening a page that would otherwise finish without a place to save.
  if (connectionState(app) === "locked") throw new VaultRefusal(423, "Unlock this computer’s vault before signing in.");
  const current = relay;
  if (!current) throw new VaultRefusal(503, "The Marketplace connection service is starting. Try again in a moment.");
  for (const [id, flow] of flows) if (flow.expires <= Date.now()) flows.delete(id);
  if (flows.size >= 20) throw new VaultRefusal(409, "Finish or cancel a sign-in before starting another.");
  const challenge = await network(app.url, { method: "POST", headers: { accept: "application/json, text/event-stream", "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", method: "initialize", id: 1, params: { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "Kuvo Code", version: "1" } } }) });
  const { resourceMetadataUrl, scope: offeredScope } = extractWWWAuthenticateParams(challenge);
  const scope = app.origin === "catalog" && app.id === "notion" ? "default" : offeredScope;
  await challenge.body?.cancel();
  const state = newKey();
  const flow: Flow = { app, state, scope, redirect: localhost(current.port) + callbackPath, expires: Date.now() + 10 * 60_000 };
  const status = await auth(provider(flow), { serverUrl: app.url, resourceMetadataUrl, scope, fetchFn: network });
  if (status !== "REDIRECT" || !flow.authorization) throw new Error("The service didn’t provide a sign-in page.");
  flows.set(state, flow);
  return { attempt: state, opens: flow.authorization.href, title: `Sign in to ${app.title}`,
    callbackLabel: "Paste the address the sign-in page ends on", submit: "Finish Sign-in" };
}

function activeFlow(app: InstalledApp, attempt: unknown): Flow {
  const flow = typeof attempt === "string" ? flows.get(attempt) : undefined;
  if (!flow || flow.app.id !== app.id || flow.app.url !== app.url || flow.expires <= Date.now())
    throw new VaultRefusal(404, "That sign-in has ended. Start again.");
  return flow;
}
export const signInStatus = (app: InstalledApp, attempt: unknown) => ({ state: activeFlow(app, attempt).done ? "connected" : "waiting" });
export const cancelSignIn = (app: InstalledApp, attempt: unknown) => {
  const flow = activeFlow(app, attempt);
  if (flow.wait) throw new VaultRefusal(409, "This sign-in is finishing.");
  flows.delete(flow.state);
};
export async function finishSignIn(app: InstalledApp, attempt: unknown, address: unknown) {
  const flow = activeFlow(app, attempt);
  let url: URL;
  try { url = new URL(String(address)); } catch { throw new VaultRefusal(400, "Paste the full address from the sign-in page."); }
  const callback = new URL(flow.redirect);
  const code = url.searchParams.get("code");
  if (url.origin !== callback.origin || url.pathname !== callback.pathname || url.username || url.password
    || url.searchParams.get("state") !== flow.state || !code || code.length > 8192) throw new VaultRefusal(400, "That address doesn’t match this sign-in.");
  if (flow.done) return;
  await (flow.wait ??= (async () => {
    await auth(provider(flow), { serverUrl: flow.app.url, authorizationCode: code, scope: flow.scope, fetchFn: network });
    if (!flow.tokens?.access_token) throw new Error("The service didn’t return a token.");
    const installed = installations().apps.find(each => each.id === app.id && each.url === app.url);
    if (!installed) throw new VaultRefusal(409, "Add the app again before connecting it.");
    await saveFlow(flow);
    flow.done = true;
  })().finally(() => { flow.wait = undefined; }));
}

async function access(app: InstalledApp): Promise<string> {
  const item = itemNamed(credentialsName(app));
  if (!item || pausedFor(undefined)) throw new Error("Connect this app in Marketplace.");
  const values = await valuesOf(item.id);
  if (app.auth === "token" && values.token) return values.token;
  const kept = JSON.parse(values.oauth ?? "{}") as Partial<Flow>;
  if (kept.tokens?.access_token && (!kept.expiry || kept.expiry > Date.now() + 30_000)) return kept.tokens.access_token;
  if (!kept.tokens?.refresh_token || !kept.redirect) throw new Error("Sign in to this app again.");
  const flow: Flow = { ...kept, app, redirect: kept.redirect, expires: 0, state: "" };
  const key = credentialsName(app);
  const pending = renewable.get(key);
  if (pending) return pending;
  const renewal = (async () => {
    const result = await auth(provider(flow), { serverUrl: app.url, scope: flow.scope, fetchFn: network });
    if (result !== "AUTHORIZED" || !flow.tokens?.access_token) throw new Error("Sign in to this app again.");
    if (!itemNamed(key)) throw new Error("Connect this app in Marketplace.");
    await saveFlow(flow);
    return flow.tokens.access_token;
  })().finally(() => { renewable.delete(key); });
  renewable.set(key, renewal);
  return renewal;
}

export async function startMarketplaceRelay() {
  const before = storedRelay();
  const key = before?.key ?? newKey();
  const requestFromEngine = async (request: http.IncomingMessage, response: http.ServerResponse) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    if (url.pathname === callbackPath && request.method === "GET") {
      const flow = flows.get(url.searchParams.get("state") ?? "");
      try {
        if (!flow || !relay || request.headers.host !== `127.0.0.1:${relay.port}`) throw new Error();
        await finishSignIn(flow.app, flow.state, localhost(relay.port) + url.pathname + url.search);
        projectApps();
        response.writeHead(200, { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store", "referrer-policy": "no-referrer" });
        response.end("Connected. Return to Kuvo Code.");
      } catch { response.writeHead(400).end("The sign-in did not finish. Return to Kuvo Code."); }
      return;
    }
    const id = /^\/marketplace\/mcp\/([a-z0-9-]+)$/.exec(url.pathname)?.[1];
    const app = id && installations().apps.find(each => each.id === id && each.auth !== "open");
    const expected = Buffer.from(app ? "Bearer " + appKey(key, app) : "");
    const given = Buffer.from(request.headers.authorization ?? "");
    if (!app || !relay || request.headers.host !== `127.0.0.1:${relay.port}` || url.search
      || given.length !== expected.length || !crypto.timingSafeEqual(given, expected)) { response.writeHead(404).end(); return; }
    const method = request.method ?? "";
    if (!["GET", "POST", "DELETE"].includes(method)) { response.writeHead(405).end(); return; }
    const headers = new Headers();
    const forwarded = ["accept", "content-type", "mcp-session-id", "mcp-protocol-version", "last-event-id"];
    for (const name of forwarded) if (typeof request.headers[name] === "string") headers.set(name, request.headers[name]);
    try {
      headers.set("authorization", "Bearer " + await access(app));
    } catch { response.writeHead(403).end("Connect this app in Marketplace or unlock this computer’s vault."); return; }
    const controller = new AbortController();
    response.once("close", () => controller.abort());
    try {
      const body: Buffer[] = [];
      let size = 0;
      for await (const chunk of request) {
        size += chunk.length;
        if (size > 4 * 1024 * 1024) { response.writeHead(413).end(); return; }
        body.push(chunk);
      }
      const upstream = await fetch(app.url, { method, headers, redirect: "error", signal: controller.signal,
        ...(method === "POST" ? { body: Buffer.concat(body) } : {}) });
      for (const name of ["content-type", "mcp-session-id", "mcp-protocol-version"]) {
        const value = upstream.headers.get(name);
        if (value) response.setHeader(name, value);
      }
      response.writeHead(upstream.status);
      if (!upstream.body) response.end();
      else Readable.from(upstream.body).once("error", error => response.destroy(error)).pipe(response);
    } catch { if (response.headersSent) response.destroy(); else response.writeHead(502).end("The app could not be reached."); }
  };
  const server = http.createServer((request, response) => {
    void requestFromEngine(request, response).catch(() => {
      if (response.headersSent) response.destroy();
      else response.writeHead(503).end("This computer couldn’t load its Marketplace connections.");
    });
  });
  const listen = (port: number) => new Promise<number>((resolve, reject) => {
    const onError = (error: Error) => reject(error);
    server.once("error", onError);
    server.listen(port, "127.0.0.1", () => {
      server.off("error", onError);
      resolve((server.address() as { port: number }).port);
    });
  });
  const port = await listen(before?.port ?? 0).catch(error => {
    if ((error as NodeJS.ErrnoException).code !== "EADDRINUSE") throw error;
    return listen(0);
  });
  try {
    fs.mkdirSync(marketplaceFolder, { recursive: true, mode: 0o700 });
    placeWhole(endpointFile, JSON.stringify({ port, key }), 0o600);
  } catch (error) { server.close(); throw error; }
  relay = { port, key };
  server.once("close", () => { if (relay?.port === port) relay = undefined; });
  server.unref();
  return server;
}

export function projectApps(apps = installations().apps): string[] {
  if (!ownsSettings()) return apps.length ? ["Update this computer’s box before using Marketplace apps."] : [];
  fs.mkdirSync(marketplaceFolder, { recursive: true, mode: 0o700 });
  return locked(path.join(marketplaceFolder, "mcp-projection.lock"), () => writeAppProjection(apps), { required: true });
}

function writeAppProjection(apps: InstalledApp[]): string[] {
  const { settings, problem } = readSettings();
  if (problem) return [problem];
  const current = relay ?? storedRelay();
  let before: Record<string, unknown> = {};
  let pending: Record<string, unknown> = {};
  try {
    const record: unknown = JSON.parse(fs.readFileSync(appIndex, "utf8"));
    if (isObject(record) && isObject(record.before)) before = record.before;
    if (isObject(record) && isObject(record.pending)) pending = record.pending;
  } catch {}
  const wanted: Record<string, unknown> = {};
  for (const app of apps) {
    if (app.auth === "open") wanted[serverName(app.id)] = { type: "remote", url: app.url, oauth: false, codemode: false };
    else if (current && connectionState(app) === "ready") wanted[serverName(app.id)] = { type: "remote", oauth: false, codemode: false,
      url: localhost(current.port) + "/marketplace/mcp/" + app.id, headers: { Authorization: "Bearer " + appKey(current.key, app) } };
  }
  const changes = [];
  const problems: string[] = [];
  const equal = (a: unknown, b: unknown) => JSON.stringify(sortedKeys(a)) === JSON.stringify(sortedKeys(b));
  for (const name of new Set([...Object.keys(before), ...Object.keys(pending), ...Object.keys(wanted)])) {
    if (!name.startsWith(prefix)) continue;
    const where = [...SERVERS, name];
    const was = valueAt(settings, where);
    const value = wanted[name];
    if (equal(was, value)) continue;
    if (was !== undefined && !equal(was, before[name]) && !equal(was, pending[name])) {
      problems.push(`The MCP server ${name} on this computer is already in use.`);
      delete wanted[name];
      continue;
    }
    changes.push({ path: where, value: value ?? null, was: was ?? null });
  }
  if (changes.length) {
    const confirmed = Object.fromEntries(Object.keys({ ...before, ...pending }).flatMap(name => {
      const now = valueAt(settings, [...SERVERS, name]);
      return now !== undefined && (equal(now, before[name]) || equal(now, pending[name])) ? [[name, now]] : [];
    }));
    placeWhole(appIndex, JSON.stringify({ before: confirmed, pending: wanted }), 0o600, true);
    const written = changeSettings(changes);
    if (written.status !== 200) return [...problems, written.message];
  }
  if (changes.length || Object.keys(pending).length || !equal(wanted, before))
    placeWhole(appIndex, JSON.stringify({ before: wanted }), 0o600, true);
  return problems;
}

/** Remove locally stored sign-ins once the signed installation list no longer includes their app. */
let cleaning: Promise<void> | undefined;
export function cleanRemovedConnections() {
  return cleaning ??= (async () => {
    const names = new Set(installations().apps.map(credentialsName));
    for (const item of readItems()) {
      if (/^Marketplace · [a-z0-9-]+ · [a-f0-9]{16}$/.test(item.name) && !names.has(item.name) && (item.fields.includes("token") || item.fields.includes("oauth"))) {
        if (!installations().apps.some(app => credentialsName(app) === item.name)) await removeItem(item.id);
      }
    }
  })().finally(() => { cleaning = undefined; });
}
