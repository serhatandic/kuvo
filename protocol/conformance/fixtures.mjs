// Given steps seed private files or supply input at the engine/browser boundaries.
import fs from "node:fs";
import { createHash } from "node:crypto";
import path from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { launch, typescriptRuntime, write } from "./adapter.mjs";
import { git, sessionOf } from "./engine.mjs";

const files = new URL("../../box/test/fixtures/routes/box/", import.meta.url);
const words = new URL("../../box/src/shared/bot-words.json", import.meta.url);
const wake = new URL("../../box/src/keeper/kuvo-wake", import.meta.url);

export function seed(home, root, mac, engine) {
  write(path.join(home, "folder-version.json"), '{"version":23}\n');
  for (const name of ["integrations.json", "mcp-catalog.json"]) {
    write(path.join(home, "bin", name), fs.readFileSync(new URL(name, files)));
  }
  write(path.join(home, "bin", "bot-words.json"), fs.readFileSync(words));
  write(path.join(home, "bin", "kuvo-wake"), fs.readFileSync(wake), { mode: 0o755 });
  if (mac) {
    write(path.join(home, "box", "kind"), "mac\n");
    write(path.join(home, "bots", "kit.json"), fs.readFileSync(new URL("mac-bot-kit.json", files)));
  }
  write(path.join(home, "box", "opencode.json"), "{}\n");
  write(path.join(home, "run", "engine.json"), JSON.stringify(engine));
  fs.mkdirSync(path.join(root, "work"));
}

