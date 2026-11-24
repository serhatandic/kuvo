// Minimal OpenCode HTTP stand-in shared by the route scenarios.
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import http from "node:http";
import path from "node:path";

export const git = (env, cwd, ...args) => execFileSync("git", args, { cwd, env, encoding: "utf8" }).trim();

export async function standInEngine(root, env) {
  // Its sessions (`given.session`), which run, its sign-ins, and every request it got.
  const engine = { sessions: [], models: [], providers: [], integrations: [], defaultModel: null, messages: new Map(), permissions: new Map(), forms: new Map(), inboxes: new Map(), interrupted: [], running: new Set(), credentials: [], asked: [], made: 0, worktrees: 0, projects: new Map(), refuseDeletes: new Set() };
  const folderOf = request => {
    try { return decodeURIComponent(request.headers["x-opencode-directory"] ?? ""); } catch { return ""; }
  };
  const streams = new Set();
  const publish = value => {
    for (const stream of streams) stream.write(`data: ${JSON.stringify(value)}\n\n`);
  };
  const server = http.createServer((request, response) => {
    let body = "";
    request.on("data", chunk => { body += chunk; });
    request.on("end", () => {
      const url = new URL(request.url, "http://engine");
      let given = {};
      try { given = body ? JSON.parse(body) : {}; } catch {}
      engine.asked.push({ method: request.method, path: url.pathname, json: given });
      const send = (value, status = 200) => { response.writeHead(status, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
      const queueRead = (body, status = 200) => {
        const hold = engine.holdQueueRead;
        if (hold?.path !== url.pathname) return send(body, status);
        engine.holdQueueRead = null;
        hold.entered();
        hold.finish.then(() => send(body, status));
      };
      const route = `${request.method} ${url.pathname}`;
      if (route === "GET /api/event") {
        response.writeHead(200, { "content-type": "text/event-stream" });
        response.write('data: {"type":"server.connected"}\n\n');
        streams.add(response);
        response.once("close", () => streams.delete(response));
        return;
      }
      if (route === "GET /api/session") {
        const parent = url.searchParams.get("parentID");
        if (engine.refuseWholeList && (parent === null || parent === "null")) {
          return send({ message: "The full session list is temporarily unavailable." }, 503);
        }
        const data = engine.sessions.filter(each => parent === null || (parent === "null" ? !each.parentID : each.parentID === parent));
        const limit = Number(url.searchParams.get("limit"));
        if (parent && parent !== "null" && limit > 0) {
          const start = Number(url.searchParams.get("cursor"));
          const end = start + limit;
          return send({ data: data.slice(start, end), ...(end < data.length ? { cursor: { next: String(end) } } : {}) });
        }
        if (engine.holdWholeList && (parent === null || parent === "null")) {
          const gate = engine.holdWholeList;
          gate.entered();
          gate.finish.then(() => send({ data }));
          return;
        }
        return send({ data });
      }
      if (route === "GET /api/session/active") {
        if (engine.refuseActive) return send({ message: "The engine is unavailable." }, 503);
        return send({ data: Object.fromEntries([...engine.running].map(id => [id, { type: "busy" }])) });
      }
      if (route === "POST /api/session") {
        if (engine.failStart === "before") return send({ message: "Cut short." }, 500);
        const made = sessionOf({ id: `ses_made${++engine.made}`, title: given.title ?? "", directory: given.location?.directory ?? "" });
        for (const field of ["metadata", "permissions", "agent", "model", "parentID"]) {
          if (Object.hasOwn(given, field)) made[field] = given[field];
        }
        engine.sessions.push(made);
        publish({ type: "session.created", data: { sessionID: made.id, info: made } });
        if (engine.failStart === "after") return send({ message: "Cut short." }, 500);
        return send({ data: made });
      }
      const one = url.pathname.match(/^\/api\/session\/([^/]+)$/);
      const moveExport = url.pathname.match(/^\/api\/experimental\/session\/([^/]+)\/export$/);
      if (moveExport && request.method === "GET") {
        const id = decodeURIComponent(moveExport[1]);
        if (engine.refuseExport === id) return send({ message: "The backup export is unavailable." }, 503);
        const info = engine.sessions.find(each => each.id === id);
        return info ? send({ data: { info, messages: engine.messages.get(id) ?? [] } }) : send({}, 404);
      }
      if (route === "POST /api/experimental/session/import") {
        const id = given.info?.id;
        engine.beforeImport?.(id);
        if (!id || engine.sessions.some(each => each.id === id)) return send({ message: "Session already exists." }, 409);
        const info = { ...given.info, location: given.location };
        engine.sessions.push(info);
        engine.messages.set(id, given.messages);
        publish({ type: "session.created", data: { sessionID: id, info } });
        if (engine.loseImportReply === id) {
          engine.loseImportReply = null;
          response.destroy();
          return;
        }
        if (engine.holdImport?.id === id) {
          const held = engine.holdImport;
          engine.holdImport = null;
          held.accepted();
          if (held.finish) held.finish.then(() => send({ data: info }));
          return;
        }
        return send({ data: info });
      }
      if (one && request.method === "GET") {
        const id = decodeURIComponent(one[1]);
        if (engine.refuseLookup === id) return send({ message: "The conversation is temporarily unavailable." }, 503);
        const found = engine.sessions.find(each => each.id === id);
        return found ? send({ data: found }) : send({ message: "No such session." }, 404);
      }
      // What's changed, kept, as the engine keeps it (each field given in place of its own).
      if (one && request.method === "PATCH") {
        const found = engine.sessions.find(each => each.id === decodeURIComponent(one[1]));
        if (!found) return send({ message: "No such session." }, 404);
        Object.assign(found, given);
        publish({ type: "session.updated", data: { sessionID: found.id, info: found } });
        return send({ data: found });
      }
      if (one && request.method === "DELETE") {
        const id = decodeURIComponent(one[1]);
        if (engine.refuseDeletes.has(id)) {
          return send({ message: "The engine cannot remove this copy yet." }, 503);
        }
        const at = engine.sessions.findIndex(each => each.id === id);
        if (at < 0) return send({ message: "No such session." }, 404);
        const [removed] = engine.sessions.splice(at, 1);
        publish({ type: "session.deleted", data: { sessionID: removed.id } });
        return send({ data: true });
      }
      const ask = url.pathname.match(/^\/api\/session\/([^/]+)\/(permission|form)(?:\/([^/]+)(?:\/reply)?)?$/);
      if (ask) {
        const [, id, type, key] = ask;
        const items = type === "permission" ? engine.permissions : engine.forms;
        if (request.method === "GET") return send({ data: items.get(id) ?? [] });
        const pending = items.get(id) ?? [];
        const index = pending.findIndex(item => item.id === decodeURIComponent(key));
        if (index < 0) return send({}, 404);
        const [answered] = pending.splice(index, 1);
        publish({ type: type + (request.method === "DELETE" ? ".cancelled" : ".replied"),
          data: { sessionID: id, requestID: answered.id } });
        return send({});
      }
      const prompt = url.pathname.match(/^\/api\/session\/([^/]+)\/prompt$/);
      if (prompt && request.method === "POST") {
        const session = decodeURIComponent(prompt[1]);
        if (!engine.sessions.some(row => row.id === session)) return send({}, 404);
        const hold = engine.holdPrompt;
        const held = hold && (hold.id ? hold.id === given.id : hold.session === session) ? hold : null;
        if (held) engine.holdPrompt = null;
        if (held?.beforeAcceptance) {
          held.accepted();
          return;
        }
        const messages = engine.messages.get(session) ?? [];
        const inbox = engine.inboxes.get(session) ?? [];
        if (!messages.some(row => row.id === given.id) && !inbox.some(row => row.id === given.id)) {
          if (engine.running.has(session)) {
            const { id, delivery, ...payload } = given;
            inbox.push({ id, delivery, type: "user", payload });
            engine.inboxes.set(session, inbox);
          } else {
            messages.push({ ...given, type: "user" });
            engine.messages.set(session, messages);
            if (engine.runAfterPrompt === given.id) engine.running.add(session);
          }
          publish({ type: "session.inbox.updated", data: { sessionID: session, item: { id: given.id } } });
        }
        if (held) {
          held.accepted();
          return;
        }
        return send({});
      }
      const waiting = url.pathname.match(/^\/api\/session\/([^/]+)\/inbox(?:\/([^/]+))?$/);
      if (waiting) {
        const session = decodeURIComponent(waiting[1]);
        const inbox = engine.inboxes.get(session) ?? [];
        if (!waiting[2] && request.method === "GET") {
          if (engine.refuseInbox === session) return send({ message: "The inbox is unavailable." }, 503);
          return queueRead({ data: inbox });
        }
        const index = inbox.findIndex(row => row.id === decodeURIComponent(waiting[2]));
        if (index < 0) return send({}, 404);
        if (request.method === "PATCH") Object.assign(inbox[index], given);
        else if (request.method === "DELETE") inbox.splice(index, 1);
        else return send({}, 404);
        publish({ type: "session.inbox.updated", data: { sessionID: session } });
        return send({});
      }
      const message = url.pathname.match(/^\/api\/session\/([^/]+)\/message\/([^/]+)$/);
      if (message && request.method === "GET") {
        const stored = engine.messages.get(decodeURIComponent(message[1]))?.find(row => row.id === decodeURIComponent(message[2]));
        return stored ? queueRead({ data: stored }) : queueRead({}, 404);
      }
      const interrupt = url.pathname.match(/^\/api\/session\/([^/]+)\/interrupt$/);
      if (interrupt && request.method === "POST") {
        engine.interrupted.push({ session: decodeURIComponent(interrupt[1]), resume: url.searchParams.get("resume") === "true" });
        return send({});
      }
      if (route === "GET /api/model") return send({ data: engine.models });
      if (route === "GET /api/model/default") return send({ data: engine.defaultModel });
      if (route === "GET /api/provider") return send({ data: engine.providers });
      if (route === "GET /api/integration") return send({ data: engine.integrations });
      const messages = url.pathname.match(/^\/api\/session\/([^/]+)\/message$/);
      if (messages && request.method === "GET") {
        const session = decodeURIComponent(messages[1]);
        const cursor = url.searchParams.get("cursor");
        if (engine.refuseMessagePage?.session === session && engine.refuseMessagePage.cursor === cursor) {
          engine.refuseMessagePage = null;
          return send({ message: "That page was temporarily unavailable." }, 503);
        }
        const pages = engine.messagePages?.get(session);
        const page = pages?.get(cursor);
        if (page) return send(page);
        const history = engine.messages.get(session) ?? [];
        if (url.searchParams.get("order") === "desc") {
          if (!engine.sessions.some(row => row.id === session)) return send({}, 404);
          return send({ data: history.toReversed().slice(0, Number(url.searchParams.get("limit")) || history.length) });
        }
        return send({ data: history });
      }
      if (route === "GET /api/credential") return send({ data: engine.credentials });
      if (route === "POST /api/credential") {
        if (engine.blockCredentialImport) return send({ message: "The credential store is not ready." }, 503);
        engine.credentials.push(given);
        return send({ data: given });
      }
      // Its tools connected (Kuvo Code's own browser's, no integration's), so nothing waits for them.
      if (route === "GET /api/mcp") return send({ data: [{ name: "kuvo-browser", status: { status: "connected" } }] });
      if (["GET /api/skill", "GET /api/provider"].includes(route)) return send({ data: [] });
      // Worktrees, made and removed with git as the engine does, beside the scenario's repositories:
      // a folder's project is its repository's, by an id of its own, and a worktree is made of the
      // project its id names.
      if (route === "GET /api/location") {
        const folder = folderOf(request);
        let top = null;
        try { top = git(env, folder, "rev-parse", "--show-toplevel"); } catch {}
        const id = "prj_" + crypto.createHash("sha256").update(top ?? folder).digest("hex").slice(0, 12);
        if (top) engine.projects.set(id, top);
        return send({ directory: folder, project: { id, directory: top ?? folder, canonical: top ?? folder } });
      }
      if (route === "POST /api/worktree") {
        const folder = path.join(root, "worktrees", `brave-otter-${++engine.worktrees}`);
        try { git(env, engine.projects.get(given.projectID), "worktree", "add", "--quiet", "--detach", folder); } catch { return send({ message: "Not a git project." }, 400); }
        return send({ directory: folder });
      }
      if (route === "DELETE /api/worktree") {
        try { git(env, engine.projects.get(given.projectID), "worktree", "remove", ...(given.force ? ["--force"] : []), given.directory); } catch { return send({ message: "git refused." }, 400); }
        return send({});
      }
      // What it doesn't have to read, as an engine without it; what's sent, taken.
      send({}, request.method === "GET" ? 404 : 200);
    });
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  return Object.assign(engine, {
    emit: publish,
    subscribers: () => streams.size,
    record: { port: server.address().port, password: "engine", version: "0.0.0", pid: process.pid },
    close: () => new Promise(resolve => {
      server.close(resolve);
      server.closeAllConnections();
    }),
  });
}

// A session as the engine lists it.
export function sessionOf({ id, title = "", directory = "", parent, archived, updated = 1 }) {
  return {
    id, title, projectID: "p", cost: 0, ...(parent ? { parentID: parent } : {}),
    tokens: { input: 0, output: 0, reasoning: 0, cache: { read: 0, write: 0 } },
    time: { created: updated, updated }, location: { directory },
    ...(archived ? { metadata: { "kuvo.archived": true } } : {}),
  };
}
