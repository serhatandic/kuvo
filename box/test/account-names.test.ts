import "./safety"
import { expect, test } from "bun:test";
import { spendTitle, workAccountTitle } from "../src/shared/account-names";

test("work titles retain account words and labels, unknown providers retain their name", () => {
  for (const [provider, name] of Object.entries({"claude-code": "Claude", "codex-cli": "Codex", "grok-cli": "Grok", cursor: "Cursor", antigravity: "Antigravity", "github-copilot": "Copilot", openrouter: "OpenRouter", zai: "Z.ai", "zai-coding-plan": "Z.ai", "opencode-go": "OpenCode Go"})) {
    expect(workAccountTitle(provider + "@work", "Provider", "Work")).toBe(name + " · Work");
    expect(workAccountTitle(provider, "Provider", name)).toBe(name);
  }
  expect(workAccountTitle("new-provider", "New Provider")).toBe("New Provider");
  expect(workAccountTitle("constructor", "Constructor Account")).toBe("Constructor Account");
});

test("spend titles keep card words and capitalize unknown services", () => {
  expect(spendTitle("constructor")).toBe("Constructor");
  expect(spendTitle("opencode")).toBe("OpenCode");
  expect(spendTitle("future-service")).toBe("Future-Service");
});
