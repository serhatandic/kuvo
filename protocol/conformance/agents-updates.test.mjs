// Updating the coding agents' programs, as every client sees it: Settings › Accounts' rows
// (`kuvo/agents/api/integration`, `metadata.programUpdate` and `metadata.updateAll`), the updates
// (`kuvo/agents/api/kuvo/updates`), the update notices, and the agents' events on the line
// (`kuvo/agents/api/event`). Real: each box, its sessions and its records. Stand-ins: Codex,
// Claude Code, Grok and Antigravity (`agy`) (their versions and own updaters, finished when the
// case says) and npm's registry (the newest). Cursor (Bun SDK, no installed CLI yet) and OpenCode
// (pinned with each box release under `engine/<version>/`) do not update through this path.
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, openLine, request, until, write } from "./adapter.mjs";
import { providers as acpProviders, scriptedAcp } from "./acp.mjs";
import { provider as claudeProvider, scriptedClaude } from "./claude.mjs";
import { provider, scriptedCodex } from "./codex.mjs";

const headers = { "x-kuvo-from": "app" };
const updates = "kuvo/agents/api/kuvo/updates";
const skip = (await import("./adapter.mjs")).boxBinary().typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;

/** npm's registry, as far as the boxes ask it: Codex's newest. */
async function registry(latest) {
  const said = { latest };
  const server = http.createServer((ask, answer) => {
    const found = ask.url === "/@openai%2Fcodex/latest" && said.latest;
    answer.writeHead(found ? 200 : 404, { "content-type": "application/json" });
    answer.end(found ? JSON.stringify({ name: "@openai/codex", version: said.latest }) : "{}");
  });
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  return { said, url: `http://127.0.0.1:${server.address().port}`, close: () => new Promise((done) => server.close(done)) };
}

/** A box with Codex at `installed`, npm's newest `latest`, and (`work`) an account added to it. */
async function withPrograms({ installed, latest, work = false, prepare = () => {} }, check) {
  const npm = await registry(latest);
  const codex = await scriptedCodex();
  try {
    codex.env.KUVO_TEST_NPM_REGISTRY = npm.url;
    write(path.join(codex.root, "codex-version"), installed);
    prepare(codex);
    const listener = await codex.start();
    if (work) {
      const added = await request(listener, "POST", "kuvo/agents/api/kuvo/accounts", { agent: "codex", label: "Work" }, headers);
      assert.equal(added.status, 200, added.text);
      codex.work = added.json.data.providerID;
    }
    const box = {
      codex, npm, listener,
      /** Each account's row, by provider (`fresh`: asked anew). */
      async rows(fresh = false) {
        const answer = await request(box.listener, "GET", "kuvo/agents/api/integration" + (fresh ? "?fresh=1" : ""), undefined, headers, 30000);
        assert.equal(answer.status, 200, answer.text);
        return Object.fromEntries(answer.json.data.map((row) => [row.id, row.metadata]));
      },
      /** The rows asked anew, once its version and the newest have come (a read waits two
       * seconds for them at most, and says them the next time). */
      async known() {
        await box.rows(true);
        return until(async () => {
          const rows = await box.rows();
          return rows[provider].latestVersion && rows;
        }, "Its newest version never came.");
      },
      async ask(route, method = "POST", body = {}) {
        const answer = await request(box.listener, method, route, method === "GET" || method === "DELETE" ? undefined : body, headers, 30000);
        return { status: answer.status, value: answer.json, text: answer.text };
      },
      /** Until its update is no longer under way: how it went. */
      async settled() {
        let row;
        return until(async () => {
          row = (await box.rows())[provider];
          return row.programUpdate && row.programUpdate.state !== "updating" && row;
        }, () => "The update never ended: " + JSON.stringify(row) + "\n" + codex.log(), 30000);
      },
      updatesRun: () => codex.heard().filter((line) => line.update).length,
      notices: async () => (await box.ask(updates + "/notices", "GET")).value.data.notices,
      /** Read anew, until the notices served are `ids` (each read decides them once its versions come). */
      async noticesAre(ids) {
        let now = [];
        await until(async () => {
          await box.rows(true);
          now = (await box.notices()).map((each) => each.id);
          return JSON.stringify(now) === JSON.stringify(ids);
        }, () => `The notices were ${JSON.stringify(now)}, not ${JSON.stringify(ids)}.`);
      },
    };
    await check(box);
  } catch (error) {
    throw new Error([error.message, codex.log()].join("\n"), { cause: error });
  } finally {
    await codex.close();
    await npm.close();
  }
}


