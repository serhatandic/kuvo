import { readInstalled } from "../../shared/release-formats";
// What a box makes its bots with and tells them: Kuvo Code's words for bots, New Bot's templates,
// and the box's own facts (its name, your time zone, its projects, the personalities you made);
// and whether a model runs with the coding agents.

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { isObject } from "../../shared/json";
import { boxProjects, changeProjects, PROJECTS_FILE } from "../areas/projects";
import { textOf } from "../files";
import { IS_MAC, K, UPDATES } from "../home";
import { botPersonalities, meetPersonalities, readPersonalities } from "../personalities/library";
import { readLink } from "../state";
import { zoneFilesFor, zoneHere } from "../zone/setting";
import { KIT_FILE, readMacKit } from "./mac-kit";
import type { BotPersonality } from "../personalities/library";
import type { BotModel, BotTemplates } from "../../shared/routes/bots";

/** One of New Bot's templates, as the words have it: what the route gives (`kuvo/bots/templates`),
 * and the first message an older release's had, which the route leaves out. */
export type BotTemplate = BotTemplates["templates"][number] & { firstMessage?: string };

/** One of Kuvo Code's own personalities, as the words have it: its name, what it is, and its SF
 * Symbol. */
export interface PresetPersonality {
  id: string;
  name: string;
  description: string;
  symbol: string;
}

/** The colors, shapes and faces a bot can have, by their names. */
export interface BotLooks {
  colors: string[];
  shapes: string[];
  faces: string[];
}

/** Where a bot runs, as it's told: the place, its clock and its browser (`here`'s `mac` or
 * `machine`). */
export type BotPlace = Record<"place" | "clock" | "browser", string> & {
  /** What a machine calls its system in `place`'s ⟦system⟧, by `process.platform` ("darwin", "linux") and "other". */
  systems?: Record<string, string>;
};

/** A rule a new conversation gets, as the engine's settings have it. */
export interface KitRule {
  action: string;
  resource: string;
  /** Or one a newer engine has that this box doesn't know: held back, as "deny" is, and kept as it is. */
  effect: "allow" | "deny" | "ask" | (string & {});
}

/** Kuvo Code's words for bots (`bin/bot-words.json`, `box/src/shared/bot-words.json` in the repository). */
export interface BotWords {
  kuvo: 1;
  templates: BotTemplate[];
  personalities: PresetPersonality[];
  personalityWords: string;
  looks: BotLooks;
  /** What a bot is told, paragraph by paragraph. */
  instructions: string[];
  here: { mac: BotPlace; machine: BotPlace };
  /** A bot's job, as it's told: none, its line, and all of it. */
  job: { none?: string; line?: string; details?: string };
  projectNote: string;
  identityNote: string;
  memory: string;
  greeting: string;
}


/** What this box makes bots with (`readKit`): the words filled in for where it runs, with what's
 * its own. */
export interface BotKit {
  kuvo: 1;
  templates: BotTemplate[];
  /** Kuvo Code's own, each with what a bot is told (yours are the library's: `botPersonalities`). */
  personalities: BotPersonality[];
  personalityWords: string;
  looks: BotLooks;
  /** What a bot is told, whole, with ⟦name⟧, ⟦folder⟧, ⟦personality⟧, ⟦jobLine⟧ and ⟦jobPart⟧ left. */
  instructions: string;
  job: { none: string; line: string; details: string };
  projectNote: string;
  identityNote: string;
  memory: string;
  greeting: string;
  /** Your time zone, as what a bot is told has it. */
  timeZone: string;
}

/** A bot's job, for what it's told: its line, and all of it. */
export interface BotJobText {
  line?: string;
  section?: string;
}

