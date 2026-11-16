import "./safety"
import { expect, test } from "bun:test";
import { computerWords } from "../src/link/areas/computers";

test("computer reach and working display words are supplied by the box", () => {
  for (const reach of ["connected", "connecting", "away", "unknown"] as const) {
    const words = computerWords({ reach, name: "mini", through: "devbox", working: 2 }, "Studio");
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
  // How it's reached, in a line: one known only through another names that computer.
  const title = (reach: "connected" | "connecting" | "away" | "unknown", through?: string) => computerWords({ reach, name: "mini", through }, "Studio").title;
  expect([title("connected"), title("connecting"), title("away")]).toEqual(["Connected", "Connecting", "Can’t be reached"]);
  expect(title("unknown", "devbox")).toBe("Known through devbox");
  expect(title("unknown")).toBe("Not reached directly");
  expect(computerWords({ reach: "connected", name: "mini", working: 1 }, "Studio").workingTitle).toBe("1 session at work");
  expect(computerWords({ reach: "unknown", name: "mini" }, "Studio").workingTitle).toBeUndefined();
});


test("local app reach states have box supplied words even when the answering computer is connected", () => {
  const { reachTitles } = computerWords({ reach: "connected", name: "mini" }, "Studio");
  expect(reachTitles).toEqual({
    connecting: "Connecting…", held: "Needs an update", away: "Can’t be reached",
    notPaired: "Hasn’t taken this app yet", shared: "Shared machine", apart: "Another setup’s",
    unreached: "Can’t be reached from this app",
  });
});
