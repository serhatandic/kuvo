import "./safety"
import { expect, test } from "bun:test";
import fs from "node:fs";
import path from "node:path";
import { appMemberWipePending, appMemberWipeReceipt, finishReinstall, lastSetupWipe, lastSetupWipeReceipt,
  prepareWipeDirectory, retainedWipeApproval, resumeWipeDirectory, wipeApprovedDirectory, wipeReceipts } from "../src/link/setup/install-offer";

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
    prepareWipeDirectory(home, setup, operation, false);
    const file = path.join(wipeReceipts(home), "last-setup-wipe");
    const { formatVersion, ...legacy } = JSON.parse(fs.readFileSync(file, "utf8"));
    expect(formatVersion).toBe(1);
    const future = JSON.stringify({ ...legacy, phase: "erasing", formatVersion: 90 });
    fs.writeFileSync(file, future);
    await expect(resumeWipeDirectory(home)).rejects.toThrow("format version 90");
    expect(() => prepareWipeDirectory(home, setup, operation, false)).toThrow("format version 90");
    expect(fs.readFileSync(file, "utf8")).toBe(future);
    expect(fs.existsSync(home)).toBe(true);
    fs.writeFileSync(file, JSON.stringify(legacy));
    expect(lastSetupWipe(home)).toBe("erasing");
    expect(retainedWipeApproval(home, setup, operation)).toBe(true);
    expect(retainedWipeApproval(home, setup, "d".repeat(32))).toBe(false);
    await resumeWipeDirectory(home);
    expect(fs.readFileSync(path.join(home, "session"), "utf8")).toBe("old data");
    wipeApprovedDirectory(home, setup, operation, false);
    expect(fs.existsSync(home)).toBe(false);
    expect(lastSetupWipe(home)).toBe("erased");
    await resumeWipeDirectory(home);
    finishReinstall(home);
    finishReinstall(home);
    expect(lastSetupWipe(home)).toBeNull();
    expect(fs.readdirSync(wipeReceipts(home))).toEqual([]);
  } finally {
    fs.rmSync(account, { recursive: true, force: true });
  }
});

test("an app member wipe resumes after detach and yields only a member receipt", async () => {
  const account = fs.mkdtempSync(path.join(process.env.HOME!, "app-member-wipe-"));
  const home = path.join(account, "Library", "Application Support", "run.kuvo.code.fixture");
  const setup = "set_" + "A".repeat(22);
  const operation = "c".repeat(32);
  try {
    fs.mkdirSync(home, { recursive: true });
    fs.writeFileSync(path.join(home, "session"), "old data");
    prepareWipeDirectory(home, setup, operation, true);
    await resumeWipeDirectory(home, true);
    expect(fs.readFileSync(path.join(home, "session"), "utf8")).toBe("old data");
    expect(appMemberWipeReceipt(home)).toBeUndefined();
    expect(() => wipeApprovedDirectory(home, setup, operation, true, () => { throw new Error("key store unavailable"); })).toThrow("key store unavailable");
    expect(fs.existsSync(home)).toBe(false);
    expect(appMemberWipePending(home)).toBe(true);
    expect(appMemberWipeReceipt(home)).toBeUndefined();
    expect(fs.readFileSync(path.join(wipeReceipts(home), "erase-" + operation, "session"), "utf8")).toBe("old data");
    await resumeWipeDirectory(home, true, () => expect(fs.existsSync(home)).toBe(false));
    expect(appMemberWipePending(home)).toBe(false);
    expect(appMemberWipeReceipt(home)).toEqual({ setup, operation });
    expect(lastSetupWipeReceipt(home)).toBeUndefined();
  } finally {
    fs.rmSync(account, { recursive: true, force: true });
  }
});
