param([string]$run, [string]$tools, [string]$config)
$ErrorActionPreference = 'Stop'
$system = Join-Path $env:SystemRoot 'System32'
$repo = Join-Path $run 'repo'
$pins = Get-Content -Raw -LiteralPath "$repo\.windows-checks\pins.json" | ConvertFrom-Json

# Only OS variables needed to load native executables survive. No user tokens, agent paths,
# SSH sockets, KUVO_* settings, or per-user tool configuration reach the tests.
foreach ($name in [Environment]::GetEnvironmentVariables().Keys) {
    if ($name -notin @('SystemRoot', 'SystemDrive', 'windir', 'ComSpec', 'PATHEXT')) {
        [Environment]::SetEnvironmentVariable($name, $null, 'Process')
    }
}
$env:PATH = $system
$homeDir = Join-Path $run 'home'
$env:WINDOWS_TEST_HOME = $homeDir
# This PC's designated disposable account. A host override still has to log on as this account.
$env:KUVO_TEST_DISPOSABLE_ACCOUNT = 'kuvo'
$env:HOME = $homeDir
$env:USERPROFILE = $homeDir
$env:APPDATA = Join-Path $homeDir 'appdata'
$env:LOCALAPPDATA = Join-Path $homeDir 'localdata'
$env:TEMP = Join-Path $homeDir 'temp'
$env:TMP = $env:TEMP
$env:TMPDIR = $env:TEMP
$env:XDG_CONFIG_HOME = Join-Path $homeDir 'config'
$env:XDG_CACHE_HOME = Join-Path $homeDir 'cache'
$env:XDG_DATA_HOME = Join-Path $homeDir 'data'
$env:XDG_STATE_HOME = Join-Path $homeDir 'state'
$env:XDG_RUNTIME_DIR = Join-Path $homeDir 'runtime'
$env:BUN_INSTALL_CACHE_DIR = Join-Path $env:XDG_CACHE_HOME 'bun'
$env:BUN_RUNTIME_TRANSPILER_CACHE_PATH = '0'
$env:npm_config_cache = Join-Path $env:XDG_CACHE_HOME 'npm'
$env:UV_CACHE_DIR = Join-Path $env:XDG_CACHE_HOME 'uv'
foreach ($dir in @($env:APPDATA, $env:LOCALAPPDATA, $env:TEMP, $env:XDG_CONFIG_HOME, $env:XDG_CACHE_HOME, $env:XDG_DATA_HOME, $env:XDG_STATE_HOME, $env:XDG_RUNTIME_DIR)) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
}

function Ensure-Directory($dir) {
    if (!(Test-Path -LiteralPath $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
    if ((Get-Item -LiteralPath $dir -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Refusing linked directory: $dir"
    }
}

Ensure-Directory $tools
function Install-Tool($name, $version, $url, $sha, $top) {
    $target = Join-Path $tools "$name-$version"
    if (Test-Path -LiteralPath $target) {
        Ensure-Directory $target
    } else {
        $archive = Join-Path $run "$name.zip"
        & "$system\curl.exe" -q -fsSL $url -o $archive
        if ($LASTEXITCODE -ne 0) { throw "$name download failed ($LASTEXITCODE)." }
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $sha) {
            throw "Wrong SHA256 for $name $version."
        }
        & "$system\tar.exe" -xf $archive -C $run
        if ($LASTEXITCODE -ne 0) { throw "$name extraction failed ($LASTEXITCODE)." }
        try { [IO.Directory]::Move((Join-Path $run $top), $target) } catch {
            if (!(Test-Path -LiteralPath $target)) { throw }
            Ensure-Directory $target
        }
        Remove-Item -LiteralPath $archive
    }
    $binary = Join-Path $target "$name.exe"
    $file = Get-Item -LiteralPath $binary -Force
    if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Refusing linked runtime: $binary"
    }
    return $binary
}

# Only one run may own the account's credential baseline at a time.
$lock = [IO.File]::Open((Join-Path $tools '.native-run.lock'), 'OpenOrCreate', 'ReadWrite', 'None')
$status = 1
$baselineEmpty = $false
$powershell = Join-Path $system 'WindowsPowerShell\v1.0\powershell.exe'
$credentials = Join-Path $repo 'box\bridge-tests\windows-test-credentials.ps1'
try {
    & $credentials -Action define
    $baselineEmpty = [TestCredentials]::BeginRun()
    if (!$baselineEmpty) { throw 'Windows Credential Manager already holds credentials. Use an empty disposable account.' }

    $bunVersion = (Get-Content (Join-Path $repo 'box/.bun-version') -Raw).Trim()
    $nodeVersion = $pins.node.version
    $bun = Install-Tool bun $bunVersion "https://github.com/oven-sh/bun/releases/download/bun-v$bunVersion/bun-windows-x64.zip" $pins.bun.'windows-x64' 'bun-windows-x64'
    $node = Install-Tool node $nodeVersion "https://nodejs.org/dist/v$nodeVersion/node-v$nodeVersion-win-x64.zip" $pins.node.'windows-x64' "node-v$nodeVersion-win-x64"
    $git = "$env:SystemDrive\Program Files\Git"
    $env:PATH = "$(Split-Path -Parent $bun);$(Split-Path -Parent $node);$system;$git\cmd;$git\bin;$git\usr\bin"
    $env:GIT_CONFIG_NOSYSTEM = '1'
    Set-Location -LiteralPath "$repo\box"
    & $powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$repo\.windows-checks\job.ps1" $node "$repo\.windows-checks\run.mjs" $config
    $status = $LASTEXITCODE
} finally {
    try {
        if ($baselineEmpty) {
            try {
                $removed = [TestCredentials]::EndRun()
                Write-Output "Credential Manager: removed $removed run entries."
            } catch {
                Write-Output "Credential Manager cleanup failed: $_"
                if ($status -eq 0) { $status = 1 }
            }
        }
    } finally {
        $lock.Dispose()
    }
}
exit $status
