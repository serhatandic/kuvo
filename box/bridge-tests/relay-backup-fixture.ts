import { createInterface } from "node:readline";
import fs from "node:fs";
import path from "node:path";
import { openMail } from "../src/link/peers/known";
import { readFrame, readRelay } from "../src/link/relay/frames";
import { readLink } from "../src/link/state";
import { sendForwards } from "../src/link/work-queue/forward";
import { takeWorkQueueMail } from "../src/link/work-queue/across";

export async function deliver(frames: string[]): Promise<string[]> {
  const replies: string[] = [];
  for (const raw of frames) {
    const routed = readRelay(Buffer.from(raw, "base64url"));
    const frame = routed?.kind === "deliver" && readFrame(routed.payload);
    const mail = frame && openMail(frame.body, readLink());
    if (mail) await takeWorkQueueMail(mail.sender, mail.payload.workQueue, {
      send: bytes => { replies.push(bytes.toString("base64url")); },
    }, () => ({ targets: [], bridge: true }));
  }
  return replies;
}

if (import.meta.main) {
  if (process.argv.includes("--emit-mac-spool")) {
    const frames: string[] = [];
    sendForwards({ welcomed: true, send: bytes => { frames.push(bytes.toString("base64url")); } },
      readLink(), Date.now(), { ownOnly: true });
    console.log(JSON.stringify(frames));
  } else {
    const cutPublish = process.env.KUVO_TEST_CUT_BACKUP_PUBLISH;
    if (cutPublish) {
      const link = fs.linkSync;
      fs.linkSync = (from, to) => {
        link(from, to);
        if (typeof to === "string" && path.basename(to) === cutPublish + ".kuvo") process.exit(93);
      };
    }
    if (process.env.KUVO_TEST_FAIL_BACKUP_SYNC) {
      fs.fsyncSync = () => { throw new Error("Fixture could not sync the receiver directory."); };
    }
    for await (const line of createInterface({ input: process.stdin })) {
      console.log(JSON.stringify(await deliver(JSON.parse(line))));
    }
  }
}
