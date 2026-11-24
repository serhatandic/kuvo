import "./safety";
// Marketplace routes, Recently Deleted and the agent skill projection use real files.
// The engine's skill discovery and Claude's cloud sync are stood in for.
import { afterEach, beforeEach, expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { ask } from "./route-answer";
import { engineHandler } from "../src/link/server/router";
import { CONFIG_FILE } from "../src/link/areas/config";
import { SKILLS_FOLDER, SKILL_TRASH_FOLDER } from "../src/link/areas/skills";
import { keepMarketplaceProjection } from "../src/link/areas/marketplace";
import { marketplaceFolder } from "../src/link/marketplace/catalog";
import { homeFolder, K } from "../src/link/home";
import { setSkillSource, skillsFor, forgetSkills } from "../src/agents/skills";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signingKeyOf } from "../src/link/documents/format";
import { newSetupID, type SetupRecord } from "../src/link/setup/record";
import { publishedShared, takeShared } from "../src/link/setup/shared";
import { readLink, writeLink, type LinkState } from "../src/link/state";
import { grantRelayLease } from "./relay-lease";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { RUN } from "../src/link/home";
import type { EngineLink } from "../src/agents/engine";
import { isObject } from "../src/shared/json";

const handler = engineHandler(() => ({ pid: process.pid, port: 12345, password: "test" }), { name: "Test box" }, (() => null) as never);
const route = (verb: string, at = "", body?: unknown) => ask(handler, verb, "kuvo/marketplace" + at, body);
const network = globalThis.fetch;
let engineSkills: { id: string; path: string; name?: string }[] = [];
const skillText = (name: string) => `---\nname: ${name}\ndescription: A test skill\n---\nInstructions.\n`;
const put = (folder: string, name: string) => {
  fs.mkdirSync(folder, { recursive: true });
  fs.writeFileSync(path.join(folder, "SKILL.md"), skillText(name));
  fs.writeFileSync(path.join(folder, "example.txt"), "Keep this file.");
};
const listed = async () => (await route("GET")).json.areas.find((area: any) => area.id === "skills");
const details = (name: string) => route("GET", "/skills/" + name);
const remove = async (name: string) => {
  const card = await details(name);
  expect(card.status).toBe(200);
  expect(card.json.actions.find((action: any) => action.id === "remove")).toMatchObject({
    style: "destructive", confirm: { title: expect.any(String), body: expect.any(String), button: "Remove" },
  });
  expect((await route("DELETE", "/skills/" + name, { was: card.json.revision })).status).toBe(200);
};
const expectGone = async (name: string) => {
  expect((await listed()).items.find((skill: any) => skill.id === name)?.installed ?? false).toBe(false);
  expect((await skillsFor(K, [], () => false)).map(skill => skill.name)).not.toContain(name);
};
const restore = async (name: string) => {
  const deleted = (await listed()).here.deleted.items.find((item: any) => item.title === name);
  expect(deleted?.id).toEqual(expect.any(String));
  expect((await route("POST", "/skills/restore", { id: deleted.id })).status).toBe(200);
};

beforeEach(() => {
  for (const folder of [marketplaceFolder, SKILLS_FOLDER, SKILL_TRASH_FOLDER]) fs.rmSync(folder, { recursive: true, force: true });
  fs.mkdirSync(path.dirname(CONFIG_FILE), { recursive: true });
  fs.writeFileSync(CONFIG_FILE, "{}");
  engineSkills = [];
  globalThis.fetch = (async (input: string | URL) => Response.json({ data: new URL(input).pathname === "/api/skill" ? engineSkills : [] })) as typeof fetch;
  setSkillSource({ ready: true, request: async (_: string, route: string) => route === "/api/skill" ? engineSkills : [] } as unknown as EngineLink, path.join(K, "agents"));
});
afterEach(() => {
  globalThis.fetch = network;
  setSkillSource(undefined, undefined);
  forgetSkills();
});

