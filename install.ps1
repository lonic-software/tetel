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
#
# Note: tetel runs git to pin observations to a commit and grep for its
# searches. Git for Windows provides both; its grep lives in usr\bin, which
# Git Bash puts on PATH and a plain PowerShell does not.

$ErrorActionPreference = "Stop"

$Repo = if ($env:TETEL_REPO) { $env:TETEL_REPO } else { "lonic-software/tetel" }
$Version = if ($env:TETEL_VERSION) { $env:TETEL_VERSION } else { "latest" }
$InstallDir = if ($env:TETEL_INSTALL_DIR) { $env:TETEL_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\tetel" }

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

    # Best effort: verify against checksums.txt. Only the fetch is in the
    # catch - a missing file throws WebException on Windows PowerShell but
    # HttpResponseException on pwsh 7, and either way we skip. A genuine
    # mismatch throws below, outside the catch, so it still aborts.
    $Sums = $null
    try {
        $Sums = (Invoke-WebRequest "$Base/checksums.txt" -UseBasicParsing).Content -split "`n"
    } catch {
        Write-Host "warning: could not fetch checksums.txt; skipping verification"
    }
    if ($Sums) {
        $line = $Sums | Where-Object { $_ -match "\s$([regex]::Escape($Asset))\s*$" } | Select-Object -First 1
        if (-not $line) { throw "$Asset missing from checksums.txt" }
        $expected = ($line -split '\s+')[0]
        $actual = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower()
        if ($expected -ne $actual) { throw "checksum verification FAILED - refusing to install" }
        Write-Host "checksum ok"
    }

    Expand-Archive $Zip -DestinationPath $Tmp
    $New = Join-Path $Tmp "tetel.exe"
    if (-not (Test-Path $New)) { throw "archive did not contain tetel.exe" }

    New-Item -ItemType Directory -Force $InstallDir | Out-Null
    $Dest = Join-Path $InstallDir "tetel.exe"
    $Upgrading = Test-Path $Dest
    # A running `tetel mcp` server locks tetel.exe against overwrite but not
    # against rename, so move it aside first; the server keeps the old file.
    if ($Upgrading) {
        Remove-Item "$Dest.old" -Force -ErrorAction SilentlyContinue
        Move-Item $Dest "$Dest.old" -Force
    }
    Copy-Item $New $Dest -Force

    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $Entries = if ($UserPath) { $UserPath -split ';' } else { @() }
    if ($Entries -notcontains $InstallDir) {
        $NewPath = if ($UserPath) { "$UserPath;$InstallDir" } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
        Write-Host "added $InstallDir to your user PATH (restart your terminal)"
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
    if ($Upgrading) {
        Write-Host "Upgraded. A 'tetel mcp' server that was already running keeps the old build"
        Write-Host "and refuses tool calls until restarted - restart Claude Code to pick this one up."
    } else {
        Write-Host "To use it from Claude Code:"
        Write-Host "    claude mcp add --scope user tetel -- tetel mcp"
    }
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
