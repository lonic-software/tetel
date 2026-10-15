# tetel installer for Windows (PowerShell).
#
#   irm https://raw.githubusercontent.com/lonic-software/tetel/main/install.ps1 | iex
#
# Environment overrides:
#   TETEL_VERSION      install a specific tag, e.g. v0.1.0 (default: latest release)
#   TETEL_INSTALL_DIR  where to put the binary (default: %LOCALAPPDATA%\Programs\tetel)
#   TETEL_REPO         GitHub repo slug        (default: lonic-software/tetel)
#   TETEL_BASE_URL     full base URL for the assets (mirrors / air-gapped setups);
#                      overrides TETEL_REPO/TETEL_VERSION entirely
#   TETEL_NO_VERIFY    set to 1 to install without a checksums.txt to verify against
#
# Note: tetel runs git to pin observations to a commit and grep for its
# searches. Git for Windows provides both; its grep lives in usr\bin, which
# Git Bash puts on PATH and a plain PowerShell does not.

# In a script block so that `irm | iex`, which runs in the caller's scope,
# leaves neither these variables nor the preferences behind in the session.
& {
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # the progress bar slows IWR down badly on 5.1
# Windows PowerShell 5.1 on older .NET defaults offers no TLS 1.2, which GitHub requires.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Repo = if ($env:TETEL_REPO) { $env:TETEL_REPO } else { "lonic-software/tetel" }
$Version = if ($env:TETEL_VERSION) { $env:TETEL_VERSION } else { "latest" }
$InstallDir = if ($env:TETEL_INSTALL_DIR) { $env:TETEL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\tetel" }
# Absolute, against the PowerShell location (not the process directory),
# since it is written into the persistent user PATH. A drive root keeps its
# backslash: `C:` alone means the current directory on C.
$InstallDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($InstallDir)
if ($InstallDir.Length -gt 3) { $InstallDir = $InstallDir.TrimEnd('\') }

if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -eq "Arm64") {
    throw "no Windows ARM build yet - build from source: cargo install --path ."
}

$Asset = "tetel-x86_64-pc-windows-msvc.zip"
$Base = if ($env:TETEL_BASE_URL) {
    $env:TETEL_BASE_URL
} elseif ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory $Tmp | Out-Null
try {
    $Zip = Join-Path $Tmp $Asset
    Write-Host "downloading $Base/$Asset"
    Invoke-WebRequest "$Base/$Asset" -OutFile $Zip -UseBasicParsing

    # Saved to a file and read back as text: GitHub serves release assets as
    # application/octet-stream, for which IWR's .Content is a byte array.
    $SumsFile = Join-Path $Tmp "checksums.txt"
    $Fetched = $true
    try {
        Invoke-WebRequest "$Base/checksums.txt" -OutFile $SumsFile -UseBasicParsing
    } catch {
        $Fetched = $false
        if ($env:TETEL_NO_VERIFY -ne "1") {
            throw "could not fetch $Base/checksums.txt ($($_.Exception.Message)) - refusing to install unverified; set TETEL_NO_VERIFY=1 to override"
        }
        Write-Host "warning: no checksums.txt; installing UNVERIFIED (TETEL_NO_VERIFY=1)"
    }
    if ($Fetched) {
        $line = Get-Content $SumsFile | Where-Object { $_ -match "\s\*?$([regex]::Escape($Asset))\s*$" } | Select-Object -First 1
        if (-not $line) { throw "$Asset missing from checksums.txt" }
        $expected = ($line -split '\s+')[0]
        $actual = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) { throw "checksum verification FAILED - refusing to install" }
        Write-Host "checksum ok"
    }

    Expand-Archive $Zip -DestinationPath $Tmp
    $New = Join-Path $Tmp "tetel.exe"
    if (-not (Test-Path $New)) { throw "archive did not contain tetel.exe" }

    # Stage beside the destination, then rename into place. A running
    # `tetel mcp` locks tetel.exe against overwrite but not against rename,
    # so an existing one is renamed aside first, to a fresh name: an earlier
    # aside may itself still be running. If the final rename fails, the
    # aside goes back, so a failed upgrade never leaves no tetel.exe.
    New-Item -ItemType Directory -Force $InstallDir | Out-Null
    $Dest = Join-Path $InstallDir "tetel.exe"
    $Staged = Join-Path $InstallDir ".tetel.new.$PID.exe"
    Copy-Item $New $Staged -Force
    $Aside = $null
    try {
        if (Test-Path $Dest) {
            $Aside = "$Dest.old-$([DateTime]::UtcNow.Ticks)"
            Move-Item $Dest $Aside
        }
        Move-Item $Staged $Dest
    } catch {
        $Failure = $_
        Remove-Item $Staged -Force -ErrorAction SilentlyContinue
        if ($Aside -and (Test-Path $Aside)) {
            try { Move-Item $Aside $Dest } catch {
                throw "could not install $Dest ($Failure), nor restore the previous build - it is at $Aside; rename it back to tetel.exe"
            }
        }
        throw
    }
    # Best effort: an aside that cannot be removed (a server still runs from
    # it) is left for a later install to clean up.
    Get-ChildItem $InstallDir -Filter "tetel.exe.old-*" |
        Where-Object { $_.FullName -ne $Aside } |
        Remove-Item -Force -ErrorAction SilentlyContinue

    # Edit the user PATH as stored, not as expanded: reading it through
    # GetEnvironmentVariable and writing it back would flatten %VAR% entries
    # and turn the value from REG_EXPAND_SZ into REG_SZ.
    $Key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
    try {
        $Raw = $Key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $Kind = if ($Raw) { $Key.GetValueKind("Path") } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
        $Entries = @($Raw -split ';' | Where-Object { $_ } |
            ForEach-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') })
        if ($Entries -notcontains $InstallDir.TrimEnd('\')) {
            $NewPath = if ($Raw) { "$($Raw.TrimEnd(';'));$InstallDir" } else { $InstallDir }
            $Key.SetValue("Path", $NewPath, $Kind)
            # A registry write tells no one; this round-trip makes Windows
            # broadcast the environment change so new terminals see it.
            [Environment]::SetEnvironmentVariable("TETEL_INSTALL_NOTIFY", "1", "User")
            [Environment]::SetEnvironmentVariable("TETEL_INSTALL_NOTIFY", $null, "User")
            Write-Host "added $InstallDir to your user PATH (restart your terminal)"
        }
    } finally {
        $Key.Close()
    }

    $Ver = try { & $Dest --version } catch { "tetel" }
    Write-Host "installed $Dest - $Ver"

    # Runtime dependencies - warn specifically rather than a blanket reminder.
    $MissingDeps = @("git", "grep") | Where-Object { -not (Get-Command $_ -ErrorAction SilentlyContinue) }
    if ($MissingDeps) {
        Write-Host ""
        Write-Host "warning: $($MissingDeps -join ' and ') not found on PATH. tetel runs git to"
        Write-Host "pin observations to a commit and grep for its searches. Git for Windows"
        Write-Host "provides both (https://gitforwindows.org) - install it and either run"
        Write-Host "tetel from Git Bash or add its usr\bin directory to your PATH."
    }

    Write-Host ""
    if ($Aside) {
        Write-Host "Upgraded. A 'tetel mcp' server that was already running keeps the old build"
        Write-Host "and refuses tool calls until restarted - restart Claude Code to pick this one up."
    } else {
        Write-Host "To use it from Claude Code:"
        Write-Host "    claude mcp add --scope user tetel -- tetel mcp"
    }
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
}
