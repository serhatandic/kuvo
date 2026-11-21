import "./test-safety.js";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { removeAtEnd } from "./at-end.js";
import { NO_BUN, testBun } from "./bun-for-tests.js";

const checkout = fileURLToPath(new URL("../..", import.meta.url));
const linkFile = "swift/KuvoLink/Sources/KuvoLink/Resources/kuvo-link.js";
const built = fs.readFileSync(path.join(checkout, linkFile), "utf8");
const { UPDATE_KEYS } = await import(path.join(checkout, linkFile));
const bun = testBun();
const findOld = ref => spawnSync("git", ["show", `${ref}:${linkFile}`], {
  cwd: checkout, encoding: "utf8", maxBuffer: 16_000_000,
});
const history = [
  ["0.8.0-beta.35", findOld("fccda2de1")],
  ["0.8.0-beta.38", findOld("e85f7d504")],
];
const sha = data => crypto.createHash("sha256").update(data).digest("hex");

function write(file, bytes, mode = 0o600) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, bytes, { mode });
}

function pack(release, version, pinned, privateKey, released = "2026-11-21T12:00:00Z") {
  const files = fs.mkdtempSync(path.join(os.tmpdir(), "unlock-pack-"));
  removeAtEnd(files);
  const engine = "1.0.0";
  const contents = {
    "bin/kuvo-link.js": built.replace(UPDATE_KEYS[0], pinned) + `\n// Fixture release ${released}\n`,
    "bin/box-version": version + "\n",
    // The engine and keeper stand in for the ones this update keeps on the same version.
    "bin/kuvo-start": "#!/bin/sh\nexit 0\n",
  };
  for (const [name, bytes] of Object.entries(contents)) write(path.join(files, name), bytes, 0o700);
  write(path.join(files, "box.json"), JSON.stringify({ version, engine,
    files: Object.fromEntries(Object.entries(contents).map(([name, bytes]) => [name, sha(bytes)])) }));
  const file = `box-${version}.tar.gz`;
  fs.mkdirSync(release, { recursive: true });
  const made = spawnSync("tar", ["czf", path.join(release, file), "-C", files, "."], {
    env: { ...process.env, COPYFILE_DISABLE: "1" },
  });
  assert.equal(made.status, 0, made.stderr.toString());
  const bytes = fs.readFileSync(path.join(release, file));
  const layout = Number(/^\/\/ Kuvo folder layout (\d+)/.exec(built)[1]);
  const manifest = Buffer.from(JSON.stringify({ kuvo: "box/1", version, engine, layout, released, spreads: true,
    pack: { url: `https://example.invalid/${file}`, sha256: sha(bytes), size: bytes.length },
  }));
  const signature = crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), privateKey);
  fs.writeFileSync(path.join(release, "boxes.json"), JSON.stringify({ manifest: manifest.toString("base64"), signature: signature.toString("base64"), key: pinned }));
  fs.copyFileSync(path.join(checkout, "box/src/install/kuvo-install"), path.join(release, "kuvo-install"));
}

