# The Windows install and service e2e, for a CI runner: the real install.ps1
# against a local release made from the binary under test, then the lifecycle
# through a real Task Scheduler task. It installs into the runner's own profile
# under the production names, so it refuses to start where an agent-hub is
# already installed or running, and outside CI unless AGENT_HUB_E2E_DISPOSABLE=1
# says the profile is disposable. Whatever it installed is removed on any exit,
# and the user PATH is put back.
#
#   windows.ps1 [-Bin PATH] [-ReleaseDir DIR]
#
# Without -ReleaseDir the release is built from -Bin. With it, DIR is a served
# release root (DIR\agent-hub-vX.Y.Z\...) that holds the zip to prove, and -Bin
# is only asked for its version.
param(
    [string]$Bin,
    [string]$ReleaseDir
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = (Resolve-Path (Join-Path $here '..\..\..')).Path
if (-not $Bin) { $Bin = Join-Path $repo 'target\release\agent-hub.exe' }
if (-not (Test-Path -LiteralPath $Bin)) { throw "no binary at $Bin" }

$exe = Join-Path $env:LOCALAPPDATA 'Programs\agent-hub\agent-hub.exe'
$dataRoot = Join-Path $env:LOCALAPPDATA 'agent-hub'

function Test-Ready {
    try {
        $r = Invoke-WebRequest -UseBasicParsing -TimeoutSec 3 -Uri 'http://127.0.0.1:8080/readyz'
        return ($r.StatusCode -eq 200)
    } catch { return $false }
}
function Task-Exists {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & schtasks.exe /Query /TN 'Agent Hub' *> $null; return ($LASTEXITCODE -eq 0) }
    finally { $ErrorActionPreference = $prev }
}

# The test uses the production task name, install path and port, so it must
# never meet a real install.
if (($env:CI -ne 'true') -and ($env:AGENT_HUB_E2E_DISPOSABLE -ne '1')) {
    throw 'windows.ps1 installs and purges agent-hub in this profile: run it in CI, or set AGENT_HUB_E2E_DISPOSABLE=1 on a disposable machine'
}
if ((Task-Exists) -or (Test-Path -LiteralPath $exe) -or (Test-Path -LiteralPath $dataRoot) -or (Test-Ready)) {
    throw 'an agent-hub install, task, data directory or running hub already exists here; refusing to touch it'
}

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$savedEnv = @{}
foreach ($k in 'AGENT_HUB_DOWNLOAD_BASE', 'AGENT_HUB_LATEST_URL', 'PATH') { $savedEnv[$k] = [Environment]::GetEnvironmentVariable($k, 'Process') }

$work = Join-Path $repo ('target\tmp\install-e2e-' + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force -Path $work | Out-Null
$server = $null
$script:fails = 0

function Pass([string]$Name) { Write-Host "ok   $Name" }
function Fail([string]$Name) { Write-Host "FAIL $Name"; $script:fails++ }
function Check([string]$Name, [scriptblock]$Test) {
    $ok = $false
    try { $ok = [bool](& $Test) } catch { $ok = $false }
    if ($ok) { Pass $Name } else { Fail $Name }
}

# Run a native program: its exit code lands in $script:rc and its output in
# $script:out, and stderr never turns into a PowerShell error.
function Run-Native([string]$Path, [string[]]$Arguments) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $script:out = (& $Path @Arguments 2>&1 | Out-String)
        $script:rc = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
}

function Wait-Until([scriptblock]$Cond, [int]$Seconds = 40) {
    $end = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $end) {
        if (& $Cond) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}
