// The box itself (PROTOCOL.md, "Messages in a session" and "Updates"): what it is and runs, its
// updates, the phones paired with it, whether you're at a computer, and who `&` can name.

import { FEATURES } from "../features";
import { bool, BYTES, type Infer, list, literal, map, NOTHING, nullable, number, object, oneOf, type RouteShape, string, words } from "../shape";
import { MAC_FEATURES } from "./mac";

/** One of a machine's pieces (`kuvo/info`'s `pieces`): the version it runs (null while it doesn't,
 * or doesn't say), the one its files hold now, and whether it runs another than those (`behind`). */
const PIECE = object({ running: nullable(string), current: nullable(string), behind: bool });

/** What the box is and runs (`kuvo/info`): a Mac's names its app's version and its integrations, a
 * machine's its bridge's version and whether its engine runs or is stopped on purpose. `started`:
 * when the engine running now came up, in seconds since 1970 (null while none runs). A machine's
 * `home`: the Mac that started its engine (its id, and its name where it noted one). `macs`: the
 * Macs that set it up (each one's key, its name where it said one, when it was last heard from, ms),
 * and `phones`, how many phones reach it: who an uninstall would stop it for. A machine's `pieces`:
 * which version its link, coding agents' bridge and engine each run, against the one in place, and
 * while a newer bridge waits to take over, since when and whether new turns wait for it. */
export const INFO = oneOf(
  object({
    kind: literal("mac"),
    name: string,
    app: string,
    engine: string,
    features: list(words(...FEATURES, ...MAC_FEATURES)),
    agents: bool,
    started: nullable(number),
    chats: string,
    integrations: map(string),
    // How moving this Mac's box into its home of its own stands, while one waits, is under way or
    // went back ("Moving this Mac's box"): left out otherwise.
    "moving?": object({ state: words("waiting", "moving", "clearing", "failed"), step: nullable(string), overdue: bool, "why?": string }),
  }),
  object({
    kind: literal("machine"),
    name: string,
    bridge: string,
    engine: nullable(string),
    features: list(words(...FEATURES)),
    agents: bool,
    started: nullable(number),
    chats: string,
    running: bool,
    stopped: bool,
    "home?": object({ mac: string, "name?": string }),
    "macs?": list(object({ key: string, "name?": string, seen: number })),
    "phones?": number,
    "pieces?": object({
      link: PIECE,
      agents: object({
        running: nullable(string), current: nullable(string), behind: bool, renewal: nullable(object({ since: number, holding: bool })),
        waits: nullable(words("quiet", "holding", "engine", "restart")),
      }),
      engine: PIECE,
    }),
  }),
);

/** An update's step under way. */
const STEP = words("checking", "downloading", "downloading the engine", "checking it", "installing", "starting");
/** How the last one went: `from` and `to` once one's done, or what it said. */
const LAST_UPDATE = object({
  at: number,
  result: words("updated", "current", "waiting", "busy", "off", "blocked", "failed"),
  "version?": string,
  "message?": string,
  "from?": string,
  "to?": string,
});
/** This box's updates (`kuvo/update`): whether they're set up, its version and its engine's, the
 * latest it knows of, how the last one went, the step under way, why it can't update, and the
 * version of a release offered from a folder here (`kuvo/box/update`) that waits to be taken. */
export const UPDATE = object({
  setUp: bool,
  version: nullable(string),
  engine: nullable(string),
  latest: nullable(string),
  last: nullable(LAST_UPDATE),
  updating: nullable(STEP),
  blocked: nullable(string),
  "offered?": string,
  "offerFailed?": object({ version: string, at: number, takenBack: bool, "message?": string }),
});

/** A step of moving this Mac's box into a home of its own (PROTOCOL.md, "Moving this Mac's box"). */
const REHOME_STEP = words("plan", "install", "keys", "clone", "upgrade", "database", "rewrite", "check", "place", "fix", "clear", "done");
/** How moving this Mac's box into a home of its own stands (`kuvo/box/rehome`): its state (none
 * yet; `waiting` for a quiet moment; `moving` or `clearing`, under way, and the step; `done`;
 * `failed`, gone back; `back`, moved back), which way, its tries and the next one, since when it
 * waits and whether that's three days (`overdue`), why it last failed, when it moved and when its
 * kept copy goes, what the app still owes it, Move Now or Move Back asked, and both folders. */