// What's said while this box hasn't Kuvo Code's words for bots (an install that's missing them).
export const NO_KIT = "Kuvo Code on this computer is missing its words for bots: update it, then try again.";
// The coding agents' providers (the Mac's OutsideAgents): a bot on one of theirs runs with them.
const AGENT_PROVIDERS = ["claude-code", "codex-cli", "cursor", "grok-cli", "antigravity"];
export const runsWithAgents = (model: { providerID?: string } | null | undefined): boolean => model?.providerID === "auto" || AGENT_PROVIDERS.includes(String(model?.providerID ?? "").split("@")[0]);

// The release installs `box/src/shared/bot-words.json` in `bin/`. It holds New
// Bot's templates, Kuvo Code's own personalities and the looks a bot can have, and what a bot is
// told, paragraph by paragraph, with ⟦place⟧, ⟦clock⟧ and ⟦browser⟧ for where it runs (`here`:
// this Mac, or a machine, by ⟦machine⟧ and ⟦timeZone⟧), ⟦jobLine⟧ and ⟦jobPart⟧ for its job
// (`job`), and ⟦name⟧, ⟦folder⟧ and ⟦personality⟧ for its own. Every box tells its bots its own
// release's words, whichever Mac connects.
const WORDS_FILE = path.join(K, "bin", "bot-words.json");

export function readWords(file = WORDS_FILE): BotWords | null {
  try {
    const words: BotWords | null = JSON.parse(textOf(file) ?? "");
    return words?.kuvo === 1 && Array.isArray(words.templates) && Array.isArray(words.personalities) && Array.isArray(words.instructions)
      && isObject(words.looks) && isObject(words.here?.mac) && isObject(words.here?.machine) && isObject(words.job)
      && (["personalityWords", "projectNote", "identityNote", "memory", "greeting"] as const).every(key => typeof words[key] === "string") ? words : null;
  } catch {
    return null;
  }
}

// What this box makes bots with: its own words, filled in for where it runs, with its own facts:
// its name (as its peers know it), your time zone (`zone/setting.ts`). What an older Mac gave it
// (`bots/kit.json`) is carried into its own facts first, once (`carryMacKit`). Null without its words.
// `system` is a platform name, only a test passes it: a box says which system it is from its own.
export function readKit(system: string = process.platform): BotKit | null {
  const words = readWords();
  if (!words) return null;
  carryMacKit();
  const timeZone = zoneHere().zone;
  const machine = readLink()?.id || os.hostname();
  // (This Mac's bots are told they go by its clock only while they do: with a zone of yours in their
  // folders, they're told it, as a machine's are.)
  const here = !IS_MAC ? words.here.machine : zoneFilesFor().timezone ? { ...words.here.mac, clock: words.here.machine.clock } : words.here.mac;
  const told = (each: PresetPersonality) => words.personalityWords.replaceAll("⟦name⟧", () => each.name).replaceAll("⟦description⟧", () => each.description);
  return {
    kuvo: 1,
    templates: words.templates,
    personalities: words.personalities.map(each => ({ id: each.id, name: each.name, description: each.description, custom: false, instruction: told(each) })),
    personalityWords: words.personalityWords,
    looks: words.looks,
    instructions: wordsHere(words, here, system, machine, timeZone),
    job: { none: String(words.job.none ?? ""), line: String(words.job.line ?? ""), details: String(words.job.details ?? "") },
    projectNote: words.projectNote, identityNote: words.identityNote, memory: words.memory, greeting: words.greeting,
    timeZone,
  };
}

