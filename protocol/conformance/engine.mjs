// Minimal OpenCode HTTP stand-in shared by the route scenarios.
import { execFileSync } from "node:child_process";
import crypto from "node:crypto";
import http from "node:http";
import path from "node:path";

export const git = (env, cwd, ...args) => execFileSync("git", args, { cwd, env, encoding: "utf8" }).trim();

export async function standInEngine(root, env) {
  // Its sessions (`given.session`), which run, its sign-ins, and every request it got.
  const engine = { sessions: [], models: [], providers: [], integrations: [], defaultModel: null, messages: new Map(), permissions: new Map(), forms: new Map(), inboxes: new Map(), interrupted: [], running: new Set(), credentials: [], asked: [], made: 0, worktrees: 0, projects: new Map() };
  engine.open = new Set();
  engine.closed = [];
  engine.terminals = new Set();
  engine.globalForms = new Map();
  engine.dismissedGlobalForms = [];
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
      const paged = (rows, size) => {
        const start = Number(url.searchParams.get("cursor") ?? 0);
        const take = Number(url.searchParams.get("limit") ?? size);
        const next = start + take;
        return { data: rows.slice(start, next), cursor: next < rows.length ? { next: String(next) } : null };
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
        if (engine.sessionListUnavailable) return send({ message: "The list is unavailable." }, 503);
        const parent = url.searchParams.get("parentID");
        if (parent === null && url.searchParams.get("limit") !== "1") engine.inventoryReads = (engine.inventoryReads ?? 0) + 1;
        const rows = engine.sessions.filter(each => parent === null || (parent === "null" ? !each.parentID : each.parentID === parent));
        const descendants = parent && parent !== "null" && Number(url.searchParams.get("limit")) > 0;
        const page = engine.paged || descendants ? paged(rows, 100) : { data: rows };
        if (engine.holdSessionList && url.searchParams.get("limit") !== "1") {
          if (!parent) engine.heldInventories = (engine.heldInventories ?? 0) + 1;
          void engine.holdSessionList.then(() => send(page));
          return;
        }
        return send(page);
      }
      if (route === "GET /api/session/active") {
        if (engine.activeUnavailable) return send({}, 503);
        const data = Object.fromEntries([...engine.running].map(id => [id, { type: "busy" }]));
        engine.activeRead?.();
        return send({ data });
      }
      if (route === "POST /api/session") {
        if (engine.failStart === "before") {
          engine.startCutShort?.();
          return send({ message: "Cut short." }, 500);
        }
        const made = sessionOf({ id: `ses_made${++engine.made}`, title: given.title ?? "", directory: given.location?.directory ?? "" });
        for (const field of ["metadata", "permissions", "agent", "model", "parentID"]) {
          if (Object.hasOwn(given, field)) made[field] = given[field];
        }
        engine.sessions.push(made);
        publish({ type: "session.created", data: { sessionID: made.id, info: made } });
        if (engine.failStart === "after") {
          engine.startCutShort?.();
          return send({ message: "Cut short." }, 500);
        }
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
        if (!id || engine.sessions.some(each => each.id === id)) return send({ message: "Session already exists." }, 409);
        const info = { ...given.info, location: given.location };
        engine.sessions.push(info);
        engine.messages.set(id, given.messages);
        publish({ type: "session.created", data: { sessionID: id, info } });
        if (engine.holdImport?.id === id) {
          const held = engine.holdImport;
          engine.holdImport = null;
          held.accepted();
          return;
        }
        return send({ data: info });
      }
      const instructions = url.pathname.match(/^\/api\/experimental\/session\/([^/]+)\/instructions\/entries\/kuvo\.worktree$/);
      if (instructions && request.method === "PUT") {
        return engine.sessions.some(row => row.id === decodeURIComponent(instructions[1])) ? send({}) : send({}, 404);
      }
      if (one && request.method === "GET") {
        if (engine.unavailableSession === decodeURIComponent(one[1])) return send({ message: "Busy." }, 500);
        const index = engine.sessions.findIndex(each => each.id === decodeURIComponent(one[1]));
        const found = engine.sessions[index];
        if (found && engine.delistOnRead === found.id) {
          engine.delistOnRead = null;
          engine.sessions.splice(index, 1);
        }
        if (found && engine.holdSessionRead?.when(found.id)) {
          const held = engine.holdSessionRead;
          engine.holdSessionRead = null;
          held.accepted();
          void held.release.then(() => send({ data: found }));
          return;
        }
        return found ? send({ data: found }) : send({ message: "No such session." }, 404);
      }
      // What's changed, kept, as the engine keeps it (each field given in place of its own).
      if (one && request.method === "PATCH") {
        const found = engine.sessions.find(each => each.id === decodeURIComponent(one[1]));
        if (!found) return send({ message: "No such session." }, 404);
        const apply = () => {
          Object.assign(found, given);
          publish({ type: "session.updated", data: { sessionID: found.id, info: found } });
          return send({ data: found });
        };
        if (engine.holdSessionPatch?.id === found.id) {
          const held = engine.holdSessionPatch;
          engine.holdSessionPatch = null;
          held.accepted();
          void held.release.then(() => held.status ? send({ message: "The write failed." }, held.status) : apply());
          return;
        }
        return apply();
      }
      if (one && request.method === "DELETE") {
        const id = decodeURIComponent(one[1]);
        if (engine.refuseDelete === id) return send({ message: "The engine cannot remove this copy yet." }, 503);
        const at = engine.sessions.findIndex(each => each.id === id);
        if (at < 0) return send({ message: "No such session." }, 404);
        const [removed] = engine.sessions.splice(at, 1);
        publish({ type: "session.deleted", data: { sessionID: removed.id } });
        return send({ data: true });
      }
      const ask = url.pathname.match(/^\/api\/session\/([^/]+)\/(permission|form)(?:\/([^/]+)(?:\/reply)?)?$/);
      if (ask) {
        const [, id, type, key] = ask;
        const global = id === "global" && type === "form";
        const items = global ? engine.globalForms : type === "permission" ? engine.permissions : engine.forms;
        const location = global ? folderOf(request) : id;
        if (request.method === "GET") {
          if (type === "permission" && url.searchParams.get("live") === "1") engine.beforeLivePermission?.();
          return send({ data: items.get(location) ?? [] });
        }
        const pending = items.get(location) ?? [];
        const index = pending.findIndex(item => item.id === decodeURIComponent(key));
        if (index < 0) return send({}, 404);
        if (engine.refusedAsk === decodeURIComponent(key)) return send({ message: "It can't take that now." }, 400);
        const [answered] = pending.splice(index, 1);
        if (global && request.method === "DELETE") engine.dismissedGlobalForms.push({ id: answered.id, folder: location });
        publish({ type: type + (request.method === "DELETE" ? ".cancelled" : ".replied"),
          data: { sessionID: id, requestID: answered.id } });
        return send({});
      }
      const prompt = url.pathname.match(/^\/api\/session\/([^/]+)\/prompt$/);
      if (prompt && request.method === "POST") {
        const session = decodeURIComponent(prompt[1]);
        if (!engine.sessions.some(row => row.id === session)) return send({}, 404);
        if (engine.refusePrompt?.id === given.id) {
          const refusal = engine.refusePrompt;
          refusal.seen?.();
          engine.refusePrompt = null;
          return send(refusal.body ?? { message: "Try the prompt again." }, refusal.status ?? 503);
        }
        if (engine.unconfirmedPrompt === given.id) return send({});
        const held = engine.holdPrompt?.id === given.id ? engine.holdPrompt : null;
        if (held) engine.holdPrompt = null;
        const accept = () => {
          const messages = engine.messages.get(session) ?? [];
          const inbox = engine.inboxes.get(session) ?? [];
          if (messages.some(row => row.id === given.id) || inbox.some(row => row.id === given.id)) return;
          if (engine.running.has(session)) {
            const { id, delivery, ...payload } = given;
            inbox.push({ id, delivery, type: "user", payload });
            engine.inboxes.set(session, inbox);
          } else {
            messages.push({ ...given, type: "user" });
            engine.messages.set(session, messages);
          }
          publish({ type: "session.inbox.updated", data: { sessionID: session, item: { id: given.id } } });
        };
        if (held?.beforeAcceptance) {
          held.accepted();
          if (held.release) void held.release.then(() => {
            if (!held.status) accept();
            send({}, held.status);
          });
          return;
        }
        accept();
        if (held) {
          held.accepted();
          if (held.release) void held.release.then(() => send({}, held.status));
          return;
        }
        return send({});
      }
      const waiting = url.pathname.match(/^\/api\/session\/([^/]+)\/inbox(?:\/([^/]+))?$/);
      if (waiting) {
        const session = decodeURIComponent(waiting[1]);
        const inbox = engine.inboxes.get(session) ?? [];
        if (!waiting[2] && request.method === "GET") {
          if (engine.inboxUnavailable?.has(session)) return send({}, 503);
          const reply = engine.inboxReplies?.get(session);
          if (reply) return send(reply.body, reply.status);
          if (engine.holdInbox?.session === session) {
            const held = engine.holdInbox;
            engine.holdInbox = null;
            const read = structuredClone(inbox);
            held.accepted();
            void held.release.then(() => send({ data: read }));
            return;
          }
          return send({ data: inbox });
        }
        const index = inbox.findIndex(row => row.id === decodeURIComponent(waiting[2]));
        if (index < 0) return send({}, 404);
        if (request.method === "PATCH") {
          Object.assign(inbox[index], given);
          engine.afterInboxPatch?.(session, inbox[index]);
        }
        else if (request.method === "DELETE") {
          if (engine.refuseRemoval?.has(waiting[2])) return send({}, 503);
          inbox.splice(index, 1);
          engine.afterInboxRemoval?.(session, decodeURIComponent(waiting[2]));
        }
        else return send({}, 404);
        publish({ type: "session.inbox.updated", data: { sessionID: session } });
        return send({});
      }
      const message = url.pathname.match(/^\/api\/session\/([^/]+)\/message\/([^/]+)$/);
      if (message && request.method === "GET") {
        if (engine.messageUnavailable?.has(decodeURIComponent(message[2]))) return send({}, 503);
        const stored = engine.messages.get(decodeURIComponent(message[1]))?.find(row => row.id === decodeURIComponent(message[2]));
        return stored ? send({ data: stored }) : send({}, 404);
      }
      const interrupt = url.pathname.match(/^\/api\/session\/([^/]+)\/interrupt$/);
      if (interrupt && request.method === "POST") {
        engine.interrupted.push({ session: decodeURIComponent(interrupt[1]), resume: url.searchParams.get("resume") === "true" });
        return send({});
      }
      const view = url.pathname.match(/^\/api\/session\/([^/]+)\/view$/);
      if (view && request.method === "POST") {
        const found = engine.sessions.find(row => row.id === decodeURIComponent(view[1]));
        if (!found || found.time?.idle !== given.idle) return send({}, 409);
        found.time.viewed = given.idle;
        return send({ data: found });
      }
      if (["GET /api/model", "GET /api/model/default", "GET /api/provider", "GET /api/integration"].includes(route)) {
        const catalog = engine.catalogs?.get(path.normalize(folderOf(request))) ?? engine;
        if (route === "GET /api/model") return send({ data: catalog.models });
        if (route === "GET /api/model/default") {
          if (catalog.defaultUnavailable) return send({ message: "The default is unavailable." }, 503);
          return send({ data: catalog.defaultModel });
        }
        if (route === "GET /api/provider") return send({ data: catalog.providers });
        return send({ data: catalog.integrations ?? [] });
      }
      if (request.method === "GET" && url.pathname.startsWith("/api/integration/")) {
        const account = engine.integrations.find(each => each.id === decodeURIComponent(url.pathname.slice("/api/integration/".length)));
        return account ? send({ data: account }) : send({}, 404);
      }
      if (route === "GET /api/debug/location") {
        engine.locationsRead?.();
        return send([...engine.open].map(directory => ({ directory })));
      }
      if (route === "DELETE /api/debug/location") {
        const folder = url.searchParams.get("location[directory]");
        if (engine.cantClose) return send({}, 405);
        engine.open.delete(folder);
        engine.closed.push(folder);
        engine.locationClosed?.(folder);
        response.writeHead(204).end();
        return;
      }
      if (route === "GET /api/pty") return send({ data: engine.terminals.has(folderOf(request)) ? [{ id: "pty_here", status: "running" }] : [] });
      if (["GET /api/form", "GET /api/permission/request", "GET /api/shell"].includes(route)) return send({ data: [] });
      const messages = url.pathname.match(/^\/api\/session\/([^/]+)\/message$/);
      if (messages && request.method === "GET") {
        const rows = engine.messages.get(decodeURIComponent(messages[1])) ?? [];
        engine.messagePageRead?.(decodeURIComponent(messages[1]), url.searchParams);
        return send(engine.paged ? paged(rows, 200) : { data: rows });
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
