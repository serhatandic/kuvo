import "./safety"
import { expect, test, beforeEach, afterEach } from "bun:test";
import fs from "node:fs";
import crypto from "node:crypto";
import snapshot from "../../catalog/index.json";
import path from "node:path";
import { ask } from "./route-answer";
import { engineHandler } from "../src/link/server/router";
import { CONFIG_FILE } from "../src/link/areas/config";
import { SKILLS_FOLDER } from "../src/link/areas/skills";
import { marketplaceFolder } from "../src/link/marketplace/catalog";
import { startMarketplaceRelay } from "../src/link/marketplace/connections";
import { keepMarketplaceProjection } from "../src/link/areas/marketplace";
import { makeVault, valuesOf, readItems, forgetKeys } from "../src/link/vault/store";
import { BOTS, K, RUN } from "../src/link/home";
import { addressOf, newKey, publicOf } from "../src/link/crypto/keys";
import { newSigningSeed, readDoc, signingKeyOf } from "../src/link/documents/format";
import { publishedShared, takeShared } from "../src/link/setup/shared";
import { newSetupID } from "../src/link/setup/record";
import { readLink, writeLink, type LinkState } from "../src/link/state";
import { grantRelayLease } from "./relay-lease";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { setServerSource, computerServers, forgetServers } from "../src/agents/servers";
import { setSkillSource, skillsFor, skillsFolder, copySkillsInto, forgetSkills } from "../src/agents/skills";
import type { EngineLink } from "../src/agents/engine";

// The real routes, filesystem, vault, signed setup mail and bridge projections. Only external
// HTTP is stood in for. No running box or native coding-agent account is used.
const engine = { pid: process.pid, port: 12345, password: "marketplace-test" };
const handler = engineHandler(() => engine, { name: "Test box" }, (() => null) as never);
const route = (verb: string, at = "", body?: unknown) => ask(handler, verb, "kuvo/marketplace" + at, body);
const network = globalThis.fetch;
const useNetwork = (answer: (input: URL, init?: RequestInit) => Response | Promise<Response>) => {
  globalThis.fetch = ((input: string | URL, init?: RequestInit) => Promise.resolve(answer(new URL(input), init))) as typeof fetch;
};
const commit = "aa".repeat(20);
const catalogFile = "https://marketplace.invalid/index.json";
const skillFile = "---\nname: private-skill\ndescription: Write a short summary\n---\n\nPinned instructions.\n";

beforeEach(() => {
  for (const at of [marketplaceFolder, SKILLS_FOLDER, BOTS, CONFIG_FILE, path.join(RUN, "link.json"), path.join(K, "box", "vault")]) fs.rmSync(at, { recursive: true, force: true });
  fs.mkdirSync(path.dirname(CONFIG_FILE), { recursive: true });
  fs.writeFileSync(CONFIG_FILE, "{}");
  process.env.KUVO_TEST_MARKETPLACE_CATALOG = catalogFile;
});
afterEach(() => {
  globalThis.fetch = network;
  delete process.env.KUVO_TEST_MARKETPLACE_CATALOG;
  forgetKeys();
  forgetSkills();
  forgetServers();
});

