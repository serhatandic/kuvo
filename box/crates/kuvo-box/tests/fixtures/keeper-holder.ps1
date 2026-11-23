# Native keeper handover and abrupt-exit proof with a stand-in resident/holder.
param(
    [Parameter(Mandatory)][string]$KuvoBox,
    [Parameter(Mandatory)][string]$Fixture,
    [Parameter(Mandatory)][string]$Scratch
)

$ErrorActionPreference = 'Stop'
$box = (Resolve-Path -LiteralPath $KuvoBox).Path
$holderFixture = (Resolve-Path -LiteralPath $Fixture).Path
$root = Join-Path (Resolve-Path -LiteralPath $Scratch).Path ('holder-live-' + [guid]::NewGuid().ToString('N').Substring(0,8))
$privateProfile = Join-Path $root 'profile'
$target = 'versions/0.8.0-beta.1-123456789abc/bin'
$bin = Join-Path $root $target
$run = Join-Path $root 'run'
New-Item -ItemType Directory $bin,$run,$privateProfile | Out-Null
Get-ChildItem Env:KUVO_* | Remove-Item
$env:KUVO_HOME = $root
foreach ($name in 'HOME','USERPROFILE','APPDATA','LOCALAPPDATA','TEMP','TMP') {
    Set-Item "Env:$name" $privateProfile
}
$successorBox = Join-Path $root 'successor-box.exe'
Copy-Item $box $successorBox
$unwind = Join-Path (Split-Path $box) 'libunwind.dll'
if (Test-Path -LiteralPath $unwind) { Copy-Item -LiteralPath $unwind $root }

Copy-Item $holderFixture (Join-Path $bin 'kuvo-box.exe')
Set-Content -Encoding Ascii (Join-Path (Split-Path $bin) 'box.json') '{"kuvo":"box/1","version":"0.8.0-beta.1","launch":"rust"}'
function Starts($name,$arguments) {
    $log = Join-Path $run ([guid]::NewGuid().ToString('N'))
    $child = Start-Process -FilePath $name -ArgumentList $arguments -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput "$log.out" -RedirectStandardError "$log.err" -PassThru
    return $child
}
function ReadNumber($name) {
    $path = Join-Path $run $name
    if (Test-Path $path) { return [int](Get-Content $path -ErrorAction SilentlyContinue) }
    return 0
}
function Until($condition,$description) {
    $deadline = [datetime]::UtcNow.AddSeconds(20)
    while (-not (& $condition)) {
        if ([datetime]::UtcNow -gt $deadline) { throw $description }
        Start-Sleep -Milliseconds 80
    }
}
function AssertAlive($child) {
    $child.Refresh()
    if ($child.HasExited) { throw "owned PID $($child.Id) already exited" }
}
function SignalTurn($number) {
    Set-Content -Encoding Ascii (Join-Path $run 'test-turn') $number
    Until { (ReadNumber 'test-answer') -eq $number } "holder never replied to turn $number"
}
$keepers = @()
$holder = $null
try {
    & $box keep select $target
    if ($LASTEXITCODE) { throw 'initial selection failed' }
    $first = Starts $box @('keep')
    $keepers += $first
    Until { (ReadNumber 'test-holder.pid') -gt 1 -and (ReadNumber 'test-resident.pid') -gt 1 } 'stand-in resident did not start the holder'
    $holder = Get-Process -Id (ReadNumber 'test-holder.pid')
    $resident = Get-Process -Id (ReadNumber 'test-resident.pid')
    SignalTurn 1

    $successor = Starts $successorBox @('keep','handover')
    $keepers += $successor
    if (-not $first.WaitForExit(16000) -or $first.ExitCode) { throw 'predecessor did not yield cleanly' }
    Until { (ReadNumber 'box.pid') -eq $successor.Id } 'successor never became the keeper'
    AssertAlive $resident
    AssertAlive $holder
    SignalTurn 2
    "planned handover preserved resident $($resident.Id) and holder $($holder.Id)"

    $successor.Kill()
    if (-not $successor.WaitForExit(5000) -or -not $resident.WaitForExit(5000)) { throw 'closing the last job handle did not terminate the resident' }
    AssertAlive $holder
    SignalTurn 3
    $replacement = Starts $successorBox @('keep')
    $keepers += $replacement
    Until { (ReadNumber 'test-adopted.pid') -eq $holder.Id } 'replacement resident did not find the existing holder'
    SignalTurn 4
    "abrupt keeper loss stopped the resident, preserved holder $($holder.Id), and the replacement re-adopted the holder"

    & $box keep off
    if ($LASTEXITCODE -or -not $replacement.WaitForExit(16000) -or $replacement.ExitCode) { throw 'off did not stop the replacement' }
} catch {
    Get-ChildItem $run | Select-Object Name,Length | Format-Table | Out-Host
    Get-ChildItem $run -Filter '*.err' | ForEach-Object { Get-Content $_.FullName }
    if (Test-Path (Join-Path $run 'serve.log')) { Get-Content (Join-Path $run 'serve.log') }
    throw
} finally {
    foreach ($keeper in $keepers) {
        $keeper.Refresh()
        if (-not $keeper.HasExited) {
            $keeper.Kill()
            $keeper.WaitForExit()
        }
    }
    if (-not $holder) {
        $holderPid = ReadNumber 'test-holder.pid'
        if ($holderPid -gt 1) {
            $candidate = Get-Process -Id $holderPid -ErrorAction SilentlyContinue
            if ($candidate -and $candidate.Path -eq (Join-Path $bin 'kuvo-box.exe')) {
                $holder = $candidate
            }
        }
    }
    if ($holder) {
        $holder.Refresh()
        if (-not $holder.HasExited) {
            $holder.Kill()
            $holder.WaitForExit()
        }
    }
    Remove-Item -LiteralPath $root -Recurse -Force
}