// What an older Mac's kit had that the box keeps itself now, carried in whenever there's a kit (a
// Mac app of 0.7 may write it again): its projects, while the box has no list of its own yet; the
// personalities you made that the library hasn't had (at count 0, so any copy's own, or a removal,
// wins). Your zone is carried by its setting (`zone/setting.ts`).
// COMPAT(0.7 bot-kit): what a Mac of 0.7 or before gave a box, carried into its own facts. Goes 3 months after 0.8.0.
export function carryMacKit(): void {
  const kit = readMacKit();
  if (!kit) return;
  try {
    const projects = Array.isArray(kit.projects) ? kit.projects.filter(each => typeof each?.directory === "string" && path.isAbsolute(each.directory)) : [];
    if (projects.length && !fs.existsSync(PROJECTS_FILE)) {
      changeProjects(now => (now.length ? now : projects.map(each => ({ directory: each.directory, ...(typeof each.name === "string" && each.name && each.name !== path.basename(each.directory) ? { name: each.name } : {}), folders: [] }))));
    }
  } catch {}
  try {
    const library = readPersonalities();
    const missing = (Array.isArray(kit.personalities) ? kit.personalities : []).filter(each => each?.custom && typeof each.id === "string"
      && !library.personalities.some(mine => mine.id === each.id) && !library.removed.some(gone => gone.id === each.id));
    if (missing.length) meetPersonalities({ personalities: missing.map(({ id, name, description }) => ({ id, name, description, updated: 0 })), removed: [] });
  } catch {}
}

// What a bot is told, filled in for where it runs: `here` (this Mac, or a machine by its name,
// system and your time zone).
export function wordsHere(words: Pick<BotWords, "instructions">, here: BotPlace, system: string, machine: string, timeZone: string | null | undefined): string {
  // A machine says which system it is, from the box's own: "the Mac", "the Linux machine".
  const systems = here.systems ?? {};
  const named = String(systems[system] ?? systems.other ?? "machine");
  return words.instructions.join("\n\n").replace(/⟦(place|clock|browser)⟧/g, (_, key: "place" | "clock" | "browser") => String(here[key] ?? ""))
    .replaceAll("⟦system⟧", () => named).replaceAll("⟦machine⟧", () => machine).replaceAll("⟦timeZone⟧", () => timeZone || "UTC");
}

// A release's kit from before boxes had their words of their own (`bin/bot-kit.json`), gone once no
// release this box installed lists it (so its own files still look as it installed them).
export function forgetOldKit(): void {
  const file = path.join(K, "bin", "bot-kit.json");
  try {
    const installed: { files?: unknown } | null = readInstalled(UPDATES);
    if (isObject(installed?.files) && "bin/bot-kit.json" in installed.files) return;
  } catch { return; }
  fs.rmSync(file, { force: true });
}



// `kuvo/bots/templates`, as the Mac's: what New Bot offers, the model a bot starts on here (by
// this machine's name, as its bots' `machine` has it; `startingModel`, by the box's rule), and its projects.
export function botTemplates(kit: BotKit, machine?: string | null, startingModel: BotModel | null = null): BotTemplates {
  return {
    templates: kit.templates.map(({ firstMessage: _, ...template }) => template),
    personalities: botPersonalities(kit).map(({ instruction: _, ...personality }) => personality),
    looks: kit.looks,
    startingModel: { [IS_MAC ? "mac" : machine || "machine"]: startingModel },
    projects: boxProjects().map(project => ({ ...project, machine: IS_MAC ? undefined : machine || undefined })),
  };
}

// What a bot is told, from the kit's words with its own name, folder, personality and project,
// and its job (`{line, section}`), for one that has one.
export function botInstructions(kit: BotKit, name: string, folder: string, personality: string | null | undefined, project?: string | null, job?: BotJobText): string {
  const working = Boolean(job?.section || job?.line);
  let text = kit.instructions.replaceAll("⟦name⟧", () => name).replaceAll("⟦folder⟧", () => folder);
  text = personality ? text.replace("⟦personality⟧", () => personality) : text.replace("\n⟦personality⟧\n", "");
  if (project) text = text.replace(`\n\n${kit.identityNote}`, () => `\n\n${kit.projectNote}\n\n${kit.identityNote}`);
  text = text.replace("⟦jobLine⟧", () => (job?.line ? kit.job.line.replace("⟦job⟧", () => job.line!) : ""));
  const part = working ? kit.job.details.replace("⟦jobDetails⟧", () => job!.section || job!.line!) : kit.job.none;
  return part ? text.replace("⟦jobPart⟧", () => part) : text.replace(/(\n\n)?⟦jobPart⟧/, "");
}