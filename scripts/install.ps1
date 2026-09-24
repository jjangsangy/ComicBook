<#
.SYNOPSIS
    Installs the comic-book CLI from GitHub Releases on Windows.

.DESCRIPTION
    Downloads a prebuilt release binary, verifies its SHA-256 checksum when one
    is published, installs it under %LOCALAPPDATA%\Programs\comic-book\bin, and
    adds that directory to the user PATH.

.PARAMETER Version
    Release tag to install, for example "v0.1.0". Defaults to the latest release.

.PARAMETER InstallDir
    Directory to install comic-book.exe into.

.PARAMETER NoPathUpdate
    Do not modify the user PATH.

.EXAMPLE
    irm https://raw.githubusercontent.com/jjangsangy/ComicBook/main/scripts/install.ps1 | iex

.EXAMPLE
    .\install.ps1 -Version v0.1.0 -InstallDir "$env:USERPROFILE\bin"
#>
[CmdletBinding()]
param(
    [string] $Version = 'latest',
    [string] $InstallDir = '',
    [switch] $NoPathUpdate
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The progress bar rendered by Invoke-WebRequest makes downloads dramatically
# slower on Windows PowerShell 5.1.
$ProgressPreference = 'SilentlyContinue'

if ($PSVersionTable.PSVersion.Major -lt 5) {
    throw "comic-book installer requires PowerShell 5.1 or newer."
}

$Repo = 'jjangsangy/ComicBook'
$Bin = 'comic-book'

# --- Resolve the install location ---------------------------------------------

if (-not $InstallDir) {
    if ($env:LOCALAPPDATA) {
        $InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\comic-book\bin'
    }
    else {
        $InstallDir = Join-Path $env:USERPROFILE '.comic-book\bin'
    }
}

# --- Detect the architecture --------------------------------------------------

$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }

switch ($arch) {
    'AMD64' { $target = 'x86_64-pc-windows-msvc' }
    'ARM64' { throw "No prebuilt Windows ARM64 binary is published yet. Build from source instead: cargo build --release" }
    default { throw "Unsupported Windows architecture: $arch" }
}

# --- Resolve download location ------------------------------------------------

if ($Version -eq 'latest') {
    $baseUrl = "https://github.com/$Repo/releases/latest/download"
}
else {
    $tag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
    $baseUrl = "https://github.com/$Repo/releases/download/$tag"
}

$assetName = "$Bin-$target.zip"
$assetUrl = "$baseUrl/$assetName"

# Windows PowerShell 5.1 still defaults to TLS 1.0 on some systems.
if ($PSVersionTable.PSVersion.Major -lt 6) {
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
}

# --- Download, verify, install ------------------------------------------------

$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) ("comic-book-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

try {
    $zipPath = Join-Path $tmpDir $assetName

    Write-Host "Downloading $assetUrl"
    try {
        Invoke-WebRequest -Uri $assetUrl -OutFile $zipPath -UseBasicParsing
    }
    catch {
        throw ('Failed to download {0} ({1}). Check that the release exists: https://github.com/{2}/releases' -f $assetUrl, $_.Exception.Message, $Repo)
    }

    $shaPath = Join-Path $tmpDir "$assetName.sha256"
    try {
        Invoke-WebRequest -Uri "$assetUrl.sha256" -OutFile $shaPath -UseBasicParsing
        $expected = ((Get-Content -Raw -Path $shaPath) -split '\s+')[0].Trim().ToLowerInvariant()
    }
    catch {
        Write-Warning "No checksum published for $assetName; skipping verification."
        $expected = ''
    }

    if ($expected) {
        $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "Checksum verification failed for $assetName."
        }
        Write-Host "Checksum verified."
    }

    $extractDir = Join-Path $tmpDir 'unpacked'
    Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force

    $exePath = Join-Path $extractDir "$Bin.exe"
    if (-not (Test-Path -Path $exePath)) {
        throw "The downloaded archive did not contain $Bin.exe."
    }

    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    $dest = Join-Path $InstallDir "$Bin.exe"
    Copy-Item -Path $exePath -Destination $dest -Force
}
finally {
    Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "Installed $Bin to $dest"

try {
    & $dest --version
}
catch {
    # Running the freshly installed binary is best-effort only.
}

# --- PATH guidance ------------------------------------------------------------

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$entries = @()
if ($userPath) {
    $entries = @($userPath -split ';' | Where-Object { $_ -ne '' })
}

if ($NoPathUpdate) {
    if (-not ($entries -contains $InstallDir)) {
        Write-Host ""
        Write-Host "$InstallDir is not on your PATH. Add it manually with:"
        Write-Host ''
        Write-Host ('  $env:Path += ";' + $InstallDir + '"   # current session only')
        Write-Host ''
        Write-Host 'To make it permanent for future sessions:'
        Write-Host ('  [Environment]::SetEnvironmentVariable("Path", ([Environment]::GetEnvironmentVariable("Path", "User") + ";' + $InstallDir + '"), "User")')
    }
    return
}

if ($entries -contains $InstallDir) {
    Write-Host "$InstallDir is already on your user PATH."
}
else {
    $newPath = (@($entries) + $InstallDir) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    Write-Host "Added $InstallDir to your user PATH."
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Restart your terminal for the PATH change to take effect."
}