export const REHOME = object({
  state: nullable(words("waiting", "moving", "clearing", "done", "failed", "back")),
  way: nullable(words("move", "back")),
  step: nullable(REHOME_STEP),
  tries: number,
  nextTry: nullable(number),
  waitingSince: nullable(number),
  overdue: bool,
  failed: nullable(object({ at: number, step: nullable(REHOME_STEP), message: string })),
  movedAt: nullable(number),
  keptUntil: nullable(number),
  owed: list(words("keychain", "agent", "start", "passwords")),
  asked: nullable(words("now", "back")),
  from: nullable(string),
  to: nullable(string),
  "progress?": nullable(object({ step: REHOME_STEP, title: string, index: number, of: number })),
  "dropped?": list(string),
});

/** What macOS allows the program this Mac's box runs under (`kuvo/box/permissions`, PROTOCOL.md,
 * "What macOS allows"): whether there's such a program here (`applies`: a Mac's own box in a home of
 * its own; never on Linux) and its name as System Settings lists it, whether it runs and has said
 * (`known`), and each permission macOS can be asked about, null where that isn't known; that
 * program's bundle on the Mac (`bundle`), and each permission in words (`list`): what System
 * Settings calls it, the address of its list there, whether you add the program to it by hand, and
 * whether a client brings it up now (`asks`). */
export const BOX_PERMISSIONS = object({
  applies: bool,
  name: nullable(string),
  known: bool,
  screenRecording: nullable(bool),
  accessibility: nullable(bool),
  bundle: nullable(string),
  list: list(object({ id: words("screenRecording", "accessibility"), name: string, allowed: nullable(bool), settings: string, byHand: bool, asks: bool })),
});

/** This Mac's app as its box knows it (`kuvo/box/app`, PROTOCOL.md, "This Mac's app"): where it's
 * installed, as it last registered; whether it's open (its tools answer), quits to install an update
 * now, is being opened for a browser call; and the last opening that failed, within its five-minute
 * rest (`at` in ms, and what agents were told). */
export const BOX_APP = object({
  bundle: nullable(string),
  open: bool,
  updating: bool,
  opening: bool,
  failed: nullable(object({ at: number, message: string })),
});

/** Whether a client wants the box's server kept running (`kuvo/box/serve`), and whether it runs. */
export const BOX_SERVE = object({ needed: bool, serving: bool });

/** The browser agents use on a machine (`kuvo/browser`, PROTOCOL.md, "The browser on a machine"):
 * `missing` (none, and none being set up), `checking`, `downloading` (with `progress`, 0 to 1),
 * `installing`, `ready`, or `failed` (`message`, and when it tries again by itself, ms, or null for
 * never). `chromium` is the version this box pins, `installed` the one in use (an older one works
 * while a newer one is set up). */
export const BROWSER = object({
  state: words("missing", "checking", "downloading", "installing", "ready", "failed"),
  chromium: nullable(string),
  installed: nullable(string),
  progress: nullable(number),
  message: nullable(string),
  retryAt: nullable(number),
});

/** The phones paired here, `you` being the one asking. */
export const PHONES = object({ phones: list(object({ key: string, name: string, added: nullable(number), pairing: string, kind: words("phone", "desktop"), you: bool })) });
/** Kuvo Code's desktop app paired as one of your devices (`kuvo/devices/pair`): the pairing it has. */
export const DEVICE_PAIRED = object({ paired: literal(true), pairing: string });
/** A link for another phone to pair with (good once), and when it ends, in seconds since 1970. */
export const PAIRING = object({ link: string, ends: number });

/** Your setup as this box says it (`kuvo/setup`, PROTOCOL.md "Your setup"): its id and epoch, its
 * main computer, this box (its key, and its role, null in no setup), each computer in it, another
 * main computer named at the same epoch (never taken), the #181 switch: the value you set, or
 * null, and what it is then; the machines holding this Mac back from leading a setup, by key; and
 * whether you've had the first run with this box (`welcomed`, set by `kuvo/setup/welcomed`). */
export const SETUP = object({
  setup: nullable(string),
  epoch: nullable(number),
  primary: nullable(object({ key: string, name: string, kind: words("mac", "machine") })),
  you: object({ key: nullable(string), role: nullable(words("primary", "member")) }),
  computers: list(object({ key: string, name: string, kind: words("mac", "machine"), role: words("primary", "member"), app: bool })),
  "conflict?": object({ primary: string, epoch: number }),
  startsFromOthers: object({ value: nullable(words("on", "off")), default: words("on", "off") }),
  heldBy: list(string),
  welcomed: bool,
});

