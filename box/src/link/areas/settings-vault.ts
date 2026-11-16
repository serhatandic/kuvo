// Settings › Vault (`kuvo/settings/vault`, PROTOCOL.md "A box's settings", Vault): this computer's
// vault as a settings page every client draws by kind: what agents wait on you for, its items (Add,
// Edit…, Who Can Use It…, Delete…), Pause Agents' Use and who's paused, and its activity. Everything
// is read and changed through the vault's own routes (areas/vault.ts), asked as this request's client
// asks them, so who may do what, a locked vault, the activity it notes and its other rules are
// theirs. No value is ever on the page: a secret field opens empty, and Show reads it through the
// vault's own values route (`shows`), which notes that it was shown. Values typed in a form reach
// the vault only with its submit; a check gets them masked (the client's) and never says them.
// Platform: the same on a Mac's box (its values in the Keychain) and a machine's (its own encrypted
// store, locked after a restart until a client that keeps its key unlocks it).

import { ITEM_KINDS } from "../../shared/routes/vault";
import type { VaultActivity, VaultAsk, VaultGrant, VaultItem, VaultState, VaultWho } from "../../shared/routes/vault";
import type { SettingAction, SettingBadge, SettingField, SettingFieldWords, SettingForm, SettingGroup, SettingItem, SettingRow, SettingsChecked, SettingsPage, SettingsSection } from "../../shared/routes/settings-pages";
import { isObject } from "../../shared/json";
import path from "node:path";
import { listBots } from "../bots/list";
import { BOTS, VAULT_FOLDER } from "../home";
import type { RouteContext } from "../server/context";
import { listAsks } from "../vault/asks";
import { agentsUseVault } from "../vault/gate";
import { holdsVault } from "../vault/holder";
import { isLocked, siteOf } from "../vault/store";
import { readZoneSetting, zoneHere } from "../zone/setting";
import { asked, item, mtime, refusal, row, said, STALE, staleOrRefusal, unreadPage } from "./settings-kit";
import type { Refused, Section } from "./settings-kit";
import { vaultRoute } from "./vault";

const TITLE = "Vault";
type Kind = VaultItem["kind"];

/** This computer in a sentence: "this Mac" to the app beside its box, its name to any other. */
const computerHere = (context: RouteContext): string =>
  context.request.fromApp === true ? (process.platform === "darwin" ? "this Mac" : "this computer") : context.info.name || "this computer";
const capitalized = (words: string) => words.charAt(0).toUpperCase() + words.slice(1);
/** At a sentence's start: "This Mac", a computer's name as it is. */
const atStart = (words: string) => (words.startsWith("this ") ? capitalized(words) : words);
/** "lokum’s", "this Mac’s". */
const whose = (computer: string) => `${computer}’s`;

// MARK: Reading

/** What the page is made from, as the vault's routes answer this client. */
interface Facts {
  state: VaultState;
  items: VaultItem[];
  grants: VaultGrant[];
  asks: VaultAsk[];
  activity: VaultActivity[];
  /** The bots on this computer, by id. */
  bots: { id: string; name: string }[];
}

/** The most activity the page lists. */
const ACTIVITY_SHOWN = 50;

async function facts(context: RouteContext): Promise<Facts | { status: number; value: any }> {
  const vault = await asked(context, vaultRoute, "GET", "kuvo/vault");
  if (vault.status !== 200) return vault;
  const state = vault.value.state as VaultState;
  const asks = await asked(context, vaultRoute, "GET", "kuvo/vault/asks");
  const activity = state.locked || !state.made ? null : await asked(context, vaultRoute, "GET", `kuvo/vault/activity?limit=${ACTIVITY_SHOWN}`);
  return {
    state,
    items: [...(vault.value.items as VaultItem[])].sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" })),
    grants: vault.value.grants as VaultGrant[],
    asks: asks.status === 200 ? asks.value.asks : [],
    activity: activity?.status === 200 ? activity.value.entries : [],
    bots: listBots().map(bot => ({ id: bot.id, name: bot.name })).sort((a, b) => a.name.localeCompare(b.name)),
  };
}

const KIND_TITLES: Record<Kind, string> = { login: "Sign-in", key: "API Key", file: "File", note: "Note" };

/** Its line in the list: what it is, then who it signs in as and where, the file it is, or where it's used. */
function lineOf(each: VaultItem): string {
  const more = each.kind === "login" ? [each.username, each.sites[0]]
    : each.kind === "file" ? [each.fileName && each.fileName !== each.name ? each.fileName : undefined]
    : each.kind === "note" ? [each.readable ? "Bots may read it" : undefined]
    : [each.sites.length ? each.sites.join(", ") : each.fields.length > 1 ? each.fields.join(", ") : undefined];
  return [KIND_TITLES[each.kind], ...more].filter(Boolean).join(" · ");
}

