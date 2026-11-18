// Settings › MCP Servers (`kuvo/settings/mcp`, PROTOCOL.md "A box's settings"): this computer's MCP
// servers, the first section that's a flow (a list, forms to add and edit one, a sign-in the box
// leads), so every client draws it by kind. Its servers are the engine's settings' `mcp.servers`
// (read and written through `kuvo/config`, with `was`), how each is doing is the engine's
// (`api/mcp`), its popular servers are `kuvo/mcp/catalog`'s, and a server's sign-in is the engine's
// own (`api/integration/<id>/connect/oauth`), followed by the box until it ends.
// Platform: the same on a Mac's box and a machine's; the popular servers that use a Mac's own apps
// are only offered on a Mac (`mcpCatalog`).

import { commandLine, commandWords, serverSetup, serverSetups, webAddress } from "../../shared/mcp-setup";
import type { ServerSetup } from "../../shared/mcp-setup";
import type { SettingAction, SettingBadge, SettingField, SettingFieldWords, SettingFlow, SettingForm, SettingGroup, SettingItem, SettingsChecked, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import { isObject } from "../../shared/json";
import type { Target } from "../../shared/records";
import { call } from "../engine/client";
import { fromLocalApp, type RouteContext } from "../server/context";
import { CHANGED_SINCE, CONFIG_FILE, readSettings, SERVERS, valueAt } from "./config";
import { loadSignInReturn } from "./engine";
import { integrationCatalog, reconnectEverywhere } from "./integrations";
import { addFromCatalog, CATALOG_FILE, mcpCatalog } from "./mcp-catalog";
import { findProgram } from "./programs";
import { settingsRoute } from "./settings";
import { dropFlow, endFlow, FLOWS_FILE, flowsOf, keepFlow } from "./settings-flows";
import { asked, item, mtime, refusal, row, said, STALE, staleOrRefusal, unreadPage } from "./settings-kit";
import type { Refused, Section } from "./settings-kit";
import { failureWords, oauthPage, oauthStartBody } from "./sign-in";

const TITLE = "MCP Servers";

/** Kuvo Code's own tools: not yours to change. */
const BUILT_IN = new Set(["kuvo-browser"]);

/** This computer in a sentence: "this Mac" to the app beside its box, its name to any other. */
const computerHere = (context: RouteContext): string =>
  context.request.fromApp === true ? (process.platform === "darwin" ? "this Mac" : "this computer") : context.info.name || "this computer";
const capitalized = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);

// MARK: Reading

/** An MCP server as the engine lists it (`api/mcp`). */
interface EngineServer {
  name?: string;
  status?: { status?: string; error?: string };
  integrationID?: string;
}

/** What the page is made from: the engine's settings, its servers as it has them now (null: it
 * couldn't say), and the integrations' servers' names, which Integrations keeps. */
interface Facts {
  settings: Record<string, unknown>;
  servers: EngineServer[] | null;
  engine: Target | null;
  integrations: Map<string, string>;
}

async function facts(context: RouteContext): Promise<Facts | { status: number; value: any }> {
  const config = await asked(context, settingsRoute, "GET", "kuvo/config");
  if (config.status !== 200) return config;
  let engine: Target | null = null;
  try { engine = context.readEngine(); } catch {}
  if (!engine?.port) engine = null;
  const listed = engine ? await call(engine, "GET", "/api/mcp", undefined, AbortSignal.timeout(5_000)).catch(() => null) : null;
  const integrations = new Map(integrationCatalog().map(each => [each.id, each.title || each.id] as const));
  return { settings: config.value, servers: Array.isArray(listed) ? listed as EngineServer[] : null, engine, integrations };
}

/** Your servers, by name: not Kuvo Code's own, nor an integration's. */
function yourServers(known: Facts): [string, Record<string, unknown>][] {
  const servers = valueAt(known.settings, SERVERS);
  if (!isObject(servers)) return [];
  return Object.entries(servers)
    .filter((entry): entry is [string, Record<string, unknown>] => isObject(entry[1]) && !BUILT_IN.has(entry[0]) && !known.integrations.has(entry[0]))
    .sort(([a], [b]) => a.localeCompare(b));
}

