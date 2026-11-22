#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Installer for fusibile on Windows.

.DESCRIPTION
    Downloads the latest (or a specified) fusibile release for Windows from
    GitHub, verifies its checksum, extracts the binary into an install
    directory and adds it to the current user's PATH.

.PARAMETER Version
    The fusibile version to install (defaults to the latest released version).

.PARAMETER InstallDir
    The directory the fusibile.exe binary is installed into.
    Defaults to "$env:LOCALAPPDATA\Programs\fusibile".

.PARAMETER Force
    Skip the confirmation prompt during installation. Alias: -Yes.

.EXAMPLE
    irm https://remotefs-rs.github.io/remotefs-rs-fuse/install.ps1 | iex

.EXAMPLE
    .\install.ps1 -Version 1.0.0 -Force
#>
[CmdletBinding()]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSAvoidUsingWriteHost', '', Justification = 'Colored console output is the point of an interactive installer script')]
[Diagnostics.CodeAnalysis.SuppressMessageAttribute('PSReviewUnusedParameter', '', Justification = '$InstallDir and $Force are read from script scope inside Install-Fusibile and Confirm-Action')]
param(
    [string]$Version = "",
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\fusibile",
    [Alias("Yes")]
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$GithubRepo = "remotefs-rs/remotefs-rs-fuse"
$IssuesUrl = "https://github.com/$GithubRepo/issues/new"

# -- output helpers ----------------------------------------------------------

function Write-Info {
    param([string]$Message)
    Write-Host "> " -ForegroundColor DarkGray -NoNewline
    Write-Host $Message
}

function Write-Warn {
    param([string]$Message)
    Write-Host "! $Message" -ForegroundColor Yellow
}

function Write-Err {
    param([string]$Message)
    Write-Host "x $Message" -ForegroundColor Red
}

function Write-Completed {
    param([string]$Message)
    Write-Host "✓ " -ForegroundColor Green -NoNewline
    Write-Host $Message
}

function Confirm-Action {
    param([string]$Message)
    if ($Force) {
        return
    }
    $answer = Read-Host "? $Message [y/N]"
    if ($answer -ne "y" -and $answer -ne "yes") {
        Write-Err 'Aborting (please answer "yes" to continue)'
        exit 1
    }
}

# -- platform detection ------------------------------------------------------

# Currently supporting:
#   - x86_64 (AMD64)
#
# `dokany-rs` has no aarch64 support, so no ARM64 artifact is produced.
function Get-FusibileTarget {
    $arch = $env:PROCESSOR_ARCHITECTURE
    if ($env:PROCESSOR_ARCHITEW6432) {
        $arch = $env:PROCESSOR_ARCHITEW6432
    }

    switch ($arch.ToUpper()) {
        "AMD64" { return "x86_64-pc-windows-msvc" }
        "ARM64" {
            Write-Err "Windows on ARM64 is not supported yet."
            Write-Info "fusibile depends on Dokany, whose Rust bindings have no aarch64 support at the moment."
            Write-Info "You can still build from source: cargo install fusibile --locked"
            exit 1
        }
        default {
            Write-Err "Unsupported architecture: $arch"
            Write-Info "Only x86_64 (AMD64) is supported by this installer."
            Write-Info "Alternatively you can install fusibile with Cargo <https://www.rust-lang.org/tools/install>: cargo install fusibile --locked"
            exit 1
        }
    }
}

# -- version resolution ------------------------------------------------------

function Get-LatestFusibileVersion {
    try {
        $release = Invoke-RestMethod `
            -Uri "https://api.github.com/repos/$GithubRepo/releases/latest" `
            -Headers @{ "User-Agent" = "fusibile-installer" } `
            -UseBasicParsing
    } catch {
        Write-Err "Could not query the latest fusibile release: $($_.Exception.Message)"
        Write-Warn "If no release has been published yet, pass a version explicitly with '-Version X.Y.Z'."
        Write-Warn "If you believe this is a bug, please report an issue at <$IssuesUrl>"
        exit 1
    }
    return $release.tag_name.TrimStart("v")
}

# -- Dokany detection ---------------------------------------------------------

function Test-DokanyInstalled {
    if (Get-Service -Name "dokan2" -ErrorAction SilentlyContinue) {
        return $true
    }
    $dll = Join-Path $env:SystemRoot "System32\dokan2.dll"
    return (Test-Path $dll)
}

function Confirm-DokanyPresent {
    if (Test-DokanyInstalled) {
        return
    }
    Write-Warn "The Dokany driver does not appear to be installed."
    Write-Info "fusibile needs Dokany to mount anything on Windows. Install it with either:"
    Write-Info "  choco install dokany"
    Write-Info "  winget install Dokan.Dokany"
    Write-Info "or download the installer from <https://github.com/dokan-dev/dokany/releases>"
    Write-Info "A reboot may be required after installing the driver."
}

# -- installation ------------------------------------------------------------

function Install-Fusibile {
    param(
        [string]$InstallVersion,
        [string]$Target
    )

    $asset = "fusibile-v$InstallVersion-$Target.zip"
    $url = "https://github.com/$GithubRepo/releases/download/v$InstallVersion/$asset"

    Write-Host ""
    Write-Host "  fusibile configuration"
    Write-Info "Version:       $InstallVersion"
    Write-Info "Target:        $Target"
    Write-Info "Install dir:   $InstallDir"
    Write-Host ""

    Confirm-Action "Install fusibile $InstallVersion?"

    $tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "fusibile-$([System.IO.Path]::GetRandomFileName())"
    New-Item -ItemType Directory -Force -Path $tmpDir | Out-Null

    try {
        $archive = Join-Path $tmpDir $asset
        Write-Info "Downloading fusibile from $url …"
        try {
            Invoke-WebRequest -Uri $url -OutFile $archive -UseBasicParsing
        } catch {
            Write-Err "Failed to download fusibile: $($_.Exception.Message)"
            Write-Warn "Check that release v$InstallVersion exists and provides artifacts for $Target."
            Write-Warn "If you believe this is a bug, please report an issue at <$IssuesUrl>"
            exit 1
        }

        $checksumFile = "$archive.sha256"
        try {
            Invoke-WebRequest -Uri "$url.sha256" -OutFile $checksumFile -UseBasicParsing
            $expected = (Get-Content $checksumFile -Raw).Trim().ToLower()
            $actual = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
            if ($expected -ne $actual) {
                Write-Err "Checksum mismatch for the downloaded archive (expected $expected, got $actual)."
                Write-Err "Please retry, and report an issue at <$IssuesUrl> if the problem persists."
                exit 1
            }
            Write-Info "Checksum verified"
        } catch {
            Write-Warn "Could not verify the archive checksum: $($_.Exception.Message)"
        }

        Write-Info "Extracting archive …"
        Expand-Archive -Path $archive -DestinationPath $tmpDir -Force

        $binary = Join-Path $tmpDir "fusibile.exe"
        if (-not (Test-Path $binary)) {
            Write-Err "fusibile.exe was not found in the downloaded archive."
            Write-Warn "Please report an issue at <$IssuesUrl>"
            exit 1
        }

        if (-not (Test-Path $InstallDir)) {
            New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        }

        Write-Info "Installing fusibile to $InstallDir …"
        Copy-Item -Path $binary -Destination (Join-Path $InstallDir "fusibile.exe") -Force

        Add-ToUserPath -Directory $InstallDir
    } finally {
        Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Add-ToUserPath {
    param([string]$Directory)

    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $entries = @()
    if ($userPath) {
        $entries = $userPath.Split(";") | Where-Object { $_ -ne "" }
    }

    if ($entries -contains $Directory) {
        return
    }

    Write-Info "Adding $Directory to your user PATH …"
    $newPath = (@($entries) + $Directory) -join ";"
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    # make fusibile available in the current session too
    $env:Path = "$env:Path;$Directory"
    Write-Warn "Restart your terminal for the PATH change to take effect in new sessions."
}

# -- main --------------------------------------------------------------------

$target = Get-FusibileTarget
if (-not $Version) {
    Write-Info "Resolving the latest fusibile version…"
    $Version = Get-LatestFusibileVersion
}

Install-Fusibile -InstallVersion $Version -Target $target

Write-Completed "fusibile has successfully been installed on your system!"
Write-Info "Usage: fusibile --help"
Write-Info "If you encounter any issue, please report it at <$IssuesUrl>"

Confirm-DokanyPresent

exit 0