// Simulating a real terminal update means the installed executable changed, not just the
// stand-in's version answer. Append a harmless blank line to change its file stamp.
function terminalUpdate(codex, version) {
  write(path.join(codex.root,"codex-version"),version);
  const executable=path.join(codex.root,".local","bin",process.platform==="win32"?"codex.cmd":"codex");
  fs.appendFileSync(executable,process.platform==="win32"?"\r\n":"\n");
}

test("a version read is lazy, and reopening Settings or restarting doesn't run the unchanged binary", {skip:boxBinary().typescript,timeout:90000}, () =>
  withPrograms({installed:"1.0.0",latest:"2.0.0"}, async box => {
    const versions=()=>box.codex.heard().filter(line=>line.version).length;
    // Reading the model picker is not opening the agents settings.
    const picker = await box.ask("kuvo/models/offered","GET");
    assert.equal(picker.status,200,picker.text);
    assert.equal(versions(),0);
    assert.equal((await box.known())[provider].version,"1.0.0");
    assert.equal(versions(),1);
    await box.rows(true);
    assert.equal(versions(),1);
    await box.codex.stop();
    box.listener=await box.codex.start();
    assert.equal((await box.known())[provider].version,"1.0.0");
    assert.equal(versions(),1);
    terminalUpdate(box.codex,"1.1.0");
    assert.equal((await box.known())[provider].version,"1.1.0");
    assert.equal(versions(),2);
  }));

test("Update All: the box picks a program behind once, answers as it starts, a second client sees it under way, then how it went", { skip, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "2.0.0", work: true, prepare: (codex) => {
    codex.go("codex-update-waits");
    write(path.join(codex.root, "codex-update-to"), "2.0.0");
  } }, async (box) => {
    const before = await box.known();
    // Said of the agent's own sign-in: once for its program.
    assert.deepEqual([before[provider].updateAll, before[box.codex.work].updateAll], [true, false]);
    assert.deepEqual([before[provider].version, before[provider].latestVersion, before[provider].outdated, before[provider].updates], ["1.0.0", "2.0.0", true, true]);
    assert.equal(before[provider].updateCommand, "codex update");
    assert.equal(before[provider].programUpdate, undefined);
    if (!boxBinary().typescript) {
      // One program shows one update across all its accounts; added accounts still show the version.
      assert.equal(before[box.codex.work].version, "1.0.0");
      assert.equal(before[box.codex.work].latestVersion, undefined);
      assert.equal(before[box.codex.work].outdated, false);
      assert.equal(before[box.codex.work].updates, false);
      assert.equal(before[box.codex.work].updateCommand, undefined);
      assert.equal(before[box.codex.work].programUpdate, undefined);
    }

    // The Mac's Update All: answered while the update still runs.
    assert.deepEqual((await box.ask(updates)).value.data, { updating: ["codex"] });
    // A second client sees it under way and not offered again.
    const during = await box.rows();
    assert.equal(during[provider].programUpdate.state, "updating");
    if (boxBinary().typescript) {
      assert.equal(during[box.codex.work].programUpdate.state, "updating");
    } else {
      assert.equal(during[box.codex.work].programUpdate, undefined);
    }
    assert.equal(during[provider].updateAll, false);
    assert.deepEqual((await box.ask(updates)).value.data, { updating: [] });

    box.codex.go("codex-update-go");
    const after = await box.settled();
    assert.equal(after.programUpdate.state, "updated", JSON.stringify(after));
    assert.equal(after.programUpdate.version, "2.0.0");
    assert.deepEqual([after.version, after.outdated, after.updateAll], ["2.0.0", false, false]);
    if (boxBinary().typescript) {
      assert.equal((await box.rows())[box.codex.work].programUpdate.state, "updated");
    } else {
      assert.equal((await box.rows())[box.codex.work].programUpdate, undefined);
    }
    assert.equal(box.updatesRun(), 1);
  }));