/** How a server stands, as the engine says. */
function badgeOf(on: boolean, server: EngineServer | undefined, known: Facts): SettingBadge | undefined {
  if (!on) return { words: "Off", tone: "quiet" };
  if (!known.servers) return undefined;
  switch (server?.status?.status) {
    case "connected": return { words: "Connected", tone: "good" };
    case "pending": return { words: "Connecting…", tone: "busy" };
    case "needs_auth": case "needs_client_registration": return { words: "Needs sign-in", tone: "warning" };
    case "failed": return { words: "Couldn’t connect", tone: "failed" };
    case "disabled": return { words: "Off", tone: "quiet" };
    default: return { words: "Not started", tone: "quiet" };
  }
}

/** Its line: the command it runs, or its URL. */
const lineOf = (entry: Record<string, unknown>): string | undefined =>
  typeof entry.url === "string" && entry.url ? entry.url : commandLine(entry.command) || undefined;

// MARK: The form

const pairsOf = (value: unknown): { key: string; value: string }[] =>
  isObject(value) ? Object.entries(value).map(([key, each]) => ({ key, value: typeof each === "string" ? each : String(each) })) : [];

/** A server's form: Add Server's (`name` null) or one's own (Edit…), with its values now. */
function serverForm(context: RouteContext, name: string | null, entry: Record<string, unknown> = {}): SettingForm {
  const where = computerHere(context);
  const remote = entry.type === "remote";
  const customSignIn = isObject(entry.oauth);
  const keyNote = "To keep a key out of the settings file, write {env:NAME} to use one of your environment variables.";
  const fields: SettingField[] = [
    ...(name === null ? [{ id: "name", title: "Name", kind: "text" as const, value: "", placeholder: "files, or paste its setup from the README" }] : []),
    { id: "runs", title: "Runs", kind: "choice", value: remote ? "remote" : "local", choices: [{ id: "local", title: `On ${where}` }, { id: "remote", title: "On the web" }] },
    { id: "command", title: "Command", kind: "lines", mono: true, value: commandLine(entry.command), placeholder: "npx -y @modelcontextprotocol/server-filesystem ~/Documents", shown: { field: "runs", is: ["local"] } },
    { id: "cwd", title: "Working folder", kind: "folder", value: typeof entry.cwd === "string" ? entry.cwd : "", placeholder: "The project’s folder", folder: { choose: "Choose…", clear: "The project’s folder", prompt: "Choose", message: "Choose the folder the server runs in." }, shown: { field: "runs", is: ["local"] } },
    { id: "environment", title: "Environment Variables", kind: "pairs", value: pairsOf(entry.environment), pairs: { key: "NAME", value: "Value" }, line: `Each by its name, like GITHUB_TOKEN. ${keyNote}`, shown: { field: "runs", is: ["local"] } },
    { id: "url", title: "URL", kind: "text", value: typeof entry.url === "string" ? entry.url : "", placeholder: "https://example.com/mcp", shown: { field: "runs", is: ["remote"] } },
    {
      id: "oauth", title: "Sign in when the server asks", kind: "switch", value: customSignIn || entry.oauth !== false, shown: { field: "runs", is: ["remote"] },
      line: customSignIn ? "With the sign-in set up in the settings file." : "Turn off for a server that takes an API key in a header instead.",
      ...(customSignIn ? { can: { why: "Its sign-in is set up in the settings file: change it there." } } : {}),
    },
    { id: "headers", title: "Headers", kind: "pairs", value: pairsOf(entry.headers), pairs: { key: "Header, like Authorization", value: "Value" }, line: keyNote, shown: { field: "runs", is: ["remote"] } },
    {
      id: "codemode", title: "Let the agent call its tools from code", kind: "switch", value: name === null ? false : entry.codemode !== false,
      line: "Code Mode: the agent looks up this server's tools and combines several in one step, which keeps each message small for a server with many tools. Smaller models may not find its tools this way.",
    },
  ];
  return name === null
    ? { title: "Add Server", submit: "Add Server", fields, note: `A server on ${where} runs a command; one on the web is a URL.` }
    : { title: name, submit: "Save", fields };
}

/** A server's name as you typed it: hyphens for spaces, only letters, digits, hyphens and underscores. */
const serverId = (typed: string): string => typed.trim().replaceAll(" ", "-").replace(/[^\p{L}\p{N}_-]/gu, "");

