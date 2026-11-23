// WSL keeps Linux paths, processes and executables. Windows interop is used explicitly
// for host facts and lifecycle, even when Windows folders are missing from PATH.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export function isWsl2(platform = process.platform, release?: string): boolean {
  return platform === "linux" && /microsoft-standard|wsl2/i.test(release ?? os.release());
}

export function distroName(value: unknown): string | undefined {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$/.test(value) ? value : undefined;
}

export function windowsHostName(value: unknown): string | undefined {
  return typeof value === "string" && /^[A-Za-z0-9][A-Za-z0-9-]{0,62}$/.test(value) ? value.toLowerCase() : undefined;
}

export function windowsSystemProgram(program: string): string {
  return execFileSync("/usr/bin/wslpath", ["-u", `C:\\Windows\\System32\\${program}`], { encoding: "utf8", timeout: 3000 }).trim();
}

let host: string | undefined;
export function wslFacts(): { version: 2; distro?: string; host?: string } | undefined {
  if (!isWsl2()) return undefined;
  const distro = distroName(process.env.WSL_DISTRO_NAME);
  if (host === undefined) {
    try {
      host = windowsHostName(execFileSync(windowsSystemProgram("hostname.exe"), [], {
        encoding: "utf8", timeout: 3000, stdio: ["ignore", "pipe", "ignore"],
      }).trim());
    } catch {}
    // WSL normally inherits the Windows hostname, including without interop.
    host ??= windowsHostName(os.hostname()) ?? "";
  }
  return { version: 2, ...(distro ? { distro } : {}), ...(host ? { host } : {}) };
}

export function windowsDrive(folder: string): boolean {
  if (!isWsl2()) return false;
  let real: string;
  let mounts: string;
  try {
    real = fs.realpathSync(folder);
    mounts = fs.readFileSync("/proc/self/mountinfo", "utf8");
  } catch { return false; }
  // Choose the innermost mount (including bind mounts and non-default automount roots).
  let best = "";
  let drive = false;
  for (const line of mounts.split("\n")) {
    const [left, right] = line.split(" - ");
    const mount = left?.split(" ")[4]?.replace(/\\([0-7]{3})/g, (_, octal) => String.fromCharCode(parseInt(octal, 8)));
    if (!mount || mount.length < best.length || (real !== mount && !real.startsWith(mount === "/" ? "/" : mount + path.sep))) continue;
    best = mount;
    drive = /^drvfs |^9p .*\baname=drvfs(?:[;, ]|$)/.test(right ?? "");
  }
  return drive;
}