for (const [agent, root] of [
  ["Claude Code", () => path.join(homeFolder(), ".claude/skills")],
  ["Codex", () => path.join(homeFolder(), ".codex/skills")],
  ["shared agent folders", () => path.join(homeFolder(), ".agents/skills")],
  ["Antigravity", () => path.join(K, "agents/antigravity/profile/antigravity-cli/skills")],
  ["Grok", () => path.join(homeFolder(), ".grok/skills")],
  ["Cursor", () => path.join(homeFolder(), ".cursor/skills")],
  ["the engine", () => SKILLS_FOLDER],
] as const) {
  test(`removing a skill from ${agent} archives the whole folder and restores it to its source`, async () => {
    const name = "local-example";
    const folder = path.join(root(), name);
    put(folder, name);
    engineSkills = [{ id: name, path: path.join(folder, "SKILL.md") }];
    try {
      expect((await listed()).items.find((skill: any) => skill.id === name)?.installed).toBe(true);
      expect((await skillsFor(K, [], () => false)).map(skill => skill.name)).toContain(name);
      await remove(name);
      expect(fs.existsSync(folder)).toBe(false);
      await expectGone(name);
      const deleted = (await listed()).here.deleted.items.find((item: any) => item.title === name);
      expect(fs.readFileSync(path.join(SKILL_TRASH_FOLDER, deleted.id, "example.txt"), "utf8")).toBe("Keep this file.");
      await restore(name);
      expect(fs.readFileSync(path.join(folder, "example.txt"), "utf8")).toBe("Keep this file.");
      expect((await details(name)).json.installed).toBe(true);
    } finally { fs.rmSync(folder, { recursive: true, force: true }); }
  });
}

test("a resynced Claude skill stays removed and can still be restored", async () => {
  const name = "synced-example";
  const folder = path.join(homeFolder(), ".claude/skills/synced/test-bucket", name);
  engineSkills = [{ id: name, path: path.join(folder, "SKILL.md") }];
  try {
    put(folder, name);
    await remove(name);
    put(folder, name);
    await expectGone(name);
    clearInterval(keepMarketplaceProjection());
    expect(fs.existsSync(folder)).toBe(false);
    await restore(name);
    expect(fs.readFileSync(path.join(folder, "example.txt"), "utf8")).toBe("Keep this file.");
  } finally { fs.rmSync(folder, { recursive: true, force: true }); }
});

test("a created Marketplace skill can be restored after setup-wide removal", async () => {
  const name = "created-example";
  expect((await route("POST", "/skills/create", { name, description: "test", instructions: "Instructions." })).status).toBe(200);
  await remove(name);
  await expectGone(name);
  expect(fs.existsSync(path.join(SKILLS_FOLDER, name))).toBe(false);
  await restore(name);
  expect((await details(name)).json).toMatchObject({ installed: true, origin: "created" });
});

test("a removed built-in skill can be added back from the Kuvo catalog", async () => {
  engineSkills = [{ id: "opencode", name: "OpenCode", path: "skill://opencode/SKILL.md" }];
  await remove("opencode");
  await expectGone("opencode");
  const entry = (await listed()).items.find((skill: any) => skill.id === "opencode");
  expect(entry).toMatchObject({ installed: false, origin: "catalog", actions: [{ id: "add", title: "Install" }] });
  const { available } = (await details("opencode")).json;
  expect((await route("POST", "/skills/opencode/add", { version: available.version, revision: available.revision })).status).toBe(200);
  expect((await details("opencode")).json.installed).toBe(true);
});

test("a catalog skill goes to Recently Deleted and restores as the installed version", async () => {
  const name = "review-changes";
  const { available } = (await details(name)).json;
  expect((await route("POST", "/skills/" + name + "/add", { version: available.version, revision: available.revision })).status).toBe(200);
  const installed = fs.readFileSync(path.join(SKILLS_FOLDER, name, "SKILL.md"), "utf8");
  await remove(name);
  await expectGone(name);
  await restore(name);
  expect((await details(name)).json).toMatchObject({ installed: true, origin: "catalog", version: available.version });
  expect(fs.readFileSync(path.join(SKILLS_FOLDER, name, "SKILL.md"), "utf8")).toBe(installed);
});

test("a skill installed from a folder restores its supporting files", async () => {
  const name = "folder-example";
  const folder = path.join(K, "folder-import");
  put(folder, name);
  const { preview } = (await route("POST", "/skills/preview", { folder })).json;
  expect((await route("POST", "/skills/install", { preview })).status).toBe(200);
  await remove(name);
  await expectGone(name);
  await restore(name);
  expect((await details(name)).json).toMatchObject({ installed: true, origin: "folder" });
  expect(fs.readFileSync(path.join(SKILLS_FOLDER, name, "example.txt"), "utf8")).toBe("Keep this file.");
});

