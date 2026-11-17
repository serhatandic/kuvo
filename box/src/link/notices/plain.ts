// A notice is in plain words, kept short (PROTOCOL.md, "Notifications"): what is asked in a few
// words, never a command line, a path, a flag or a code. Those belong on the card in the
// conversation, and in logs. What the box words itself is here; an agent's own sentence is cut by
// `oneSentence`.

import { LINE_LONGEST, SENTENCE_LONGEST, cutAtWord, oneSentence, plain, plainAddresses } from "./sentence";
import type { Notice } from "./notice";

// A program's name as people say it: "du", "git", "npm", not a path or a quoted string.
const PROGRAM = /^[A-Za-z][\w.+-]{0,24}$/;
// What a command line runs first in each of its parts (a pipeline's first: what it pipes into is
// part of it), past a `cd` and settings before it.
const programsOf = (line: string): string[] => line.split(/\s*(?:&&|\|\||;)\s*/).flatMap(part => {
  const words = part.trim().split(/\s+/);
  while (words.length && /^[A-Za-z_]\w*=/.test(words[0]!)) words.shift();
  const name = words[0]?.replace(/^.*\//, "") ?? "";
  return PROGRAM.test(name) && name !== "cd" ? [name] : [];
});

/** The programs a command's lines run, once each, in order ("du", "git"). */
export const programsRun = (lines: unknown[]): string[] => [...new Set(lines.flatMap(line => programsOf(String(line))))];

/** "du", "du and git", "du, git and 2 more": up to two named. */
const named = (names: string[]): string => names.length <= 2 ? names.join(" and ") : `${names[0]}, ${names[1]} and ${names.length - 2} more`;

/** What a command asks to run, in plain words: "a command", "du", "du and git". Its lines are never
 * shown. `whole`: the words say all it covers (each line is a program's name alone). */
export function commandWords(lines: unknown[]): { words: string; whole: boolean } {
  const specific = lines.map(line => String(line)).filter(line => line.trim() && line.trim() !== "*");
  const names = programsRun(specific);
  if (!names.length) return { words: "a command", whole: specific.length === 0 };
  return { words: named(names), whole: names.length <= 2 && specific.every(line => PROGRAM.test(line.trim())) };
}

// A file as people name it ("notes.md"), where it's one plain name; null for a pattern or anything else.
const fileName = (resource: string): string | null => {
  const name = resource.replace(/\/+$/, "").split("/").pop() ?? "";
  return /^[\w][\w .+@()-]{0,38}$/.test(name) ? name : null;
};
const hostOf = (resource: string): string | null => /^https?:\/\/(?:www\.)?([^\s/?#:]+)/i.exec(resource)?.[1] ?? null;

const OTHER: Record<string, string> = { task: "start a helper", skill: "use a skill", websearch: "search the web", codesearch: "search code", glob: "look through files", grep: "look through files", list: "look at a folder" };

/** A permission, asked of you, in a short plain line ("Wants to run du."): what it would do, naming
 * at most a program, a file's name or a site, never a command line, a path or a flag. `whole`: the
 * line says all it covers, so a notification's buttons may answer it; false where it covers what the
 * line leaves out, as most do: you open the conversation, where the card shows it all. */
export function permissionLine(request: { action: string; resources?: unknown }): { line: string; whole: boolean } {
  const resources = (Array.isArray(request.resources) ? request.resources : []).map(String).filter(each => each !== "*");
  const none = resources.length === 0;
  const say = (words: string, whole = none) => ({ line: cutAtWord(`Wants to ${words}.`, LINE_LONGEST), whole });
  const files = (verb: string, one: string, many: string) => {
    const only = resources.length === 1 ? fileName(resources[0]!) : null;
    return say(none ? one : only ? `${verb} ${only}` : resources.length > 1 ? `${verb} ${resources.length} ${many}` : one);
  };
  switch (request.action) {
    case "edit": case "write": case "patch": return files("edit", "edit files", "files");
    case "read": return files("read", "read a file", "files");
    case "shell": case "bash": {
      const { words, whole } = commandWords(resources);
      return say(`run ${words}`, whole);
    }
    case "external_directory": return say("work outside the project");
    case "webfetch": {
      const hosts = [...new Set(resources.map(hostOf))];
      return say(hosts.length === 1 && hosts[0] ? `open ${hosts[0]}` : "open a web page");
    }
    default: return say(OTHER[request.action] ?? "use a tool");
  }
}

/** `text` (an agent's question, a prompt) as it's shown: its Markdown and addresses in plain words,
 * cut at a word. */
export const shortWords = (text: unknown, longest = SENTENCE_LONGEST): string => cutAtWord(plain(plainAddresses(String(text ?? ""))), longest);

/** A failure's reason, where it's in words (a sentence of a few plain words) and not a code: an
 * error name, a status number, a path, an address or something in braces. "" where it isn't. */
export function reasonWords(reason: unknown): string {
  const first = String(reason ?? "").split("\n").map(each => each.trim()).find(Boolean) ?? "";
  // (Looked at as written: an address or path in it is a code, though said as a name below.)
  const sentence = /[/\\]/.test(first) ? "" : oneSentence(first, 60);
  if (!sentence || sentence.split(/\s+/).length < 3) return "";
  const code = /[{}[\]<>/\\]|\b[A-Z][A-Z0-9_]{3,}\b|\b\w*\d{3,}\w*\b|\w+_\w+|\b\w+\.\w+\(|0x[0-9a-fA-F]+/;
  if (code.test(sentence)) return "";
  return sentence;
}

/** `notice` as it's shown, kept short: a title and subtitle cut at a word, a body of more than a
 * sentence (a report, a long reason) as its first, in plain words. What the box words short is
 * as it was. */
export function shortNotice(notice: Notice): Notice {
  const short = { ...notice };
  if (short.title) short.title = cutAtWord(short.title.replace(/\s+/g, " ").trim(), 60);
  if (short.subtitle) short.subtitle = cutAtWord(short.subtitle.replace(/\s+/g, " ").trim(), 60);
  if (short.body && (short.body.length > SENTENCE_LONGEST || short.body.includes("\n"))) short.body = oneSentence(short.body) || cutAtWord(short.body.replace(/\s+/g, " ").trim(), SENTENCE_LONGEST);
  return short;
}