/** Whether a first run has anything to ask about your accounts here (`kuvo/setup/ready`): a model
 * on one of your accounts runs and no plan needs you; with whether one runs, the plans that need
 * you, by id, and the sides that can't say yet. */
export const SETUP_READY = object({ ready: bool, runs: bool, needsYou: list(string), pending: list(words("engine", "agents")) });

/** Whether you've had the first run with this box, as `POST kuvo/setup/welcomed` left it. */
export const WELCOMED = object({ welcomed: bool });

/** One box found on a computer, as `kuvo/setup/judge` and `kuvo-link.js here` judge it: where it
 * is (its folder, from the home folder; `app`, Kuvo Code's own box on a Mac), its verdict, and what
 * it says of itself (its key, name and kind; its setup, role and main computer, `heard` when that
 * was last heard from, ms; the Macs that set it up; how many computers and Work Queue items). */
const JUDGED_BOX = object({
  folder: string,
  app: bool,
  verdict: words("self", "free", "ours", "older", "elsewhere", "main"),
  key: nullable(string),
  name: string,
  kind: nullable(words("mac", "machine")),
  setup: nullable(string),
  role: nullable(words("primary", "member")),
  primary: nullable(object({ key: string, name: string, heard: nullable(number) })),
  macs: list(object({ key: string, name: string })),
  computers: nullable(number),
  items: nullable(number),
});

/** What's on a computer, judged (PROTOCOL.md, "Your setup", "Already in a setup?"): one verdict,
 * whether adding it joins its setup with nothing to ask (`joins`: another setup's main computer, no
 * Mac's own app, with no other computer and no work in it), and each box found there. */
export const JUDGED = object({ verdict: words("self", "free", "ours", "older", "elsewhere", "main", "two"), joins: bool, boxes: list(JUDGED_BOX) });

/** What `kuvo-link.js here` prints as Kuvo Code opens on a Mac (PROTOCOL.md, "Already in a
 * setup?"): one verdict, whether the app uses its own box or the one already here (`attach`), how it
 * opens (`main`: its setup's main computer; `window`: onto a setup another computer is main of, its
 * box one of its members), that box, and the app's own. While a move of this Mac's box into its home
 * is under way (its journal there, not done), `moving`, to `wait`: neither the app nor its keeper
 * starts a box then. Not a route: the app decodes it as it opens. */
export const HERE = object({
  verdict: words("self", "free", "ours", "older", "elsewhere", "main", "two", "moving"),
  use: words("own", "attach", "wait"),
  opens: words("main", "window"),
  runs: nullable(words("running", "stopped", "gone")),
  box: nullable(JUDGED_BOX),
  own: nullable(JUDGED_BOX),
});

/** Whether a client's in use here, and how many notices wait. */
export const ATTENTION = object({ inUse: bool, held: number });

/** Who `&` can name in a message here: this box's bots and latest sessions (`machine` null), then
 * those its other computers told it of. */
export const NAMED = object({
  agents: list(object({ session: string, name: string, kind: words("bot", "session"), machine: nullable(string), project: nullable(string), about: nullable(string) })),
});

/** The plans whose sign-in ran out and couldn't be renewed, by provider. */
export const ENDED = object({ providers: list(string) });
/** This box's sign-ins. */
export const CREDENTIALS = object({ credentials: list(object({ id: string, integrationID: string, label: string, active: bool })) });
/** A provider's models asked of again: the folders that get them once their work is done. */
export const MODELS_REFRESHED = object({ waiting: list(string) });
/** The providers Refresh Models brings new models to. */
export const REFRESHABLE = object({ providers: list(string) });

/** What this computer already has that Kuvo Code can use, for the first launch (names only, never a
 * key): the coding agents' programs and their sign-ins; the keys (`key`, brought over with
 * `kuvo/accounts/found/use`) and sign-ins (`signIn`, done again here) of an opencode used apart
 * from Kuvo Code, `connected` when the engine has one for it already; and the keys in the shell
 * the engine uses; and the engine's providers signed in to already (`connected`, by integration).
 * `unread` names the parts whose server isn't running or didn't answer (`engine`, `agents`): theirs
 * are empty for now, not signed out. */
