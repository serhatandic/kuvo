import "./safety"
import { expect, test } from "bun:test";
import { computerWords } from "../src/link/areas/computers";

test("computer reach and working display words are supplied by the box", () => {
  for (const reach of ["connected", "connecting", "away"] as const) {
    const words = computerWords({ reach, name: "mini", working: 2 }, "Studio");
    expect(words.title.length).toBeGreaterThan(0);
    expect(words.workingTitle).toBe("2 sessions at work");
    expect(Boolean(words.notice)).toBe(reach !== "connected");
    if (words.notice) expect(words.detail).toBe(words.notice.detail);
    // Where its settings would show, the words are about its settings, not what it runs.
    expect(Boolean(words.settingsNotice)).toBe(reach !== "connected");
    if (words.settingsNotice) {
      expect(words.settingsNotice.detail).toContain("settings");
      expect(words.settingsNotice.detail).not.toBe(words.notice!.detail);
    }
  }
  expect(computerWords({ reach: "connected", name: "mini", working: 1 }, "Studio").workingTitle).toBe("1 session at work");
});


test("local app reach states have box supplied words even when the answering computer is connected", () => {
  const { reachTitles } = computerWords({ reach: "connected", name: "mini" }, "Studio");
  expect(reachTitles).toEqual({
    connecting: "Connecting…", held: "Needs an update", away: "Can’t be reached",
    notPaired: "Hasn’t taken this app yet",
    unreached: "Can’t be reached from this app",
  });
});