/** What a pasted setup fills in. */
const filled = (setup: ServerSetup, name: boolean): Record<string, unknown> => ({
  ...(name && setup.name ? { name: setup.name } : {}),
  runs: setup.url ? "remote" : "local",
  ...(setup.url
    ? { url: setup.url, headers: Object.entries(setup.headers).map(([key, value]) => ({ key, value })) }
    : { command: commandLine(setup.command), environment: Object.entries(setup.environment).map(([key, value]) => ({ key, value })) }),
});

const text = (value: unknown): string => (typeof value === "string" ? value : "");
const pairs = (value: unknown): { key: string; value: string }[] =>
  Array.isArray(value) ? value.filter(isObject).map(each => ({ key: text(each.key).trim(), value: text(each.value) })) : [];

/** The form's values read: each field's words, whether it can be saved, values filled in from a
 * paste, and the server's settings it makes (`entry`, with its name for a new one). */
function readForm(context: RouteContext, known: Facts, name: string | null, values: Record<string, unknown>, before: Record<string, unknown> = {}): SettingsChecked & { entry?: Record<string, unknown>; id?: string } {
  const fields: SettingFieldWords[] = [];
  const say = (id: string, words: string, tone: SettingFieldWords["tone"]) => fields.push({ id, words, tone });
  let ready = true;
  let fill: Record<string, unknown> | undefined;

  // A setup pasted into its name or command fills in the rest.
  for (const at of name === null ? ["name", "command"] : ["command"]) {
    const typed = text(values[at]);
    const several = serverSetups(typed);
    if (several.length > 1) {
      say(at, `This sets up ${several.length} servers (${several.map(each => each.name ?? "unnamed").join(", ")}). Paste one server’s part at a time.`, "warning");
      ready = false;
    }
    const setup = serverSetup(typed);
    if (setup) {
      fill = filled(setup, at === "name");
      if (at === "name" && !setup.name) fill.name = "";
      values = { ...values, ...fill };
      say(at === "name" ? (setup.url ? "url" : "command") : at, "Filled in from the setup you pasted.", "good");
    }
  }

  let id: string | undefined;
  if (name === null) {
    const typed = text(values.name).trim();
    id = serverId(typed);
    const taken = valueAt(known.settings, [...SERVERS, id]) !== undefined || (known.servers ?? []).some(each => each?.name === id);
    const why = !id ? (typed ? "A name needs a letter or a number." : null)
      : BUILT_IN.has(id) ? "Kuvo Code uses that name for its own tools."
      : known.integrations.has(id) ? `${known.integrations.get(id)} uses that name: add it in Integrations, or pick another name.`
      : taken ? `There's a server called ${id} already.` : null;
    if (why) say("name", why, "warning");
    else if (id && id !== typed) say("name", `It'll be called ${id}.`, "quiet");
    else if (!id) say("name", fill ? "Name it to add it with the setup you pasted." : "A short name without spaces. The server's tools are named after it.", "quiet");
    if (!id || why) ready = false;
  }

  const remote = values.runs === "remote";
  const entry: Record<string, unknown> = { ...before };
  for (const key of ["command", "cwd", "environment", "url", "headers", "oauth"]) delete entry[key];
  entry.type = remote ? "remote" : "local";
  if (remote) {
    // (A sign-in set up by hand in the settings file stays as it is.)
    if (isObject(before.oauth)) entry.oauth = before.oauth;
    else if (values.oauth === false) entry.oauth = false;
    const typed = text(values.url).trim();
    const url = webAddress(typed);
    if (!typed) {
      say("url", "Its address, like https://example.com/mcp.", "quiet");
      ready = false;
    } else if (!url) {
      say("url", "That isn’t a web address. Give one like https://example.com/mcp.", "warning");
      ready = false;
    } else {
      if (url !== typed) say("url", `It'll be saved as ${url}.`, "quiet");
      entry.url = url;
    }
  } else {
    const words = commandWords(text(values.command));
    if (!words.length) {
      if (!fields.some(each => each.id === "command")) say("command", "The command that starts it.", "quiet");
      ready = false;
    } else {
      entry.command = words;
      if (!fields.some(each => each.id === "command")) {
        const found = findProgram(`?name=${encodeURIComponent(words[0]!)}`).value as { name?: string; path?: string | null; unknown?: boolean };
        const where = computerHere(context);
        if (found.path) say("command", `${found.name} is on ${where}.`, "good");
        else if (found.unknown !== true && found.name) say("command", `${found.name} isn’t on ${where}` + (sourceOf(found.name) ? `. It comes with ${sourceOf(found.name)}: install that there first.` : ". Install it there first, or use a command it has."), "warning");
      }
    }
    const cwd = text(values.cwd).trim();
    if (cwd) entry.cwd = cwd;
  }

  const listed = pairs(values[remote ? "headers" : "environment"]).filter(each => each.key || each.value);
  const kept: Record<string, string> = {};
  for (const each of listed) {
    const field = remote ? "headers" : "environment";
    if (!each.key) {
      say(field, "Give each value a name.", "warning");
      ready = false;
    } else if (!remote && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(each.key)) {
      say(field, `${each.key} isn't a variable's name: letters, digits and underscores, like GITHUB_TOKEN.`, "warning");
      ready = false;
    } else if (each.key in kept) {
      say(field, `${each.key} is given twice.`, "warning");
      ready = false;
    } else {
      kept[each.key] = each.value;
    }
  }
  if (Object.keys(kept).length) entry[remote ? "headers" : "environment"] = kept;
  entry.codemode = values.codemode === true;
  return { fields, ready, ...(fill ? { values: fill } : {}), ...(ready ? { entry } : {}), ...(id ? { id } : {}) };
}