export const FOUND = object({
  agents: list(object({ id: string, name: string, installed: bool, signedIn: bool, "label?": string })),
  opencode: list(object({ id: string, integrationID: string, name: string, kind: words("key", "signIn"), connected: bool })),
  shell: list(object({ integrationID: string, name: string, variable: string })),
  "connected?": list(string),
  "unread?": list(words("engine", "agents")),
});

export type BoxInfo = Infer<typeof INFO>;
/** A machine's, as its link answers. */
export type MachineInfo = Extract<BoxInfo, { kind: "machine" }>;
export type UpdateStatus = Infer<typeof UPDATE>;
export type UpdateStep = Infer<typeof STEP>;
export type LastUpdate = Infer<typeof LAST_UPDATE>;
export type RehomeStatus = Infer<typeof REHOME>;
export type BoxPermissions = Infer<typeof BOX_PERMISSIONS>;
export type RehomeStep = Infer<typeof REHOME_STEP>;
export type BrowserStatus = Infer<typeof BROWSER>;
export type BoxApp = Infer<typeof BOX_APP>;
export type BoxServe = Infer<typeof BOX_SERVE>;
export type Phones = Infer<typeof PHONES>;
export type Named = Infer<typeof NAMED>;
export type Credentials = Infer<typeof CREDENTIALS>;
export type ModelsRefreshed = Infer<typeof MODELS_REFRESHED>;
export type Found = Infer<typeof FOUND>;

/** What can be done with one of your computers from the box that lists it (`computers.manage`):
 * which, its button's words, and where a client asks first, the question, what it comes to and the
 * words of the button that goes ahead, all the box's; `why`, it can't be done now, in words a
 * client shows beside it. `allow-devices` (`devices.allow`): on your setup's main computer, a
 * computer kept from adding Kuvo Code's app to your devices is let do it again. `setup` and
 * `uninstall` (`computers.add`): on the box that set a machine up over SSH, setting it up again
 * and uninstalling Kuvo Code from it. */
export const COMPUTER_ACTION = object({ id: words("rename", "connect", "remove", "allow-devices", "setup", "uninstall"), label: string, "confirm?": object({ title: string, message: string, label: string }), "why?": string });
export type ComputerAction = Infer<typeof COMPUTER_ACTION>;

/** One of your computers, as a box knows it (`kuvo/computers`, PROTOCOL.md "Computers"): its key
 * (`""` for the box itself while it has none), name and kind; `here`, the box that answered; `id`,
 * a machine's name on its Mac; whether it's a machine Macs of different setups share; `apart`, it
 * isn't one of your own computers (such a machine, or a box of another setup); whether it's
 * heard from (`connected`), not yet or being reached again (`connecting`), not lately (`away`), or not this box's to say (`unknown`: one known by a peer's word alone); Kuvo Code's version
 * there, its engine's and its system, as it said them; what its box answers (`features`, as it
 * said them, whole); how this box knows it (`here`, itself; `relay`, a peer it talks to; `peer`, by
 * another computer's word alone, `through` that one); where a client of your own reaches its box
 * itself (`address`, on the relay, and `relay`, its URL: only on your own computers); how many of its sessions are at work and
 * which (the first few, by id and title), where it said; the services whose plan limits it reads;
 * what can be done with it from here (`actions`); and `vouching` (`devices.allow`, on your setup's
 * main computer alone): it's kept from adding Kuvo Code's app to your devices, since one it added
 * was removed, in words a client shows as they are. */
export const LISTED_COMPUTER = object({
  key: string, name: string, kind: words("mac", "machine"), "here?": literal(true), "id?": string,
  title: string, reachTitles: map(string), "detail?": string, "notice?": object({ title: string, detail: string }), "workingTitle?": string,
  shared: bool, "apart?": literal(true), reach: words("connected", "connecting", "away", "unknown"),
  "version?": string, "engine?": string, "system?": string, "features?": list(string),
  via: words("here", "relay", "peer"), "through?": string, "address?": string, "relay?": string,
  "working?": number, "atWork?": list(object({ id: string, "title?": string })), limits: list(string), limitServices: list(object({ id: string, title: string })),
  actions: list(COMPUTER_ACTION), "vouching?": object({ blocked: literal(true), words: string }),
});
export const COMPUTERS = object({ computers: list(LISTED_COMPUTER) });
export type ListedComputer = Infer<typeof LISTED_COMPUTER>;

/** A file as a viewer shows it (`kuvo/file/text`): its text (`content`, read as `encoding`, UTF-8,
 * the only one read), or `text` false for one that isn't text (a picture, a program, another
 * encoding), which a viewer says instead of showing. */