/** How it's used, at a glance: on its own, or by asking; nothing where no one may use it, as most items are. */
function badgeOf(grants: VaultGrant[]): SettingBadge | undefined {
  if (grants.some(grant => grant.how === "own")) return { words: "On its own", tone: "good" };
  return grants.length ? { words: "Asks", tone: "quiet" } : undefined;
}

/** A field's name as a label: "password" as Password, "issuer_id" as Issuer Id. */
const label = (field: string): string =>
  field === "password" ? "Password" : field === "text" ? "Text" : field === "value" ? "Value" : field.split("_").filter(Boolean).map(capitalized).join(" ");

/** The kept fields an item opens with, its first one (`password`, `value`, `file`, `text`) apart. */
const FIRST: Record<Kind, string> = { login: "password", key: "value", file: "file", note: "text" };

// MARK: Forms

const SHOW = (id: string, field: string) => ({ route: `kuvo/vault/items/${encodeURIComponent(id)}/values`, field });

/** An item's fields: Add Item's (`each` null: every kind's, each shown for its kind) or one item's
 * own (Edit…), with what it has now, never a value. */
function itemFields(each: VaultItem | null): SettingField[] {
  const kind = each?.kind;
  const only = (...kinds: Kind[]) => (each ? {} : { shown: { field: "kind", is: kinds } });
  const has = (kinds: Kind[]) => !kind || kinds.includes(kind);
  const unchanged = each ? { placeholder: "Unchanged" } : {};
  const fields: SettingField[] = [];
  if (!each) fields.push({ id: "kind", title: "Kind", kind: "choice", value: "login", choices: ITEM_KINDS.map(id => ({ id, title: KIND_TITLES[id] })) });
  fields.push({ id: "name", title: "Name", kind: "text", value: each?.name ?? "", placeholder: "What it’s for, like GitHub" });
  if (has(["login"])) {
    fields.push({ id: "site", title: "Site", kind: "text", value: each ? each.sites.join(", ") : "", placeholder: "github.com", line: "It’s filled in only on this site and those under it.", ...only("login") });
    fields.push({ id: "username", title: "Username", kind: "text", value: each?.username ?? "", placeholder: "Email or username", ...only("login") });
    fields.push({ id: "password", title: "Password", kind: "secret", ...unchanged, ...(each?.fields.includes("password") ? { shows: SHOW(each.id, "password") } : {}), ...only("login") });
    fields.push({ id: "signInOnly", title: "Only for Signing In", kind: "switch", value: each ? each.signInOnly === true : true, line: "Agents sign in with it in the browser, and can’t hand it to a command.", ...only("login") });
  }
  if (has(["key"])) {
    fields.push({ id: "hosts", title: "Hosts", kind: "text", value: each ? each.sites.join(", ") : "", placeholder: "api.example.com (optional)", ...only("key") });
    fields.push({ id: "value", title: "Value", kind: "secret", ...unchanged, ...(each?.fields.includes("value") ? { shows: SHOW(each.id, "value") } : {}), ...only("key") });
    // Its other kept fields, each its own; new ones by name.
    for (const field of each?.fields.filter(name => name !== "value") ?? []) {
      fields.push({ id: `field-${field}`, title: label(field), kind: "secret", placeholder: "Unchanged", shows: SHOW(each!.id, field) });
    }
    // (One of them taken away, a save at a time: what 0.7's remove button on each did.)
    const extras = each?.fields.filter(name => name !== "value") ?? [];
    if (extras.length) {
      fields.push({ id: "remove", title: "Remove a Field", kind: "choice", value: "none", choices: [{ id: "none", title: "None" }, ...extras.map(field => ({ id: field, title: label(field) }))] });
    }
    fields.push({
      id: "more", title: each ? "Add Fields" : "Other Fields", kind: "pairs", secret: true, value: [], pairs: { key: "field_name", value: "Value" },
      line: "For a key with more than one part, like an issuer ID.", ...only("key"),
    });
  }
  if (has(["file"])) {
    fields.push({ id: "file", title: "File", kind: "file", ...(each?.fileName ? { value: { name: each.fileName } } : {}), file: { choose: "Choose File…", none: "No file chosen" }, line: "At most about 700 KB.", ...only("file") });
  }
  if (has(["note"])) {
    fields.push({ id: "text", title: "Text", kind: "secret", multi: true, ...(each ? { placeholder: "Unchanged. Type here to replace it." } : {}), ...(each?.fields.includes("text") ? { shows: SHOW(each.id, "text") } : {}), ...only("note") });
    fields.push({ id: "readable", title: "Bots May Read It", kind: "switch", value: each?.readable === true, line: "For what isn’t a secret, like a contact for App Review.", ...only("note") });
  }
  return fields;
}

