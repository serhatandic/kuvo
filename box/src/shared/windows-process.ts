// The OS adapter reads the creation FILETIME from an open process handle. A number by itself is
// never enough to recognize a process on Windows.

export type WindowsProcessIdentity = { pid: number; startIdentity: string };
export type WindowsProcessState = "live" | "stale" | "unknown";

/** null means the OS confirmed the process is gone, undefined that it could not tell. */
export type ReadWindowsProcessStart = (pid: number) => string | null | undefined;

export function windowsProcessState(record: Partial<WindowsProcessIdentity>, readStart: ReadWindowsProcessStart): WindowsProcessState {
  const { pid, startIdentity } = record;
  if (typeof pid !== "number" || !Number.isSafeInteger(pid) || pid <= 0 || pid > 0xffffffff ||
      typeof startIdentity !== "string" || !/^[1-9][0-9]{0,19}$/.test(startIdentity)) return "unknown";
  const current = readStart(pid);
  if (current === undefined) return "unknown";
  return current === startIdentity ? "live" : "stale";
}
