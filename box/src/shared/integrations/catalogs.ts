// The catalogs a box's release carries in `bin`, written from the one table (`table.ts`,
// `servers.ts`) as the release is built (`release/files.ts`): what Settings › Integrations and
// Settings › MCP Servers offer on every box (`link/areas/integrations.ts`, `mcp-catalog.ts`).

import { APART, keysOf, KINDS, LAUNCHER, REDIRECT, type Service, SERVICES, SIGN_IN_COOKIES, SIGN_IN_FILES } from "./table";
import { MAC_ONLY_SERVERS, SERVERS } from "./servers";

/** The variable an integration's key goes in: KUVO_KEY_<ID>_<NAME>. */
export const keyVariable = (integration: string, name: string): string =>
  "KUVO_KEY_" + `${integration}_${name}`.toUpperCase().replace(/[^A-Z0-9]/g, "_");

/** How a box's engine settings name a key: the file the box keeps it in (`box/keys`). */
const keyReference = (variable: string): string => `{file:~/.kuvo-code/box/keys/${variable}}`;

/** Integrations that only work on a Mac, which machines don't offer: those that use its apps. */
export const MAC_ONLY_INTEGRATIONS: string[] = SERVICES.filter(service => service.way === "package" && service.macOnly).map(service => service.id);

/** The tag Settings shows after its name: how it's added. */
function how(service: Service): string {
  switch (service.way) {
    case "official": return "Official";
    case "token": return "Official · your token";
    case "app": return "Official · your app";
    case "package": return service.keys.length ? "Open source · your key" : service.macOnly ? "On this Mac" : "Open source";
    case "browser": return "In the browser";
  }
}

/** Its MCP server in the engine's settings, its keys named by their files (their values never go
 * in the settings file). Its tools are plain tools, not behind the engine's Code Mode, where the
 * agent sees only a short list of them under one tool and goes to the service's site instead.
 * None for one used on its site. */
function settings(service: Service): Record<string, unknown> | undefined {
  const key = (name: string) => keyReference(keyVariable(service.id, name));
  switch (service.way) {
    case "official":
      return { type: "remote", url: service.url, codemode: false };
    case "token":
      return { type: "remote", url: service.url, oauth: false, headers: { Authorization: `Bearer ${key(service.key.name)}` }, codemode: false };
    case "app":
      return { type: "remote", url: service.url, oauth: { clientId: key("CLIENT_ID"), clientSecret: key("CLIENT_SECRET") }, codemode: false };
    case "package": {
      const environment: Record<string, string> = { ...service.environment };
      for (const each of service.keys) environment[each.name] = key(each.name);
      // Apart from the project it's used in (`kuvo-npx`).
      environment[APART] = "1";
      return { type: "local", command: [LAUNCHER, ...service.command], environment, codemode: false };
    }
    case "browser":
      return undefined;
  }
}

/** An integration as a box's catalog has it: what it is and how it's added, as `kuvo/integrations`
 * lists it, with its server's settings, its keys' variables and the file its sign-in is in. */
function entry(service: Service): Record<string, unknown> {
  const listed: Record<string, unknown> = {
    id: service.id, title: service.title, summary: service.summary, kind: KINDS[service.kind],
    symbol: service.symbol, site: service.site, how: how(service), way: service.way,
  };
  if (service.way === "token" || service.way === "app") listed.help = service.help;
  if (service.way === "app") listed.redirect = REDIRECT;
  if (service.way === "package") {
    listed.by = service.by;
    if (service.help) listed.help = service.help;
    if (service.macOnly) listed.macOnly = true;
  }
  // The cookie its site keeps while you're signed in, which tells a box whether its browser is.
  if (service.way === "browser" && SIGN_IN_COOKIES[service.id]) listed.signInCookie = SIGN_IN_COOKIES[service.id];
  const keys = keysOf(service);
  if (keys.length) {
    listed.keys = keys.map(({ name, label, prompt }) => ({ name, label, prompt }));
    listed.variables = Object.fromEntries(keys.map(each => [each.name, keyVariable(service.id, each.name)]));
  }
  const server = settings(service);
  if (server) listed.settings = server;
  const signInFile = SIGN_IN_FILES[service.id]?.[0];
  if (signInFile) listed.signInFile = signInFile;
  return listed;
}

/** As JSON with each object's keys in order, on one line: the same bytes whatever order the
 * tables are written in. */
function ordered(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(ordered).join(",")}]`;
  if (value && typeof value === "object") {
    const object = value as Record<string, unknown>;
    return `{${Object.keys(object).filter(key => object[key] !== undefined).sort().map(key => `${JSON.stringify(key)}:${ordered(object[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

/** `bin/integrations.json`, a machine's: every integration but those that use a Mac's own apps.
 * `bin/integrations-mac.json` (`mac`), a Mac's box's: those too. */
export const integrationsCatalog = (mac: boolean): string =>
  ordered({ integrations: SERVICES.filter(service => mac || !MAC_ONLY_INTEGRATIONS.includes(service.id)).map(entry) });

/** `bin/mcp-catalog.json`: the popular servers, each saying whether it's an integration's too and
 * whether it uses a Mac's own apps, and every server that does (integrations' too), which stay on
 * Macs. */
export const mcpCatalog = (): string =>
  ordered({
    servers: SERVERS.map(server => ({
      ...server,
      integration: SERVICES.some(service => service.id === server.id),
      macOnly: MAC_ONLY_SERVERS.includes(server.id),
    })),
    macOnly: [...MAC_ONLY_INTEGRATIONS, ...MAC_ONLY_SERVERS].sort(),
  });