/** What a program that isn't there comes with, for the ones servers' READMEs use most. */
function sourceOf(program: string): string | null {
  const base = program.split("/").pop() ?? program;
  if (["npx", "npm", "node"].includes(base)) return "Node.js";
  if (["uvx", "uv"].includes(base)) return "uv";
  if (base === "docker") return "Docker";
  if (["python", "python3", "pip", "pip3"].includes(base)) return "Python";
  if (["bun", "bunx"].includes(base)) return "Bun";
  if (base === "deno") return "Deno";
  return null;
}

// MARK: The page

/** A server's sign-in, as the page states it. */
function flowOf(context: RouteContext, flow: ReturnType<typeof flowsOf>[number]): SettingFlow {
  const title = `Signing in to ${flow.row.slice("server-".length)}`;
  if (flow.state === "done") return { id: flow.id, state: "done", title, line: "Signed in. The agent can use its tools now.", actions: [{ id: "dismiss", title: "OK" }] };
  if (flow.state === "failed") return { id: flow.id, state: "failed", title, line: flow.line ?? "Signing in didn’t work.", actions: [{ id: "sign-in", title: "Try Again" }, { id: "dismiss", title: "OK" }] };
  const opens = flow.about.url;
  return {
    id: flow.id, state: "waiting", title, line: fromLocalApp(context.request) ? "Finish signing in on its page in your browser." : "Finish signing in in your browser. If the last page can’t load, copy its address and paste it here.",
    ...(opens ? { opens } : {}),
    // (Its page ends at an address on this box's computer: from elsewhere, pasted back.)
    ...(fromLocalApp(context.request) ? {} : { code: { title: "Or paste the address the page ended on", placeholder: "http://127.0.0.1:19876/mcp/oauth/callback?code=…", action: "finish" } }),
    // (`finish` takes a pasted address: offered only with the field for it.)
    actions: [...(opens ? [{ id: "open-sign-in", title: "Open Sign-In Page", opens }] : []), ...(fromLocalApp(context.request) ? [] : [{ id: "finish", title: "Finish" }]), { id: "cancel", title: "Cancel" }],
  };
}

/** The sign-ins waiting, asked about: one that ended is kept so, and a server signed in to is
 * connected again everywhere its engine runs one. */
async function followFlows(known: Facts): Promise<void> {
  if (!known.engine) return;
  for (const flow of flowsOf("mcp").filter(each => each.state === "waiting")) {
    const route = `/api/integration/${encodeURIComponent(flow.about.integration ?? "")}/connect/oauth/${encodeURIComponent(flow.about.attempt ?? "")}`;
    let said: any;
    try {
      said = await call(known.engine, "GET", route, undefined, AbortSignal.timeout(5_000));
    } catch (error) {
      // (An attempt the engine no longer knows: started over, or long gone.)
      if ((error as { status?: number }).status === 404) endFlow("mcp", flow.id, "failed", failureWords("expired"));
      continue;
    }
    if (said?.status === "complete") {
      if (endFlow("mcp", flow.id, "done") && flow.about.server) await reconnectEverywhere(known.engine, flow.about.server).catch(() => {});
    } else if (said?.status === "failed" || said?.status === "expired") {
      endFlow("mcp", flow.id, "failed", failureWords(said.status, said.message));
    }
  }
}

