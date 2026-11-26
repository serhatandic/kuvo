import "./safety"
import { describe, expect, test } from "bun:test";
import { bringMergeAccess, planMerge, rewriteMergeReferences } from "../src/link/setup/merge-data";
import { membershipOf, mergePlan } from "../src/link/setup/membership";
import { sharedFits } from "../src/link/setup/shared";
import { MOST_DOC, newSigningSeed, readDoc, signDoc, signingKeyOf } from "../src/link/documents/format";
import { matching } from "../src/link/work-queue/store";
import { newKey, publicOf } from "../src/link/crypto/keys";
import { acceptedOwner, mayActivate, sessionsFencedHere } from "../src/link/ownership/resolve";
import { recordReferences } from "../src/link/backup/locations";
import type { LinkState } from "../src/link/state";

describe("merging setups", () => {
  test("a full setup still has space to save an accepted refusal", () => {
    const entry = { sequence: 1, value: "" };
    const capacity = Math.floor(MOST_DOC / 3);
    const overhead = Buffer.byteLength(JSON.stringify({ setupPersonalities: entry }));
    const full = { setupPersonalities: { ...entry, value: "a".repeat(capacity - overhead) } };
    const denial = { setupMergeDenials: { ...entry, value: { refused: "The combined setup is full." } } };
    expect(sharedFits({ ...full, ...denial })).toBe(true);
    expect(sharedFits({ ...full, setupPersonalities: { ...entry, value: full.setupPersonalities.value + "a" }, ...denial })).toBe(false);
  });

  test("the roster accepts a source-bound merge receipt and an unused replacement range", () => {
    const receipt = { operation: "a".repeat(43), source: "set_" + "a".repeat(22), digest: "b".repeat(43), sourceHigh: 100, sourceName: "Old office", offset: 200 };
    expect(mergePlan(receipt)).toEqual(receipt);
    expect(mergePlan({ ...receipt, digest: "old" })).toBeNull();
    expect(mergePlan({ ...receipt, source: ".." })).toBeNull();
    expect(mergePlan({ ...receipt, offset: -1 })).toBeNull();
    expect(mergePlan({ ...receipt, offset: 1_000_000 })).toBeNull();
  });

  test("rewrites Work Queue references and leaves outside issues, code and out-of-range references alone", () => {
    const text = "Look at #1 and #20. See gh:kuvo/work#1 or `#1`. Unknown #45.";
    expect(rewriteMergeReferences(text, { sourceHigh: 30, offset: 40 })).toBe("Look at #41 and #60. See gh:kuvo/work#1 or `#1`. Unknown #45.");
  });

  test("rewrites visible Work Queue numbers with their parsed Markdown destination", () => {
    const text = '[see **#12** and `#12`](</work-queue/items/12> "title")\n\n'
      + '[see #12][queue] [see #12][] [see #12]\n\n[queue]: /work-queue/items/12\n[see #12]: /work-queue/items/12\n\n'
      + '```md\n[see #12](/work-queue/items/12)\n```';
    expect(rewriteMergeReferences(text, { sourceHigh: 30, offset: 40 })).toBe(
      '[see **#52** and `#12`](</work-queue/items/52> "title")\n\n'
      + '[see #52][queue] [see #52][see #12] [see #52][see #12]\n\n[queue]: /work-queue/items/52\n[see #12]: /work-queue/items/52\n\n'
      + '```md\n[see #12](/work-queue/items/12)\n```');
  });

  test("an explicit Work Queue link gives issue-style labels their new number", () => {
    const text = "[Issue #12](/work-queue/items/12) [External issue #3 blocks PR #12][queue] [Issue #12](https://example.org/issues/12) Issue #12\n\n[queue]: /work-queue/items/12";
    expect(rewriteMergeReferences(text, { sourceHigh: 30, offset: 40 })).toBe(
      "[Issue #52](/work-queue/items/52) [External issue #3 blocks PR #52][queue] [Issue #12](https://example.org/issues/12) Issue #12\n\n[queue]: /work-queue/items/52");
  });

  test("a bare number names the retained item while an origin names the matching alias", () => {
    const origin = "set_" + "a".repeat(22);
    const kept = { id: "retained", number: 1 };
    const from = { id: "joining", number: 2, numberAliases: [{ setup: origin, name: "Old office", number: 1 }] };
    expect(matching([from, kept], { number: 1 })).toEqual([kept]);
    expect(matching([from, kept], { origin, number: 1 })).toEqual([from]);
  });

  test("activation and restore retain the same session owner across a merge", () => {
    const kept = newKey();
    const retainedKey = publicOf(kept).toString("base64url");
    const joiningKey = publicOf(newKey()).toString("base64url");
    const sign = newSigningSeed();
    const setup = "set_" + "a".repeat(22);
    const source = "set_" + "b".repeat(22);
    const session = "ses_conflicted";
    const box = (key: string) => ({ key, sign: signingKeyOf(newSigningSeed()), address: "a".repeat(22), name: key, kind: "machine" });
    const local: LinkState = { key: kept.toString("base64url"), sign, phones: [], sharedSetup: setup,
      setup: { id: setup, epoch: 1, primary: retainedKey, sign: signingKeyOf(sign), role: "primary", checkpointed: true },
      macMoved: [{ session, key: retainedKey }],
    };
    const saved = planMerge(local, {
      source, name: "Joining", digest: "c".repeat(43), high: 0,
      authority: readDoc(signDoc(sign, "setup-shared", 1, {}))!, readers: {},
      membership: membershipOf({ boxes: [box(joiningKey)], moved: [{ session, key: joiningKey }] })!,
      preferences: {}, choices: {}, owners: { ["session computer:" + session]: { computer: joiningKey } }, earlier: {},
    }, "d".repeat(43));
    bringMergeAccess(local, saved);
    local.shared = { ...Object.fromEntries(Object.entries(saved.together).map(([kind, entry]) => [kind, { sequence: 1, ...entry }])),
      membershipAndAccess: { sequence: 1, value: { boxes: [box(retainedKey), box(joiningKey)], moved: local.macMoved } },
    };
    const [files] = recordReferences("agents/sessions/" + session + "/session.json", (_root, id) => id);
    expect(local.macMoved).toEqual([{ session, key: retainedKey }]);
    // (Who has a record is no longer read from membership's moved list, since b584bfd7f: here,
    // the move journal, then who says it has it. So the list decides no owner of its own.)
    expect(acceptedOwner(local, files!)).toBeUndefined();
    expect(mayActivate({ kind: "conversations", id: session }, { link: local })).toBe(true);
    expect(sessionsFencedHere({ link: local, origin: joiningKey })).not.toContain(session);
  });
});
