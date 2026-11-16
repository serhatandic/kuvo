// An MCP server's setup as people type and paste it (Settings › MCP Servers): a command line as its
// words, and a server's setup from its README read so the form fills in by itself. The box's, so
// every client takes a paste the same way (the retired Mac app's `CommandWords` and `MCPSnippet`).

/** A command line's words, as a shell splits them: quotes keep spaces in one word, and a backslash
 * takes the next character as it is (not inside single quotes; inside double ones, only before ",
 * \, $ or `, as in "a\.b", a pattern pasted from a README). */
export function commandWords(line: string): string[] {
  const words: string[] = [];
  let current = "";
  let quote: string | null = null;
  let hasWord = false, escaped = false, hadWord = false;
  for (const character of line) {
    if (escaped) {
      // A backslash at the end of a line carries it on to the next, as in a shell.
      if (character === "\n" || character === "\r") {
        escaped = false;
        hasWord = hadWord;
        continue;
      }
      if (quote === "\"" && !"\"\\$`".includes(character)) current += "\\";
      current += character;
      escaped = false;
    } else if (character === "\\" && quote !== "'") {
      hadWord = hasWord;
      escaped = true;
      hasWord = true;
    } else if (quote) {
      if (character === quote) quote = null;
      else current += character;
    } else if (character === "\"" || character === "'") {
      quote = character;
      hasWord = true;
    } else if (/\s/.test(character)) {
      if (hasWord) words.push(current);
      current = "";
      hasWord = false;
    } else {
      current += character;
      hasWord = true;
    }
  }
  if (hasWord) words.push(current);
  return words;
}

/** A word as it can be typed back: in double quotes, quotes and backslashes escaped, when it has
 * spaces, quotes or backslashes. */
export const quotedWord = (word: string): string =>
  word !== "" && !/[\s"'\\]/.test(word) ? word : `"${word.replaceAll("\\", "\\\\").replaceAll("\"", "\\\"")}"`;

/** A command as one line to edit. */
export const commandLine = (command: unknown): string =>
  Array.isArray(command) ? command.filter((word): word is string => typeof word === "string").map(quotedWord).join(" ") : "";

/** A server's setup a paste gives: its name where it says one, and what it runs (`command`, with
 * its `environment`) or reaches (`url`, with its `headers`). */
export interface ServerSetup {
  name?: string;
  command: string[];
  environment: Record<string, string>;
  url?: string;
  headers: Record<string, string>;
}

/** JSON as READMEs write it: comments and trailing commas allowed. */
function lenientJSON(text: string): unknown {
  let out = "";
  let inString = false;
  for (let at = 0; at < text.length; at++) {
    const character = text[at]!;
    if (inString) {
      out += character;
      if (character === "\\") out += text[++at] ?? "";
      else if (character === "\"") inString = false;
    } else if (character === "\"") {
      inString = true;
      out += character;
    } else if (character === "/" && text[at + 1] === "/") {
      while (at < text.length && text[at] !== "\n") at++;
    } else if (character === "/" && text[at + 1] === "*") {
      at = text.indexOf("*/", at + 2);
      if (at < 0) return undefined;
      at++;
    } else {
      out += character;
    }
  }
  try { return JSON.parse(out.replace(/,(\s*[}\]])/g, "$1")); } catch { return undefined; }
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);

/** A JSON object; `"name": { … }` alone, as READMEs often show one entry, as an object of its own. */
function jsonObject(text: string): Record<string, unknown> | null {
  if (!text.startsWith("{") && !text.startsWith("\"")) return null;
  const wrapped = text.startsWith("\"") ? `{${text.endsWith(",") ? text.slice(0, -1) : text}}` : text;
  const value = lenientJSON(wrapped);
  return isRecord(value) ? value : null;
}

/** A value as the settings write it: a string as it is, a number or true or false as written. */
const asText = (value: unknown): string | null =>
  typeof value === "string" ? value : typeof value === "number" || typeof value === "boolean" ? String(value) : null;

const strings = (value: unknown): Record<string, string> => {
  const out: Record<string, string> = {};
  if (isRecord(value)) for (const [key, each] of Object.entries(value)) { const text = asText(each); if (text !== null) out[key] = text; }
  return out;
};

/** One server's entry: a command (a word and its `args`, or opencode's list of words) or a URL
 * (`url`, or Windsurf's `serverUrl`). */
function server(entry: Record<string, unknown>, name: string | undefined): ServerSetup | null {
  const url = entry.url ?? entry.serverUrl;
  if (typeof url === "string" && url) return { ...(name ? { name } : {}), command: [], environment: {}, url, headers: strings(entry.headers) };
  let command: string[];
  if (Array.isArray(entry.command) && entry.command.every(word => typeof word === "string")) command = entry.command as string[];
  else if (typeof entry.command === "string" && entry.command) command = [entry.command, ...(Array.isArray(entry.args) ? entry.args.map(asText).filter((word): word is string => word !== null) : [])];
  else return null;
  if (!command.length) return null;
  if (command.length === 1 && command[0]!.includes(" ") && entry.args === undefined) command = commandWords(command[0]!);
  return { ...(name ? { name } : {}), command, environment: strings(entry.env ?? entry.environment), headers: {} };
}

