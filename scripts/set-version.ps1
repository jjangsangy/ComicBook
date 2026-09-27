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

    -Changelog additionally rolls CHANGELOG.md's top `## [Unreleased]` section
    over to the released version: the heading is dated and retitled
    `## [<version>] - <YYYY-MM-DD>`, a fresh empty `## [Unreleased]` heading is
    re-opened above it so the next release has a section to accumulate changes
    under, and the link references at the foot of the file are updated
    (`[Unreleased]` now compares from the new tag, and a `[<version>]` entry is
    added comparing from the previous release — the first dated heading below
    `Unreleased`). The switch is deliberately opt-in: only the `bump-version`
    job, which commits the released version back to the default branch, passes
    it. The per-target build jobs stamp a throwaway checkout and must leave the
    changelog alone.

.PARAMETER Version
    Version to write, for example "v0.2.4" or "0.2.0-rc.1". A leading "v" is
    optional.

.PARAMETER Check
    Validate the version and exit without modifying Cargo.toml.

.PARAMETER Changelog
    Also roll CHANGELOG.md's `## [Unreleased]` section over to the version.

.EXAMPLE
    .\set-version.ps1 v0.2.4

.EXAMPLE
    .\set-version.ps1 -Check v0.2.4

.EXAMPLE
    .\set-version.ps1 -Changelog v0.2.4
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string] $Version,

    [switch] $Check,

    [switch] $Changelog
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Edit the files belonging to this script's checkout, not the caller's cwd.
$Manifest = Join-Path (Join-Path $PSScriptRoot '..') 'Cargo.toml'
$ChangelogPath = Join-Path (Join-Path $PSScriptRoot '..') 'CHANGELOG.md'

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

# Roll the changelog's unreleased section over to this version, when asked.
if (-not $Changelog) {
    return
}

if (-not (Test-Path -LiteralPath $ChangelogPath)) {
    Write-Host "notice: no $ChangelogPath; leaving it unchanged"
    return
}

$changelogFile = (Resolve-Path -LiteralPath $ChangelogPath).Path
$changelogText = [System.IO.File]::ReadAllText($changelogFile) -replace "`r`n", "`n"

if ($changelogText -notmatch '(?m)^## \[Unreleased\]') {
    Write-Host "notice: no ## [Unreleased] section in $ChangelogPath; leaving it unchanged"
    return
}

$changelogLines = $changelogText -split "`n"

# The previous release is the first dated heading below `## [Unreleased]`; its
# tag is the base of the new version's compare link. `v0.0.0` is a harmless
# placeholder for a changelog whose first release is still unreleased.
$prevTag = $null
$seenUnreleased = $false

foreach ($line in $changelogLines) {
    if ($line -match '^## \[Unreleased\]') {
        $seenUnreleased = $true
        continue
    }
    if ($seenUnreleased -and ($line -match '^## \[(.+?)\]')) {
        $prevTag = 'v' + $Matches[1]
        break
    }
}
if (-not $prevTag) {
    $prevTag = 'v0.0.0'
}

# Reuse the repository URL the existing `[Unreleased]` link already points at so
# a fork's changelog is not rewritten to upstream.
$baseMatch = [regex]::Match($changelogText, '(?m)^\[Unreleased\]: (\S+)')
if ($baseMatch.Success) {
    $baseUrl = $baseMatch.Groups[1].Value -replace '/compare/.*$', ''
}
else {
    $baseUrl = 'https://github.com/jjangsangy/ComicBook'
}

$today = (Get-Date).ToString('yyyy-MM-dd')

$headingDone = $false
$linkDone = $false
$out = [System.Collections.Generic.List[string]]::new()

foreach ($line in $changelogLines) {
    if ((-not $headingDone) -and ($line -match '^## \[Unreleased\]')) {
        # Re-open an empty Unreleased section above the dated release so the
        # next release still has a heading to record its changes under.
        $out.Add("## [Unreleased]")
        $out.Add("")
        $out.Add("## [$Version] - $today")
        $headingDone = $true
        continue
    }

    if ((-not $linkDone) -and ($line -match '^\[Unreleased\]:')) {
        $out.Add("[Unreleased]: $baseUrl/compare/v$Version...HEAD")
        $out.Add("[$Version]: $baseUrl/compare/$prevTag...v$Version")
        $linkDone = $true
        continue
    }

    $out.Add($line)
}

# A changelog with the heading but no link reference still gets the links,
# appended after whatever references it already has.
if ($headingDone -and (-not $linkDone)) {
    $out.Add("[Unreleased]: $baseUrl/compare/v$Version...HEAD")
    $out.Add("[$Version]: $baseUrl/compare/$prevTag...v$Version")
}

$newText = ($out -join "`n") + "`n"
[System.IO.File]::WriteAllText($changelogFile, $newText, (New-Object System.Text.UTF8Encoding($false)))

Write-Host "released $Version in $ChangelogPath (previous $prevTag)"
