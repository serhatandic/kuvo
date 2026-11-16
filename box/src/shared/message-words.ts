// The box's words on a conversation's messages, added to each page of them it forwards (`GET
// api/session/<id>/message…`, PROTOCOL.md "Tool steps" and "Its sessions' news"), so every client
// draws the same without reading names or text itself:
// - a tool step that isn't one of the engine's own gets `title` (what it did) and `doing` (while it
//   runs): Kuvo Code's own tools in the words kept beside each one's definition, another MCP
//   server's as "<server> · <tool>";
// - a note of the box's telling a lead of its sessions gets `metadata.kuvo.news`, each session's
//   news as its card shows it;
// - a wake-up's message (a wake-up's or a watch's) gets `metadata.kuvo.wake`: its card's `summary`
//   (the one its agent gave it, else its note's first sentence) and its `note` whole.
// Pure data: the same on a Mac's box and a machine's.

import { isObject } from "./json";
import { boxNoteOf, wakeMarkOf } from "./queues";

/** How a tool's call is told: its MCP `title` (a short imperative name, sent in `tools/list`), and
 * what a step of it says done and while it runs. `target` names an input field to put in them where
 * the call has it ("Sent a message to Mira"), with `{}` where it goes. */
export interface ToolWords {
  title: string;
  done: string;
  doing: string;
  target?: { field: string; done: string; doing: string; quoted?: boolean };
}

/** What a page is told with: Kuvo Code's own tools' words, by tool name, and the MCP servers the
 * engine knows (its settings' and the integrations'), by the name its tools are named after, each
 * with the name shown for it. */
export interface KnownTools {
  own: ReadonlyMap<string, ToolWords>;
  servers: ReadonlyMap<string, string>;
}

/** Kuvo Code's own tools' server, which is never named to anyone. */
export const OWN_SERVER = "kuvo-browser";

/** Tool steps clients word themselves: the engine's own tools, by their names now and before, and
 * the kinds the coding agents' bridge names their steps by. */
export const BUILT_IN_TOOLS: ReadonlySet<string> = new Set([
  "bash", "shell", "read", "edit", "multiedit", "patch", "apply_patch", "write", "grep", "glob", "list", "ls",
  "webfetch", "websearch", "task", "subagent", "todowrite", "todoread", "question", "skill", "codesearch", "lsp",
  "invalid", "batch", "plan_enter", "plan_exit", "execute", "context",
]);

/** The bridge's own kinds of step that no engine has, in words. */
const BRIDGE_WORDS: Record<string, ToolWords> = {
  plan: { title: "Plan", done: "Proposed a plan", doing: "Proposing a plan" },
  memory: { title: "Recall", done: "Recalled from memory", doing: "Recalling from memory" },
  hook: { title: "Hook", done: "Ran a hook", doing: "Running a hook" },
  sleep: { title: "Wait", done: "Waited", doing: "Waiting" },
  think: { title: "Think", done: "Thought it over", doing: "Thinking it over" },
  generate_image: { title: "Make an image", done: "Made an image", doing: "Making an image" },
  record_screen: { title: "Record the screen", done: "Recorded the screen", doing: "Recording the screen" },
  lints: { title: "Check for problems", done: "Checked for problems", doing: "Checking for problems" },
  tool_search: { title: "Look up tools", done: "Looked up tools", doing: "Looking up tools" },
  tool: { title: "Use a tool", done: "Used a tool", doing: "Using a tool" },
};

const SESSION = /^ses_[A-Za-z0-9]{1,60}$/;

/** A target from a call's input as a step says it: one short line, never an address. */
function targetOf(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const line = value.replace(/^&/, "").replace(/\s+/g, " ").trim();
  if (!line || SESSION.test(line)) return null;
  return line.length > 40 ? `${line.slice(0, 39).trimEnd()}…` : line;
}

/** What `words` say of a call with `input`, done and while it runs. */
export function toolSaid(words: ToolWords, input: unknown): { title: string; doing: string } {
  const target = words.target && isObject(input) ? targetOf(input[words.target.field]) : null;
  if (!words.target || !target) return { title: words.done, doing: words.doing };
  const named = words.target.quoted ? `“${target}”` : target;
  return { title: words.target.done.replace("{}", named), doing: words.target.doing.replace("{}", named) };
}

/** A name as words: `list_issues` "List issues", `getIssue` "Get issue", `my-server` "My server". */
export function humanized(name: string): string {
  const words = name.replace(/([a-z0-9])([A-Z])/g, "$1 $2").split(/[\s_\-.:/]+/).filter(Boolean).join(" ").toLowerCase();
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : name;
}