export const FILE_TEXT = oneOf(
  object({ text: literal(false) }),
  object({ text: literal(true), encoding: literal("utf-8"), content: string }),
);
export type FileText = Infer<typeof FILE_TEXT>;
const FILE_REFUSED = object({ message: string });

/** A step of a try at adding a computer (`computers.add`): its title, in the box's words; whether
 * it waits, is under way, is done or failed; what it says beside it; and how far a copy has got,
 * in bytes. */
export const ADD_STEP = object({ id: string, title: string, state: words("waiting", "now", "done", "failed"), "detail?": string, "sent?": number, "total?": number });
export type AddStep = Infer<typeof ADD_STEP>;
/** What a try asks you before it goes on: the question, what it comes to, facts to look at (a
 * host key's fingerprint, `code`: drawn in the code font), and its answers, each a button
 * (`primary`, `plain` or `danger`). All the box's words. `at`: this ask's number, which an answer
 * names so one meant for an earlier ask isn't taken for it. */
export const ADD_QUESTION = object({
  id: string, title: string, message: string, at: number,
  facts: list(object({ label: string, value: string, "code?": bool })),
  answers: list(object({ id: string, label: string, kind: words("primary", "plain", "danger") })),
});
export type AddQuestion = Infer<typeof ADD_QUESTION>;
/** A try at adding a computer over SSH, setting one up again or uninstalling Kuvo Code from one
 * (PROTOCOL.md, "Computers", Adding one): where it is, what it's called, how it stands, its steps,
 * what it asks now, how it failed (`again`: trying again could go otherwise), and the computer it
 * came to (`known`: one that was here already, at another address). */
export const ADD_TRY = object({
  id: string, kind: words("add", "setup", "uninstall"), destination: string, name: string,
  state: words("working", "asking", "stopping", "failed", "done"), title: string,
  steps: list(ADD_STEP),
  "question?": ADD_QUESTION,
  "failure?": object({ title: string, message: string, again: bool }),
  "computer?": object({ key: string, name: string }),
  "known?": bool,
});
export type AddTry = Infer<typeof ADD_TRY>;
/** Adding computers, as this box says it: whether it adds them (`why`, in words, where it
 * doesn't), the hosts to suggest (this computer's `~/.ssh/config`, those not added yet), and its
 * tries, those under way first. */
export const ADDING = object({ can: bool, "why?": string, hosts: list(string), tries: list(ADD_TRY) });
export type Adding = Infer<typeof ADDING>;
const ADD_REFUSED = object({ message: string, "primary?": string });
const ADD_ANSWERS = { 400: ADD_REFUSED, 404: ADD_REFUSED, 405: ADD_REFUSED, 409: ADD_REFUSED, 503: ADD_REFUSED };

