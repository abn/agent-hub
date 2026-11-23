# Install, upgrade or remove Agent Hub for the current user on Windows.
#
#   irm https://hub.agync.dev/install.ps1 | iex
#   & ([scriptblock]::Create((irm https://hub.agync.dev/install.ps1))) -Version v1.13.0
#
# Exact exit codes need a file:
#
#   irm https://hub.agync.dev/install.ps1 -OutFile install.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 [-Flags]
#
# The script downloads a release archive, checks it, places the binary and
# hands over to `agent-hub service install`. The binary owns the lifecycle.
# Options also come from AGENT_HUB_VERSION, AGENT_HUB_BIN_DIR and
# AGENT_HUB_NO_SERVICE=1. AGENT_HUB_DOWNLOAD_BASE overrides the release
# download URL and AGENT_HUB_LATEST_URL the page whose redirect names the
# latest release.
#
# Exit, when run as a file: 0 done, 1 download, checksum, provenance or
# service failure, 2 usage, 3 unsupported platform (including a release with
# no Windows build) or a release without service commands. Run through `iex` or a script block, a failure is a
# thrown error instead (the shell stays open, `powershell -Command` exits 1)
# and the code is in $LASTEXITCODE.
param(
    [string]$Version = $env:AGENT_HUB_VERSION,
    [string]$BinDir = $env:AGENT_HUB_BIN_DIR,
    [switch]$NoService,
    [string]$Bind,
    [switch]$Boot,
    [switch]$Login,
    [switch]$RequireProvenance,
    [switch]$Force,
    [switch]$Uninstall,
    [switch]$Purge,
    [switch]$Yes,
    [switch]$AllowAdmin
)

function Stop-Install([int]$Code, [string]$Message) {
    $e = New-Object System.Exception($Message)
    $e.Data['code'] = $Code
    throw $e
}

# Run a native program and leave its exit code in $script:NativeRc. Its stderr
# never becomes a PowerShell error, whatever $ErrorActionPreference says, and
# its output is not piped, so a prompt without a newline still shows.
function Invoke-Native([string]$Path, [string[]]$Arguments, [switch]$Quiet) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        if ($Quiet) { & $Path @Arguments *> $null } else { & $Path @Arguments }
        $script:NativeRc = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $prev
    }
}

