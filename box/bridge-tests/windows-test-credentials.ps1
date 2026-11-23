# Credential Manager is scoped to a Windows logon, not HOME. Run only on the opted-in test account.
param(
    [ValidateSet('check', 'clear', 'remove-fixture')][string]$Action,
    [string]$Target
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent().Name.Split('\')[-1]
if (!$env:KUVO_TEST_DISPOSABLE_ACCOUNT -or $identity -ine $env:KUVO_TEST_DISPOSABLE_ACCOUNT) {
    throw 'The Windows logon does not match KUVO_TEST_DISPOSABLE_ACCOUNT.'
}

Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public static class TestCredentials {
    [StructLayout(LayoutKind.Sequential)]
    private struct CredentialHeader {
        public uint Flags, Type;
        public IntPtr Target;
    }

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CredEnumerate(string filter, uint flags, out uint count, out IntPtr entries);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool CredDelete(string target, uint type, uint flags);
    [DllImport("advapi32.dll")]
    private static extern void CredFree(IntPtr buffer);

    private const int NotFound = 1168;

    public static int Count() { return Inspect(false); }

    // After a run whose baseline `check` found empty: everything in the store is the run's.
    public static int Clear() {
        int removed = Inspect(true);
        if (Count() != 0) throw new InvalidOperationException("Credential Manager is still populated after cleanup.");
        return removed;
    }

    // Read only the header of each entry. Never marshal or log usernames, passwords or blobs.
    private static int Inspect(bool clear) {
        uint count;
        IntPtr entries;
        if (!CredEnumerate(null, 0, out count, out entries)) {
            int error = Marshal.GetLastWin32Error();
            if (error == NotFound) return 0;
            throw new Win32Exception(error);
        }
        try {
            if (clear) {
                for (int index = 0; index < count; index++) {
                    IntPtr address = Marshal.ReadIntPtr(entries, index * IntPtr.Size);
                    var header = (CredentialHeader)Marshal.PtrToStructure(address, typeof(CredentialHeader));
                    Delete(Marshal.PtrToStringUni(header.Target), header.Type);
                }
            }
            return checked((int)count);
        } finally {
            CredFree(entries);
        }
    }

    public static void Delete(string target, uint type) {
        if (!CredDelete(target, type, 0)) {
            int error = Marshal.GetLastWin32Error();
            if (error != NotFound) throw new Win32Exception(error);
        }
    }
}
'@

switch ($Action) {
    'check' {
        # Generic targets are arbitrary. An empty store is the only unambiguous baseline.
        if ([TestCredentials]::Count() -ne 0) { exit 42 }
    }
    'clear' { Write-Output ([TestCredentials]::Clear()) }
    'remove-fixture' {
        if ($Target -cnotmatch '^kuvo-test-[a-f0-9-]{36}$') { throw 'Invalid fixture target.' }
        [TestCredentials]::Delete($Target, 1)
    }
    default { throw 'A credential check action is required.' }
}