export const BOX_ROUTES: RouteShape[] = [
  { route: "GET kuvo/computers/add", answers: { 200: ADDING, ...ADD_ANSWERS } },
  { route: "GET kuvo/computers/add/events", answers: { 200: ADDING, ...ADD_ANSWERS } },
  { route: "POST kuvo/computers/add", answers: { 202: object({ try: ADD_TRY }), ...ADD_ANSWERS } },
  { route: "POST kuvo/computers/add/:id/answer", answers: { 200: object({ try: ADD_TRY }), ...ADD_ANSWERS } },
  { route: "DELETE kuvo/computers/add/:id", answers: { 200: NOTHING, ...ADD_ANSWERS } },
  { route: "POST kuvo/computers/:key/setup", answers: { 202: object({ try: ADD_TRY }), ...ADD_ANSWERS } },
  { route: "POST kuvo/computers/:key/uninstall", answers: { 202: object({ try: ADD_TRY }), ...ADD_ANSWERS } },
  { route: "GET kuvo/computers", answers: { 200: COMPUTERS, 405: object({ message: string }) } },
  { route: "GET kuvo/computers/events", answers: { 200: COMPUTERS, 405: object({ message: string }) } },
  { route: "PATCH kuvo/computers/:key", answers: { 200: object({ name: string }), 400: object({ message: string }), 404: object({ message: string }), 405: object({ message: string }), 409: object({ message: string }), 500: object({ message: string }), 503: object({ message: string }) } },
  { route: "DELETE kuvo/computers/:key", answers: { 200: NOTHING, 404: object({ message: string }), 405: object({ message: string }), 409: object({ message: string }), 500: object({ message: string }), 503: object({ message: string }) } },
  { route: "POST kuvo/computers/:key/connect", answers: { 202: NOTHING, 404: object({ message: string }), 409: object({ message: string }), 503: object({ message: string }) } },
  { route: "POST kuvo/computers/:key/allow-devices", answers: { 200: NOTHING, 403: object({ message: string }), 404: object({ message: string }), 405: object({ message: string }), 409: object({ message: string, "primary?": string }) } },
  { route: "GET kuvo/info", answers: { 200: INFO } },
  { route: "GET kuvo/update", answers: { 200: UPDATE } },
  { route: "POST kuvo/update", answers: { 202: UPDATE } },
  { route: "POST kuvo/box/update", answers: { 202: UPDATE } },
  { route: "GET kuvo/box/rehome", answers: { 200: REHOME } },
  { route: "POST kuvo/box/rehome", answers: { 200: REHOME } },
  { route: "GET kuvo/box/permissions", answers: { 200: BOX_PERMISSIONS } },
  { route: "POST kuvo/box/permissions", answers: { 200: BOX_PERMISSIONS, 400: object({ message: string }), 409: object({ message: string }), 500: object({ message: string }) } },
  { route: "GET kuvo/box/app", answers: { 200: BOX_APP } },
  { route: "POST kuvo/box/app", answers: { 200: BOX_APP } },
  { route: "POST kuvo/box/app/open", answers: { 200: BOX_APP } },
  { route: "GET kuvo/box/serve", answers: { 200: BOX_SERVE } },
  { route: "POST kuvo/box/serve", answers: { 200: BOX_SERVE } },
  { route: "GET kuvo/browser", answers: { 200: BROWSER } },
  { route: "POST kuvo/browser", answers: { 202: BROWSER } },
  { route: "GET kuvo/file", answers: { 200: BYTES } },
  { route: "GET kuvo/file/text", answers: { 200: FILE_TEXT, 400: FILE_REFUSED, 403: FILE_REFUSED, 404: FILE_REFUSED, 413: FILE_REFUSED } },
  { route: "GET kuvo/phones", answers: { 200: PHONES } },
  { route: "POST kuvo/phones/remove", answers: { 200: NOTHING } },
  { route: "POST kuvo/phones/pair", answers: { 200: PAIRING } },
  { route: "POST kuvo/devices/pair", answers: { 200: DEVICE_PAIRED, 400: object({ message: string }), 403: object({ message: string }), 409: object({ message: string, "reason?": words("removed", "blocked") }), 503: object({ message: string }) } },
  { route: "DELETE kuvo/phones/pair", answers: { 200: NOTHING } },
  { route: "GET kuvo/setup", answers: { 200: SETUP } },
  { route: "GET kuvo/setup/events", answers: { 200: SETUP } },
  { route: "POST kuvo/setup/judge", answers: { 200: JUDGED } },
  { route: "POST kuvo/setup/attached", answers: { 200: NOTHING } },
  { route: "GET kuvo/setup/ready", answers: { 200: SETUP_READY } },
  { route: "POST kuvo/setup/welcomed", answers: { 200: WELCOMED, 400: object({ message: string }) } },
  { route: "GET kuvo/attention", answers: { 200: ATTENTION } },
  { route: "POST kuvo/attention", answers: { 200: object({ for: number }) } },
  { route: "POST kuvo/attention/notice", answers: { 200: NOTHING } },
  { route: "POST kuvo/attention/answered", answers: { 200: NOTHING } },
  { route: "GET kuvo/named", answers: { 200: NAMED } },
  { route: "GET kuvo/accounts/ended", answers: { 200: ENDED } },
  { route: "POST kuvo/accounts/:provider/forget", answers: { 200: NOTHING } },
  { route: "GET kuvo/accounts/refreshable", answers: { 200: REFRESHABLE } },
  { route: "POST kuvo/accounts/:provider/refresh", answers: { 200: MODELS_REFRESHED } },
  { route: "GET kuvo/accounts/credentials", answers: { 200: CREDENTIALS } },
  { route: "GET kuvo/accounts/found", answers: { 200: FOUND } },
  { route: "POST kuvo/accounts/found/use", answers: { 200: NOTHING } },
  { route: "POST kuvo/engine/:control", answers: { 202: NOTHING } },
  { route: "POST kuvo/shell/:id/stop", answers: { 200: object({ stopped: words("signal", "removed") }) } },
];
