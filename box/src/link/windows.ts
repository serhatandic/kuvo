// Host-side lifecycle for the Linux installation in WSL 2. Task Scheduler owns the
// foreground WSL invocation, and the ordinary Linux keeper owns the workers.
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { distroName, isWsl2, windowsDrive, windowsSystemProgram } from "../shared/wsl";
import { writePrivate } from "./files";
import { K, RUN } from "./home";

const taskFile = path.join(RUN, "wsl-task");
const encode = (script: string) => Buffer.from(script, "utf16le").toString("base64");
const literal = (value: string) => "'" + value.replaceAll("'", "''") + "'";
const flags = ["-NoLogo", "-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-EncodedCommand"];
const powershell = () => windowsSystemProgram("WindowsPowerShell\\v1.0\\powershell.exe");
const windows = (script: string) => execFileSync(powershell(), [...flags, encode("$ErrorActionPreference = 'Stop';\n" + script)], {
  encoding: "utf8", timeout: 30_000, stdio: ["ignore", "pipe", "pipe"],
});

export function installedWslTask(): string | undefined {
  if (!isWsl2()) return undefined;
  try {
    const name = fs.readFileSync(taskFile, "utf8").trim();
    if (/^KuvoCode-WSL-[a-f0-9]{32}$/.test(name)) return name;
  } catch {}
  return undefined;
}

export function startWslTask(distro: string | undefined): boolean {
  if (!isWsl2() || !distro || distroName(distro) !== distro || distro !== process.env.WSL_DISTRO_NAME) return false;
  const launch = path.join(RUN, "launch.env");
  const env = fs.readFileSync(launch, "utf8");
  const linux = env.replace(/^PATH=(.*)$/m, (_, list: string) => "PATH=" + list.split(":").filter(folder => !windowsDrive(folder)).join(":"));
  if (linux !== env) writePrivate(launch, linux);
  const user = windows("[Security.Principal.WindowsIdentity]::GetCurrent().User.Value").trim();
  if (!/^S-[0-9-]+$/.test(user)) return false;
  const task = "KuvoCode-WSL-" + crypto.createHash("sha256").update(JSON.stringify([user, distro, K])).digest("hex").slice(0, 32);
  writePrivate(taskFile, task + "\n");
  const action = `& "$env:SystemRoot\\System32\\wsl.exe" -d ${literal(distro)} --cd ~ --exec /usr/bin/setsid --wait /usr/bin/env ${literal("HOME=" + path.dirname(K))} /bin/sh ${literal(path.join(K, "bin/kuvo-start"))} wsl-keep
exit $LASTEXITCODE`;
  windows(`
$user = ${literal(user)}
$name = ${literal(task)}
$action = New-ScheduledTaskAction -Execute (Join-Path $env:SystemRoot 'System32\\WindowsPowerShell\\v1.0\\powershell.exe') -Argument ${literal(flags.join(" ") + " " + encode(action))}
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Limited
$settings = New-ScheduledTaskSettingsSet -Hidden -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -MultipleInstances IgnoreNew -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName $name -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
# OpenSSH can register with an elevated token while the task runs as the same
# user with a limited token. Give that user SID control of this task directly.
$scheduler = New-Object -ComObject 'Schedule.Service'
$scheduler.Connect()
$task = $scheduler.GetFolder('\\').GetTask($name)
$acl = [Security.AccessControl.RawSecurityDescriptor]::new($task.GetSecurityDescriptor(4))
$ace = [Security.AccessControl.CommonAce]::new('None', 'AccessAllowed', 0x1f01ff, [Security.Principal.SecurityIdentifier]::new($user), $false, $null)
$acl.DiscretionaryAcl.InsertAce(0, $ace)
$task.SetSecurityDescriptor($acl.GetSddlForm('Access'), 0)
Start-ScheduledTask -TaskName $name`);
  return true;
}

/** Remove only the task registered for this installation, leaving its distro and account alone. */
export function removeWslTask(): void {
  const name = installedWslTask();
  if (!name) return;
  // Let Linux's ordinary stop command shut the keeper down, including on a relay wipe.
  // Unregistering the task prevents a new start during that stop.
  windows(`$name = ${literal(name)}
$task = Get-ScheduledTask -TaskName $name -TaskPath '\\' -ErrorAction SilentlyContinue -ErrorVariable lookup
if ($task) {
  $task | Unregister-ScheduledTask -Confirm:$false
} elseif ($lookup | Where-Object { $_.CategoryInfo.Category -ne 'ObjectNotFound' }) {
  throw $lookup[0]
}`);
  fs.rmSync(taskFile, { force: true });
}

const POWER_HOLD = `
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class KuvoPower {
  [DllImport("kernel32.dll")]
  private static extern uint SetThreadExecutionState(uint flags);
  public static void Hold() {
    const uint Continuous = 0x80000000;
    const uint SystemRequired = 1;
    if (SetThreadExecutionState(Continuous | SystemRequired) == 0)
      throw new InvalidOperationException("Windows refused the power request.");
    try { Console.OpenStandardInput().ReadByte(); }
    finally { SetThreadExecutionState(Continuous); }
  }
}
'@
[KuvoPower]::Hold()
`;

/** The pipe releases the Windows request on idle, shutdown, or loss of the resident process. */
export function keepWindowsAwake(busy: () => Promise<boolean>): () => void {
  if (!isWsl2()) return () => {};
  let held: ChildProcess | undefined;
  let stopped = false;
  let checking = false;
  let reported = false;
  const failed = () => {
    if (!reported) console.error("Kuvo Code couldn’t keep Windows awake while working.");
    reported = true;
  };
  const release = () => { held?.stdin?.end(); held = undefined; };
  const look = async () => {
    if (stopped || checking) return;
    checking = true;
    try {
      const active = await busy();
      if (stopped) return;
      if (!active) { release(); return; }
      if (held) return;
      const child = held = spawn(powershell(), [...flags, encode(POWER_HOLD)], { stdio: ["pipe", "ignore", "ignore"] });
      child.stdin?.on("error", () => {});
      child.on("error", failed);
      child.once("close", code => {
        if (held === child) held = undefined;
        if (code !== 0) failed();
      });
      child.unref();
    } catch { failed(); }
    finally { checking = false; }
  };
  const timer = setInterval(() => void look(), 5000);
  timer.unref();
  void look();
  return () => { stopped = true; clearInterval(timer); release(); };
}