test("an update waits for the turns of its program under way, and a second ask doesn't start it early", { skip: skip || (!boxBinary().typescript && "The Rust box installs updates side by side without waiting for running turns."), timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "2.0.0", prepare: (codex) => {
    write(path.join(codex.root, "codex-update-to"), "2.0.0");
    codex.script("keep working", [{ until: "turn-go" }, ["item/completed", { item: { type: "agentMessage", id: "a1", text: "Done working." } }]]);
    codex.seed("ses_drain");
  } }, async (box) => {
    await box.codex.prompt("ses_drain", "keep working");
    await until(() => box.codex.sent("turn/start").length === 1, "Codex's turn never started.");
    assert.deepEqual((await box.ask(updates + "/" + provider)).value.data, { updating: ["codex"] });
    await new Promise((done) => setTimeout(done, 300));
    assert.equal(box.updatesRun(), 0);
    assert.deepEqual((await box.ask(updates + "/" + provider, "POST", { confirmed: true })).value.data, { updating: ["codex"] });
    await new Promise((done) => setTimeout(done, 300));
    assert.equal(box.updatesRun(), 0);
    box.codex.go("turn-go");
    await box.codex.idles("ses_drain");
    const after = await box.settled();
    assert.equal(after.programUpdate.state, "updated", JSON.stringify(after));
    assert.equal(box.updatesRun(), 1);
  }));

test("one that fails says why to every client, and is offered again until the version moves on", { skip, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "1.1.0", work: !boxBinary().typescript, prepare: (codex) => codex.go("codex-update-fails") }, async (box) => {
    await box.known();
    assert.deepEqual((await box.ask(updates + "/" + provider)).value.data, { updating: ["codex"] });
    const failed = await box.settled();
    assert.equal(failed.programUpdate.state, "failed");
    assert.equal(failed.programUpdate.message, "Codex is still 1.0.0 after “codex update”.\n\nno network");
    assert.equal(failed.updateAll, true);
    if (!boxBinary().typescript) {
      // A failed update is shown once for the program, never repeated on added accounts.
      assert.equal((await box.rows())[box.codex.work].programUpdate, undefined);
    }
    // Updated in a terminal since: no longer news.
    terminalUpdate(box.codex, "1.1.0");
    assert.equal((await box.rows(true))[provider].programUpdate, undefined);
  }));

test("why one failed, seen on one client, is gone for every client; one seen before a newer outcome leaves that one", { skip, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "1.1.0", prepare: (codex) => codex.go("codex-update-fails") }, async (box) => {
    await box.known();
    await box.ask(updates + "/" + provider);
    const failed = (await box.settled()).programUpdate;
    // Seen on an older outcome: this one stays.
    assert.equal((await box.ask(`${updates}/${provider}?at=${failed.at - 1}`, "DELETE")).status, 200);
    assert.equal((await box.rows())[provider].programUpdate.state, "failed");
    assert.equal((await box.ask(`${updates}/${provider}?at=${failed.at}`, "DELETE")).status, 200);
    const now = (await box.rows())[provider];
    assert.equal(now.programUpdate, undefined);
    assert.equal(now.updateAll, true);
  }));

test("an update asked for goes on after the client stops waiting, a client opened again sees it, then that it was already the newest; the old way of asking is gone", { skip, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "1.0.0", prepare: (codex) => codex.go("codex-update-waits") }, async (box) => {
    await box.rows(true);
    assert.deepEqual((await box.ask(updates + "/" + provider)).value.data, { updating: ["codex"] });
    assert.equal((await box.rows())[provider].programUpdate.state, "updating");
    // Asked again meanwhile: the one under way.
    assert.deepEqual((await box.ask(updates + "/" + provider)).value.data, { updating: ["codex"] });
    box.codex.go("codex-update-go");
    const after = await box.settled();
    assert.equal(after.programUpdate.state, "current", JSON.stringify(after));
    assert.equal(after.programUpdate.version, "1.0.0");
    // The way a Mac of 0.7 or before asked, waiting for its answer, is no route now.
    assert.equal((await box.ask(`kuvo/agents/api/integration/${provider}/update`)).status, 404);
    assert.equal(box.updatesRun(), 1);
  }));