for (const [before, old] of history) {
  for (const kind of ["mac", "machine"]) {
    test(`the carried installer upgrades a ${kind} from ${before}`, {
      skip: !bun ? NO_BUN : old.status !== 0 ? "the historical bundle is absent" : kind === "mac" && process.platform !== "darwin",
    }, () => {
      const account = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "unlock-home-")));
      removeAtEnd(account);
      const home = path.join(account, ".kuvo-code");
      const run = path.join(home, "run");
      const app = path.join(account, "Kuvo Test.app");
      const version = "0.8.0-beta.39";
      const release = kind === "mac" ? path.join(app, "Contents/Resources/box-release") : path.join(account, "release");
      const { privateKey, publicKey } = crypto.generateKeyPairSync("ed25519");
      const pinned = publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64");
      pack(release, version, pinned, privateKey);
      write(path.join(app, "Contents/Info.plist"), '<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>run.kuvo.test.unlock</string></dict></plist>');
      const commands = path.join(account, "commands");
      if (kind === "machine" && process.platform === "darwin") {
        // Run the Linux branch of the POSIX installer locally, including its no-app invocation.
        write(path.join(commands, "uname"), '#!/bin/sh\n[ "$1" != "-s" ] || { echo Linux; exit; }\nexec /usr/bin/uname "$@"\n', 0o700);
      }
      const env = { HOME: account, PATH: `${commands}:/usr/bin:/bin:/usr/sbin:/sbin` };
      const engine = path.join(home, "engine/1.0.0/opencode");
      write(engine, `#!/bin/sh\n[ "$1" != --version ] || { echo 1.0.0; exit; }\nexec "${bun}" "$@"\n`, 0o700);
      const oldLink = path.join(home, "bin/kuvo-link.js");
      write(oldLink, old.stdout);
      write(path.join(home, "bin/box-version"), before + "\n");
      write(path.join(home, "bin/kuvo-start"), "#!/bin/sh\nexit 0\n", 0o700);
      write(path.join(home, "box/kind"), kind);
      write(path.join(home, "data/opencode/opencode.db"), "existing conversations");
      const layout = Number(/^\/\/ Kuvo folder layout (\d+)/.exec(old.stdout)[1]);
      write(path.join(home, "folder-version.json"), JSON.stringify({ version: layout }));
      write(path.join(run, "engine.json"), JSON.stringify({ version: "1.0.0" }));
      const id = "set_" + "a".repeat(22);
      const primary = crypto.randomBytes(32).toString("base64url");
      const roster = { setup: { id, epoch: 1, primary, role: kind === "mac" ? "member" : "primary", checkpointed: true, managed: true },
        sharedSetup: id, shared: { setupSecurityAndReleaseChoice: { sequence: 1, value: {
          version: before, sha256: "a".repeat(64), signed: "old", layout,
        } } },
      };
      write(path.join(run, "link.json"), JSON.stringify(roster));
      const resultOf = said => JSON.parse(said.stdout.trim().split("\n").at(-1));
      if (before.endsWith(".38")) {
        const underOld = spawnSync(bun, [oldLink, "update", "--installer", "--now", "--from", release, "--key", pinned], {
          env: { ...env, KUVO_HOME: home, BUN_BE_BUN: "1" }, encoding: "utf8", timeout: 20_000,
        });
        const refused = resultOf(underOld);
        assert.ok(["waiting", "blocked"].includes(refused.result), JSON.stringify(refused));
        assert.equal(fs.readFileSync(path.join(home, "bin/box-version"), "utf8").trim(), before);
      }

      const install = () => spawnSync("sh", [path.join(release, "kuvo-install"), "update", ...(kind === "mac" ? ["--app", app] : ["--now"])], {
        env, encoding: "utf8", timeout: 20_000,
      });
      const installed = install();
      assert.equal(installed.status, 0, installed.stdout + installed.stderr);
      assert.deepEqual(resultOf(installed), { result: "updated", version });
      assert.equal(fs.readFileSync(path.join(home, "bin/box-version"), "utf8").trim(), version);
      assert.equal(fs.readFileSync(path.join(home, "data/opencode/opencode.db"), "utf8"), "existing conversations");
      assert.deepEqual(JSON.parse(fs.readFileSync(path.join(run, "link.json"), "utf8")), roster);
      if (before.endsWith(".38") && kind === "mac") {
        const installedBytes = fs.readFileSync(oldLink);
        fs.rmSync(path.join(run, "update-trial"), { force: true });
        pack(release, version, pinned, privateKey, "2026-11-21T10:00:00Z");
        const olderBuild = install();
        assert.equal(olderBuild.status, 0, olderBuild.stdout + olderBuild.stderr);
        assert.deepEqual(resultOf(olderBuild), { result: "current", version });
        assert.deepEqual(fs.readFileSync(oldLink), installedBytes);
      }
    });
  }
}
