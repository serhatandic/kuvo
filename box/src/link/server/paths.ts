// Which paths a phone may ask for, written one way (PROTOCOL.md, "Messages in a session"), held to protocol/vectors/link.json

// The sockets a phone may open: a terminal's.
const SOCKETS = [/^kuvo\/browser\/view$/, /^api\/pty\/[^/?]+\/connect(\?|$)/];
export const REQUEST_HEADERS = ["content-type", "accept", "x-opencode-directory", "last-event-id"];
export const RESPONSE_HEADERS = ["content-type", "cache-control"];

// `path` written one way only, which is what's checked and what's sent, or null when it isn't
// what it seems, as the Mac's EngineRoutes.canonical: each part between slashes, and each name
// and value in its query, percent-decoded once and encoded again the same way every time; empty
// parts left out. Also refused: a fragment (`#`, which isn't sent) or a space at either end,
// which a phone never sends; a part between slashes that holds a slash, or is "." or ".."; a name
// or value in the query with a ".." between its slashes. (Only what's ASCII counts there, so a
// mark or joiner hides neither, and "[...slug]" is a name.)
export const canonicalPath = (path: string): string | null => {
  try {
    if (path.includes("#") || path.startsWith(" ") || path.endsWith(" ")) return null;
    const at = path.indexOf("?");
    const parts: string[] = [];
    for (const raw of (at < 0 ? path : path.slice(0, at)).split("/").filter(Boolean)) {
      const part = decodedPart(raw);
      if (part === null || part.includes("/") || [".", ".."].includes(ascii(part))) return null;
      parts.push(encodeURIComponent(part).replace(/%(24|26|2B|2C|3B|3D|3A|40)/g, plainly));
    }
    const fields: string[] = [];
    const route = parts.join("/");
    for (const raw of at < 0 ? [] : path.slice(at + 1).split("&").filter(Boolean)) {
      // Its name and value, a "+" in them a space, as a query is read.
      const spaced = raw.replaceAll("+", " "), equals = spaced.indexOf("=");
      const name = decodedPart(equals < 0 ? spaced : spaced.slice(0, equals));
      if (name === null || name.split("/").some(piece => ascii(piece) === "..")) return null;
      const halves = [name];
      if (equals >= 0) {
        // The session page validates its key as it looks up the inventory.
        const projectKey = route === "kuvo/sessions/page" && name === "project";
        const folder = FOLDER_PATHS.has(`${route}?${name}`);
        const value = decodedPart(spaced.slice(equals + 1), projectKey || folder);
        if (value === null || (!projectKey && value.split(folder ? /[/\\]/ : "/").some(piece => ascii(piece) === ".."))) return null;
        halves.push(value);
      }
      fields.push(halves.map(half => encodeURIComponent(half!).replace(/%(24|2C|3B|3A|40|2F|3F)/g, plainly).replaceAll("'", "%27")).join("="));
    }
    return parts.join("/") + (fields.length ? `?${fields.join("&")}` : "");
  } catch {
    // A lone surrogate, which can't be encoded.
    return null;
  }
};
// The query values that name a folder on this computer, by route.
const FOLDER_PATHS = new Set(["kuvo/folders?path", "kuvo/folders/tree?path", "kuvo/files/find?directory"]);
// Percent-decoded once, or null when it doesn't decode or holds a control character, or a
// backslash where it can't be part of a query path or project key.
const decodedPart = (raw: string, allowBackslashes = false): string | null => {
  let text: string;
  try {
    text = decodeURIComponent(raw);
  } catch {
    return null;
  }
  return /[\u0000-\u001f\u007f]/.test(text) || (!allowBackslashes && text.includes("\\")) ? null : text;
};
const ascii = (text: string) => text.replace(/[^\u0000-\u007f]/g, "");
// What's written as it is beyond what encodeURIComponent leaves (letters, digits, `-_.!~*'()`):
// between slashes `$&+,;=:@`, and in a query `$,;:@/?`, but not `'`, which URL parsers encode
// there. As the Mac writes them.
const plainly = (_: string, hex: string) => String.fromCharCode(parseInt(hex, 16));

// A paired phone is yours: the engine's whole API, as the app has it (sessions, files, terminals,
// commands, accounts and keys, settings, MCP servers). Only a path that isn't what it seems
// (`canonicalPath` refuses it) is refused. As the Mac allows them (KuvoLink's EngineRoutes.allows).
export const allowed = (path: string, method = "GET"): boolean => /^api(\/|\?|$)/.test(canonicalPath(path) ?? "");
export const socketAllowed = (path: string): boolean => SOCKETS.some(pattern => pattern.test(canonicalPath(path) ?? ""));

// The same of the coding agents' bridge (`kuvo/agents/api/…`), but for the routes only the box
// and the app wire things with: where the engine is, its group chats' own steps (moves), which a
// phone does through `kuvo/groups`, and what its browser downloaded (`api/kuvo/artifacts/noted`;
// a client reads the record through `kuvo/artifacts`). Checked on the path as it's sent, which the bridge reads part
// by part. As the Mac allows them (EngineRoutes.allowsAgents; both are checked against
// protocol/vectors/link.json).
export const agentsAllowed = (path: string, method = "GET"): boolean => {
  const sent = canonicalPath(path) ?? "";
  return /^api(\/|\?|$)/.test(sent) && !/^api\/kuvo\/(engine|groups|artifacts)(\/|\?|$)/.test(sent);
};

// Where a sign-in's page sent the browser back to this machine, for it to load
// (`kuvo/sign-in/return`), as the Mac's EngineRoutes.signInReturn: only its own address
// (127.0.0.1, localhost, [::1]) over http, at a port, with what the page sent back (a query),
// written plainly, in the characters an address may hold as it is (so it's loaded exactly as
// written). The address without its fragment, which a browser doesn't send; null otherwise.
const RETURN = /^(http:\/\/(?:127\.0\.0\.1|localhost|\[::1\]):([1-9][0-9]{0,4})(?:\/(?:[A-Za-z0-9\-._~!$&'()*+,;=:@/]|%[0-9A-Fa-f]{2})*)?\?(?:[A-Za-z0-9\-._~!$&'()*+,;=:@/?]|%[0-9A-Fa-f]{2})+)(?:#.*)?$/;
export const signInReturn = (url: unknown): string | null => {
  if (typeof url !== "string" || Buffer.byteLength(url) > 8192 || !/^[!-~]*$/.test(url) || url.includes("\\")) return null;
  const match = url.match(RETURN);
  return match && Number(match[2]) <= 65535 ? match[1]! : null;
};