function Install-Script([string[]]$More) {
    $ps = Join-Path $repo 'deploy\install.ps1'
    # An elevated CI shell would be refused without -AllowAdmin.
    Run-Native 'powershell.exe' (@('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $ps, '-Yes', '-AllowAdmin') + $More)
    Write-Host $script:out
}

try {
    Run-Native $Bin @('--version')
    if ($script:out -notmatch '^agent-hub (\S+)') { throw "no version from $Bin" }
    $version = $Matches[1]
    if ($ReleaseDir) {
        # The zip under test was packaged by the caller.
        $root = (Resolve-Path -LiteralPath $ReleaseDir).Path
    } else {
        # The release: the zip binaries-windows.yml makes, with its checksum.
        $name = "agent-hub-v$version-x86_64-pc-windows-msvc"
        $root = Join-Path $work 'release'
        $rel = Join-Path $root "agent-hub-v$version"
        $stage = Join-Path $work "stage\$name"
        New-Item -ItemType Directory -Force -Path $rel, $stage | Out-Null
        Copy-Item -LiteralPath $Bin -Destination (Join-Path $stage 'agent-hub.exe')
        $zip = Join-Path $rel "$name.zip"
        Compress-Archive -Path $stage -DestinationPath $zip
        $sum = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLowerInvariant()
        Set-Content -Encoding ascii -LiteralPath (Join-Path $rel "$name.sha256") -Value "$sum  $name.zip"
    }

    $server = Start-Process -PassThru -WindowStyle Hidden -FilePath python -ArgumentList @(
        '-I', '-m', 'http.server', '8099', '--bind', '127.0.0.1', '--directory', $root)
    # Downloads, latest-version discovery and provenance all stay on this
    # machine: a gh that accepts only the exact attestation call replaces the
    # real one.
    $env:AGENT_HUB_DOWNLOAD_BASE = 'http://127.0.0.1:8099'
    $env:AGENT_HUB_LATEST_URL = 'http://127.0.0.1:8099/releases/latest'
    $stubs = Join-Path $work 'stubs'
    New-Item -ItemType Directory -Force -Path $stubs | Out-Null
    Set-Content -Encoding ascii -LiteralPath (Join-Path $stubs 'gh.cmd') -Value @(
        '@echo off',
        'if not "%1 %2"=="attestation verify" exit /b 64',
        'if not exist "%3" exit /b 65',
        'if not "%4 %5"=="--repo agync/agent-hub" exit /b 67',
        'exit /b 0')
    $env:PATH = "$stubs;$env:PATH"
    if (-not (Wait-Until { try { (Invoke-WebRequest -UseBasicParsing -TimeoutSec 2 -Uri 'http://127.0.0.1:8099/').StatusCode -eq 200 } catch { $false } } 20)) {
        throw 'the release server did not start'
    }


    # 1. A first install registers the task, starts it and answers.
    Install-Script @('-Version', "v$version")
    Check 'install.ps1 exits 0' { $script:rc -eq 0 }
    Check 'the binary is placed' { Test-Path -LiteralPath $exe }
    Check 'the task is registered' { Task-Exists }
    Check 'the hub answers /readyz' { Wait-Until { Test-Ready } }
    Run-Native $exe @('status')
    Check 'status says running' { $script:rc -eq 0 }

    # 2. The same version is a no-op; -Force reinstalls over a running hub.
    Install-Script @('-Version', "v$version")
    Check 'the same version is a no-op' { $script:out -match 'already installed' }
    Install-Script @('-Version', "v$version", '-Force')
    Check 'a forced reinstall exits 0' { $script:rc -eq 0 }
    Check 'the hub answers after a forced reinstall' { Wait-Until { Test-Ready } }

    # 3. stop checkpoints through the stop event, not TerminateProcess.
    Run-Native $exe @('stop')
    Check 'stop checkpoints the store' { ($script:rc -eq 0) -and ($script:out -match 'store checkpointed') }
    Check 'the hub is down after stop' { Wait-Until { -not (Test-Ready) } }
    Run-Native $exe @('status')
    Check 'status exits 3 when stopped' { $script:rc -eq 3 }

    # 4. The task starts without a logon: schtasks /Run, then the CLI verbs.
    Run-Native 'schtasks.exe' @('/Run', '/TN', 'Agent Hub')
    Check 'schtasks /Run exits 0' { $script:rc -eq 0 }
    Check 'the hub answers after schtasks /Run' { Wait-Until { Test-Ready } }
    Run-Native $exe @('stop')
    Check 'stop checkpoints again' { ($script:rc -eq 0) -and ($script:out -match 'store checkpointed') }
    Run-Native $exe @('start')
    Check 'start exits 0' { $script:rc -eq 0 }
    Check 'the hub answers after start' { Wait-Until { Test-Ready } }
    Run-Native $exe @('restart')
    Check 'restart exits 0' { $script:rc -eq 0 }
    Check 'the hub answers after restart' { Wait-Until { Test-Ready } }

    # 5. A hard kill of the hub, not of its supervisor (both run the same
    # exe), is restarted by the supervisor. The hub's pid is in its record.
    $record = Join-Path $dataRoot 'data\hub.pid.json'
    $hubPid = { try { (Get-Content -LiteralPath $record -Raw | ConvertFrom-Json).pid } catch { $null } }
    $old = & $hubPid
    if ($old) {
        Stop-Process -Id $old -Force
        Check 'the hub is back after a hard kill' {
            Wait-Until { (Test-Ready) -and ((& $hubPid) -as [int]) -and ((& $hubPid) -ne $old) } 90
        }
    } else {
        Fail 'no hub record to read a pid from'
    }

    # 6. Uninstall with purge leaves no task, no binary and no data.
    Install-Script @('-Uninstall', '-Purge')
    Check 'uninstall exits 0' { $script:rc -eq 0 }
    Check 'the task is gone' { -not (Task-Exists) }
    Check 'the hub is down' { Wait-Until { -not (Test-Ready) } }
    # The running image removes itself after it exits.
    Check 'the binary is gone' { Wait-Until { -not (Test-Path -LiteralPath $exe) } 30 }
    Check 'the data directory is gone' { -not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'agent-hub\data\hub.db')) }
} finally {
    # Whatever an early exit left behind: the task, its process, the files and
    # the PATH entry. The guard above means all of it is this run's own.
    if (Task-Exists) {
        & schtasks.exe /End /TN 'Agent Hub' *> $null
        & schtasks.exe /Delete /TN 'Agent Hub' /F *> $null
    }
    Get-Process -Name 'agent-hub' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $exe } | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
    Remove-Item -Recurse -Force -LiteralPath (Split-Path -Parent $exe) -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force -LiteralPath $dataRoot -ErrorAction SilentlyContinue
    [Environment]::SetEnvironmentVariable('Path', $userPath, 'User')
    foreach ($k in $savedEnv.Keys) { [Environment]::SetEnvironmentVariable($k, $savedEnv[$k], 'Process') }
    if ($server) { Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item -Recurse -Force -LiteralPath $work -ErrorAction SilentlyContinue
}

Write-Host ''
Write-Host "$($script:fails) failed"
exit $(if ($script:fails -eq 0) { 0 } else { 1 })
