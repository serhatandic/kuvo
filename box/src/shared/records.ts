// What a box's programs write in its `run` folder for each other, as each reads it.

import type { BotLook } from "./routes/bots";

/** `run/engine.json`: the engine running here, as its launch wrote it (this Mac's app, or a machine's keeper). */
export interface EngineRecord {
  pid?: number;
  port: number;
  password: string;
  version?: string;
  notes?: Record<string, unknown>;
  /** On a machine, where its coding agents' bridge is too (RemoteEngine.swift writes them). */
  agentsPort?: number;
  agentsPassword?: string;
}

/** `run/bridge.json`: the coding agents' bridge beside the engine, when it runs. On a machine it
 * also says which build it runs (`build`, as its bundle names it), and while a newer one waits in
 * its place, since when and whether it holds new turns for it (`renewal`). */
export interface BridgeRecord {
  pid?: number;
  port: number;
  password: string;
  build?: string | null;
  renewal?: RenewalRecord | null;
}

/** A newer bridge in place of the one running (`bridge.json`'s `renewal`): seen since `since` (ms),
 * `holding` once new turns wait for it (it found no quiet moment in time), and what it waits for
 * (`waits`). */
export interface RenewalRecord {
  since: number;
  holding: boolean;
  waits: RenewalWait;
}

/** What a bridge behind its bundle waits for before it makes way: a quiet moment (`quiet`), the
 * turns under way while new ones wait (`holding`), the engine that bundle came with (`engine`), or
 * a restart of its engine, as it won't make way by itself (`restart`: a bridge from before it did,
 * one whose launch wouldn't start it again, or a bundle that doesn't run here). */
export type RenewalWait = "quiet" | "holding" | "engine" | "restart";

/** `run/tools.json`: the port of this Mac's app's own server for Kuvo Code's tools, as the app last
 * told the agents bridge (the box's sandbox refuses it, as Kuvo Code's other servers). */
export interface ToolsRecord {
  port: number;
}

/** Where a box's program answers, with its password: the engine, or the agents bridge. */
export type Target = Pick<EngineRecord, "port" | "password">;

/** An agent as boxes list them for each other (a peer's mail, `run/agents.json`): a conversation,
 * by its session, a bot (with its id and look) or a session; its project, what it does (`about`),
 * and the box it's on (`machine`, that box's name on its Macs). */
export interface ListedAgent {
  session: string;
  name: string;
  kind: "bot" | "session";
  id?: string;
  look?: BotLook;
  project?: string;
  about?: string;
  machine?: string;
}

/** What one peer told this box of its agents (`run/peer-agents.json`, by its key): when it said
 * so, by its clock, and when that reached this box, by this one's; what it is, its id on its Macs,
 * its agents, and what it answers (`features`). */
export interface ToldAgents {
  at: number;
  heard?: number;
  kind: "mac" | "machine";
  id?: string;
  list: ListedAgent[];
  features?: string[];
  /** Its `features` were longer than a peer keeps (`keptFeatures`): the list is cut, so it can't say what's missing. */
  featuresCut?: true;
  /** Conversations it moved away lately, by the key of the box each lives on now, and when. */
  moved?: { session: string; key: string; at: number }[];
  /** The boxes it knows (its own peers), which this box may take plan limits from and tell its own to (PROTOCOL.md, "Plan limits"). */
  boxes?: { key: string; address: string; kind: "mac" | "machine"; name: string }[];
  /** The setup it follows (its id), where it said one (PROTOCOL.md, "Your setup"). */
  setup?: string;
  /** Where a bot deleted there ends up, as it said: the Finder's Trash, or Kuvo Code's own trash
   * folder (`bots.deletes`). */
  trash?: "trash" | "kuvo";
  /** What it says of itself (PROTOCOL.md, "Computers"). */
  computer?: ToldComputer;
}

/** What a box says of itself, to its clients (`kuvo/computers`) and to its peers with its agents:
 * Kuvo Code's version there and its engine's, its system as people know it, how many of its
 * sessions are at work, and which (the first few, newest first, by id and title). A box from
 * before said no `working` (or `atWork`): left out, not taken as none. */
export interface ToldComputer {
  version?: string;
  engine?: string;
  system?: string;
  working?: number;
  atWork?: AtWorkSession[];
  /** How many of its sessions wait on you now, and which (the first few, the latest asking first,
   * each with what it asks in that box's words): said with `working` since #781, for the menu bar
   * of your other computers. One that didn't say it is left out, never taken as none. */
  waiting?: number;
  asking?: AskingSession[];
  /** A Mac's box with the old Kuvo Code app (before 1.0) attached: it takes no removals now. */
  oldApp?: true;
  /** Where else its Macs may reach it over SSH, as it knows itself: its tailnet's addresses first,
   * its name, its other networks' (IPv4). What a Mac tries where the address it set it up at
   * doesn't answer. */
  addresses?: string[];
  /** The Kuvo version this box runs (`bin/box-version`): on a Mac, `version` is its app's. */
  kuvo?: string;
  /** The version of the signed release this box holds and hands to your other computers that are
   * behind (PROTOCOL.md, "Updates", Your computers follow each other). */
  release?: string;
}

/** A session at work on a computer: its id there, and its title (none for one that has none yet). */
export interface AtWorkSession {
  id: string;
  title?: string;
}

/** A session waiting on you on a computer: its id there, its title, and what it asks, in that
 * box's words ("asks to run a command"). */
export interface AskingSession {
  id: string;
  title?: string;
  asks?: string;
}

/** `run/agents.json`: everyone, for list_agents here (kuvo-browser.js reads it), and this box's
 * name among them. */
export interface AgentsFile {
  machine: string | null;
  agents: ListedAgent[];
}
