# A foreground child used by native.ps1. Its descendants inherit a Windows Job Object.
param([string]$Node, [string]$Runner, [string]$Config)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class TestJob {
    [StructLayout(LayoutKind.Sequential)]
    private struct BasicLimits {
        public long ProcessUserTime, JobUserTime;
        public uint Flags;
        public UIntPtr MinWorkingSet, MaxWorkingSet;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass, SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ExtendedLimits {
        public BasicLimits Limits;
        public ulong ReadOperations, WriteOperations, OtherOperations;
        public ulong ReadBytes, WriteBytes, OtherBytes;
        public UIntPtr ProcessMemoryLimit, JobMemoryLimit, PeakProcessMemory, PeakJobMemory;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(IntPtr job, int infoClass, ref ExtendedLimits limits, int size);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool QueryInformationJobObject(IntPtr job, int infoClass, out ExtendedLimits limits, int size, IntPtr returned);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll")]
    private static extern IntPtr GetCurrentProcess();

    private const int ExtendedLimitInformation = 9;
    private static readonly int Size = Marshal.SizeOf(typeof(ExtendedLimits));

    public static IntPtr ContainSelf() {
        IntPtr job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception();
        var limits = new ExtendedLimits { Limits = new BasicLimits { Flags = 0x2000 } }; // KILL_ON_JOB_CLOSE
        if (!SetInformationJobObject(job, ExtendedLimitInformation, ref limits, Size) ||
            !AssignProcessToJobObject(job, GetCurrentProcess())) throw new Win32Exception();
        // This handle is non-inheritable and lives until the supervisor exits.
        return job;
    }

    public static ulong PeakBytes(IntPtr job) {
        ExtendedLimits limits;
        if (!QueryInformationJobObject(job, ExtendedLimitInformation, out limits, Size, IntPtr.Zero))
            throw new Win32Exception();
        return limits.PeakJobMemory.ToUInt64();
    }
}
'@

$job = [TestJob]::ContainSelf()
& $Node $Runner $Config
$status = $LASTEXITCODE
try {
    Write-Output "Peak job committed memory: $([Math]::Ceiling([TestJob]::PeakBytes($job) / 1KB)) KiB"
} catch {
    Write-Output "Peak job committed memory unavailable: $_"
}
exit $status