test("a program under someone else's node_modules isn't the box's to update, alone or in Update All", { skip: skip || (process.platform === "win32" && "Links to programs are Unix's here."), timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "1.1.0", prepare: (codex) => {
    const bin = path.join(codex.root, ".local", "bin");
    const project = path.join(codex.root, "project", "node_modules", "codex-wrapper");
    fs.mkdirSync(project, { recursive: true });
    fs.renameSync(path.join(bin, "codex"), path.join(project, "codex"));
    fs.symlinkSync(path.join(project, "codex"), path.join(bin, "codex"));
  } }, async (box) => {
    const row = (await box.known())[provider];
    assert.deepEqual([row.outdated, row.updates, row.updateAll, row.updateCommand], [true, false, false, undefined]);
    assert.deepEqual((await box.ask(updates)).value.data, { updating: [] });
    assert.equal(box.updatesRun(), 0);
  }));

test("update notices, off: a newer version is available with Update, once, also after a restart, and not once seen; once it's updated, its notice goes", { skip, timeout: 120000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "2.0.0" }, async (box) => {
    await box.rows(true);
    const [notice] = await until(async () => {
      const notices = await box.notices();
      return notices.length && notices;
    }, "No notice was raised.");
    assert.deepEqual({ ...notice, at: 0 }, {
      id: "codex-available-2.0.0", program: "codex", provider, kind: "available", version: "2.0.0",
      title: "Codex 2.0.0 is available", body: "You have 1.0.0.", at: 0, action: { id: "update", label: "Update" },
    });
    // Read again, and after the box starts again: still the one.
    await box.known();
    await box.codex.stop();
    box.listener = await box.codex.start();
    await box.known();
    assert.deepEqual((await box.notices()).map((each) => each.id), ["codex-available-2.0.0"]);
    // Seen on one client: gone for every client, and not raised again.
    assert.equal((await box.ask(`${updates}/notices/${notice.id}`, "DELETE")).status, 200);
    await box.known();
    assert.deepEqual(await box.notices(), []);
    // A newer one is raised; once it's updated in Terminal, it goes.
    box.npm.said.latest = "2.1.0";
    await box.noticesAre(["codex-available-2.1.0"]);
    terminalUpdate(box.codex, "2.1.0");
    await box.noticesAre([]);
  }));

test("update notices, on: nothing is raised for what's out; once it updated itself, it says so, once", { skip, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "2.0.0", prepare: (codex) => write(path.join(codex.home, "box", "agents-update-themselves.json"), "true") }, async (box) => {
    await box.known();
    assert.deepEqual(await box.notices(), []);
    terminalUpdate(box.codex, "2.0.0");
    await box.rows(true);
    const notices = await until(async () => {
      const notices = await box.notices();
      return notices.length && notices;
    }, "No notice was raised.");
    assert.deepEqual(notices.map((each) => [each.id, each.title, each.body, each.action]), [
      ["codex-updated-2.0.0", "Codex updated to 2.0.0", "It was 1.0.0. It updates itself, as set in Settings › Computer.", undefined],
    ]);
    await box.rows(true);
    assert.equal((await box.notices()).length, 1);
  }));