/** The engine's name for an MCP server, as its tools are named after it. */
export const engineServerName = (name: string): string => name.replace(/[^a-zA-Z0-9_-]/g, "_");

/** A tool step's words, or null for one clients word themselves. */
export function toolWords(name: string, input: unknown, known: KnownTools): { title: string; doing: string } | null {
  if (!name || BUILT_IN_TOOLS.has(name)) return null;
  const bridge = BRIDGE_WORDS[name];
  if (bridge) return toolSaid(bridge, input);
  // Its server: the longest one known whose name it starts with.
  let server: string | null = null;
  const servers = new Map([...known.servers].map(([id, title]) => [engineServerName(id), title] as const));
  servers.set(OWN_SERVER, "");
  for (const id of servers.keys()) {
    if (name.length > id.length + 1 && name.startsWith(`${id}_`) && (!server || id.length > server.length)) server = id;
  }
  if (server === OWN_SERVER) {
    const tool = name.slice(OWN_SERVER.length + 1);
    const own = known.own.get(tool);
    if (own) return toolSaid(own, input);
    const plain = humanized(tool);
    return { title: plain, doing: plain };
  }
  if (!server) {
    const plain = humanized(name);
    return { title: plain, doing: plain };
  }
  const shown = servers.get(server) || humanized(server);
  // Its own name, without its server's again (Notion's `notion-search`).
  let tool = name.slice(server.length + 1);
  const again = new RegExp(`^${server.replace(/[-_]/g, "[-_]")}[-_](?=.)`, "i");
  tool = tool.replace(again, "");
  const title = `${shown} · ${humanized(tool)}`;
  return { title, doing: title };
}

// MARK: Its sessions' news

/** One session's news in a note to its lead, as its card shows it: its address, its title, what
 * happened in a word or two, and what it said. */
export interface SessionNews {
  session: string;
  title: string;
  status: string;
  said: string;
}