test("a GitHub skill restores the pinned source and all of its files", async () => {
  const name = "github-example";
  const commit = "f".repeat(40);
  globalThis.fetch = (async (input: string | URL) => {
    const { pathname } = new URL(input);
    if (pathname === "/repos/test/skills") return Response.json({ default_branch: "main" });
    if (pathname === "/repos/test/skills/commits/main") return Response.json({ sha: commit });
    if (pathname === "/repos/test/skills/git/trees/" + commit) return Response.json({ tree: [
      { type: "blob", mode: "100644", path: "one/SKILL.md" }, { type: "blob", mode: "100644", path: "one/example.txt" },
    ] });
    if (pathname === `/test/skills/${commit}/one/SKILL.md`) return new Response(skillText(name));
    if (pathname === `/test/skills/${commit}/one/example.txt`) return new Response("Keep this file.");
    return Response.json({ data: [] });
  }) as typeof fetch;
  const { preview } = (await route("POST", "/skills/preview", { link: "https://github.com/test/skills/tree/main/one" })).json;
  expect((await route("POST", "/skills/install", { preview })).status).toBe(200);
  await remove(name);
  await expectGone(name);
  await restore(name);
  expect((await details(name)).json).toMatchObject({ installed: true, origin: "github", source: { repository: "test/skills", commit } });
  expect(fs.readFileSync(path.join(SKILLS_FOLDER, name, "example.txt"), "utf8")).toBe("Keep this file.");
});

test("a computer applies a signed removal and restores its own copy from Recently Deleted", async () => {
  upgradeKuvoFolder(K);
  const identity = () => {
    const key = newKey();
    const sign = newSigningSeed();
    return { seed: key.toString("base64url"), key: publicOf(key).toString("base64url"), sign, signer: signingKeyOf(sign), address: addressOf(key).toString("base64url") };
  };
  const primary = identity(), member = identity();
  const setup: Omit<SetupRecord, "role"> = { id: newSetupID(), epoch: 1, primary: primary.key, sign: primary.signer, managed: true, checkpointed: true };
  const boxes = [primary, member].map(box => ({ key: box.key, sign: box.signer, address: box.address, kind: "machine", name: "Skill box" }));
  const a: LinkState = {
    key: primary.seed, sign: primary.sign, phones: [], setup: { ...setup, role: "primary" }, sharedSetup: setup.id,
    shared: { membershipAndAccess: { sequence: 1, value: { boxes } } },
  };
  const b: LinkState = { key: member.seed, sign: member.sign, phones: [], setup: { ...setup, role: "member" } };
  const name = "shared-native";
  const folder = path.join(homeFolder(), ".claude/skills", name);
  const localTrash = SKILL_TRASH_FOLDER + "-primary";
  const applyOnMember = () => {
    const doc = publishedShared(readLink()!).map(envelope => readDoc(envelope)!)
      .find(({ body }) => isObject(body.replica) && body.replica.marketplaceInstallations);
    expect(doc).toBeDefined();
    takeShared(b, primary.key, doc!);
    writeLink(b);
    clearInterval(keepMarketplaceProjection());
  };
  try {
    put(folder, name);
    writeLink(a);
    grantRelayLease(a);
    await remove(name);
    const primaryAfterRemove = readLink()!;
    fs.renameSync(SKILL_TRASH_FOLDER, localTrash);
    put(folder, name);
    fs.writeFileSync(path.join(folder, "example.txt"), "The member’s own copy.");
    applyOnMember();
    expect(fs.existsSync(folder)).toBe(false);
    await expectGone(name);

    const memberTrash = SKILL_TRASH_FOLDER + "-member";
    fs.renameSync(SKILL_TRASH_FOLDER, memberTrash);
    fs.renameSync(localTrash, SKILL_TRASH_FOLDER);
    writeLink(primaryAfterRemove);
    await restore(name);
    fs.rmSync(folder, { recursive: true });
    fs.renameSync(memberTrash, SKILL_TRASH_FOLDER);
    applyOnMember();
    expect(fs.readFileSync(path.join(folder, "example.txt"), "utf8")).toBe("The member’s own copy.");
  } finally {
    fs.rmSync(path.join(RUN, "link.json"), { force: true });
    fs.rmSync(folder, { recursive: true, force: true });
  }
});
