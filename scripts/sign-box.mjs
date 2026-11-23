// Signs a box release, for machines to update themselves with (protocol/PROTOCOL.md,
// "Updates"), and prints boxes.json: the manifest and its signature, in one file, so the two
// can't come from different releases.
//   KUVO_BOX_SIGNING_KEY=<PEM> node scripts/sign-box.mjs <version> <pack> <pack's URL> <box.json>
// For a Rust release, add one --target <triple> <pack> <URL> for each target.
// The first pack remains the compatibility pack read by TypeScript computers.
//   KUVO_BOX_SPREADS=1   a release for everyone (the release workflow) or the user's private feed:
//                        marked `spreads`, so a computer holding it hands it to the user's other
//                        computers. Without it (a build made for trying an update), no computer
//                        hands it on or follows it, so it never takes a real release's number
//                        elsewhere (box/src/link/updates/fleet.ts).
// The key is Ed25519 (openssl genpkey -algorithm ed25519). Machines take a manifest only when its
// key is one pinned in kuvo-link.js (UPDATE_KEYS), and the signature covers "kuvo-box v1\n" and
// the manifest's bytes, so nothing else signed with the key passes for one.
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const [version, pack, url, boxFile, ...targets] = process.argv.slice(2);
if (!version || !pack || !url || !boxFile || !process.env.KUVO_BOX_SIGNING_KEY) {
  console.error("Usage: KUVO_BOX_SIGNING_KEY=<PEM> node scripts/sign-box.mjs <version> <pack> <pack's URL> <box.json>");
  process.exit(2);
}
const key = crypto.createPrivateKey(process.env.KUVO_BOX_SIGNING_KEY);
if (key.asymmetricKeyType !== "ed25519") {
  console.error("KUVO_BOX_SIGNING_KEY isn't an Ed25519 key.");
  process.exit(1);
}
const box = JSON.parse(fs.readFileSync(boxFile, "utf8"));
if (box.version !== version) {
  console.error(`The box's files are ${box.version}'s, not ${version}'s.`);
  process.exit(1);
}
// The built link's header is also used by the keeper before starting the code.
const linkFile = path.join(path.dirname(boxFile), "bin", "kuvo-link.js");
const link = targets.length && !fs.existsSync(linkFile) ? "" : fs.readFileSync(linkFile, "utf8");
const layout = /^\/\/ Kuvo folder layout ([1-9][0-9]*) [0-9a-f]{12}\n/.exec(link);
function packed(file, address) {
  const data = fs.readFileSync(file);
  return { url: address, sha256: crypto.createHash("sha256").update(data).digest("hex"), size: data.length };
}
const required = new Set([
  "aarch64-apple-darwin", "x86_64-apple-darwin",
  "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "x86_64-pc-windows-gnullvm",
]);
const packs = {};
if (targets.length) {
  for (let i = 0; i < targets.length; i += 4) {
    const [flag, target, file, address] = targets.slice(i, i + 4);
    if (flag !== "--target" || !required.delete(target) || !file || !address) {
      throw new Error("Give one --target <triple> <pack> <URL> for each supported Rust target.");
    }
    packs[target] = packed(file, address);
  }
  if (required.size || box.launch !== "rust") {
    throw new Error("A Rust release needs launch: rust and a pack for every supported target.");
  }
}
// (box/src/install/kuvo-install reads this with `sed`, on a computer with nothing to parse JSON
// yet: one line, with `version`, `engine` and `sha256` each named once, and each system's builds
// as objects of `url` and `integrity`. Keep it so, or change that script with it.)
const manifest = Buffer.from(JSON.stringify({
  kuvo: "box/1",
  version,
  released: new Date().toISOString(),
  engine: box.engine,
  ...(layout ? { layout: Number(layout[1]) } : {}),
  linux: box.linux,
  darwin: box.darwin,
  ...(targets.length ? { launch: "rust", packs } : {}),
  pack: packed(pack, url),
  ...(process.env.KUVO_BOX_SPREADS === "1" ? { spreads: true } : {}),
}));
const signature = crypto.sign(null, Buffer.concat([Buffer.from("kuvo-box v1\n"), manifest]), key);
const publicKey = crypto.createPublicKey(key).export({ type: "spki", format: "der" }).subarray(-32).toString("base64url");
process.stdout.write(JSON.stringify({ manifest: manifest.toString("base64"), signature: signature.toString("base64"), key: publicKey }) + "\n");