export function routeFixtures({ binary, home, root, env, engine, children }) {
  const botsRoot = path.join(home, "bots");
  const wakesRoot = path.join(home, "wakes");
  const vars = { chats: path.join(home, "chats"), skills: path.join(home, "config", "opencode", "skills"),
    work: path.join(root, "work"), keys: path.join(home, "box", "keys"), home };
  let browserProcess;

  const look = { color: "mint", shape: "round", face: "calm" };
  const botFolder = id => path.join(botsRoot, id);

  // `given`: box state, as this box keeps it.
  const fixtures = {
    boxServices() {
      // The resident starts services itself.
    },
    session(given) {
      engine.sessions.push({ ...sessionOf(given), ...(given.model ? { model: given.model } : {}), ...(given.metadata ? { metadata: given.metadata } : {}) });
      if (given.running) engine.running.add(given.id);
      engine.emit({ type: "session.created", data: { sessionID: given.id } });
      if (given.running) engine.emit({ type: "session.execution.started", data: { sessionID: given.id } });
    },
    bot(given) {
      const folder = botFolder(given.id);
      const created = given.created ?? "2026-10-01T09:00:00Z";
      write(path.join(folder, "bot.json"), JSON.stringify({
        kuvo: 1, id: given.id, name: given.name, job: given.job ?? "", look: given.look ?? look,
        ...(given.personality ? { personality: given.personality } : {}), ...(given.model ? { model: given.model } : {}),
        ...(given.project ? { project: given.project } : {}), created, updated: created,
      }));
      write(path.join(folder, "session"), given.session);
      write(path.join(folder, "MEMORY.md"), `# ${given.name}'s memory\n`);
      fs.mkdirSync(path.join(folder, "wakes"), { recursive: true });
      const directory = given.project ?? path.join(folder, "workspace");
      fs.mkdirSync(path.join(folder, "workspace"), { recursive: true });
      write(path.join(folder, "directory"), directory);
      engine.sessions.push(sessionOf({ id: given.session, title: given.name, directory }));
    },
    wake(given) {
      const due = Math.floor(Date.now() / 1000) + given.in;
      write(path.join(ownerFolder(given.session), "wakes", `${due}-${given.letters}${given.every ? `-${given.every}` : ""}`), given.note);
    },
    watch(given) {
      const folder = path.join(ownerFolder(given.session), "watches", given.letters);
      write(path.join(folder, "check"), given.check);
      write(path.join(folder, "every"), given.every);
      write(path.join(folder, "note"), given.note ?? "");
    },
    pause(given) {
      write(path.join(ownerFolder(given.session), "paused"), "paused");
    },
    skill(given) {
      const folder = path.join(vars.skills, given.name);
      write(path.join(folder, "SKILL.md"), given.text);
      if (given.github) write(path.join(folder, ".kuvo-source.json"), JSON.stringify({ branch: "main", path: given.github.path, repository: given.github.repository }));
    },
    // One in Recently Deleted, put away `daysAgo` days ago, named by when, as the box keeps them.
    trashedSkill(given) {
      const when = Date.now() - given.daysAgo * 86_400_000;
      const folder = path.join(home, "trash", "skills", `${given.name}-${Math.floor(when / 1000)}`);
      write(path.join(folder, "SKILL.md"), given.text);
    },
    settings(given) {
      write(path.join(home, "box", "opencode.json"), given.text ?? JSON.stringify(given.value));
    },
    key(given) {
      write(path.join(home, "box", "keys", given.name), given.value);
    },
    credential(given) {
      engine.credentials.push({ id: given.id, integrationID: given.integrationID, label: given.label ?? "", active: given.active ?? false, value: { type: "api" } });
    },
    file(given, vars) {
      const file = path.join(root, "work", given.name);
      write(file, given.text);
      vars[`${given.as}.path`] = file;
      vars[`${given.as}.uri`] = pathToFileURL(file).href;
    },
    repository(given, vars) {
      const folder = path.join(root, "work", given.as);
      fs.mkdirSync(folder, { recursive: true });
      git(env, folder, "init", "-q", "-b", "main");
      for (const [name, text] of Object.entries(given.files ?? { "README.md": "# Project\n" })) write(path.join(folder, name), text);
      git(env, folder, "add", "-A");
      git(env, folder, "commit", "-q", "-m", "First");
      vars[given.as] = folder;
    },
    write(given) {
      const relative = path.relative(root, given.path);
      if (!relative || relative === ".." || relative.startsWith(".." + path.sep) || path.isAbsolute(relative)) {
        throw new Error(`Fixture write escapes its scratch folder: ${given.path}`);
      }
      write(given.path, given.text);
    },
    // The engine's models, as `api/model` lists them.
    models(given) {
      engine.models = given;
    },
    // A session's messages, oldest first, as its conversation has them (read newest first).
    messages(given) {
      engine.orderedMessages = true;
      engine.messages.set(given.session, given.rows);
    },
    // A command a session left running in its folder, as the engine lists it.
    shell(given) {
      engine.shells = [...(engine.shells ?? []), { id: given.id, status: "running", command: given.command ?? "npm test", folder: given.folder, metadata: { sessionID: given.session }, time: { started: Date.now() } }];
      engine.emit({ type: "shell.created", location: { directory: given.folder }, data: { id: given.id } });
    },
    // That command ended.
    shellEnded(given) {
      const job = (engine.shells ?? []).find(each => each.id === given.id);
      engine.shells = (engine.shells ?? []).filter(each => each.id !== given.id);
      engine.emit({ type: "shell.exited", location: { directory: job?.folder }, data: { id: given.id } });
    },
    // One of the engine's events, as its feed tells it.
    engineEvent(given) {
      engine.emit(given);
    },
    // What a conversation asks before it's done (`run/work/asks.json`), asked just now: making a
    // project unless it says otherwise.
    workAsk(given) {
      const file = path.join(home, "run", "work", "asks.json");
      const asks = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")).asks : [];
      const { id, session, name = "Your chat", project, action = "makeProject", clone, late, git } = given;
      const made = { name: project, ...(git === undefined ? {} : { git }), ...(clone ? { clone } : {}) };
      const ask = { id, action, session, name, project, ...(clone ? { clone } : {}), folderName: project.toLowerCase(), folder: null, at: Date.now(), given: made, ...(late ? { late } : {}), ...(given.answered ? { answered: { answer: "allow", text: given.answered, isError: false, at: Date.now() } } : {}) };
      write(file, JSON.stringify({ asks: [...asks, ask] }));
    },
    // What an agent browsing here asks you for, as the browser's keeper leaves it for the bridge
    // (`run/browser-asks.json`, while the keeper runs: a process named as it is stands in for it).
    ask(given) {
      if (!browserProcess) {
        browserProcess = launch(process.execPath, ["-e", "process.stdin.resume()", "kuvo-browser"], env, home);
        children.push(browserProcess);
      }
      write(path.join(home, "run", "browser.pid"), String(browserProcess.child.pid));
      const file = path.join(home, "run", "browser-asks.json");
      const asks = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")).asks : [];
      const { id, kind, host } = given;
      const details = kind === "code" ? { prompt: given.prompt ?? "" } : kind === "action" ? { instruction: given.instruction ?? "" } : { username: given.username ?? true, password: given.password ?? true };
      write(file, JSON.stringify({ asks: [...asks, { id, kind, host, ...details }] }));
    },
    async agentsBridge(given) {
      if (!binary.typescript) return;
      if (!given.groups) throw new Error("No stand-in for the legacy agents bridge fixture");
      const bundled = path.join(path.dirname(binary.file), "kuvo-agents.js");
      const child = launch(typescriptRuntime(), ["run", bundled], {
        ...env, BUN_BE_BUN: "1", BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
        KUVO_AGENTS_HOME: path.join(home, "agents"), OPENCODE_PASSWORD: "conformance-agents",
        KUVO_GROUPS: path.join(home, "groups"), KUVO_BOTS: botsRoot, KUVO_TRASH: path.join(home, "trash"),
        KUVO_ARTIFACTS: path.join(home, "artifacts"), KUVO_RUN: path.join(home, "run"),
      }, home, "pipe");
      children.push(child);
      const lines = createInterface({ input: child.child.stdout });
      try {
        for await (const line of lines) {
          if (!line.startsWith("{")) continue;
          const port = Number(new URL(JSON.parse(line).url).port);
          write(path.join(home, "run", "bridge.json"), JSON.stringify({
            formatVersion: 1, pid: child.child.pid, port, password: "conformance-agents",
          }));
          return;
        }
        throw new Error(`Agents bridge did not publish its endpoint\n${child.log()}`);
      } finally {
        lines.close();
      }
    },
    peers(given) {
      // Seed what a resident keeps after a peer announces its agents. The running
      // box reads the directory and shares it with its workers on startup.
      const keyFor = key => createHash("sha256").update(key).digest("base64url");
      const macPeers = given.boxes.map(box => ({
        key: keyFor(box.key), sign: keyFor(box.key + ":sign"), name: box.name, kind: "machine",
        address: createHash("sha256").update(box.key).digest().subarray(0, 16).toString("base64url"),
      }));
      const known = Object.fromEntries(given.boxes.map(box => [keyFor(box.key), {
        heard: Date.now(), features: box.seats ? ["groups.seats"] : [],
        list: given.bots.filter(bot => bot.box === box.key).map(bot => ({
          kind: "bot", id: bot.id, name: bot.name, session: bot.session ?? "", about: bot.job, look: bot.look,
        })),
      }]));
      write(path.join(home, "run", "link.json"), JSON.stringify({ phones: [], macPeers }));
      write(path.join(home, "run", "peer-agents.json"), JSON.stringify(known));
    },
  };

  // Where a session's wake-ups are kept: its bot's folder, or its own (made as kuvo-wake makes one).
  function ownerFolder(session) {
    for (const id of fs.existsSync(botsRoot) ? fs.readdirSync(botsRoot) : []) {
      if (fs.existsSync(path.join(botFolder(id), "session")) && fs.readFileSync(path.join(botFolder(id), "session"), "utf8").trim() === session) return botFolder(id);
    }
    const folder = path.join(wakesRoot, session);
    if (!fs.existsSync(path.join(folder, "session"))) {
      write(path.join(folder, "session"), session);
      write(path.join(folder, "directory"), engine.sessions.find(each => each.id === session)?.location.directory ?? "");
    }
    return folder;
  }

  return { fixtures, vars };
}