// `[Session finished] "Title" in place (address ses_1), after 3 min:` and what follows; those gathered
// into one message go one after the other, a blank line between them (`wakeLine`, `stoppedLine`,
// `notSummarizedLine` in link/work/wakes.ts). Any kind ends the one before it.
const HEADER = /^\[Session ([a-z ]+)\] "(.*?)"[^"\n]*?\(address ([^)\s]+)\)/gm;
const ENDED = "(It ended without a reply.)";

/** Words as a sentence: a capital first, a stop at the end. */
function sentence(words: string): string {
  const trimmed = words.trim();
  const capital = trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
  return /[.!?…]$/.test(capital) ? capital : `${capital}.`;
}

function newsOf(kind: string, title: string, session: string, rest: string): SessionNews | null {
  switch (kind) {
    case "finished": {
      // `, after 3 min:` then its reply on the lines after, and what it left serving.
      let said = rest.includes(":\n") ? rest.slice(rest.indexOf(":\n") + 2) : rest.startsWith(":") ? rest.slice(1) : "";
      said = said.replace(/\n?\(Still serving: [^\n]*\)\s*$/, "").replace(/\n?\(Still running in the background: [\s\S]+\. It may write again once that ends\.\)\s*$/, "").trim();
      if (said === ENDED) said = "";
      return { session, title, status: "finished", said: said || "It ended without a reply." };
    }
    case "failed": {
      // `stopped: <why>. Starting it again there fails the same way: …`, or `stopped with an error:
      // <why>. Read it with read_conversation, …`: what happened, not what the lead's told to do.
      let said = rest;
      for (const advice of [". Starting it again there fails the same way", ". Read it with read_conversation"]) {
        const at = said.indexOf(advice);
        if (at >= 0) said = said.slice(0, at);
      }
      return { session, title, status: "stopped", said: sentence(said.trim() || "stopped with an error") };
    }
    case "not summarized": {
      let said = rest.startsWith(":") ? rest.slice(1) : rest;
      const at = said.indexOf(" Its conversation and work are as they were.");
      if (at >= 0) said = said.slice(0, at);
      said = said.trim();
      return { session, title, status: "not summarized", said: sentence(!said || said === "It couldn't." ? "It couldn’t summarize its conversation" : said) };
    }
    case "stopped":
      return { session, title, status: "stopped by you", said: "You stopped it." };
    case "asks": {
      // `asks to <what> (ask per_1). Answer with …`, or `asks: <question> (ask frm_1). …`
      let said = rest;
      const at = said.lastIndexOf(" (ask ");
      if (at >= 0) said = said.slice(0, at);
      if (said.startsWith("asks: ")) said = said.slice("asks: ".length);
      return { session, title, status: "asks", said: sentence(said.trim() || "asks something") };
    }
    default:
      // ("still working", from boxes before #420, shows nothing; a kind this box doesn't know, too.)
      return null;
  }
}

/** Each session's news in a note the box wrote to a lead, in order; null where it tells of none in
 * words this box reads (a client shows such a note's text whole). */
export function sessionNews(text: string): SessionNews[] | null {
  const found = [...text.matchAll(HEADER)];
  if (!found.length) return null;
  return found.flatMap((match, index) => {
    const end = index + 1 < found.length ? found[index + 1]!.index! : text.length;
    const rest = text.slice(match.index! + match[0].length, end).trim();
    const news = newsOf(match[1]!, match[2] || "New Session", match[3]!, rest);
    return news ? [news] : [];
  });
}

// MARK: Wake-ups

/** How a wake-up's (or a watch's) message opens, as `kuvo-wake` and the box write it. */
export const WAKE_OPENING = "[Wake-up] ";

/** What a wake-up's card shows: its summary, and the note it opens to. */
export interface WakeCard {
  summary: string;
  note: string;
}

/** The first sentence of `note`'s first line (Markdown's `*` and `` ` `` out of it, and a heading's,
 * quote's or list item's marks at its start, but not underscores, which names and addresses have),
 * or that line whole when it has no sentence's end: a `.`, `!`, `?` or `…` with a space or the
 * line's end after it (not in 1.2 or a site's name). */
export function firstSentence(note: string): string {
  const line = note.split(/\r?\n/).find(each => each.trim()) ?? "";
  const plain = line.replace(/[*`]+/g, "").replace(/^[\s#>-]+/, "").trim();
  const end = /[.!?…](?=\s|$)/.exec(plain);
  return end ? plain.slice(0, end.index + 1) : plain;
}

/** The card a message with `text` and `metadata` is, if it's a wake-up's; null for any other. */
export function wakeCard(text: string, metadata: unknown): WakeCard | null {
  if (!text.startsWith(WAKE_OPENING)) return null;
  const note = text.slice(WAKE_OPENING.length).trim();
  return { summary: wakeMarkOf(metadata)?.kuvo.wake.summary || firstSentence(note) || "Wake-up", note };
}

// MARK: Pages

/** A message with the box's words: its tool steps', its news, or its wake-up's card. */
export function messageWords(message: unknown, known: KnownTools): unknown {
  if (!isObject(message)) return message;
  if (message.type === "assistant" && Array.isArray(message.content)) {
    let changed = false;
    const content = message.content.map(part => {
      if (!isObject(part) || part.type !== "tool" || typeof part.name !== "string") return part;
      const words = toolWords(part.name, isObject(part.state) ? part.state.input : undefined, known);
      if (!words) return part;
      changed = true;
      return { ...part, title: words.title, doing: words.doing };
    });
    return changed ? { ...message, content } : message;
  }
  if (message.type === "user" && typeof message.text === "string" && boxNoteOf(message.text, message.metadata)?.from === "Its sessions") {
    const news = sessionNews(message.text);
    if (!news) return message;
    const metadata = isObject(message.metadata) ? message.metadata : {};
    const kuvo = isObject(metadata.kuvo) ? metadata.kuvo : {};
    return { ...message, metadata: { ...metadata, kuvo: { ...kuvo, news } } };
  }
  if (message.type === "user" && typeof message.text === "string") {
    const wake = wakeCard(message.text, message.metadata);
    if (!wake) return message;
    const metadata = isObject(message.metadata) ? message.metadata : {};
    const kuvo = isObject(metadata.kuvo) ? metadata.kuvo : {};
    return { ...message, metadata: { ...metadata, kuvo: { ...kuvo, wake } } };
  }
  return message;
}

/** A message page or a single-message response with the box's words, its cursor and the rest as
 * they were. */
export function messageWordsPage(value: unknown, known: KnownTools): unknown {
  if (!isObject(value)) return value;
  return { ...value, data: Array.isArray(value.data) ? value.data.map(each => messageWords(each, known)) : messageWords(value.data, known) };
}

/** `tools` as a tool server lists them, each with its title from `words` (after its name, as MCP
 * has it). */
export function titled<T extends { name: string }>(tools: readonly T[], words: Readonly<Record<T["name"], ToolWords>>): (T & { title: string })[] {
  return tools.map(({ name, ...rest }) => ({ name, title: words[name as T["name"]].title, ...rest }) as unknown as T & { title: string });
}