/** Where its values are kept, in a sentence. */
const keptWords = (state: VaultState, computer: string) => state.store === "keychain"
  ? `Kept in ${whose(computer)} Keychain. Agents fill them in or hand them to a command without ever seeing them, only as you allow.`
  : `Kept on ${computer}, encrypted, with a key that Kuvo on your iPhone keeps. Agents fill them in or hand them to a command without ever seeing them, only as you allow. Its browser runs as you, so a program you or an agent runs there could get an item an agent there may use.`;

const addForm = (known: Facts, computer: string): SettingForm => ({ title: "Add Item", submit: "Add", fields: itemFields(null), note: keptWords(known.state, computer) });
const editForm = (each: VaultItem): SettingForm => ({ title: each.name, submit: "Save", fields: itemFields(each), note: "A value left empty stays as it is." });

const text = (value: unknown): string => (typeof value === "string" ? value : "");
const pairs = (value: unknown): { key: string; value: string }[] =>
  Array.isArray(value) ? value.filter(isObject).map(each => ({ key: text(each.key).trim(), value: text(each.value) })) : [];

const SHORTEST = 4;
const FIELD = /^[a-z][a-z0-9_]{0,39}$/;
/** The biggest file kept, in bytes: its base64 stays under the vault's megabyte a value. */
const LARGEST_FILE = 700_000;

/** What a client sends to make or change an item (`kuvo/vault/items`). */
interface Given {
  kind: Kind;
  name: string;
  sites: string[];
  username: string;
  fileName?: string;
  readable?: boolean;
  signInOnly?: boolean;
  values: Record<string, string | null>;
}

/** The form's values read, for its check and its submit alike: each field's words, whether it can
 * be sent, and what it makes (`given`). A value is never said, only whether it's there and long
 * enough: a check gets each masked. */
function readForm(known: Facts, each: VaultItem | null, values: Record<string, unknown>, submit = false): SettingsChecked & { given?: Given } {
  const fields: SettingFieldWords[] = [];
  let ready = true;
  const say = (id: string, words: string, tone: SettingFieldWords["tone"]) => {
    fields.push({ id, words, tone });
    if (tone === "warning") ready = false;
  };
  const kind = (each?.kind ?? values.kind) as Kind;
  if (!ITEM_KINDS.includes(kind)) return { fields: [{ id: "kind", words: "Pick what it is.", tone: "warning" }], ready: false };

  const name = text(values.name).trim();
  if (!name) {
    say("name", "Give it a name.", "quiet");
    ready = false;
  } else if (known.items.some(other => other.id !== each?.id && other.name.toLowerCase() === name.toLowerCase())) {
    say("name", `There’s an item called “${name}” already.`, "warning");
  }

  const siteField = kind === "login" ? "site" : "hosts";
  const sites: string[] = [];
  if (kind === "login" || kind === "key") {
    for (const typed of text(values[siteField]).split(/[\s,]+/).filter(Boolean)) {
      const site = siteOf(typed);
      if (!site) say(siteField, `“${typed.slice(0, 80)}” isn’t a site.`, "warning");
      else if (!sites.includes(site)) sites.push(site);
    }
    if (kind === "login" && !sites.length && !fields.some(word => word.id === "site")) {
      say("site", "The site it’s for, like github.com.", "quiet");
      ready = false;
    }
  }

  const given: Given = { kind, name, sites, username: kind === "login" ? text(values.username).trim() : "", values: {} };
  if (kind === "login") given.signInOnly = values.signInOnly === true;
  if (kind === "note") given.readable = values.readable === true;

  /** A secret: required for a new item; otherwise empty is unchanged. */
  const secret = (id: string, field: string, required: boolean) => {
    const typed = text(values[id]);
    if (!typed) {
      if (required) {
        say(id, kind === "note" ? "What it says." : kind === "login" ? "Its password." : "Its value.", "quiet");
        ready = false;
      }
      return;
    }
    if (Array.from(typed).length < SHORTEST) return say(id, `A value needs at least ${SHORTEST} characters, so it can be hidden from agents.`, "warning");
    given.values[field] = typed;
  };
  if (kind === "login") secret("password", "password", !each);
  if (kind === "key") {
    secret("value", "value", !each);
    const removed = text(values.remove);
    for (const field of each?.fields.filter(name => name !== "value") ?? []) {
      if (field === removed) {
        if (text(values[`field-${field}`])) say(`field-${field}`, `${label(field)} is being removed: leave it empty, or don’t remove it.`, "warning");
        else given.values[field] = null;
      } else secret(`field-${field}`, field, false);
    }
    for (const pair of pairs(values.more).filter(pair => pair.key || pair.value)) {
      const field = pair.key.toLowerCase().replaceAll(" ", "_");
      if (!field) say("more", "Give each value a name.", "warning");
      else if (!FIELD.test(field)) say("more", `${field.slice(0, 40)} isn’t a field’s name: lowercase letters, digits and _, starting with a letter.`, "warning");
      else if (field === "value" || field in given.values || each?.fields.includes(field)) say("more", `${field} is given twice.`, "warning");
      else if (!pair.value) say("more", `Give ${field} a value.`, "warning");
      else if (Array.from(pair.value).length < SHORTEST) say("more", `A value needs at least ${SHORTEST} characters, so it can be hidden from agents.`, "warning");
      else given.values[field] = pair.value;
    }
  }
  if (kind === "file") {
    const file = isObject(values.file) ? values.file : null;
    const data = typeof file?.data === "string" ? file.data : null;
    const size = data !== null ? Buffer.byteLength(data, "base64") : typeof file?.size === "number" ? file.size : null;
    // (Sent with its bytes only: a check's name and size alone add nothing.)
    if (size === null || (submit && data === null && !each)) {
      // (The one it has, sent back as it opened: unchanged.)
      if (!each) {
        say("file", "Choose the file.", "quiet");
        ready = false;
      }
    } else if (size > LARGEST_FILE) {
      say("file", "That file is too big for the vault (at most about 700 KB).", "warning");
    } else if (size === 0) {
      say("file", "That file is empty.", "warning");
    } else {
      const fileName = path.basename(text(file?.name).trim());
      if (fileName) given.fileName = fileName;
      if (data !== null) given.values.file = data;
    }
  }
  if (kind === "note") secret("text", "text", !each);
  return { fields, ready, ...(ready ? { given } : {}) };
}

