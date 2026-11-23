// One real dispatcher, peer mailbox and box home per process. The parent supplies the engine's
// HTTP endpoint and carries encrypted frames in place of the relay.

import { writeFileSync } from "node:fs";
import { BRIDGE_FILE } from "../../../src/link/home";
import { sendOwnerMessages } from "../../../src/link/ownership/messages";
import { machineLink, readLink } from "../../../src/link/state";
import type { LinkHost } from "../../../src/link/relay/host";
import { IKInitiator } from "../../../src/link/crypto/noise";
import { pauseForUser } from "../../../src/link/engine/queue";
import { noteAskEvent } from "../../../src/link/glance/asked";
import { shareSetupLists } from "../../../src/link/lists/setup";
import { peerMail, receiveMail } from "../../../src/link/peers/mail";
import { noteUnreachable } from "../../../src/link/peers/reach";
import { readFrame, readRelay } from "../../../src/link/relay/frames";
import { takeWorkQueueMail } from "../../../src/link/work-queue/across";
import { isObject } from "../../../src/shared/json";
import { engineHandler } from "../../../src/link/server/router";
import { answerSshRoute, sshLine } from "../../../src/link/server/ssh-route";
import { bridgeRecordFormat, type EngineRecord } from "../../../src/shared/records";

const { port, bridgePort, command } = await Bun.stdin.json();
const readers = {
  readEngine: () => ({ port, password: "test" }) as EngineRecord,
  bridge: () => bridgePort ? { port: Number(bridgePort), password: "test", pid: process.pid } : null,
};
const dispatch = engineHandler(readers.readEngine, { name: "Test box" }, () => {
  const bridge = readers.bridge();
  if (!bridge) throw new Error("No coding agents on this box.");
  return bridge;
});

async function route(path: string, body?: object, method = body ? "POST" : "GET") {
  const chunks: Buffer[] = [];
  let status = 0;
  for await (const piece of dispatch({ method, path, headers: {}, fromApp: true, served: true,
    ...(body ? { body: Buffer.from(JSON.stringify(body)) } : {}) }, undefined)) {
    if ("head" in piece) status = piece.head.status;
    if ("body" in piece) chunks.push(piece.body);
  }
  return { status, text: Buffer.concat(chunks).toString() };
}

async function run() {
  switch (command.kind) {
    case "publish": {
      if (command.waiting) {
        noteAskEvent({ type: "permission.asked", data: { id: "perm_fixture", sessionID: command.waiting, action: "shell" } }, "engine", 700);
        await pauseForUser(command.waiting);
      }
      const frames: string[] = [];
      await shareSetupLists({ send: frame => { frames.push(frame.toString("base64url")); } }, readers);
      return frames;
    }
    case "receive": {
      for (const encoded of command.frames) {
        const delivered = readRelay(Buffer.from(encoded, "base64url"));
        const frame = delivered?.kind === "deliver" && readFrame(delivered.payload);
        if (!frame) throw new Error("Expected mail on the relay.");
        let saved: ReturnType<typeof peerMail>;
        receiveMail(frame.body, null, (sender, payload) => { saved = peerMail(sender, payload, null); });
        await saved!;
      }
      return true;
    }
    case "away":
      noteUnreachable(Buffer.from(command.address, "base64url"));
      return true;
    case "route":
      return await route(command.path);
    case "start": {
      const computer = command.target;
      const handshake = new IKInitiator(Buffer.from(command.device, "base64url"), Buffer.from(computer, "base64url"), Buffer.from("kuvo-ssh v1"));
      let transport: ReturnType<IKInitiator["readMessage2"]>["transport"] | undefined;
      const replies: Record<string, unknown>[] = [];
      async function* request() {
        yield sshLine({ computer, hello: handshake.writeMessage1(Buffer.from('{"v":1,"client":"desktop"}')).toString("base64url") });
        if (transport) yield sshLine({ request: transport.seal(Buffer.from(JSON.stringify({
          method: "POST", path: "kuvo/sessions", body: { computer: command.bodyTarget, chat: true, title: "Started there" },
        }))).toString("base64url") });
      }
      await answerSshRoute({ addressed: true, internal: false }, request(), async bytes => {
        const said = JSON.parse(bytes.toString());
        if (said.hello) transport = handshake.readMessage2(Buffer.from(said.hello, "base64url")).transport;
        else replies.push(said);
      }, new AbortController().signal, dispatch, true);
      return replies[0];
    }
    default: throw new Error("Unknown box fixture command.");
  }
}

if (command.kind === "conversation") {
  if (bridgePort) writeFileSync(BRIDGE_FILE, JSON.stringify(bridgeRecordFormat.write({
    port: Number(bridgePort), password: "test", pid: process.pid,
  })));
  const host = {
    welcomed: true,
    send(frame: Buffer) { process.send!({ frame: frame.toString("base64url") }); },
  } as LinkHost;
  machineLink.host = host;
  let stop: AbortController | undefined;
  const waiting = new Map<string, string>();
  process.on("message", async (given: any) => {
    try {
      if (given.call) process.send!({ replyTo: given.replyTo, routeAnswer: await route(given.call.path, given.call.body, given.call.method) });
      if (typeof given.round === "number") {
        if (given.receipt) waiting.set(given.receipt, given.replyTo);
        await sendOwnerMessages(host, readLink(), given.round);
        if (!given.receipt) process.send!({ replyTo: given.replyTo, routeAnswer: null });
      }
      if (given.frame) {
        const delivered = readRelay(Buffer.from(given.frame, "base64url"));
        const frame = delivered?.kind === "deliver" && readFrame(delivered.payload);
        if (frame) receiveMail(frame.body, host, (sender, payload) => {
          const result = isObject(payload.workQueue)
            ? takeWorkQueueMail(sender, payload.workQueue, host, () => ({ targets: [], bridge: true }))
            : peerMail(sender, payload, host);
          const work = payload.workQueue;
          const receipt = isObject(work) ? (work.ownerDraftReceipt ?? work.ownerMessageReceipt) as { operation: string } | undefined : undefined;
          const replyTo = receipt && waiting.get(receipt.operation);
          if (replyTo) void Promise.resolve(result).then(async () => {
            const answer = await route(`kuvo/data/messages/${receipt.operation}`);
            waiting.delete(receipt.operation);
            process.send!({ replyTo, routeAnswer: JSON.parse(answer.text) });
          }).catch(error => process.send!({ failure: String(error) }));
        });
      }
      if (given.watch) {
        stop = new AbortController();
        const watch = Buffer.from(JSON.stringify(given.watch)).toString("base64url");
        for await (const piece of dispatch({ method: "GET", path: `kuvo/line?watch=${watch}`, headers: {}, fromApp: true, served: true }, undefined, stop.signal)) {
          if ("body" in piece) for (const raw of piece.body.toString().trim().split("\n")) {
            if (raw) process.send!({ line: JSON.parse(raw) });
          }
        }
      }
      if (given.read) process.send!({ read: await route(given.read) });
      if (given.send) {
        const sent = await route(given.send.path, given.send.body);
        process.send!({ sent });
        sendOwnerMessages(host, readLink(), Date.now());
      }
      if (given.stop) stop?.abort();
    } catch (error) {
      process.send!({ failure: String(error) });
    }
  });
  process.send!({ ready: true });
} else {
  await Bun.write(Bun.stdout, JSON.stringify(await run()) + "\n");
  process.exit(0);
}
