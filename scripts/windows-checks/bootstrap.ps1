# Sent as the SSH command. SFTP transports the archive because Windows OpenSSH's
# redirected console input can stall on a binary stream.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$base = Join-Path $env:USERPROFILE 'kuvo-checks'
if (Test-Path -LiteralPath $base) {
    if ((Get-Item -LiteralPath $base -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Refusing linked checks root: $base"
    }
} else {
    New-Item -ItemType Directory -Path $base -Force | Out-Null
}
$run = Join-Path $base $name
if ($action -eq 'create') {
    New-Item -ItemType Directory -Path $run | Out-Null
    Write-Output "Run: $run"
    exit 0
}
if ((Get-Item -LiteralPath $run -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
    throw "Refusing linked run folder: $run"
}
$status = 1
try {
    if ($action -eq 'run') {
        $archive = Join-Path $run 'repo.tar.gz'
        $repo = Join-Path $run 'repo'
        New-Item -ItemType Directory -Path $repo | Out-Null
        & "$env:SystemRoot\System32\tar.exe" -xzf $archive -C $repo
        if ($LASTEXITCODE -ne 0 -or !(Test-Path -LiteralPath "$repo\.windows-checks\ready")) {
            throw "Checkout staging failed."
        }
        Remove-Item -LiteralPath $archive
        & "$repo\.windows-checks\native.ps1" $run (Join-Path $base 'tools') $config
        $status = $LASTEXITCODE
    }
} finally {
    Set-Location -LiteralPath $base
    $bytes = (Get-ChildItem -LiteralPath $run -Recurse -Force -File | Measure-Object -Property Length -Sum).Sum
    Write-Output "Run footprint before cleanup: $([Math]::Ceiling($bytes / 1KB)) KiB"
    try { Remove-Item -LiteralPath $run -Recurse -Force } catch {
        Write-Output "Cleanup failed: $_"
        if ($status -eq 0) { $status = 1 }
    }
    Write-Output "Remaining in ${base}:"
    Get-ChildItem -LiteralPath $base -Force -Name
}
exit $status