/** Who may use an item: the bots and projects listed by name, then everyone (others). */
const whoId = (who: VaultWho): string => (who.bot ? `bot-${who.bot}` : who.project ? `project-${who.project}` : "everyone");
const HOWS = [{ id: "own", title: "On Its Own" }, { id: "ask", title: "Asks Each Time" }, { id: "no", title: "No" }];

function whoForm(known: Facts, each: VaultItem): SettingForm {
  const grants = known.grants.filter(grant => grant.item === each.id);
  const how = (who: VaultWho) => grants.find(grant => whoId(grant.who) === whoId(who))?.how ?? "no";
  const listedBots = known.bots.filter(bot => grants.some(grant => grant.who.bot === bot.id));
  const projects = grants.flatMap(grant => (grant.who.project ? [grant.who.project] : []));
  const fields: SettingField[] = [
    // (Each lets someone use what's yours: a client makes sure it's you first.)
    ...listedBots.map(bot => ({ id: `bot-${bot.id}`, title: bot.name, kind: "choice" as const, value: how({ bot: bot.id }), choices: HOWS, verified: true })),
    ...projects.map(project => ({ id: `project-${project}`, title: path.basename(project) || project, line: project, kind: "choice" as const, value: how({ project }), choices: HOWS, verified: true })),
    { id: "everyone", title: listedBots.length || projects.length ? "Other sessions and bots" : "Every agent", kind: "choice", value: how({ everyone: true }), choices: HOWS, verified: true },
  ];
  const others = known.bots.filter(bot => !listedBots.includes(bot));
  if (others.length) {
    fields.push({
      id: "add-bot", title: "Let a Bot Use It", kind: "choice", value: "none", verified: true, choices: [{ id: "none", title: "No One Else" }, ...others.map(bot => ({ id: bot.id, title: bot.name }))],
      line: "It asks you each time, until you choose On Its Own.",
    });
  }
  const note = each.kind === "login"
    ? `Only on ${each.sites.join(", ")}. Agents never see the password: Kuvo fills it in.`
    : "Agents never see it: Kuvo fills it in, or hands it to a command they run.";
  return { title: `Who Can Use “${each.name}”`, submit: "Save", fields, note };
}

// MARK: What agents ask

const askerOf = (ask: VaultAsk) => ask.botName ?? ask.sessionName ?? "An agent";
/** Whether you may answer it for good: never from a seat, nor where the box said it'd only be once. */
const mayAlways = (ask: VaultAsk) => !ask.group && ask.always !== false;