test("the agents' events reach a client following them on the line: an update said as it starts and as it ends", { skip, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "2.0.0", prepare: (codex) => {
    codex.go("codex-update-waits");
    write(path.join(codex.root, "codex-update-to"), "2.0.0");
  } }, async (box) => {
    await box.rows(true);
    const watch = Buffer.from(JSON.stringify({ agents: "kuvo/agents/api/event" })).toString("base64url");
    const line = await openLine(box.listener, "?watch=" + watch);
    try {
      const frames = line.frames();
      const seen = [];
      const next = async (wanted, what) => {
        for (;;) {
          const { value: frame, done } = await frames.next().catch((error) => {
            throw new Error("Waiting for " + what + ": " + JSON.stringify(seen), { cause: error });
          });
          assert.equal(done, false, "The line ended waiting for " + what + ": " + JSON.stringify(seen));
          if (frame?.t !== "agents" || !frame.d?.type) {
            seen.push(frame);
            continue;
          }
          seen.push(frame.d.type);
          if (wanted(frame.d)) return frame.d;
        }
      };
      await next((event) => event.type === "server.connected", "the feed");
      assert.equal((await box.ask(updates + "/" + provider)).status, 200);
      const told = (event) => event.type === "integration.updated" && event.data?.integrationID === provider;
      await next(told, "the update starting");
      box.codex.go("codex-update-go");
      await next(told, "the update ending");
      assert.match((await next((event) => event.type === "model.updated", "the new program")).data.program, /^codex$/);
    } finally {
      line.close();
    }
  }));

function stagedDirs(home, program) {
  const dir = path.join(home, "run", "agents", "programs", program);
  if (!fs.existsSync(dir)) return [];
  return fs.readdirSync(dir).filter((name) => name.startsWith("v_") && fs.statSync(path.join(dir, name)).isDirectory()).sort();
}

function stagedDirOf(home, program, scriptPath) {
  const dir = fs.realpathSync(path.join(home, "run", "agents", "programs", program));
  const actual = fs.existsSync(scriptPath) ? fs.realpathSync(scriptPath) : scriptPath;
  const rel = path.relative(dir, actual);
  const top = rel.split(path.sep)[0];
  assert.ok(top && top.startsWith("v_"), `Expected ${actual} inside ${dir}/v_*`);
  return top;
}

async function rowsOf(listener) {
  const answer = await request(listener, "GET", "kuvo/agents/api/integration", undefined, headers, 30000);
  assert.equal(answer.status, 200, answer.text);
  return Object.fromEntries(answer.json.data.map((row) => [row.id, row.metadata]));
}

async function waitUpdated(listener, providerId, log) {
  let row;
  return until(async () => {
    row = (await rowsOf(listener))[providerId];
    return row?.programUpdate && row.programUpdate.state !== "updating" && row;
  }, () => `The update for ${providerId} never ended: ${JSON.stringify(row)}\n${log()}`, 30000);
}

test("Codex updates side by side while a turn runs: new turns use the new version at once, the running turn finishes on the old, then the old version is removed", { skip: boxBinary().typescript, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "2.0.0", prepare: (codex) => {
    write(path.join(codex.root, "codex-update-to"), "2.0.0");
    codex.script("hold codex", [{ until: "codex-turn-go" }, ["item/completed", { item: { type: "agentMessage", id: "a1", text: "Finished on v1." } }]]);
    codex.script("fresh codex", [["item/completed", { item: { type: "agentMessage", id: "a2", text: "Ran on v2." } }]]);
    codex.seed("ses_sbs_codex_1");
    codex.seed("ses_sbs_codex_2");
  } }, async (box) => {
    await box.codex.prompt("ses_sbs_codex_1", "hold codex");
    await until(() => box.codex.sent("turn/start").length === 1, "Codex's first turn never started.");
    const firstStart = box.codex.heard().find((line) => line.argv);
    const v1 = stagedDirOf(box.codex.home, "codex", firstStart.script);
    assert.equal(firstStart.startedVersion, "1.0.0");

    // Request the update while Turn 1 is still running: it installs immediately without waiting.
    assert.deepEqual((await box.ask(updates + "/" + provider)).value.data, { updating: ["codex"] });
    const after = await box.settled();
    assert.equal(after.programUpdate.state, "updated", JSON.stringify(after));
    assert.equal(after.version, "2.0.0");

    // Both v1 (leased by Turn 1) and v2 (active for new turns) exist side by side.
    const both = stagedDirs(box.codex.home, "codex");
    assert.equal(both.length, 2, `Expected both versions side by side, saw ${JSON.stringify(both)}`);
    const v2 = both.find((each) => each !== v1);

    // While Turn 1 is still held on v1, a new turn starts on v2 immediately.
    await box.codex.prompt("ses_sbs_codex_2", "fresh codex");
    await box.codex.idles("ses_sbs_codex_2");
    const starts = box.codex.heard().filter((line) => line.argv);
    const secondStart = starts.find((line) => line.pid !== firstStart.pid);
    assert.ok(secondStart, "New turn should have started a new Codex process on v2");
    assert.equal(secondStart.startedVersion, "2.0.0");
    assert.equal(stagedDirOf(box.codex.home, "codex", secondStart.script), v2);

    // Release Turn 1: it finishes cleanly on v1, then v1 is automatically removed.
    box.codex.go("codex-turn-go");
    await box.codex.idles("ses_sbs_codex_1");
    await until(() => !fs.existsSync(path.join(box.codex.home, "run", "agents", "programs", "codex", v1)),
      () => `Old Codex version ${v1} was not removed after Turn 1 finished: ${JSON.stringify(stagedDirs(box.codex.home, "codex"))}`, 15000);
    assert.deepEqual(stagedDirs(box.codex.home, "codex"), [v2]);
  }));

