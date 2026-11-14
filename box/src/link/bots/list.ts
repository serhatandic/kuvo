// This box's bots, listed (`kuvo/bots`).

import type { BotLook, ListedBot } from "../../shared/routes/bots";
import { readWakes } from "../areas/wakes";
import path from "node:path";
import { listOf } from "../files";
import { BOTS, HERE, homeFolder, IS_MAC, K } from "../home";
import { appHost } from "../host-interface";
import { whatDeleteTakes } from "./deletes";
import type { FolderGoes } from "./deletes";
import { botHere, isModel } from "./here";
import { jobOf } from "./jobs";
import { limitsFrom } from "../work/limits";
import { allowedOf } from "../work/asks";

// This machine's bots, as the Mac lists them in `kuvo/bots`: each by its session, with its name,
// look, job, personality and model as the app last described it (or as changed here), and what its
// folder says now (paused, its next wake-up, how many things it watches, whether it runs with the
// coding agents). One without a description or a session isn't listed.
export function listBots(machine?: string): ListedBot[] {
  const bots: { created: string; bot: ListedBot }[] = [];
  const computer = IS_MAC ? HERE : machine || HERE;
  const goes = folderGoes();
  for (const id of listOf(BOTS)) {
    const here = botHere(id);
    if (!here) continue;
    const { folder, described, session, agents } = here;
    const { wakes, watches, paused } = readWakes(folder);
    const job = jobOf(here);
    bots.push({
      created: String(described.created ?? ""),
      bot: {
        id,
        session,
        name: described.name,
        // TODO(types): its look as the app wrote it in bot.json, passed on unchecked (holding it to
        // `BotLook` would take a check here).
        look: described.look as BotLook,
        machine: IS_MAC ? undefined : machine || undefined,
        job: job.line,
        jobDetails: job.details,
        paused,
        wakes: wakes.length ? Math.min(...wakes.map(wake => wake.due)) : undefined,
        watching: watches.length,
        agents,
        personality: typeof described.personality === "string" ? described.personality : undefined,
        model: isModel(described.model) ? described.model : undefined,
        // Its folder (its memory, wake-ups and workspace), and the project it works in, as this
        // box names them; when it was made.
        folder,
        project: typeof described.project === "string" ? described.project : undefined,
        created: typeof described.created === "string" ? described.created : undefined,
        // The limits you set for its work, if any.
        limits: limitsFrom((described.work as { limits?: unknown } | undefined)?.limits) ?? undefined,
        // What you allowed it to do without asking ("Always").
        allowed: allowedOf(described).length ? allowedOf(described) : undefined,
        // What deleting it takes with it, in this box's words.
        deletes: whatDeleteTakes({ computer, folder: goes, wakes: wakes.length, watching: watches.length }),
      },
    });
  }
  // In the order they were made, as the Mac's list is.
  return bots.sort((a, b) => a.created.localeCompare(b.created)).map(entry => entry.bot);
}
/** Where a bot deleted here ends up (`deleteBot`): the Finder's Trash on a Mac whose app answers to
 * take it there, else Kuvo Code's own trash folder, written as you'd find it. */
export function folderGoes(): FolderGoes {
  if (IS_MAC && appHost()) return { to: "trash" };
  const trash = path.join(K, "trash"), home = homeFolder();
  return { to: "kuvo", folder: home && trash.startsWith(home + path.sep) ? `~${trash.slice(home.length)}` : trash };
}