function askRow(ask: VaultAsk, known: Facts): SettingItem {
  const asker = askerOf(ask);
  const title = ask.kind === "use" ? `${asker} wants to use “${ask.itemName ?? "an item"}”` : `${asker} needs ${ask.need ?? "something from your vault"}`;
  const line = [ask.why, ask.site ? `On ${ask.site}` : undefined].filter(Boolean).join(" · ") || undefined;
  const no: SettingAction = { id: "no", title: "Don’t Allow" };
  if (ask.kind === "use") {
    const always: SettingAction[] = mayAlways(ask) ? [{
      id: "always", title: "Always Allow…", verified: true,
      confirm: { title: `Let ${asker} use “${ask.itemName ?? "it"}” on its own?`, body: "From now on it uses it without asking you. Change that in its Who Can Use It.", button: "Always Allow" },
    }] : [];
    return item({ id: `ask-${ask.id}`, title, ...(line ? { line } : {}), kind: "actions", actions: [no, { id: "once", title: "Allow Once" }, ...always] });
  }
  const give: SettingAction = known.items.length ? {
    id: "give", title: "Give…",
    form: {
      title: `Give ${asker} an Item`, submit: "Give",
      fields: [
        { id: "item", title: "Item", kind: "choice", value: "", choices: known.items.map(each => ({ id: each.id, title: each.name, line: lineOf(each) })) },
        ...(mayAlways(ask) ? [{ id: "how", title: "For", kind: "choice" as const, value: "once", verified: true, choices: [{ id: "once", title: "This Once" }, { id: "always", title: "From Now On" }] }] : []),
      ],
      ...(ask.need ? { note: `It needs ${ask.need}.` } : {}),
    },
  } : { id: "give", title: "Give…", can: { why: "There’s nothing in the vault to give yet." } };
  return item({ id: `ask-${ask.id}`, title, ...(line ? { line } : {}), kind: "actions", actions: [no, give] });
}

// MARK: Activity

/** Whether it's one you'd want to look at: refused, or a bot paused. */
const isWarning = (entry: VaultActivity) => entry.what === "refused" && entry.how !== "no grant" && entry.how !== "autopilot";

/** What happened, in a sentence. What you did from the client reading it (`mine`) says only what
 * happened; who and where are said when it was somewhere else. A line no client is named on (an
 * older box's) says what happened without saying who. */
export function sentence(entry: VaultActivity): string {
  const { what, how, botName, sessionName, client, site, program } = entry;
  const named = entry.itemName ? `“${entry.itemName}”` : undefined;
  if (entry.mine === true) {
    const thing = named ?? "an item";
    const agent = botName ?? sessionName ?? "an agent";
    switch (what) {
      case "allowed": return `Let ${agent} use ${thing}${how === "always" ? " from now on" : " once"}`;
      case "declined": return `Said no to ${agent} using ${thing}`;
      case "added": return `Added ${thing}`;
      case "changed": return `Changed ${thing}`;
      case "removed": return `Removed ${thing}`;
      case "shown": return `Looked at ${thing}`;
    }
  }
  if (client === undefined) {
    const agent = botName ?? sessionName ?? "An agent";
    switch (what) {
      case "allowed": return how === "always" ? `${agent} may use ${named ?? "an item"} from now on` : `${agent} was allowed ${named ?? "an item"} once`;
      case "declined": return `${agent} wasn’t allowed ${named ?? "an item"}`;
      case "added": return `${named ?? "An item"} was added`;
      case "changed": return `${named ?? "An item"} was changed`;
      case "removed": return `${named ?? "An item"} was removed`;
      case "shown": return `${named ?? "An item"} was looked at`;
    }
  }
  const you = client !== undefined ? `You, on ${client === "Mac" ? "your Mac" : client},` : "You";
  const who = botName ?? sessionName ?? (client === undefined ? "An agent" : you);
  const thing = named ?? "an item";
  const where = site ? ` on ${site}` : program ? ` with ${program}` : "";
  switch (what) {
    case "used": return `${who} used ${thing}${where}` + (how === "once" ? ", allowed once" : how === "always" ? ", now always allowed" : "");
    case "asked": return entry.itemName === undefined ? `${who} asked for something from the vault` : `${who} asked to use ${thing}${where}`;
    case "allowed": return `${you} let ${botName ?? sessionName ?? "an agent"} use ${thing}${how === "always" ? " from now on" : " once"}`;
    case "declined": return `${you} said no to ${botName ?? sessionName ?? "an agent"} using ${thing}`;
    case "refused":
      switch (how) {
        case "wrong site": return `Refused ${thing}${where}: not one of its sites. ${botName ?? "The bot"} was paused`;
        case "paused": return `${who} couldn’t use ${thing}: paused`;
        case "autopilot": return entry.itemName === undefined ? `${who} went on without something from your vault, on Autopilot` : `${who} left ${thing} for you, on Autopilot`;
        default: return `${who} wasn’t allowed ${thing}`;
      }
    case "added": return `${you} added ${thing}`;
    case "changed": return `${you} changed ${thing}`;
    case "removed": return `${you} removed ${thing}`;
    case "shown": return `${you} looked at ${thing}`;
    case "granted": return `${capitalized(thing)} is now ${how === "own" ? "used on its own" : "asked for each time"} by ${botName ?? "everyone allowed"}`;
    case "ungranted": return `${capitalized(thing)} taken away from ${botName ?? "an agent"}`;
    case "paused": return botName ? `${botName}’s use of the vault was paused` : "The vault was paused";
    case "resumed": return botName ? `${botName}’s use of the vault goes on again` : "The vault goes on again";
  }
  return `${who}: ${what} ${thing}`;
}

