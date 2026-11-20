import "./safety";
import { expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { finishReinstall, lastSetupWipe, prepareLastSetupWipe, retainedWipeApproval, resumeLastSetupWipe, wipeLastSetup, wipeReceipts } from "../src/link/setup/install-offer";

test("a saved wipe approval is tied to the old installation and only the erasing phase removes it", async () => {
  const account = fs.mkdtempSync(path.join(process.env.HOME!, "wipe-offer-"));
  const home = path.join(account, ".kuvo-code");
  const setup = `set_${"A".repeat(22)}`;
  const operation = "c".repeat(32);
  try {
    expect(lastSetupWipe(home)).toBeNull();
    expect(lastSetupWipe(path.join(account, "other-box"))).toBeNull();
    fs.mkdirSync(home);
    fs.writeFileSync(path.join(home, "session"), "old data");
    prepareLastSetupWipe(home, setup, operation);
    expect(lastSetupWipe(home)).toBe("erasing");
    expect(retainedWipeApproval(home, setup, operation)).toBe(true);
    expect(retainedWipeApproval(home, setup, "d".repeat(32))).toBe(false);
    await resumeLastSetupWipe(home);
    expect(fs.readFileSync(path.join(home, "session"), "utf8")).toBe("old data");
    wipeLastSetup(home, setup, operation);
    expect(fs.existsSync(home)).toBe(false);
    expect(lastSetupWipe(home)).toBe("erased");
    await resumeLastSetupWipe(home);
    finishReinstall(home);
    finishReinstall(home);
    expect(lastSetupWipe(home)).toBeNull();
    expect(fs.readdirSync(wipeReceipts(home))).toEqual([]);
  } finally {
    fs.rmSync(account, { recursive: true, force: true });
  }
});
