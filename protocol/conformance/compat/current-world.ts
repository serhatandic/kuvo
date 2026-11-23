// Generate a current beta home with the actual storage writers. Inputs are
// synthetic and fixed, including the vault's fixture-only entropy.
import assert from "node:assert/strict";
import crypto from "node:crypto";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { publicOf } from "../../../box/src/link/crypto/keys";
import { writeLink } from "../../../box/src/link/state";
import { writeCard } from "../../../box/src/link/setup/card";
import { startSetup } from "../../../box/src/link/setup/lead";
import { changeProjects } from "../../../box/src/link/areas/projects";
import { adoptBox } from "../../../box/src/link/compat/adopt-settings";
import { keepLabel } from "../../../box/src/link/work-queue/labels";
import { movePast } from "../../../box/src/link/work-queue/numbers";
import { changeQueues, itemOf, YOURS } from "../../../box/src/link/work-queue/store";
import { changePersonalities } from "../../../box/src/link/personalities/library";
import { setZone } from "../../../box/src/link/zone/setting";
import { addItem, makeVault, setGrant, setPaused } from "../../../box/src/link/vault/store";
import { InsightsStore } from "../../../box/src/usage/insights-store";
import { UsageSettings } from "../../../box/src/usage/settings";
import { Store } from "../../../box/src/agents/store";
import { GroupStore } from "../../../box/src/agents/groups/store";
import { moveGitSettings } from "../../../box/src/link/areas/git-words";
import { archiveContext, seal } from "../../../box/src/link/backup/archive";
import { leaveLetter } from "../../../box/src/link/letters/leave";
import { noteMovedAway, noteEnded } from "../../../box/src/link/moves/records";
import { noteStopped } from "../../../box/src/link/work/background";
import { changeQueue, keepWaiting } from "../../../box/src/link/engine/queue";

const [home, directory] = process.argv.slice(2);
assert.equal(process.env.KUVO_HOME, home);
const now = 1760000000000;
const seed = (n: number) => Buffer.alloc(32, n).toString("base64url");
const link = { key: seed(31), secret: seed(32), sign: seed(33), phones: [], name: "Golden Computer", relay: "ws://127.0.0.1:1" };
const setup = "set_8gOLfHXdUhmcVfSh9NOsAQ";
const own = publicOf(Buffer.from(link.key, "base64url")).toString("base64url");

Date.now = () => now;
startSetup(link, setup);
writeLink(link, true);
writeCard();
mkdirSync(directory, { recursive: true });
writeFileSync(path.join(directory, "README.md"), "# Golden Project\n");
changeProjects(() => [{ directory, name: "Golden Project", run: "npm test" }]);
assert.equal(adoptBox({ settings: { instructions: ["GOLDEN.md"] }, keys: { KUVO_KEY_NOTION: "fixture-integration-key" } }).seeded, true);
moveGitSettings({ branchPrefix: "golden/" });
changePersonalities(() => ({ personalities: [{ id: "custom.golden", name: "Golden Notes", description: "Keep precise notes.", symbol: "note.text", updated: 1 }], removed: [] }));
setZone("Etc/UTC");

assert.equal(keepLabel({ id: "lbl_golden", name: "Golden", color: "sky" }), true);
movePast(1);
const item = itemOf({ id: "wqi_golden12345678901234", title: "Keep this item", status: "todo", number: 1,
  labels: ["lbl_golden"], created: { at: now, by: { user: true } } }, YOURS);
assert.ok(item);
changeQueues(book => book.queue(YOURS).items.push(item), now);

const usage = path.join(home, "agents/usage");
mkdirSync(usage, { recursive: true });
const usageSettings = new UsageSettings(path.join(usage, "settings.json"), () => now, () => own);
usageSettings.inSetup(setup);
usageSettings.take({ codexFallback: "golden-model" });
const insights = new InsightsStore(path.join(usage, "insights.sqlite"));
insights.write("local", { fixture: "golden" });
insights.close();

const store = new Store(path.join(home, "agents/sessions"));
store.add({ id: "ses_golden", title: "Golden Conversation", projectID: "prj_golden", model: { providerID: "codex-cli", id: "golden-model" },
  location: { directory }, cost: 0, tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } }, time: { created: now, updated: now } });
const conversation = store.get("ses_golden")!;
conversation.messages.push({ type: "user", id: "msg_golden", text: "Keep this conversation.", time: { created: now } });
store.save("ses_golden", true);
store.flush();
changeQueue("ses_golden", state => ({ ...state, paused: true, userPaused: true }));
assert.equal(keepWaiting("ses_golden", { id: "msg_goldenqueued", text: "Keep this queued message.", delivery: "queue" }), true);

const groups = new GroupStore(path.join(home, "groups"), path.join(home, "bots"));
const created = new Date(now).toISOString();
const group = { id: "grp_golden", name: "Golden Group", members: [], seats: {}, heard: {}, created };
const kept = { group, status: "held" as const,
  messages: [{ id: "message", text: "Keep this group message.", time: created, kind: "message" as const }] };
groups.saveGroup(kept);
groups.saveMessages(kept);

const peer = publicOf(Buffer.alloc(32, 34)).toString("base64url");
leaveLetter({ from: "ses_golden", to: "ses_remote", text: "Keep this letter.", peer, wait: false }, now, "ltr_golden");
noteMovedAway(["ses_moved"], peer, now);
noteEnded("mov_golden", "switched", now, ["ses_moved"], undefined, peer);
noteStopped("ses_golden", undefined, now);

const original = crypto.randomBytes;
let counter = 0;
crypto.randomBytes = ((size: number) => {
  assert.ok(size <= 32, "fixture entropy only supplies the vault and backup writers");
  return crypto.createHash("sha256").update("public golden home " + ++counter).digest().subarray(0, size);
}) as typeof original;
try {
  makeVault();
  const item = await addItem({ kind: "key", name: "Golden secret", values: { token: "synthetic-test-value" } });
  setGrant(item.id, { everyone: true }, "ask");
  setPaused(true);
  const backups = path.join(home, "fixture-deposit", own);
  mkdirSync(backups, { recursive: true });
  seal(path.join(backups, "point.kuvo"), seed(35), [
    { name: "box/opencode.json", file: path.join(home, "box/opencode.json") },
  ], archiveContext(own, "point"));
} finally {
  crypto.randomBytes = original;
}
