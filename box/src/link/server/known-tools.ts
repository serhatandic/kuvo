// What the box words tool steps with (shared/message-words.ts): Kuvo Code's own tools' words, from
// where each is defined (the browser's and the agents' own, the vault's, the sandbox's and the Work
// Queue's), and the MCP servers this box's engine knows, with the names shown for them: the
// integrations' and the catalog's by their titles, any other in its settings by its own name.

import { WORDS } from "../../browser/tools";
import { SERVICES } from "../../shared/integrations/table";
import { SERVERS as CATALOG } from "../../shared/integrations/servers";
import type { KnownTools, ToolWords } from "../../shared/message-words";
import { isObject } from "../../shared/json";
import { SANDBOX_WORDS } from "../fence/outside";
import { VAULT_WORDS } from "../vault/tools";
import { QUEUE_WORDS } from "../work-queue/tools";
import { readSettings, SERVERS, valueAt } from "../areas/config";

/** Every one of Kuvo Code's own tools, by name, with its words. */
let ownToolWords: ReadonlyMap<string, ToolWords> | undefined;

/** The servers Kuvo Code names itself: an integration's title over the catalog's. */
const NAMED: ReadonlyMap<string, string> = new Map([
  ...CATALOG.map(server => [server.id, server.title] as const),
  ...SERVICES.filter(service => service.way !== "browser").map(service => [service.id, service.title] as const),
]);

/** What tool steps are worded with here, the engine's settings as they are now (`settings`, for a test). */
export function knownTools(settings: unknown = readSettings().settings): KnownTools {
  const servers = new Map<string, string>(NAMED);
  const yours = valueAt(settings, SERVERS);
  if (isObject(yours)) for (const name of Object.keys(yours)) if (!servers.has(name)) servers.set(name, "");
  // The tool implementations can import the dispatcher in turn. Read their
  // words after the modules have loaded, including in a backup worker.
  ownToolWords ??= new Map(Object.entries({ ...WORDS, ...VAULT_WORDS, ...SANDBOX_WORDS, ...QUEUE_WORDS }));
  return { own: ownToolWords, servers };
}