// MARK: The page

async function vaultPage(context: RouteContext): Promise<SettingsPage> {
  const known = await facts(context);
  const computer = computerHere(context);
  // (Anything but your devices and the app beside it: an agent's command, say.)
  if (!("state" in known)) return unreadPage("vault", TITLE, known.status === 403 ? { status: 403, value: { message: `Only Kuvo on your devices can open ${whose(computer)} vault.` } } : known);
  const { state } = known;
  const notes: { words: string; tone: "quiet" | "warning" }[] = [];
  if (state.locked) {
    // (Only a client that kept its key unlocks it, and a page can't carry one.)
    notes.push({ words: `${atStart(whose(computer))} vault is locked since it started again. Its agents can’t use it until you unlock it in Kuvo on your iPhone, which keeps its key.`, tone: "warning" });
    return { id: "vault", title: TITLE, groups: [], notes };
  }
  if (state.tampered) notes.push({ words: "Its files were changed outside Kuvo, so it uses none of them, and every agent’s use is paused. Add its items again.", tone: "warning" });
  if (state.agents === false) {
    notes.push({ words: state.agentsWaiting ? "Locked for now: some agents started before the sandbox was on. The vault opens when they finish." : `Agents can’t use keys, files and notes from ${whose(computer)} vault yet. Sign-ins you let them use on their own still fill.`, tone: "quiet" });
  }

  const groups: SettingGroup[] = [];
  if (known.asks.length) groups.push({ id: "asks", title: "Waiting for You", rows: known.asks.map(ask => askRow(ask, known)) });

  const items = known.items.map(each => {
    const grants = known.grants.filter(grant => grant.item === each.id);
    const badge = badgeOf(grants);
    const kept = item({
      id: `item-${each.id}`, title: each.name, line: lineOf(each), kind: "actions",
      actions: [
        { id: "edit", title: "Edit…", form: editForm(each) },
        { id: "who", title: "Who Can Use It…", form: whoForm(known, each) },
        { id: "delete", title: "Delete…", style: "destructive", confirm: { title: `Delete “${each.name}” from the vault?`, body: "Agents can’t use it any more. The account or service it’s for isn’t affected.", button: "Delete" } },
      ],
    });
    return { ...kept, ...(badge ? { badge } : {}) };
  });
  // (A machine's vault is made with its first item, and its key kept by the client that made it:
  // a page can't carry that key, so the first item is added where it's kept.)
  const add: SettingAction = { id: "add", title: "Add Item…", form: addForm(known, computer), ...(state.made ? {} : { can: { why: `There’s no vault on ${computer} yet. Add its first item in Kuvo on your iPhone, which keeps the key that unlocks it after ${computer} restarts.` } }) };
  groups.push({
    id: "items", title: "Items", note: keptWords(state, computer),
    rows: [row({
      id: "items", title: "Items", kind: "list", items, add,
      empty: "Nothing yet. Add a sign-in, a key, a file or a note for your agents to use.",
      ...(items.length > 6 ? { search: { placeholder: "Search the vault", empty: "Nothing in the vault matches “{query}”." } } : {}),
    })],
  });

  if (state.made) {
    const paused: SettingRow[] = Object.entries(state.pausedBots).sort(([a], [b]) => a.localeCompare(b)).map(([who, why]) => {
      const name = known.bots.find(bot => bot.id === who)?.name ?? (who.startsWith("bot_") ? "A bot" : "A conversation");
      return row({ id: `paused-${who}`, title: `${name} is paused`, line: why, kind: "actions", actions: [{ id: "go-on", title: "Let Go On" }] });
    });
    groups.push({
      id: "pause", rows: [
        row({ id: "pause", title: "Pause Agents’ Use", line: `Stops every agent on ${computer} from using the vault at once. What they ask still comes to you.`, kind: "switch", value: state.paused, can: true }),
        ...paused,
      ],
    });
    const zone = zoneHere(readZoneSetting()).zone;
    const when = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short", timeZone: zone });
    groups.push({
      id: "activity", title: "Activity", rows: [row({
        id: "activity", title: "Activity", kind: "list", empty: "Nothing used yet.",
        // (A bot by its name now, where the vault noted only which it was.)
        items: known.activity.map(noted => ({ ...noted, botName: noted.botName ?? known.bots.find(bot => bot.id === noted.bot)?.name })).map(entry => item({
          id: `activity-${entry.id}`, title: sentence(entry), line: [when.format(new Date(entry.at)), entry.why].filter(Boolean).join(" · "),
          kind: "status", ...(isWarning(entry) ? { tone: "warning" as const } : {}),
        })),
        ...(known.activity.length > 6 ? { search: { placeholder: "Search the activity", empty: "No activity matches “{query}”." } } : {}),
      })],
    });
  }
  return { id: "vault", title: TITLE, groups, ...(notes.length ? { notes } : {}) };
}

