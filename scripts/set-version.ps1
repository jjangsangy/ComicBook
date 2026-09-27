<#
.SYNOPSIS
    Stamp the `[package] version` in Cargo.toml from a release tag.

.DESCRIPTION
    The release workflow calls this before building so a binary built from a tag
    reports that tag's version when run as `comic-book --version`. The tag name
    is authoritative: the manifest is overwritten rather than trusted to have
    been bumped by hand. Only the `version` line inside the `[package]` table is
    touched; dependency versions are never modified. Cargo.lock is refreshed
    separately by the workflow (`cargo update -p comic-book`) so the `--locked`
    release build still succeeds.

.PARAMETER Version
    Version to write, for example "v0.2.4" or "0.2.0-rc.1". A leading "v" is
    optional.

.PARAMETER Check
    Validate the version and exit without modifying Cargo.toml.

.EXAMPLE
    .\set-version.ps1 v0.2.4

.EXAMPLE
    .\set-version.ps1 -Check v0.2.4
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $Version,

    [switch] $Check
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Edit the manifest belonging to this script's checkout, not the caller's cwd.
$Manifest = Join-Path (Join-Path $PSScriptRoot '..') 'Cargo.toml'

# Release tags are `v`-prefixed; Cargo's own version field is not.
$Version = $Version -replace '^v', ''

# Cargo requires a full SemVer version such as `0.2.4` or `0.2.0-rc.1`.
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$') {
    throw "'$Version' is not a valid SemVer version"
}

if ($Check) {
    Write-Host "valid release version: $Version"
    return
}

# Rewrite only the first `version =` line in the `[package]` table.
$inPackage = $false
$done = $false

$lines = foreach ($line in (Get-Content -LiteralPath $Manifest)) {
    if ($line -match '^\[') {
        $inPackage = ($line -match '^\[\s*package\s*\]')
    }

    if ((-not $done) -and $inPackage -and ($line -match '^version\s*=')) {
        $done = $true
        'version = "' + $Version + '"'
    }
    else {
        $line
    }
}

if (-not $done) {
    throw "no [package] version field found in $Manifest"
}

# Write LF endings and UTF-8 without a BOM, matching the rest of the repository
# on every PowerShell version.
$path = (Resolve-Path -LiteralPath $Manifest).Path
$text = ($lines -join "`n") + "`n"
[System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))

Write-Host "set $Manifest version to $Version"