async function mcpPage(context: RouteContext, quiet = false): Promise<SettingsPage> {
  const known = await facts(context);
  if (!("settings" in known)) return unreadPage("mcp", TITLE, known);
  if (!quiet) await followFlows(known);
  const flows = flowsOf("mcp");
  const where = computerHere(context);
  const items: SettingItem[] = yourServers(known).map(([name, entry]) => {
    const on = entry.disabled !== true;
    const server = (known.servers ?? []).find(each => each?.name === name);
    const status = server?.status?.status;
    const flow = flows.find(each => each.row === `server-${name}`);
    const error = on && status === "failed" ? server?.status?.error?.trim() : undefined;
    // (What follows from how it's doing now, as its badge does, isn't part of its revision: the
    // engine says it from moment to moment, and a switch read a moment before still holds.)
    const now: SettingAction[] = [
      ...(on && !flow && (status === "needs_auth" || status === "needs_client_registration") && server?.integrationID ? [{ id: "sign-in", title: "Sign In…" }] : []),
      ...(on && status === "failed" ? [{ id: "try-again", title: "Try Again" }] : []),
    ];
    const badge = badgeOf(on, server, known);
    const line = lineOf(entry);
    const kept = item({
      id: `server-${name}`, title: name, ...(line ? { line } : {}), kind: "switch", value: on, can: true,
      actions: [
        { id: "edit", title: "Edit…", form: serverForm(context, name, entry) },
        { id: "remove", title: "Remove…", style: "destructive", confirm: { title: `Remove ${name}?`, body: "The agent loses its tools.", button: "Remove" } },
      ],
      ...(flow ? { flow: flowOf(context, flow) } : {}),
    });
    return { ...kept, actions: [...now, ...kept.actions!], ...(badge ? { badge } : {}), ...(error ? { notice: { words: error, tone: "warning" as const } } : {}) };
  });

  const groups: SettingGroup[] = [{
    id: "servers", title: "Servers", note: `A server on ${where} runs a command; one on the web is a URL.`,
    rows: [row({ id: "servers", title: "Servers", kind: "list", items, empty: "None yet.", add: { id: "add", title: "Add Server…", form: serverForm(context, null) } })],
  }];

  // The popular servers, grouped by what they need, so no row carries a tag of its own: not an
  // integration's, which Integrations connects.
  const popular = mcpCatalog(known.settings, CATALOG_FILE).servers.filter(each => !known.integrations.has(each.id));
  const accessTitles = { signIn: "Popular Servers That Need an Account", open: "No Account Needed", local: `Run on ${capitalized(where)}` } as const;
  const accesses = (["signIn", "open", "local"] as const).filter(access => popular.some(each => each.access === access));
  for (const access of accesses) {
    groups.push({
      id: `popular-${access}`, title: accessTitles[access],
      ...(access === accesses[accesses.length - 1] ? { note: "Popular servers, each from the company behind it, as listed in the official MCP Registry. One that needs an account asks you to sign in once it's added." } : {}),
      rows: popular.filter(each => each.access === access).map(each => each.added
        ? row({ id: `popular-${each.id}`, title: each.title, line: each.summary, kind: "status", value: "Added" })
        : row({ id: `popular-${each.id}`, title: each.title, line: each.summary, kind: "actions", actions: [{ id: "add", title: "Add" }] })),
    });
  }

  const timeouts = [
    { id: "timeout-startup", key: "startup", title: "Time to start, in milliseconds", placeholder: "30000" },
    { id: "timeout-catalog", key: "catalog", title: "Time to list tools, in milliseconds", placeholder: "30000" },
    { id: "timeout-execution", key: "execution", title: "Time for one tool call, in milliseconds", placeholder: "43200000" },
  ];
  groups.push({
    id: "timeouts", title: "Timeouts", note: "For every server, unless one sets its own.",
    rows: timeouts.map(each => {
      const value = valueAt(known.settings, ["mcp", "timeout", each.key]);
      return row({ id: each.id, title: each.title, kind: "number", ...(typeof value === "number" ? { value } : {}), placeholder: each.placeholder, min: 1, can: value === undefined || typeof value === "number" ? true : { why: "This is set in the settings file as something other than a number. Change it there." } });
    }),
  });

  const notes = known.servers ? [] : [{ words: `The engine isn’t running on ${where}, so how each server is doing isn’t known. They’re listed as the settings have them.`, tone: "warning" as const }];
  return { id: "mcp", title: TITLE, groups, ...(notes.length ? { notes } : {}) };
}