// MARK: Changes

/** Asks one of the vault's routes, as this client. */
const vault = (context: RouteContext, method: string, route: string, body?: unknown) => asked(context, vaultRoute, method, route, body);

async function vaultChange(context: RouteContext, at: SettingItem, value: unknown): Promise<Refused | null> {
  if (at.id !== "pause") return { status: 400, message: "That isn't something to change." };
  if (value === at.value) return null;
  return refusal(await vault(context, "POST", "kuvo/vault/pause", { paused: value === true }));
}

/** A form's values refused: why, and each field's words. */
const formRefused = (checked: SettingsChecked): Refused => {
  const first = checked.fields.find(each => each.tone === "warning") ?? checked.fields[0];
  return { status: 400, message: first?.words ?? "Fill in what it needs first.", fields: [...checked.fields] };
};

/** The vault's refusal of an item's values, under the field it's about where it names one. */
function itemRefused(answer: { status: number; value: any }, kind: Kind): Refused | null {
  const refused = refusal(answer);
  if (!refused) return null;
  const field = /item called/.test(refused.message) ? "name" : /site/.test(refused.message) ? (kind === "login" ? "site" : "hosts") : null;
  return field ? { ...refused, status: 400, fields: [{ id: field, words: refused.message, tone: "warning" }] } : refused;
}

const ID = /^v_[a-z0-9]{10}$/;

