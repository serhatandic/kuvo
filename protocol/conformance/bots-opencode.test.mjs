// Bots on one of OpenCode's models (PROTOCOL.md, `kuvo/bots`), on the pinned OpenCode with a
// labelled stand-in model: made from a template, waiting, their conversation marked as the bot's,
// and told who they are and where they run (on a Mac, and on a machine). The reference makes them
// on its engine; the Rust box on the OpenCode agent. These cases assert what the app reads and what
// the model hears.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { stopFixtureHolders } from "./claude.mjs";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { STAND_IN, standInModel, standInSettings } from "./opencode-model.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const version = "2.2.6";
const headers = { "x-kuvo-from": "app" };
const binary = boxBinary();
const skip = (!pinned && "Set KUVO_CONFORMANCE_OPENCODE to the pinned OpenCode (used in place).")
  || (binary.typescript && process.platform === "win32" && "The reference runs on Unix.");
const words = new URL("../../box/src/shared/bot-words.json", import.meta.url);

for (const [where, mac, told] of [
  ["a machine", false, "You have a web browser: one on this machine"],
  ["a Mac", true, "You run on this Mac."],
]) {
  describe(`Bots on OpenCode's models, on ${where}`, { skip }, () => {
    let root, home, model, relay, box, listener, worker;
    const script = [];

    before(async () => {
      root = fs.realpathSync(scratch());
      home = path.join(root, "box");
      privateHome(home);
      write(path.join(home, "folder-version.json"), '{"version":23}\n');
      const program = path.join(home, "engine", version, process.platform === "win32" ? "opencode.exe" : "opencode");
      fs.mkdirSync(path.dirname(program), { recursive: true });
      if (process.platform === "win32") fs.copyFileSync(fs.realpathSync(pinned), program);
      else fs.symlinkSync(fs.realpathSync(pinned), program);
      model = await standInModel(script);
      write(path.join(home, "config", "opencode", "opencode.json"), JSON.stringify(standInSettings(model.url)));
      write(path.join(home, "box", "opencode.json"), "{}\n");
      write(path.join(home, "bin", "bot-words.json"), fs.readFileSync(words));
      if (mac) write(path.join(home, "box", "kind"), "mac\n");
      const env = { ...isolatedEnv(root), KUVO_SANDBOX: "off", OPENCODE_DISABLE_MODELS_FETCH: "1" };
      relay = await localRelay();
      if (binary.typescript) {
        const reserve = net.createServer();
        await new Promise(resolve => reserve.listen(0, "127.0.0.1", resolve));
        const port = reserve.address().port;
        await new Promise(resolve => reserve.close(resolve));
        const password = mac ? "conformance-local" : "scratch-engine";
        const dirs = Object.fromEntries(["data", "config", "state", "cache"].map(sub => [`XDG_${sub.toUpperCase()}_HOME`, path.join(home, sub)]));
        for (const folder of [...Object.values(dirs), path.join(home, "workspace")]) fs.mkdirSync(folder, { recursive: true });
        worker = launch(program, ["serve", "--port", String(port), "--hostname", "127.0.0.1"],
          { ...env, ...dirs, OPENCODE_PASSWORD: password, OPENCODE_SERVER_PASSWORD: password }, path.join(home, "workspace"));
        await until(async () => (await request({ port, password }, "GET", "api/session?limit=1").catch(() => null))?.status === 200, "The reference's engine", 30000);
        write(path.join(home, "run", "engine.json"), JSON.stringify({ port, password, pid: worker.child.pid, version }));
      }
      box = startBox({ binary, home, env, relay, mac });
      listener = await box.ready();
    });

    after(async () => {
      await box?.stop();
      if (home) await stopFixtureHolders(home);
      await worker?.stop?.();
      await relay?.close();
      await model?.close();
      if (root) fs.rmSync(root, { recursive: true, force: true, maxRetries: 5 });
    });

    const call = (method, route, body, timeout = 30000) => request(listener, method, route, body, headers, timeout);
    let bot;

    test("a bot made from a template on one of its models waits, with the template's job for the composer", { timeout: 90000 }, async () => {
      const made = await call("POST", "kuvo/bots", { name: "Herald", template: "news", model: STAND_IN });
      assert.equal(made.status, 201, made.text);
      bot = made.json.bot;
      assert.match(bot.id ?? "", /\S/);
      assert.match(bot.session ?? "", /^ses_/);
      assert.deepEqual([bot.name, bot.paused, bot.watching, bot.model], ["Herald", false, 0, STAND_IN]);
      assert.equal(typeof bot.look, "object");
      assert.equal(typeof made.json.firstMessage, "string");
      const listed = await call("GET", "kuvo/bots");
      assert.equal(listed.status, 200, listed.text);
      assert.ok(listed.json.bots.some(each => each.id === bot.id && each.session === bot.session && each.name === "Herald"), listed.text);
      // It waits: nothing went to the model.
      assert.equal(model.requests.filter(asked => asked.tools?.length).length, 0);
    });

    test(`a bot made is told who it is and that it runs on ${where}, and its conversation is marked as its`, { timeout: 120000 }, async () => {
      let found;
      await until(async () => (found = await call("GET", `kuvo/sessions/${bot.session}/location`)).status !== 503,
        () => `Its location never answered: ${found?.text}`, 30000, 200);
      const at = JSON.stringify(found.json).includes("agents") ? "kuvo/agents/" : "";
      // (The Rust box makes it on the OpenCode agent; the reference on its engine.)
      if (!binary.typescript) assert.equal(at, "kuvo/agents/", found.text);
      const header = await call("GET", `${at}api/session/${bot.session}`);
      assert.equal(header.status, 200, header.text);
      assert.equal((header.json.data ?? header.json).metadata?.kuvo?.bot, bot.id);
      script.push({ text: "Herald here." });
      const sent = await call("POST", `kuvo/sessions/${bot.session}/send`, { id: `msg_${Date.now().toString(36)}`, words: "Who are you?", delivery: "steer" });
      assert.equal(sent.status, 200, sent.text);
      const heard = await until(() => model.requests.find(asked => asked.tools?.length), "The bot's turn never reached the model.", 90000, 250);
      // (Where in the request it's said is each one's own: the system prompt, or a reminder.)
      const system = JSON.stringify(heard.messages).replaceAll("\\u2019", "'").replaceAll("’", "'");
      assert.ok(system.includes("You are Herald, one of the user's bots in Kuvo Code"), system.slice(0, 1500));
      assert.ok(system.includes(told), `It wasn't told "${told}": ${system.slice(0, 1500)}`);
    });
  });
}