// MARK: Changes

/** One write to the engine's settings, from what was there (`was`): a 409 for a change made since. */
const write = (context: RouteContext, path: string[], value: unknown, was: unknown) =>
  asked(context, settingsRoute, "PATCH", "kuvo/config", { changes: [{ path, value, was: was ?? null }] });

async function mcpChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  const read = readSettings(CONFIG_FILE);
  if (read.problem) return { status: 409, message: `The engine's settings file can't be changed from here: ${read.problem}` };
  if (at.id.startsWith("server-")) {
    const path = [...SERVERS, at.id.slice("server-".length)];
    const entry = valueAt(read.settings, path);
    if (!isObject(entry) || (entry.disabled !== true) !== at.value) return STALE;
    if (value === at.value) return null;
    return staleOrRefusal(await write(context, [...path, "disabled"], value ? null : true, entry.disabled));
  }
  const key = at.id.startsWith("timeout-") ? at.id.slice("timeout-".length) : null;
  if (key && ["startup", "catalog", "execution"].includes(key)) {
    const path = ["mcp", "timeout", key];
    const now = valueAt(read.settings, path);
    if (now !== at.value) return STALE;
    return staleOrRefusal(await write(context, path, Math.floor(Number(value)), now));
  }
  return { status: 400, message: "That isn't something to change." };
}

/** A form's values refused: why, and each field's words. */
const formRefused = (checked: SettingsChecked): Refused => {
  const first = checked.fields.find(each => each.tone === "warning");
  return { status: 400, message: first?.words ?? "Fill in what it needs first.", fields: [...checked.fields] };
};

async function mcpAct(context: RouteContext, at: SettingItem, action: SettingAction, values?: Record<string, unknown>): Promise<Refused | null> {
  const known = await facts(context);
  if (!("settings" in known)) return refusal(known) ?? { status: 503, message: said(known, "This computer's settings couldn't be read.") };

  if (at.id === "servers" && action.id === "add") {
    const checked = readForm(context, known, null, values ?? {});
    if (!checked.entry || !checked.id) return formRefused(checked);
    const written = await write(context, [...SERVERS, checked.id], checked.entry, null);
    if (written.status === 409 && written.value?.message === CHANGED_SINCE) return { status: 400, message: `There's a server called ${checked.id} already.`, fields: [{ id: "name", words: `There's a server called ${checked.id} already.`, tone: "warning" }] };
    return refusal(written);
  }
  if (at.id.startsWith("popular-") && action.id === "add") {
    const added = addFromCatalog(at.id.slice("popular-".length));
    return added.status === 200 ? null : added.status === 404 ? STALE : { status: added.status, message: added.message };
  }
  if (!at.id.startsWith("server-")) return { status: 404, message: "That isn't something it does." };

  const name = at.id.slice("server-".length);
  const path = [...SERVERS, name];
  const entry = valueAt(known.settings, path);
  if (!isObject(entry)) return STALE;
  switch (action.id) {
    case "edit": {
      const checked = readForm(context, known, name, values ?? {}, entry);
      if (!checked.entry) return formRefused(checked);
      return staleOrRefusal(await write(context, path, checked.entry, entry));
    }
    case "remove":
      dropFlow("mcp", at.id);
      return staleOrRefusal(await write(context, path, null, entry));
    case "try-again":
      if (!known.engine) return { status: 503, message: `The engine isn’t running on ${computerHere(context)}. Try again once it’s started.` };
      await reconnectEverywhere(known.engine, name);
      return null;
    case "sign-in":
      return signIn(context, known, at.id, name);
    case "finish": {
      const pasted = text(values?.code).trim();
      if (!pasted) return { status: 400, message: "Paste the address the sign-in page ended on." };
      const loaded = await asked(context, ctx => loadSignInReturn(ctx.request.body, ctx.signal), "POST", "kuvo/sign-in/return", { url: pasted });
      const refused = refusal(loaded);
      if (refused) return refused;
      // (Loaded, but the page it returns to turned it down: an old address, another sign-in's.)
      const answered = Number(loaded.value?.status);
      if (!(answered >= 200 && answered < 400)) return { status: 400, message: "The sign-in didn't take that address. Paste the one its page ended on for this sign-in, or start it again." };
      return null;
    }
    case "cancel":
    case "dismiss":
      dropFlow("mcp", at.id);
      return null;
  }
  return { status: 404, message: "That isn't something it does." };
}