async function vaultAct(context: RouteContext, at: SettingItem, action: SettingAction, values?: Record<string, unknown>): Promise<Refused | null> {
  const known = await facts(context);
  if (!("state" in known)) return refusal(known) ?? { status: 503, message: said(known, "This computer's vault couldn't be read.") };

  if (at.id === "items" && action.id === "add") {
    const checked = readForm(known, null, values ?? {}, true);
    if (!checked.given) return formRefused(checked);
    const { kind, ...rest } = checked.given;
    return itemRefused(await vault(context, "POST", "kuvo/vault/items", { kind, ...rest, ...(rest.username ? {} : { username: undefined }) }), kind);
  }

  if (at.id.startsWith("ask-")) {
    const id = at.id.slice("ask-".length);
    const ask = known.asks.find(each => each.id === id);
    if (!ask) return STALE;
    const answer = (body: Record<string, unknown>) => vault(context, "POST", `kuvo/vault/asks/${encodeURIComponent(id)}`, body);
    switch (action.id) {
      case "no": return staleOr404(await answer({ answer: "no" }));
      case "once": return staleOr404(await answer({ answer: "once" }));
      case "always":
        if (!mayAlways(ask)) return { status: 403, message: "This one can only be allowed once." };
        return staleOr404(await answer({ answer: "always" }));
      case "give": {
        const given = text(values?.item);
        if (!known.items.some(each => each.id === given)) return { status: 400, message: "Pick an item to give.", fields: [{ id: "item", words: "Pick an item to give.", tone: "warning" }] };
        const how = values?.how === "always" && mayAlways(ask) ? "always" : "once";
        return staleOr404(await answer({ answer: how, item: given }));
      }
    }
    return { status: 404, message: "That isn't something it does." };
  }

  if (at.id.startsWith("paused-") && action.id === "go-on") {
    return refusal(await vault(context, "POST", "kuvo/vault/pause", { paused: false, bot: at.id.slice("paused-".length) }));
  }

  if (!at.id.startsWith("item-")) return { status: 404, message: "That isn't something it does." };
  const id = at.id.slice("item-".length);
  const each = known.items.find(one => one.id === id);
  if (!each || !ID.test(id)) return STALE;
  const route = `kuvo/vault/items/${encodeURIComponent(id)}`;
  switch (action.id) {
    case "edit": {
      const checked = readForm(known, each, values ?? {}, true);
      if (!checked.given) return formRefused(checked);
      const { kind: _, values: typed, ...rest } = checked.given;
      // A value given as it's kept already isn't a change (Show, then Save): asked of the vault
      // without reading it back.
      const changes: Record<string, string | null> = {};
      for (const [field, value] of Object.entries(typed)) {
        if (typeof value === "string" && each.fields.includes(field)) {
          const same = await vault(context, "POST", `${route}/same`, { values: { [field]: value } });
          if (same.status === 200 && same.value.same === true) continue;
        }
        changes[field] = value;
      }
      const body: Record<string, unknown> = {};
      if (rest.name !== each.name) body.name = rest.name;
      if ((each.kind === "login" || each.kind === "key") && rest.sites.join(" ") !== each.sites.join(" ")) body.sites = rest.sites;
      if (each.kind === "login" && rest.username !== (each.username ?? "")) body.username = rest.username;
      if (each.kind === "login" && rest.signInOnly !== (each.signInOnly === true)) body.signInOnly = rest.signInOnly;
      if (each.kind === "note" && rest.readable !== (each.readable === true)) body.readable = rest.readable;
      if (each.kind === "file" && rest.fileName && rest.fileName !== each.fileName) body.fileName = rest.fileName;
      if (Object.keys(changes).length) body.values = changes;
      if (!Object.keys(body).length) return null;
      return itemRefused(await vault(context, "PATCH", route, body), each.kind);
    }
    case "who": {
      const grants = known.grants.filter(grant => grant.item === id);
      const now = (who: VaultWho) => grants.find(grant => whoId(grant.who) === whoId(who))?.how ?? "no";
      const wanted: { who: VaultWho; how: string }[] = [];
      for (const [field, value] of Object.entries(values ?? {})) {
        if (field === "add-bot") continue;
        const who: VaultWho | null = field === "everyone" ? { everyone: true }
          : field.startsWith("bot-") ? { bot: field.slice(4) }
          : field.startsWith("project-") && grants.some(grant => grant.who.project === field.slice(8)) ? { project: field.slice(8) }
          : null;
        if (!who || !HOWS.some(choice => choice.id === value)) return { status: 400, message: "That isn't someone it can be given to." };
        if (who.bot && !known.bots.some(bot => bot.id === who.bot)) return STALE;
        if (now(who) !== value) wanted.push({ who, how: value as string });
      }
      const more = text(values?.["add-bot"]);
      if (more && more !== "none") {
        if (!known.bots.some(bot => bot.id === more)) return STALE;
        if (now({ bot: more }) === "no" && !wanted.some(each => each.who.bot === more)) wanted.push({ who: { bot: more }, how: "ask" });
      }
      for (const each of wanted) {
        const done = refusal(await vault(context, "POST", "kuvo/vault/grants", { item: id, who: each.who, how: each.how === "no" ? null : each.how }));
        if (done) return done;
      }
      return null;
    }
    case "delete":
      return staleOrRefusal(await vault(context, "DELETE", route));
  }
  return { status: 404, message: "That isn't something it does." };
}

/** An answer to an ask that's gone meanwhile (answered elsewhere, or it gave up): the page now. */
const staleOr404 = (answer: { status: number; value: any }): Refused | null => (answer.status === 404 ? STALE : refusal(answer));

async function vaultCheck(context: RouteContext, at: SettingItem, action: SettingAction, values: Record<string, unknown>): Promise<SettingsChecked> {
  const known = await facts(context);
  if (!("state" in known)) return { fields: [], ready: false };
  if (at.id === "items" && action.id === "add") {
    const { fields, ready } = readForm(known, null, values);
    return { fields, ready };
  }
  if (at.id.startsWith("item-") && action.id === "edit") {
    const each = known.items.find(one => one.id === at.id.slice("item-".length));
    if (!each) return { fields: [], ready: false };
    const { fields, ready } = readForm(known, each, values);
    return { fields, ready };
  }
  if (action.id === "give") return { fields: [], ready: known.items.some(each => each.id === values.item) };
  return { fields: [], ready: true };
}

const FILES = ["items.json", "grants.json", "paused.json", "activity.jsonl", "key.json"].map(name => path.join(VAULT_FOLDER, name));

export const vaultSection: Section & { listed: SettingsSection } = {
  listed: {
    id: "vault", title: TITLE, group: "Agents & Tools", icon: "shield",
    line: "Sign-ins, keys, files and notes agents use without ever seeing them.",
    keywords: ["vault", "secrets", "passwords", "sign-ins", "api keys", "files", "notes", "waiting for you", "who can use it", "pause", "activity"],
  },
  read: vaultPage, change: vaultChange, act: vaultAct, check: vaultCheck,
  // (What agents ask waits in the process that holds the vault: listed there each second.)
  look: () => `${FILES.map(mtime).join(" ")} ${mtime(BOTS)} ${isLocked()} ${agentsUseVault()} ${holdsVault() ? listAsks().map(ask => ask.id).join(",") : Math.floor(Date.now() / 1000)}`,
};
