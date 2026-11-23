// Minimal OpenCode HTTP stand-in shared by the route scenarios.
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import http from "node:http";
import path from "node:path";

export const git = (env, cwd, ...args) => execFileSync("git", args, { cwd, env, encoding: "utf8" }).trim();

export async function standInEngine(root, env) {
  // Its sessions (`given.session`), which run, its sign-ins, and every request it got.
  const engine = { sessions: [], models: [], messages: new Map(), inboxes: new Map(), interrupted: [], running: new Set(), credentials: [], asked: [], made: 0, worktrees: 0, projects: new Map() };
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
        return send({ data: engine.sessions.filter(each => parent === null || (parent === "null" ? !each.parentID : each.parentID === parent)) });
      }
      if (route === "GET /api/session/active") return send({ data: Object.fromEntries([...engine.running].map(id => [id, { type: "busy" }])) });
      if (route === "POST /api/session") {
        const made = sessionOf({ id: `ses_made${++engine.made}`, title: given.title ?? "", directory: given.location?.directory ?? "" });
        for (const field of ["metadata", "permissions", "agent", "model", "parentID"]) {
          if (Object.hasOwn(given, field)) made[field] = given[field];
        }
        engine.sessions.push(made);
        publish({ type: "session.created", data: { sessionID: made.id, info: made } });
        return send({ data: made });
      }
      const one = url.pathname.match(/^\/api\/session\/([^/]+)$/);
      if (one && request.method === "GET") {
        const found = engine.sessions.find(each => each.id === decodeURIComponent(one[1]));
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
        const at = engine.sessions.findIndex(each => each.id === decodeURIComponent(one[1]));
        if (at < 0) return send({ message: "No such session." }, 404);
        const [removed] = engine.sessions.splice(at, 1);
        publish({ type: "session.deleted", data: { sessionID: removed.id } });
        return send({ data: true });
      }
      const prompt = url.pathname.match(/^\/api\/session\/([^/]+)\/prompt$/);
      if (prompt && request.method === "POST") {
        const session = decodeURIComponent(prompt[1]);
        if (!engine.sessions.some(row => row.id === session)) return send({}, 404);
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
          }
          publish({ type: "session.inbox.updated", data: { sessionID: session, item: { id: given.id } } });
        }
        if (engine.holdPrompt && engine.holdPrompt.id === given.id) {
          const held = engine.holdPrompt;
          engine.holdPrompt = null;
          held.accepted();
          return;
        }
        return send({});
      }
      const waiting = url.pathname.match(/^\/api\/session\/([^/]+)\/inbox(?:\/([^/]+))?$/);
      if (waiting) {
        const session = decodeURIComponent(waiting[1]);
        const inbox = engine.inboxes.get(session) ?? [];
        if (!waiting[2] && request.method === "GET") return send({ data: inbox });
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
        return stored ? send({ data: stored }) : send({}, 404);
      }
      const interrupt = url.pathname.match(/^\/api\/session\/([^/]+)\/interrupt$/);
      if (interrupt && request.method === "POST") {
        engine.interrupted.push({ session: decodeURIComponent(interrupt[1]), resume: url.searchParams.get("resume") === "true" });
        return send({});
      }
      if (route === "GET /api/model") return send({ data: engine.models });
      const messages = url.pathname.match(/^\/api\/session\/([^/]+)\/message$/);
      if (messages && request.method === "GET") return send({ data: engine.messages.get(decodeURIComponent(messages[1])) ?? [] });
      if (route === "GET /api/credential") return send({ data: engine.credentials });
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
