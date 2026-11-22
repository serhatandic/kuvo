// A send the link was partway through when it stopped (an update, a restart) is settled when it's
// asked again under its id: sent once, never left "unknown" (#809).
import "./safety";
import { expect, test } from "bun:test";
import { upgradeKuvoFolder } from "../src/shared/folder-version";
import { K } from "../src/link/home";
import { runMutation } from "../src/link/ownership/mutation";
import { fingerprint } from "../src/link/ownership/merge";
import { keepRecord, readRecord, removeRecord } from "../src/link/ownership/store";

const uncertain = () => ({ status: 409, value: { ambiguous: true, message: "unknown" } });

/** A claim as a link that stopped partway leaves it: made, never answered. */
function leftByAStoppedLink(key: string, change: unknown) {
  upgradeKuvoFolder(K);
  keepRecord("mutations", key, { digest: fingerprint(change) });
}

test("one that never reached its conversation is sent once, as it's asked again", async () => {
  const key = "draft-orphan-absent", change = { id: "msg_orphanabsent" };
  leftByAStoppedLink(key, change);
  let sent = 0;
  const send = async () => ({ status: 200, value: { message: "msg_orphanabsent", sent: ++sent } });
  try {
    const first = await runMutation(key, change, send, undefined, uncertain, undefined, undefined, undefined, async () => "again");
    expect(first).toMatchObject({ status: 200, value: { message: "msg_orphanabsent" } });
    // Asked again: the answer it has, nothing sent twice.
    const again = await runMutation(key, change, send, undefined, uncertain, undefined, undefined, undefined, async () => "again");
    expect(again.status).toBe(200);
    expect(sent).toBe(1);
  } finally {
    removeRecord("mutations", key);
  }
});

test("one its conversation already has is answered as sent, and not sent again", async () => {
  const key = "draft-orphan-there", change = { id: "msg_orphanthere" };
  leftByAStoppedLink(key, change);
  let sent = 0;
  const send = async () => ({ status: 200, value: { sent: ++sent } });
  try {
    const answer = await runMutation(key, change, send, undefined, uncertain, undefined, undefined, undefined,
      async () => ({ status: 200, value: { answer: { send: { text: "hi" } }, message: "msg_orphanthere" } }));
    expect(answer).toMatchObject({ status: 200, value: { message: "msg_orphanthere" } });
    expect(sent).toBe(0);
    expect(readRecord("mutations", key)).toMatchObject({ answer: { status: 200 } });
  } finally {
    removeRecord("mutations", key);
  }
});

test("two asking at once after the restart send it once", async () => {
  const key = "draft-orphan-twice", change = { id: "msg_orphantwice" };
  leftByAStoppedLink(key, change);
  let sent = 0;
  const send = async () => ({ status: 200, value: { sent: ++sent } });
  try {
    const both = await Promise.all([1, 2].map(() =>
      runMutation(key, change, send, undefined, uncertain, undefined, undefined, undefined, async () => "again")));
    expect(sent).toBe(1);
    expect(both.some(answer => answer.status === 200)).toBe(true);
  } finally {
    removeRecord("mutations", key);
  }
});

test("where it can't be looked for (its conversation unreachable), it stays unknown rather than guessed", async () => {
  const key = "draft-orphan-unreachable", change = { id: "msg_orphanaway" };
  leftByAStoppedLink(key, change);
  let sent = 0;
  try {
    const answer = await runMutation(key, change, async () => ({ status: 200, value: { sent: ++sent } }), undefined, uncertain, undefined, undefined, undefined, async () => null);
    expect(answer.status).toBe(409);
    expect(sent).toBe(0);
  } finally {
    removeRecord("mutations", key);
  }
});