# "1.13.0" for "agent-hub 1.13.0 (abc123)", $null when absent or not runnable.
function Get-ExeVersion([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $line = $null
    try {
        $line = (& $Path --version 2>$null | Select-Object -First 1)
    } catch {
        return $null
    } finally {
        $ErrorActionPreference = $prev
    }
    if ($line -match '^agent-hub (\S+)') { return $Matches[1] }
    return $null
}

# Releases before the native lifecycle have no service commands, and
# `service --help` is the cheapest probe that tells.
function Assert-HasService([string]$Path, [string]$Want) {
    Invoke-Native $Path @('service', '--help') -Quiet
    if ($script:NativeRc -ne 0) {
        Stop-Install 3 "agent-hub $Want has no service commands, which came after v1.12.0; install a newer release, or pass -NoService to place the binary only"
    }
}

function Get-ServiceArgs {
    $a = @('service', 'install', '--yes')
    if ($Bind) { $a += @('--bind', $Bind) }
    if ($Boot) { $a += '--boot' }
    if ($Login) { $a += '--login' }
    return $a
}

# v, a digit, then only letters, digits and . _ + -
function Test-ReleaseTag([string]$Tag) {
    return [bool]($Tag -cmatch '^v[0-9][0-9A-Za-z._+-]*\z')
}

function Get-Redirect([string]$Url) {
    $req = [System.Net.HttpWebRequest]::Create($Url)
    $req.Method = 'HEAD'
    $req.AllowAutoRedirect = $false
    try {
        $res = $req.GetResponse()
    } catch [System.Net.WebException] {
        $res = $_.Exception.Response
        if ($null -eq $res) { throw }
    }
    try { return $res.Headers['Location'] } finally { $res.Close() }
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

# $MarkerPath records the entry this script added, and only when it adds one:
# agent-hub uninstall takes an entry off the PATH only when the marker names it,
# so a directory the user already had on the PATH is never touched.
function Add-UserPath([string]$Dir, [string]$MarkerPath) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        # Read unexpanded and write back as an expandable string, so a
        # %VAR% entry the user keeps in Path survives.
        $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $parts = @()
        if ($current) { $parts = @($current.Split(';') | Where-Object { $_ }) }
        $wanted = $Dir.TrimEnd('\')
        foreach ($p in $parts) {
            if ($p.TrimEnd('\') -ieq $wanted) { return }
        }
        $key.SetValue('Path', (($parts + $Dir) -join ';'), [Microsoft.Win32.RegistryValueKind]::ExpandString)
    } finally {
        $key.Close()
    }
    try {
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $MarkerPath) | Out-Null
        [IO.File]::WriteAllText($MarkerPath, $Dir, (New-Object System.Text.UTF8Encoding $false))
    } catch {
        Write-Warning "Could not record the PATH entry ($($_.Exception.Message)); uninstall will leave it on the PATH."
    }
    $env:Path = "$env:Path;$Dir"
    Add-Type -Namespace AgentHub -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint flags, uint timeout, out System.UIntPtr result);
'@
    $out = [UIntPtr]::Zero
    [void][AgentHub.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$out)
    Write-Host "Added $Dir to your user PATH. Open a new terminal to use it."
}

function Install-AgentHub {
    if (-not $env:LOCALAPPDATA) { Stop-Install 2 'LOCALAPPDATA is not set' }
    $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($isAdmin -and -not $AllowAdmin) {
        Stop-Install 2 'this is a per-user install; run it from a normal shell, or pass -AllowAdmin'
    }
    if ($Boot -and $Login) { Stop-Install 2 '-Boot and -Login exclude each other' }
    if ($Purge -and -not $Uninstall) { Stop-Install 2 '-Purge only goes with -Uninstall' }

    $arch = $env:PROCESSOR_ARCHITEW6432
    if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
    switch ($arch) {
        'AMD64' { $target = 'x86_64-pc-windows-msvc' }
        default { Stop-Install 3 "unsupported architecture: $arch. Use WSL: curl -fsSL https://hub.agync.dev/install.sh | sh" }
    }

    $base = $env:AGENT_HUB_DOWNLOAD_BASE
    if (-not $base) { $base = 'https://github.com/agync/agent-hub/releases/download' }
    $latestUrl = $env:AGENT_HUB_LATEST_URL
    if (-not $latestUrl) { $latestUrl = 'https://github.com/agync/agent-hub/releases/latest' }
    foreach ($u in @($base, $latestUrl)) {
        if ($u -notmatch '^https?://') { Stop-Install 2 "not an http or https URL: $u" }
    }
    $base = $base.TrimEnd('/')

    if ($Version) {
        if ($Version -match '^[0-9]') { $Version = "v$Version" }
        if (-not (Test-ReleaseTag $Version)) { Stop-Install 2 "not a release version: $Version" }
    }

    # One absolute directory, so the PATH entry never depends on where the shell is.
    if (-not $BinDir) { $BinDir = Join-Path $env:LOCALAPPDATA 'Programs\agent-hub' }
    if (-not [IO.Path]::IsPathRooted($BinDir)) { $BinDir = Join-Path (Get-Location).Path $BinDir }
    $BinDir = [IO.Path]::GetFullPath($BinDir).TrimEnd('\')
    $stateDir = Join-Path $env:LOCALAPPDATA 'agent-hub\state'
    $cacheDir = Join-Path $env:LOCALAPPDATA 'agent-hub\cache'
    $exe = Join-Path $BinDir 'agent-hub.exe'

    if ($Uninstall) {
        if (-not (Test-Path -LiteralPath $exe)) {
            Write-Host 'agent-hub is not installed, nothing to remove'
            return
        }
        $a = @('uninstall')
        if ($Purge) { $a += '--purge' }
        if ($Yes) { $a += '--yes' }
        Invoke-Native $exe $a
        if ($script:NativeRc -ne 0) { Stop-Install $script:NativeRc 'agent-hub uninstall failed' }
        return
    }

    # TLS 1.2 is not the default on Windows PowerShell 5.1.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }

    # One install per user at a time.
    $mutex = New-Object System.Threading.Mutex($false, 'Local\agent-hub-install')
    $held = $false
    try {
        try {
            $held = $mutex.WaitOne(0)
        } catch [System.Threading.AbandonedMutexException] {
            $held = $true
        }
        if (-not $held) { Stop-Install 1 'another install is running' }
        Install-Locked
    } finally {
        if ($held) { $mutex.ReleaseMutex() }
        $mutex.Dispose()
    }
}

function Install-Locked {
    if (-not $Version) {
        $loc = Get-Redirect $latestUrl
        $abs = $null
        $tag = $null
        if ($loc -and [Uri]::TryCreate([Uri]$latestUrl, $loc, [ref]$abs)) {
            if ($abs.AbsolutePath -cmatch '/tag/agent-hub-(v[^/]+)\z') { $tag = $Matches[1] }
        }
        if (-not $tag -or -not (Test-ReleaseTag $tag)) {
            Stop-Install 1 "could not tell the latest release from the redirect: $loc"
        }
        $Version = $tag
    }
    $want = $Version.Substring(1)

    # Leftovers of an earlier upgrade; the running image could not be removed then.
    Get-ChildItem -LiteralPath $BinDir -Filter 'agent-hub.exe.*.old' -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue

    $noService = $NoService -or $env:AGENT_HUB_NO_SERVICE -eq '1'
    $retry = 'fix the cause, then re-run this installer with the same flags or run: agent-hub service install'

    # The binary is already there. The service step still runs, as it is
    # idempotent, so a failed first run or a changed -Bind takes effect.
    $sameVersion = {
        if ($noService) { return }
        Assert-HasService $exe $want
        Invoke-Native $exe (Get-ServiceArgs)
        if ($script:NativeRc -eq 69) {
            Write-Host 'No service manager is reachable here, so the binary is installed without a service.'
            Write-Host 'Run it with: agent-hub start'
        } elseif ($script:NativeRc -ne 0) {
            Stop-Install 1 "agent-hub service install failed (exit $($script:NativeRc)); $retry"
        }
    }

    $have = Get-ExeVersion $exe
    if ($have -and $have -eq $want -and -not $Force -and -not $RequireProvenance) {
        Write-Host "agent-hub $want is already installed in $BinDir (use -Force to reinstall)"
        & $sameVersion
        return
    }

    $name = "agent-hub-$Version-$target"
    $archive = "$name.zip"
    $rel = "$base/agent-hub-$Version"
    $prevExe = Join-Path $stateDir 'agent-hub.prev.exe'
    $wasRunning = $false

    New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null
    $work = Join-Path $cacheDir ('install-' + [IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $work | Out-Null
    $staged = $null
    $prevTmp = $null
    try {
        Write-Host "Downloading agent-hub $want for $target"
        $ProgressPreference = 'SilentlyContinue'
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$rel/$archive" -OutFile (Join-Path $work $archive)
        } catch {
            $status = $null
            if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode }
            if ($status -eq 404) {
                Stop-Install 3 "release $Version has no Windows build. Use WSL: curl -fsSL https://hub.agync.dev/install.sh | sh"
            }
            Stop-Install 1 "download failed: $rel/$archive ($($_.Exception.Message))"
        }
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$rel/$name.sha256" -OutFile (Join-Path $work "$name.sha256")
        } catch {
            Stop-Install 1 "download failed: $rel/$name.sha256"
        }

        $wantSum = ((Get-Content -LiteralPath (Join-Path $work "$name.sha256") -TotalCount 1) -split '\s+')[0].ToLowerInvariant()
        $gotSum = Get-Sha256 (Join-Path $work $archive)
        if (-not $wantSum) { Stop-Install 1 'the checksum file is empty' }
        if ($wantSum -ne $gotSum) { Stop-Install 1 "checksum mismatch for $archive (expected $wantSum, got $gotSum)" }
        Write-Host 'checksum: ok'

        $gh = Get-Command gh -ErrorAction SilentlyContinue
        if ($gh) {
            Invoke-Native $gh.Source @('attestation', 'verify', (Join-Path $work $archive), '--repo', 'agync/agent-hub') -Quiet
            if ($script:NativeRc -eq 0) {
                Write-Host 'provenance: verified (gh attestation)'
            } elseif ($RequireProvenance) {
                Stop-Install 1 "provenance check failed for $archive"
            } else {
                Write-Warning 'provenance: not verified (gh attestation verify failed)'
            }
        } elseif ($RequireProvenance) {
            Stop-Install 1 '-RequireProvenance needs the gh CLI, which is not installed'
        } else {
            Write-Host 'provenance: not checked (gh not installed)'
        }

        Expand-Archive -LiteralPath (Join-Path $work $archive) -DestinationPath $work -Force
        $newExe = Join-Path $work "$name\agent-hub.exe"
        if (-not (Test-Path -LiteralPath $newExe)) { Stop-Install 1 'the archive has no agent-hub.exe' }
        $newSum = Get-Sha256 $newExe

        # The unpacked binary must be the release that was asked for. A release
        # old enough to print no version at all may still be placed, but only
        # without a service.
        if (-not $noService) { Assert-HasService $newExe $want }
        $got = Get-ExeVersion $newExe
        if (-not $got -and $noService) {
            Write-Warning "agent-hub $want does not report its version"
        } elseif ($got -ne $want) {
            if (-not $got) { $got = 'no version' }
            Stop-Install 1 "the unpacked binary reports $got, not agent-hub $want; refusing to install it"
        }

        # With provenance required, the installed bytes count only if they
        # are the verified release's bytes.
        if ($have -eq $want -and -not $Force -and (Get-Sha256 $exe) -eq $newSum) {
            Write-Host "agent-hub $want is already installed in $BinDir and matches the verified release"
            & $sameVersion
            return
        }

        # Stage the verified binary beside its destination and keep the old
        # one, all before anything running is touched.
        New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
        $staged = Join-Path $BinDir ('.agent-hub.' + [guid]::NewGuid().ToString('N') + '.new')
        Copy-Item -LiteralPath $newExe -Destination $staged
        if ((Get-Sha256 $staged) -ne $newSum) { Stop-Install 1 "the copy in $BinDir does not match the verified binary" }

        if (Test-Path -LiteralPath $exe) {
            try {
                New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
                $prevTmp = Join-Path $stateDir ('.agent-hub.prev.' + [guid]::NewGuid().ToString('N'))
                Copy-Item -LiteralPath $exe -Destination $prevTmp
                Move-Item -LiteralPath $prevTmp -Destination $prevExe -Force
                $prevTmp = $null
            } catch {
                Stop-Install 1 "cannot keep the previous binary in ${stateDir}; nothing was changed ($($_.Exception.Message))"
            }
        }

        # A running hub holds its image open: stop it, swap by rename, start it again.
        if ($have) {
            Invoke-Native $exe @('status') -Quiet
            if ($script:NativeRc -eq 0 -or $script:NativeRc -eq 1) {
                $wasRunning = $true
                Invoke-Native $exe @('stop') -Quiet
                if ($script:NativeRc -ne 0) {
                    Stop-Install 1 "could not stop the running agent-hub (exit $($script:NativeRc)); nothing was changed"
                }
            }
        }

        $old = $null
        try {
            if (Test-Path -LiteralPath $exe) {
                $oldName = "$exe." + [guid]::NewGuid().ToString('N').Substring(0, 8) + '.old'
                [IO.File]::Move($exe, $oldName)
                $old = $oldName
            }
            [IO.File]::Move($staged, $exe)
            $staged = $null
            if (-not (Get-ExeVersion $exe)) { throw 'the new binary does not run' }
        } catch {
            $why = $_.Exception.Message
            $restored = $true
            if ($old -and (Test-Path -LiteralPath $old)) {
                try {
                    Remove-Item -LiteralPath $exe -Force -ErrorAction SilentlyContinue
                    [IO.File]::Move($old, $exe)
                } catch {
                    $restored = $false
                }
            }
            if ($wasRunning -and $restored) { Invoke-Native $exe @('restart') -Quiet }
            if ($restored) { Stop-Install 1 "could not activate the new binary, the previous one is back ($why)" }
            Stop-Install 1 "could not activate the new binary and could not restore ${old} ($why)"
        }
    } finally {
        if ($staged) { Remove-Item -LiteralPath $staged -Force -ErrorAction SilentlyContinue }
        if ($prevTmp) { Remove-Item -LiteralPath $prevTmp -Force -ErrorAction SilentlyContinue }
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }

    # Roll back to the .old copy when PATH or the service step fails.
    $rollback = {
        param($why)
        $restored = $false
        if ($old -and (Test-Path -LiteralPath $old)) {
            try {
                if ($wasRunning) { Invoke-Native $exe @('stop') -Quiet }
                Remove-Item -LiteralPath $exe -Force -ErrorAction SilentlyContinue
                [IO.File]::Move($old, $exe)
                $restored = $true
            } catch {
                $restored = $false
            }
        }
        if ($restored -and $wasRunning) { Invoke-Native $exe @('restart') -Quiet }
        if ($restored) { Stop-Install 1 "$why; the previous binary is back" }
        Stop-Install 1 "$why"
    }

    if ($have) {
        Write-Host "Upgraded agent-hub $have to $want ($exe)"
        Write-Host "Previous binary kept at $prevExe"
    } else {
        Write-Host "Installed agent-hub $want to $exe"
    }
    try {
        Add-UserPath $BinDir (Join-Path $stateDir 'path-added')
    } catch {
        & $rollback "could not register $BinDir on the PATH ($($_.Exception.Message))"
    }

    if ($noService) {
        Write-Host 'Binary only: no service was installed.'
    } else {
        Invoke-Native $exe (Get-ServiceArgs)
        $rc = $script:NativeRc
        if ($rc -eq 69) {
            Write-Host 'No service manager is reachable here, so the binary is installed without a service.'
            Write-Host 'Run it with: agent-hub start'
        } elseif ($rc -ne 0) {
            & $rollback "agent-hub service install failed (exit $rc); $retry"
        }
    }
    # The swap stopped a running hub; leave it running, on the new binary.
    if ($wasRunning) {
        Invoke-Native $exe @('restart')
        if ($script:NativeRc -ne 0) { & $rollback 'agent-hub restart failed' }
    }
    # Success: the previous binary is only kept in the state dir now.
    if ($old) { Remove-Item -LiteralPath $old -Force -ErrorAction SilentlyContinue }
}

$prevPreference = $ErrorActionPreference
$ErrorActionPreference = 'Stop'
$failed = $false
$code = 0
$message = ''
try {
    Install-AgentHub
} catch {
    $failed = $true
    $code = 1
    $message = $_.Exception.Message
    if ($_.Exception.Data.Contains('code')) { $code = [int]$_.Exception.Data['code'] }
} finally {
    $ErrorActionPreference = $prevPreference
}
$global:LASTEXITCODE = $code
if ($failed) {
    if ($PSCommandPath) {
        [Console]::Error.WriteLine("error: $message")
        exit $code
    }
    # Under `irm | iex` an exit would close the user's shell: throw instead.
    throw "agent-hub install failed (exit $code): $message"
}