test("Claude Code updates side by side while a turn runs: new turns use the new version at once, the running turn finishes on the old, then the old version is removed", { skip: boxBinary().typescript, timeout: 90000 }, async () => {
  const claude = await scriptedClaude({ nativeInstall: true });
  try {
    write(path.join(claude.env.HOME, "claude-version"), "1.0.0");
    write(path.join(claude.env.HOME, "claude-update-to"), "2.0.0");
    claude.script("hold claude", [
      { until: "claude-turn-go" },
      { type: "assistant", parent_tool_use_id: null, session_id: "$SESSION", message: { id: "a1", role: "assistant", content: [{ type: "text", text: "Finished on v1." }] } },
      { type: "result", subtype: "success", is_error: false, result: "", session_id: "$SESSION", user_message_uuids: ["$UUID"] },
    ]);
    claude.script("fresh claude", [
      { type: "assistant", parent_tool_use_id: null, session_id: "$SESSION", message: { id: "a2", role: "assistant", content: [{ type: "text", text: "Ran on v2." }] } },
      { type: "result", subtype: "success", is_error: false, result: "", session_id: "$SESSION", user_message_uuids: ["$UUID"] },
    ]);
    claude.seed("ses_sbs_claude_1");
    claude.seed("ses_sbs_claude_2");
    const listener = await claude.start();

    await claude.prompt("ses_sbs_claude_1", "hold claude");
    await until(() => claude.heard().some((line) => line.type === "user"), "Claude's first turn never started.");
    const firstUser = claude.heard().find((line) => line.type === "user");
    const firstStart = claude.heard().find((line) => line.argv && line.pid === firstUser.pid);
    const v1 = stagedDirOf(claude.home, "claude", firstStart.script);
    assert.equal(firstStart.startedVersion, "1.0.0");

    // Request update while Turn 1 is still running: it installs immediately without waiting.
    const ask = await request(listener, "POST", `${updates}/${claudeProvider}`, {}, headers, 30000);
    assert.equal(ask.status, 200, ask.text);
    assert.deepEqual(ask.json.data, { updating: ["claude"] });
    const after = await waitUpdated(listener, claudeProvider, claude.log);
    assert.equal(after.programUpdate.state, "updated", JSON.stringify(after));
    assert.equal(after.version, "2.0.0");

    const both = stagedDirs(claude.home, "claude");
    assert.equal(both.length, 2, `Expected both Claude versions side by side, saw ${JSON.stringify(both)}`);
    const v2 = both.find((each) => each !== v1);

    // Start a new turn on another conversation while Turn 1 is still running on v1.
    await claude.prompt("ses_sbs_claude_2", "fresh claude");
    await claude.idles("ses_sbs_claude_2");
    const secondUser = claude.heard().find((line) => line.type === "user" && line.pid !== firstUser.pid);
    assert.ok(secondUser, "New Claude turn should have run on a new process");
    const secondStart = claude.heard().find((line) => line.argv && line.pid === secondUser.pid);
    assert.equal(secondStart.startedVersion, "2.0.0");
    assert.equal(stagedDirOf(claude.home, "claude", secondStart.script), v2);

    // Release Turn 1: it finishes on v1, its outdated holder stops, and v1 is removed.
    claude.go("claude-turn-go");
    await claude.idles("ses_sbs_claude_1");
    await until(() => !fs.existsSync(path.join(claude.home, "run", "agents", "programs", "claude", v1)),
      () => `Old Claude version ${v1} was not removed after Turn 1 finished: ${JSON.stringify(stagedDirs(claude.home, "claude"))}`, 15000);
    assert.deepEqual(stagedDirs(claude.home, "claude"), [v2]);
  } catch (error) {
    throw new Error([error.message, claude.log()].join("\n"), { cause: error });
  } finally {
    await claude.close();
  }
});