/** A server's sign-in started with the engine, as its own sign-in in the browser: the box keeps it,
 * and states how it's going on the page until it ends. */
async function signIn(context: RouteContext, known: Facts, row: string, name: string): Promise<Refused | null> {
  if (!known.engine) return { status: 503, message: `The engine isn’t running on ${computerHere(context)}. Try again once it’s started.` };
  const integration = (known.servers ?? []).find(each => each?.name === name)?.integrationID;
  if (!integration) return { status: 409, message: `${name} doesn't ask for a sign-in now.` };
  try {
    const listed = await call(known.engine, "GET", "/api/integration", undefined, AbortSignal.timeout(15_000));
    const account = (Array.isArray(listed) ? listed : []).find((each: any) => each?.id === integration) as { methods?: { type?: string; id?: string }[] } | undefined;
    const method = account?.methods?.find(each => each?.type === "oauth" && typeof each.id === "string");
    if (!method) return { status: 409, message: `${name}'s sign-in can't be started from here.` };
    const started = await call(known.engine, "POST", `/api/integration/${encodeURIComponent(integration)}/connect/oauth`, oauthStartBody(context.request, { methodID: method.id }), AbortSignal.timeout(30_000)) as { attemptID?: string; url?: string } | null;
    if (!started?.attemptID) return { status: 502, message: "The sign-in couldn't be started." };
    if (!fromLocalApp(context.request) && oauthPage(started.url).local) return { status: 409, message: `Open Kuvo Code on ${computerHere(context)} and start the sign-in there. The sign-in page only opens on that computer.` };
    // (Its page is the server's to name: only a web page is kept to open.)
    const page = typeof started.url === "string" && /^https?:\/\//i.test(started.url) ? started.url : undefined;
    keepFlow({ section: "mcp", row, id: started.attemptID, about: { integration, attempt: started.attemptID, server: name, ...(page ? { url: page } : {}) } });
    return null;
  } catch (error) {
    return { status: 502, message: error instanceof Error && error.message ? error.message : "The sign-in couldn't be started." };
  }
}

async function mcpCheck(context: RouteContext, at: SettingItem, action: SettingAction, values: Record<string, unknown>): Promise<SettingsChecked> {
  const known = await facts(context);
  if (!("settings" in known)) return { fields: [], ready: false };
  const name = at.id.startsWith("server-") && action.id === "edit" ? at.id.slice("server-".length) : null;
  const before = name ? valueAt(known.settings, [...SERVERS, name]) : undefined;
  const { fields, ready, values: fill } = readForm(context, known, name, values, isObject(before) ? before : {});
  return { fields, ready, ...(fill ? { values: fill } : {}) };
}

export const mcpSection: Section & { listed: SettingsSection } = {
  listed: {
    id: "mcp", title: TITLE, group: "Agents & Tools", icon: "sliders",
    line: "Programs and web services that give agents more tools.",
    keywords: ["mcp", "servers", "tools", "add server", "sign in", "oauth", "command", "url", "timeout"],
  },
  read: mcpPage, change: mcpChange, act: mcpAct, check: mcpCheck,
  // (How each server is doing is only in the engine: read each second, sent only when it changed.)
  look: () => `${mtime(CONFIG_FILE)} ${mtime(FLOWS_FILE)} ${mtime(CATALOG_FILE)} ${Math.floor(Date.now() / 1000)}`,
};
