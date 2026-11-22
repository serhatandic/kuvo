import "./safety";
import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { asked } from "../src/link/engine/client";
import { encodeBrowserFrame, readBrowserFrame } from "../src/link/browser/media";
import { checkLegacyImport } from "../src/link/rehome/upgrades";
import { readVersionedFile } from "../src/shared/versioned-file";
import { hostRecordFormat } from "../src/link/host-interface";

test("direct bridge requests carry the Box route version and refuse a future receipt", async () => {
  const versions: (string | null)[] = [];
  let reply: string | undefined = undefined;
  const server = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    fetch(request) {
      versions.push(request.headers.get("x-kuvo-route-version"));
      return Response.json({ applied: true }, { headers: reply ? { "x-kuvo-route-version": reply } : {} });
    },
  });
  try {
    const target = { port: server.port!, password: "fixture" };
    expect(await asked(target, "GET", "api/session")).toEqual({ applied: true });
    reply = "1";
    expect(await asked(target, "GET", "api/session")).toEqual({ applied: true });
    reply = "2";
    await expect(asked(target, "GET", "api/session")).rejects.toThrow("Kuvo route format version 2 is unsupported");
    expect(versions).toEqual(["1", "1", "1"]);
  } finally { await server.stop(true); }
});

test("media framing reads preceding and current headers and holds future or cut bytes", () => {
  const make = (formatVersion?: number) => {
    const header = Buffer.from(JSON.stringify({ method: "Kuvo.frame", params: { frameId: 11 }, formatVersion }));
    const size = Buffer.alloc(4);
    size.writeUInt32BE(header.length);
    return Buffer.concat([size, header, Buffer.from("jpeg")]);
  };
  expect(readBrowserFrame(make())).toEqual({ frameId: 11 });
  const current = encodeBrowserFrame({ frameId: 11 }, Buffer.from("jpeg"));
  expect(current.equals(make(1))).toBe(true);
  expect(readBrowserFrame(current)).toEqual({ frameId: 11 });
  expect(() => readBrowserFrame(make(2))).toThrow("Browser media header format version 2 is unsupported");
  expect(() => readBrowserFrame(current.subarray(0, 5))).toThrow("Invalid browser media header length");
});

test("retired import lists and host rendezvous hold unknown bytes", () => {
  const home = mkdtempSync(join(tmpdir(), "kuvo-authority-"));
  try {
    const list = join(home, "bots.json");
    const host = join(home, "host.json");
    for (const marker of [[], [{ formatVersion: 1 }]]) {
      writeFileSync(list, JSON.stringify([...marker, { id: "old-bot" }]));
      expect(() => checkLegacyImport(home)).not.toThrow();
    }
    const kept = '[{"formatVersion":2},{"id":"old-bot"}]';
    writeFileSync(list, kept);
    expect(() => checkLegacyImport(home)).toThrow("Legacy Mac import list format version 2 is unsupported");
    expect(readFileSync(list, "utf8")).toBe(kept);
    for (const formatVersion of [undefined, 1]) {
      writeFileSync(host, JSON.stringify({ url: "http://127.0.0.1:1", password: "test", formatVersion }));
      expect(readVersionedFile(host, hostRecordFormat) as unknown).toEqual({ url: "http://127.0.0.1:1", password: "test" });
    }
    writeFileSync(host, '{"formatVersion":2,"url":"http://127.0.0.1:1","password":"test"}');
    expect(() => readVersionedFile(host, hostRecordFormat)).toThrow("Mac host rendezvous format version 2 is unsupported");
  } finally { rmSync(home, { recursive: true, force: true }); }
});