for (const { label, name, providerId, program, grokNative } of [
  { label: "Grok", name: "grok", providerId: acpProviders.grok, program: "grok", grokNative: true },
  { label: "Antigravity", name: "agy", providerId: acpProviders.agy, program: "antigravity", grokNative: false },
]) {
  test(`${label} updates side by side while a turn runs: new turns use the new version at once, the running turn finishes on the old, then the old version is removed`, { skip: boxBinary().typescript, timeout: 90000 }, async () => {
    const acp = await scriptedAcp({ names: [name], grok: name === "grok", agy: name === "agy", grokNative });
    try {
      write(path.join(acp.root, `${name}-version`), "1.0.0");
      write(path.join(acp.root, `${name}-update-to`), "2.0.0");
      acp.script("hold acp", [
        { until: "acp-turn-go" },
        { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Finished on v1." } } },
      ]);
      acp.script("fresh acp", [
        { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Ran on v2." } } },
      ]);
      acp.seed(`ses_sbs_${name}_1`, providerId);
      acp.seed(`ses_sbs_${name}_2`, providerId);
      const listener = await acp.start();

      await acp.prompt(`ses_sbs_${name}_1`, "hold acp");
      await until(() => acp.sent("session/prompt").length === 1, `${label}'s first turn never started.`);
      const firstStart = acp.started(name)[0];
      const v1 = stagedDirOf(acp.home, program, firstStart.script);
      assert.equal(firstStart.startedVersion, "1.0.0");

      // Request update while Turn 1 is still running: it installs immediately without waiting.
      const ask = await request(listener, "POST", `${updates}/${providerId}`, {}, headers, 30000);
      assert.equal(ask.status, 200, ask.text);
      assert.deepEqual(ask.json.data, { updating: [program] });
      const after = await waitUpdated(listener, providerId, acp.log);
      assert.equal(after.programUpdate.state, "updated", JSON.stringify(after));
      assert.equal(after.version, "2.0.0");

      const both = stagedDirs(acp.home, program);
      assert.equal(both.length, 2, `Expected both ${label} versions side by side, saw ${JSON.stringify(both)}`);
      const v2 = both.find((each) => each !== v1);

      // Start a new turn on another conversation while Turn 1 is still running on v1.
      await acp.prompt(`ses_sbs_${name}_2`, "fresh acp");
      await acp.idles(`ses_sbs_${name}_2`);
      const secondStart = acp.started(name).find((line) => line.pid !== firstStart.pid);
      assert.ok(secondStart, `New ${label} turn should have started a new process on v2`);
      assert.equal(secondStart.startedVersion, "2.0.0");
      assert.equal(stagedDirOf(acp.home, program, secondStart.script), v2);

      // Release Turn 1: it finishes on v1, its retired process stops, and v1 is removed.
      acp.go("acp-turn-go");
      await acp.idles(`ses_sbs_${name}_1`);
      await until(() => !fs.existsSync(path.join(acp.home, "run", "agents", "programs", program, v1)),
        () => `Old ${label} version ${v1} was not removed after Turn 1 finished: ${JSON.stringify(stagedDirs(acp.home, program))}`, 15000);
      assert.deepEqual(stagedDirs(acp.home, program), [v2]);
    } catch (error) {
      throw new Error([error.message, acp.log()].join("\n"), { cause: error });
    } finally {
      await acp.close();
    }
  });
}
