// Updating the coding agents' programs, as every client sees it: Settings › Accounts' rows
// (`kuvo/agents/api/integration`, `metadata.programUpdate` and `metadata.updateAll`), the updates
// (`kuvo/agents/api/kuvo/updates`), the update notices, and the agents' events on the line
// (`kuvo/agents/api/event`). Real: each box, its sessions and its records. Stand-ins: Codex (its
// version and its own updater, finished when the case says) and npm's registry (the newest).
import assert from "node:assert/strict";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { test } from "node:test";
import { openLine, request, until, write } from "./adapter.mjs";
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

test("Update All: the box picks a program behind once, answers as it starts, a second client sees it under way on every account with it, then how it went", { skip, timeout: 90000 }, () =>
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

    // The Mac's Update All: answered while the update still runs.
    assert.deepEqual((await box.ask(updates)).value.data, { updating: ["codex"] });
    // The phone, meanwhile: under way on every account with it, and not offered again.
    const during = await box.rows();
    assert.equal(during[provider].programUpdate.state, "updating");
    assert.equal(during[box.codex.work].programUpdate.state, "updating");
    assert.equal(during[provider].updateAll, false);
    assert.deepEqual((await box.ask(updates)).value.data, { updating: [] });

    box.codex.go("codex-update-go");
    const after = await box.settled();
    assert.equal(after.programUpdate.state, "updated", JSON.stringify(after));
    assert.equal(after.programUpdate.version, "2.0.0");
    assert.deepEqual([after.version, after.outdated, after.updateAll], ["2.0.0", false, false]);
    assert.equal((await box.rows())[box.codex.work].programUpdate.state, "updated");
    assert.equal(box.updatesRun(), 1);
  }));

test("an update waits for the turns of its program under way, and a second ask doesn't start it early", { skip, timeout: 90000 }, () =>
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
  withPrograms({ installed: "1.0.0", latest: "1.1.0", prepare: (codex) => codex.go("codex-update-fails") }, async (box) => {
    await box.known();
    assert.deepEqual((await box.ask(updates + "/" + provider)).value.data, { updating: ["codex"] });
    const failed = await box.settled();
    assert.equal(failed.programUpdate.state, "failed");
    assert.equal(failed.programUpdate.message, "Codex is still 1.0.0 after “codex update”.\n\nno network");
    assert.equal(failed.updateAll, true);
    // Updated in a terminal since: no longer news.
    write(path.join(box.codex.root, "codex-version"), "1.1.0");
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
    write(path.join(box.codex.root, "codex-version"), "2.1.0");
    await box.noticesAre([]);
  }));

test("update notices, on: nothing is raised for what's out; once it updated itself, it says so, once", { skip, timeout: 90000 }, () =>
  withPrograms({ installed: "1.0.0", latest: "2.0.0", prepare: (codex) => write(path.join(codex.home, "box", "agents-update-themselves.json"), "true") }, async (box) => {
    await box.known();
    assert.deepEqual(await box.notices(), []);
    write(path.join(box.codex.root, "codex-version"), "2.0.0");
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
