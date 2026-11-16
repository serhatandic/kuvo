import { test, expect } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { handOverPasswords } from "../src/link/rehome/passwords";

for (const scenario of ["ok", "read-fails", "different", "delete-fails"] as const) {
  test(`saved sign-ins: ${scenario}, retry never removes an unverified original`, async () => {
    const home = fs.mkdtempSync(path.join(os.tmpdir(), "kuvo-password-move-"));
    const keys = path.join(home, "keys");
    const original = path.join(home, "original");
    fs.writeFileSync(original, "secret-pass");
    fs.writeFileSync(keys, `#!${process.execPath}\nconst fs=require('fs'); const p=${JSON.stringify(original)}; if(process.argv[2]==='passwords') console.log(JSON.stringify({logins:fs.existsSync(p)?[{host:'example.org',account:'ada',password:fs.readFileSync(p,'utf8')}]:[]})); else {if(${JSON.stringify(scenario)}==='delete-fails') process.exit(1); fs.unlinkSync(p); console.log('{}');}`, { mode: 0o755 });
    let item: Record<string, unknown> | undefined;
    let made = 0;
    const server = Bun.serve({ port: 0, async fetch(request) {
      const route = new URL(request.url).pathname;
      if (route === "/kuvo/vault") return Response.json({ items: item ? [item] : [] });
      if (route === "/kuvo/vault/items") { made++; item = { id: "saved", kind: "login", name: "example.org", sites: ["example.org"], username: "ada" }; return Response.json({ item }); }
      if (scenario === "read-fails") return Response.json({}, { status: 503 });
      return Response.json({ values: { password: scenario === "different" ? "other-pass" : "secret-pass" } });
    } });
    fs.mkdirSync(path.join(home, "run"));
    fs.writeFileSync(path.join(home, "run", "serve.json"), JSON.stringify({ port: server.port, password: "local" }));
    try {
      const run = () => handOverPasswords({ home, keys, agentSites: [], agentAccounts: {}, wait: 0 });
      expect((await run()).done).toBe(scenario === "ok");
      expect(fs.existsSync(original)).toBe(scenario !== "ok");
      expect((await run()).done).toBe(scenario === "ok");
      expect(made).toBe(1);
      expect(fs.existsSync(original)).toBe(scenario !== "ok");
    } finally { server.stop(true); fs.rmSync(home, { recursive: true, force: true }); }
  });
}