test("the bundled catalog and real marks are available when the download fails", async () => {
  useNetwork(url => {
    expect(url.href).toBe(catalogFile);
    return new Response(null, { status: 503 });
  });
  const refresh = await route("POST", "/refresh");
  expect(refresh.json).toEqual({ refreshed: false, message: "The catalog couldn’t be refreshed. Showing the last available copy." });
  const listed = await route("GET");
  const [apps, agents, skills] = listed.json.areas;
  expect([apps.count, agents.count, skills.count]).toEqual([10, 6, 8]);
  expect(apps.items.find((app: any) => app.id === "github")).toMatchObject({ logo: "kuvo/marketplace/apps/github/logo", installed: false });
  expect(apps.items.map((app: any) => app.id)).not.toContain("slack");
  let status = 0;
  let bytes: Buffer | undefined;
  for await (const part of handler({ method: "GET", path: "kuvo/marketplace/apps/github/logo", fromApp: true, headers: {} }, undefined)) {
    if ("head" in part) status = part.head.status;
    else bytes = part.body;
  }
  expect(status).toBe(200);
  expect(bytes?.subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");
});

test("a GitHub preview and install use the same exact commit even after the default branch moves", async () => {
  const reads: string[] = [];
  useNetwork(url => {
    reads.push(url.hostname + url.pathname);
    if (url.pathname === "/repos/example/skills") return Response.json({ default_branch: "main" });
    if (url.pathname === "/repos/example/skills/commits/main") return Response.json({ sha: commit });
    if (url.pathname === `/repos/example/skills/git/trees/${commit}`) return Response.json({ tree: [
      { type: "blob", mode: "100644", path: "one/SKILL.md" }, { type: "blob", mode: "100644", path: "one/examples/first.md" },
    ] });
    if (url.pathname === `/example/skills/${commit}/one/SKILL.md`) return new Response(skillFile);
    if (url.pathname === `/example/skills/${commit}/one/examples/first.md`) return new Response("Pinned example.");
    return new Response(null, { status: 404 });
  });
  const preview = await route("POST", "/skills/preview", { link: "https://github.com/example/skills/tree/main/one" });
  expect(preview.status).toBe(200);
  expect(preview.json.skill.source).toEqual({ repository: "example/skills", path: "one", ref: "main", commit });
  expect(preview.json.files.map((file: any) => file.path)).toEqual(["SKILL.md", "examples/first.md"]);
  expect(reads).toHaveLength(5);
  const before = reads.slice();
  expect((await route("POST", "/skills/install", { preview: preview.json.preview })).status).toBe(200);
  expect(reads).toEqual(before);
  expect(fs.readFileSync(path.join(SKILLS_FOLDER, "private-skill", "SKILL.md"), "utf8")).toBe(skillFile);
  expect(fs.readFileSync(path.join(SKILLS_FOLDER, "private-skill", "examples/first.md"), "utf8")).toBe("Pinned example.");
});

test("installing a catalog skill writes the engine skill and the bridge’s native skill trees", async () => {
  const catalog = await route("GET", "/skills/review-changes");
  const { available } = catalog.json;
  expect((await route("POST", "/skills/review-changes/add", { version: available.version, revision: available.revision })).status).toBe(200);
  const file = path.join(SKILLS_FOLDER, "review-changes", "SKILL.md");
  const text = fs.readFileSync(file, "utf8");
  expect(text).toContain("Report concrete findings");
  const agentHome = path.join(K, "agent-test");
  setSkillSource({ ready: true, request: async (_: string, route: string) => route === "/api/skill"
    ? [{ name: "review-changes", path: file }] : [] } as unknown as EngineLink, agentHome);
  try {
    const skills = await skillsFor(agentHome, [], () => false);
    const native = skillsFolder(skills)!;
    copySkillsInto(path.join(agentHome, "antigravity", "config", "skills"), skills);
    expect(JSON.parse(fs.readFileSync(path.join(native, ".claude-plugin", "plugin.json"), "utf8")).name).toBe("kuvo-code");
    // Claude and Grok use the plugin's skills tree. Codex adds individual paths from it.
    expect(fs.readFileSync(path.join(native, "skills/review-changes/SKILL.md"), "utf8")).toBe(text);
    expect(fs.readFileSync(path.join(native, ".cursor/skills/review-changes/SKILL.md"), "utf8")).toBe(text);
    expect(fs.readFileSync(path.join(agentHome, "antigravity/config/skills/review-changes/SKILL.md"), "utf8")).toBe(text);
  } finally { setSkillSource(undefined, undefined); }
});

test("adding an official app makes it available to the engine and the coding-agent bridge", async () => {
  expect((await route("POST", "/apps/cloudflare-docs/add")).status).toBe(200);
  const config = JSON.parse(fs.readFileSync(CONFIG_FILE, "utf8"));
  expect(config.mcp.servers["marketplace-cloudflare-docs"]).toEqual({
    type: "remote", url: "https://docs.mcp.cloudflare.com/mcp", oauth: false, codemode: false,
  });
  setServerSource({ ready: true, request: async () => [{ info: config }] } as unknown as EngineLink);
  expect((await computerServers(K, true)).servers).toEqual([{
    name: "marketplace-cloudflare-docs", kind: "remote", url: "https://docs.mcp.cloudflare.com/mcp", headers: {}, timeout: 300000,
  }]);
  expect((await route("DELETE", "/apps/cloudflare-docs")).status).toBe(200);
  expect(JSON.parse(fs.readFileSync(CONFIG_FILE, "utf8")).mcp).toBeUndefined();
});

test("a service token goes in the vault and is absent from the engine and shared install files", async () => {
  makeVault();
  const relay = await startMarketplaceRelay();
  try {
    expect((await route("POST", "/apps/github/add")).status).toBe(200);
    const secret = "token-only-in-vault";
    expect((await route("POST", "/apps/github/connect", { token: secret })).json.connection).toBe("ready");
    const [item] = readItems();
    expect(await valuesOf(item!.id)).toEqual({ token: secret });
    const config = fs.readFileSync(CONFIG_FILE, "utf8");
    expect(config).not.toContain(secret);
    expect(JSON.parse(config).mcp.servers["marketplace-github"].url).toMatch(/^http:\/\/127\.0\.0\.1:/);
    expect(fs.readFileSync(path.join(marketplaceFolder, "standalone-v1.json"), "utf8")).not.toContain(secret);
  } finally { await new Promise<void>(resolve => relay.close(() => resolve())); }
});

test("OAuth exchanges a state-checked PKCE code and keeps the resulting tokens in the vault", async () => {
  makeVault();
  const relay = await startMarketplaceRelay();
  try {
    expect((await route("POST", "/apps/linear/add")).status).toBe(200);
    const service = "https://mcp.linear.app";
    let challenge: string | null = null;
    useNetwork(async (url, init) => {
      if (url.hostname === "127.0.0.1") return network(url, init);
      if (url.href === service + "/mcp") {
        if (new Headers(init?.headers).get("authorization") === "Bearer test-oauth-access")
          return Response.json({ result: { reached: "Linear" } }, { headers: { "mcp-session-id": "test-session" } });
        return new Response(null, { status: 401, headers: {
          "WWW-Authenticate": `Bearer resource_metadata="${service}/.well-known/oauth-protected-resource/mcp", scope="read"`,
        } });
      }
      if (url.href === service + "/.well-known/oauth-protected-resource/mcp")
        return Response.json({ resource: service + "/mcp", authorization_servers: [service], scopes_supported: ["read"] });
      if (url.href === service + "/.well-known/oauth-authorization-server") return Response.json({
        issuer: service, authorization_endpoint: service + "/authorize", token_endpoint: service + "/token",
        registration_endpoint: service + "/register", response_types_supported: ["code"],
        grant_types_supported: ["authorization_code", "refresh_token"],
        token_endpoint_auth_methods_supported: ["none"], code_challenge_methods_supported: ["S256"],
      });
      if (url.pathname === "/register") {
        const registration = JSON.parse(String(init?.body));
        expect(registration.token_endpoint_auth_method).toBe("none");
        return Response.json({ ...registration, client_id: "marketplace-test-client" });
      }
      if (url.pathname === "/token") {
        const parameters = new URLSearchParams(String(init?.body));
        expect(parameters.get("code")).toBe("valid-code");
        expect(crypto.createHash("sha256").update(parameters.get("code_verifier")!).digest("base64url")).toBe(challenge);
        return Response.json({ access_token: "test-oauth-access", refresh_token: "test-oauth-renewal", token_type: "Bearer" });
      }
      return new Response(null, { status: 404 });
    });
    const started = await route("POST", "/apps/linear/connect");
    expect(started.status).toBe(202);
    const page = new URL(started.json.opens);
    expect(page.origin + page.pathname).toBe(service + "/authorize");
    expect(page.searchParams.get("code_challenge_method")).toBe("S256");
    expect(page.searchParams.get("scope")).toBe("read");
    challenge = page.searchParams.get("code_challenge");
    const callback = new URL(page.searchParams.get("redirect_uri")!);
    callback.searchParams.set("code", "valid-code");
    callback.searchParams.set("state", "wrong");
    expect((await route("POST", "/apps/linear/finish", { attempt: started.json.attempt, address: callback.href })).status).toBe(400);
    expect(readItems()).toEqual([]);
    callback.searchParams.set("state", started.json.attempt);
    expect((await route("POST", "/apps/linear/finish", { attempt: started.json.attempt, address: callback.href })).status).toBe(200);
    const [item] = readItems();
    const { oauth } = await valuesOf(item!.id);
    expect(JSON.parse(oauth!).tokens).toMatchObject({ access_token: "test-oauth-access", refresh_token: "test-oauth-renewal" });
    const raw = fs.readFileSync(CONFIG_FILE, "utf8");
    expect(raw).not.toContain("test-oauth-access");
    const server = JSON.parse(raw).mcp.servers["marketplace-linear"];
    const result = await fetch(server.url, { method: "POST", headers: server.headers, body: "{}" });
    expect(await result.json()).toEqual({ result: { reached: "Linear" } });
    expect(result.headers.get("mcp-session-id")).toBe("test-session");
  } finally { await new Promise<void>(resolve => relay.close(() => resolve())); }
});

test("Add Agent creates a bot with the selected account, its job and a paused routine", async () => {
  fs.mkdirSync(path.join(K, "bin"), { recursive: true });
  fs.copyFileSync(new URL("../src/shared/bot-words.json", import.meta.url), path.join(K, "bin/bot-words.json"));
  const session = { id: "ses_marketplace", metadata: {}, permissions: [] };
  const messages: any[] = [];
  useNetwork((url, init) => {
    if (url.pathname === "/api/provider") return Response.json([{ id: "test-account" }]);
    if (url.pathname === "/api/integration") return Response.json([]);
    if (url.pathname === "/api/model") return Response.json([{ id: "test-model", providerID: "test-account", name: "Test Model" }]);
    if (url.pathname === "/api/model/default") return Response.json({ id: "test-model", providerID: "test-account" });
    if (url.pathname.endsWith("/instructions/entries/kuvo.bot")) { messages.push(JSON.parse(String(init?.body))); return Response.json({}); }
    if (url.pathname.startsWith("/api/session")) return Response.json(session);
    return Response.json({});
  });
  const body = { computer: "here", version: "1.0.0", request: "stable-marketplace-request", model: { providerID: "test-account", id: "test-model" } };
  const options = await route("GET", "/agents/coding-reviewer/options?computer=here");
  expect(options.json.models).toMatchObject([{ id: "test-model", providerID: "test-account" }]);
  expect(options.json.routines[0].enabled).toBe(false);
  const made = await route("POST", "/agents/coding-reviewer/add", body);
  expect(made.status).toBe(201);
  const { bot } = made.json;
  expect(bot).toMatchObject({ name: "Coding Reviewer", model: body.model, job: "Review changes for regressions and summarize findings for the author.", paused: true });
  const { json: schedule } = await ask(handler, "GET", `kuvo/wakes/${bot.session}`);
  expect(schedule.wakes).toHaveLength(1);
  expect(schedule.wakes[0]).toMatchObject({ every: "1w", summary: "Check for changes awaiting review" });
  expect(messages[0].value).toContain("Review a selected diff");
  expect((await route("POST", "/agents/coding-reviewer/add", body)).json.bot.id).toBe(bot.id);
  expect((await route("GET", "/agents/coding-reviewer")).json.bots).toEqual([{ id: bot.id, name: bot.name, computer: "here" }]);
});

test("an agent's routine turned on in Add runs from the start, and the sheet's words come from the box", async () => {
  fs.mkdirSync(path.join(K, "bin"), { recursive: true });
  fs.copyFileSync(new URL("../src/shared/bot-words.json", import.meta.url), path.join(K, "bin/bot-words.json"));
  const session = { id: "ses_marketplace_routine", metadata: {}, permissions: [] };
  useNetwork(url => {
    if (url.pathname === "/api/provider") return Response.json([{ id: "test-account" }]);
    if (url.pathname === "/api/integration") return Response.json([]);
    if (url.pathname === "/api/model") return Response.json([{ id: "test-model", providerID: "test-account", name: "Test Model" }]);
    if (url.pathname === "/api/model/default") return Response.json({ id: "test-model", providerID: "test-account" });
    if (url.pathname.startsWith("/api/session")) return Response.json(session);
    return Response.json({});
  });
  const details = (await route("GET", "/agents/bug-triager")).json;
  expect(details.labels).toMatchObject({ computer: "Runs on", apps: "Needed apps", routines: "Routines" });
  const options = (await route("GET", "/agents/bug-triager/options?computer=here")).json;
  expect(options.missing).toEqual([{ id: "linear", title: "Linear", connect: true, words: "Not connected" }]);
  expect(options.blocked).toBe("1 app still to connect");
  const listed = (await route("GET")).json.areas[1].items.find((agent: any) => agent.id === "coding-reviewer");
  expect(listed).toMatchObject({ detail: "1 skill", actions: [{ id: "add", title: "Add" }] });
  const body = { computer: "here", version: "1.0.0", request: "stable-marketplace-routine", routines: ["weekly"], model: { providerID: "test-account", id: "test-model" } };
  const made = await route("POST", "/agents/coding-reviewer/add", body);
  expect(made.status).toBe(201);
  expect(made.json.bot.paused).toBe(false);
  const { json: schedule } = await ask(handler, "GET", `kuvo/wakes/${made.json.bot.session}`);
  expect(schedule.wakes).toHaveLength(1);
  expect((await route("POST", "/agents/coding-reviewer/add", { ...body, request: "another-marketplace-routine", routines: ["nightly"] })).status).toBe(400);
});

test("Marketplace › Skills keeps what Settings › Skills did on this computer: a switch, Recently Deleted and Other Places", async () => {
  const skills = async () => (await route("GET")).json.areas[2];
  expect((await ask(handler, "PUT", "kuvo/skills/release", { text: skillFile.replace("private-skill", "release"), new: true })).status).toBe(200);
  let area = await skills();
  expect(area.here.switchTitle).toBe("Use on Test box");
  expect(area.categories[0].title).toBe("Your skills");
  expect(area.items.find((skill: any) => skill.id === "release")).toMatchObject({ category: "Your skills", installed: true, enabled: true });
  expect(area.items.find((skill: any) => skill.id === "review-changes").enabled).toBeUndefined();

  // Its details read like any other skill's, with the Edit form filled in, and Edit saves through it.
  const details = (await route("GET", "/skills/release")).json;
  const edit = details.actions.find((action: any) => action.id === "edit");
  expect(edit.form.fields.map((field: any) => [field.id, field.value])).toEqual([["description", "Write a short summary"], ["instructions", "Pinned instructions."]]);
  expect((await route("PATCH", "/skills/release", { description: "Write a shorter summary", instructions: "Keep it short.", was: details.revision })).status).toBe(200);
  expect((await route("GET", "/skills/release")).json.summary).toBe("Write a shorter summary");
  expect(area.actions.find((action: any) => action.id === "create").form.fields.map((field: any) => field.id)).toEqual(["name", "description", "instructions"]);
  expect((await route("POST", "/skills/preview", { from: "~/no-such-skill-folder" })).status).toBe(400);

  expect((await route("POST", "/skills/release/enabled", { on: false })).json).toEqual({ enabled: false });
  expect(JSON.parse(fs.readFileSync(CONFIG_FILE, "utf8")).permissions).toEqual([{ action: "skill", resource: "release", effect: "deny" }]);
  expect((await skills()).items.find((skill: any) => skill.id === "release").enabled).toBe(false);

  expect((await route("POST", "/skills/places", { place: "/srv/team-skills" })).status).toBe(200);
  expect((await skills()).here.places.items).toEqual([{ id: "0", title: "/srv/team-skills" }]);
  expect((await route("DELETE", "/skills/places", { index: 0 })).status).toBe(200);
  expect((await skills()).here.places.items).toEqual([]);

  expect((await route("DELETE", "/skills/release")).status).toBe(200);
  area = await skills();
  expect(area.items.some((skill: any) => skill.id === "release")).toBe(false);
  const [deleted] = area.here.deleted.items;
  expect(deleted.title).toBe("release");
  expect((await route("POST", "/skills/restore", { id: deleted.id })).status).toBe(200);
  expect((await skills()).items.some((skill: any) => skill.id === "release")).toBe(true);
});

test("your own apps list after the catalog's, and removing one asks first in the box's words", async () => {
  expect((await route("POST", "/apps/custom", { title: "Internal Deploy", url: "https://deploy.example.com/mcp", auth: "open" })).status).toBe(200);
  const apps = (await route("GET")).json.areas[0];
  expect(apps.categories.at(-1).title).toBe("Yours");
  const remove = (await route("GET", "/apps/internal-deploy")).json.actions.find((action: any) => action.id === "remove");
  expect(remove).toMatchObject({ style: "destructive", confirm: { title: "Remove Internal Deploy?", button: "Remove" } });
});

test("another computer receives the installed skill through the signed setup document", async () => {
  upgradeKuvoFolder(K);
  const identity = () => {
    const key = newKey();
    const sign = newSigningSeed();
    return { seed: key.toString("base64url"), key: publicOf(key).toString("base64url"), sign, signer: signingKeyOf(sign), address: addressOf(key).toString("base64url") };
  };
  const primary = identity(), member = identity();
  const setup = { id: newSetupID(), epoch: 1, primary: primary.key, sign: primary.signer, managed: true, checkpointed: true };
  const boxes = [primary, member].map(box => ({ key: box.key, sign: box.signer, address: box.address, kind: "machine", name: "Marketplace box" }));
  const a: LinkState = {
    key: primary.seed, sign: primary.sign, phones: [], setup: { ...setup, role: "primary" }, sharedSetup: setup.id,
    shared: { membershipAndAccess: { sequence: 1, value: { boxes } } },
  };
  const b: LinkState = { key: member.seed, sign: member.sign, phones: [], setup: { ...setup, role: "member" } };
  writeLink(a);
  grantRelayLease(a);
  const added = await route("POST", "/skills/create", { name: "shared-skill", description: "Confirm the handoff", instructions: "Check the next action." });
  expect(added.status).toBe(200);
  const docs = publishedShared(readLink()!).map(envelope => readDoc(envelope)!);
  const doc = docs.find(doc => doc.body.replica?.marketplaceInstallations);
  expect(doc).toBeDefined();
  takeShared(b, primary.key, doc!);
  writeLink(b);
  fs.rmSync(SKILLS_FOLDER, { recursive: true });
  const keep = keepMarketplaceProjection();
  try {
    expect((await route("GET", "/skills/shared-skill")).json).toMatchObject({ origin: "created", installed: true });
    expect(fs.readFileSync(path.join(SKILLS_FOLDER, "shared-skill", "SKILL.md"), "utf8")).toContain("Check the next action.");
  } finally { clearInterval(keep); }
});


test("a malformed refresh keeps the last successfully downloaded catalog", async () => {
  const changed = structuredClone(snapshot);
  changed.apps.find(app => app.id === "github")!.summary = "Updated GitHub description.";
  useNetwork(() => Response.json(changed));
  expect((await route("POST", "/refresh")).json).toEqual({ refreshed: true });
  expect((await route("GET", "/apps/github")).json.summary).toBe("Updated GitHub description.");
  useNetwork(() => Response.json({ ...changed, skills: [{}] }));
  expect((await route("POST", "/refresh")).json.refreshed).toBe(false);
  expect((await route("GET", "/apps/github")).json.summary).toBe("Updated GitHub description.");
  expect(JSON.parse(fs.readFileSync(path.join(marketplaceFolder, "catalog-v1.json"), "utf8")).skills).toHaveLength(8);
});