/** `claude mcp add-json [options] <name> '<json>'`. */
function claudeCodeJSON(line: string): ServerSetup | null {
  const words = commandWords(line).slice(3);
  while (words[0]?.startsWith("-")) words.splice(0, words[0].includes("=") || words[0] === "--client-secret" ? 1 : Math.min(2, words.length));
  const entry = words.length >= 2 ? jsonObject(words[1]!) : null;
  return entry ? server(entry, words[0]) : null;
}

/** `claude mcp add [options] <name> -- <command…>`, or `claude mcp add --transport http <name> <url>`. */
function claudeCode(line: string): ServerSetup | null {
  const words = commandWords(line).slice(3).flatMap(word => {
    const equals = word.indexOf("=");
    return word.startsWith("--") && equals > 0 ? [word.slice(0, equals), word.slice(equals + 1)] : [word];
  });
  const setup: ServerSetup = { command: [], environment: {}, headers: {} };
  let remote = false;
  const rest: string[] = [];
  for (let index = 0; index < words.length; index++) {
    const word = words[index]!;
    if (word === "--") {
      setup.command = words.slice(index + 1);
      break;
    }
    const value = words[index + 1];
    if (word === "-e" || word === "--env") {
      while (index + 1 < words.length && words[index + 1]!.indexOf("=") > 0) {
        const pair = words[++index]!;
        const equals = pair.indexOf("=");
        setup.environment[pair.slice(0, equals)] = pair.slice(equals + 1);
      }
    } else if (word === "-H" || word === "--header") {
      const colon = value?.indexOf(":") ?? -1;
      if (value && colon > 0) setup.headers[value.slice(0, colon).trim()] = value.slice(colon + 1).trim();
      index++;
    } else if (word === "-t" || word === "--transport") {
      remote = value === "http" || value === "sse";
      index++;
    } else if (word === "-s" || word === "--scope") {
      index++;
    } else {
      rest.push(word);
    }
  }
  if (!rest[0]) return null;
  setup.name = rest[0];
  if (remote) {
    if (!rest[1]) return null;
    setup.url = rest[1];
  } else if (!setup.command.length) {
    setup.command = rest.slice(1);
  }
  return setup.command.length || setup.url ? setup : null;
}

/** Every server `text` sets up, in the order it names them: Claude Desktop's and Cursor's
 * `mcpServers`, VS Code's `servers`, opencode's own `mcp`, Zed's `context_servers`, one server's
 * entry alone (named or not), or Claude Code's `claude mcp add` line; none for anything else. */
export function serverSetups(text: string): ServerSetup[] {
  const trimmed = text.trim();
  if (trimmed.startsWith("claude mcp add-json ")) return [claudeCodeJSON(trimmed)].filter((each): each is ServerSetup => each !== null);
  if (trimmed.startsWith("claude mcp add ")) return [claudeCode(trimmed)].filter((each): each is ServerSetup => each !== null);
  const object = jsonObject(trimmed);
  if (!object) return [];
  const named = (servers: Record<string, unknown>) => Object.keys(servers).sort().flatMap(name => {
    const entry = servers[name];
    const found = isRecord(entry) ? server(entry, name) : null;
    return found ? [found] : [];
  });
  for (const key of ["mcpServers", "servers", "mcp", "context_servers"]) {
    const servers = object[key];
    if (isRecord(servers)) return named(servers);
  }
  const alone = server(object, undefined);
  return alone ? [alone] : named(object);
}

/** The one server `text` sets up; null for anything else, or several. */
export function serverSetup(text: string): ServerSetup | null {
  const found = serverSetups(text);
  return found.length === 1 ? found[0]! : null;
}

/** What you typed for a server on the web, as an address to save: as it is with its scheme (a name
 * on your network too); without one, https (http for this computer's own, `localhost`); null for
 * one that isn't an address. */
export function webAddress(typed: string): string | null {
  const text = typed.trim();
  if (!text || /\s/.test(text)) return null;
  const schemed = text.includes("://");
  const local = ["localhost", "127.0.0.1", "[::1]"].some(prefix => text.toLowerCase().startsWith(prefix));
  const full = schemed ? text : (local ? "http://" : "https://") + text;
  let url: URL;
  try { url = new URL(full); } catch { return null; }
  if (!["http:", "https:"].includes(url.protocol) || !url.hostname) return null;
  // Without a scheme, a word alone is taken for a typo, not a name on your network.
  const host = url.hostname.toLowerCase();
  if (!schemed && !host.includes(".") && host !== "localhost" && !host.includes(":") && !host.startsWith("[")) return null;
  return full;
}
