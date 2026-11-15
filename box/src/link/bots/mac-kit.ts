// What an older Mac app gave this box (`bots/kit.json`, the retired Mac app's Bots.MachineKit), read
// now only to carry what it had into the box's own facts as the box updates: your time zone (its
// setting, `zone/setting.ts`), its projects (`box/projects.json`) and the personalities you made
// (the library). A Mac app of 0.7 or before may still write it; nothing else reads it.

import path from "node:path";
import { textOf } from "../files";
import { BOTS } from "../home";
import type { OfferedProject } from "../areas/projects";
import type { BotPersonality } from "../personalities/library";
import type { BotModel } from "../../shared/routes/bots";

/** What a Mac gave this box of what's its own: the name it calls this machine, the personalities you
 * made, the model a bot started on, its projects, your time zone and (for a machine) that zone's
 * rules, base64. */
export interface MacKit {
  kuvo: 1;
  machine?: string;
  personalities: BotPersonality[];
  startingModel?: BotModel | null;
  projects: OfferedProject[];
  timeZone: string;
  zoneinfo?: string;
}

// COMPAT(0.7 bot-kit): the kit a Mac of 0.7 or before wrote, read only to carry it in. Goes 3 months after 0.8.0.
export const KIT_FILE = path.join(BOTS, "kit.json");

// What a Mac gave this box of what's its own (`bots/kit.json`), or null.
export function readMacKit(file = KIT_FILE): MacKit | null {
  try {
    const kit: MacKit | null = JSON.parse(textOf(file) ?? "");
    return kit?.kuvo === 1 ? kit : null;
  } catch {
    return null;
  }
}
