// Another of your computers that holds a signed release (a Mac whose app updated, say), as the box
// under test meets it by peer mail (`release`, PROTOCOL.md "Updates", Your computers follow each
// other): it answers what it's asked as a box holding that release does, part by part. Its mail goes
// through `deliver` (what the box under test takes: `link.takeReleaseMail`) once the test drains it,
// so a whole exchange runs in order, with no timers. `change` alters what it hands over (a test's
// tampered pack, another manifest).
import fs from "node:fs";
import path from "node:path";

export const PART = 30_000;

export function peerHolding(folder, { change = {} } = {}) {
  const boxes = change.boxes ?? fs.readFileSync(path.join(folder, "boxes.json"), "utf8");
  const name = fs.readdirSync(folder).find(each => each !== "boxes.json" && !each.startsWith("kuvo-install"));
  let pack = fs.readFileSync(path.join(folder, name));
  if (change.pack) pack = change.pack(pack);
  const version = JSON.parse(Buffer.from(JSON.parse(boxes).manifest, "base64").toString("utf8")).version;
  const queue = [];
  // What it was asked, in order.
  const asked = [];
  const answer = release => {
    if (typeof release.want === "string") return release.want === version ? { version, boxes, size: pack.length } : { version: release.want, none: true };
    if (Number.isInteger(release.part)) return { version, i: release.part, d: pack.subarray(release.part * PART, (release.part + 1) * PART).toString("base64") };
    return null;
  };
  return {
    version,
    asked,
    /** Where the box under test sends its mail to this computer. */
    send: (_peer, release) => {
      asked.push(release);
      const reply = answer(release);
      if (reply) queue.push(reply);
      return true;
    },
    /** Its answers taken by the box under test (`deliver(reply)`), until none are left. */
    drain: deliver => {
      let taken = 0;
      while (queue.length) {
        deliver(queue.shift());
        taken++;
      }
      return taken;
    },
    /** Its answers lost on the way (the relay dropped them). */
    lose: () => { queue.length = 0; },
  };
}
