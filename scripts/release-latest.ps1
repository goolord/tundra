#Requires -Version 5.1
<#
.SYNOPSIS
  Rebuild and overwrite the most recent GitHub release (or a chosen tag).

.DESCRIPTION
  1. Moves the release tag to HEAD and force-pushes it
  2. Builds a release package for this host (unless -SkipBuild)
  3. Uploads assets with --clobber (replaces same-named files)
  4. Optionally dispatches the "Release builds" workflow for Linux/macOS (-Ci)

  Requires: gh, git, cargo, rustc, uv (for bundled Python on native builds)

.EXAMPLE
  .\scripts\release-latest.ps1

.EXAMPLE
  .\scripts\release-latest.ps1 -Force -Ci
#>
[CmdletBinding()]
param(
    [string] $Tag,
    [string] $Target,
    [switch] $Force,
    [switch] $SkipBuild,
    [switch] $SkipTagPush,
    [switch] $SkipUpload,
    [switch] $Ci,
    [switch] $CiOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-RepoRoot {
    $root = Resolve-Path (Join-Path $PSScriptRoot '..')
    if (-not (Test-Path (Join-Path $root 'Cargo.toml'))) {
        throw "Repo root not found (expected Cargo.toml in $($root))"
    }
    return $root
}

function Invoke-Checked {
    param([scriptblock] $Command, [string] $Label)
    & $Command
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
}

function Get-LatestReleaseTag {
    $tag = gh release list --limit 1 --json tagName -q '.[0].tagName'
    if ([string]::IsNullOrWhiteSpace($tag)) {
        throw 'No GitHub releases found. Create one first or pass -Tag.'
    }
    return $tag.Trim()
}

function Get-HostTriple {
    if ($Target) { return $Target }
    $line = rustc -vV | Select-String -Pattern '^host: '
    if (-not $line) { throw 'Could not read host triple from rustc -vV' }
    return ($line -replace '^host: ', '').Trim()
}

function Confirm-Release {
    param([string] $Message)
    if ($Force) { return }
    $answer = Read-Host "$Message [y/N]"
    if ($answer -notmatch '^[Yy]') {
        throw 'Aborted.'
    }
}

$root = Get-RepoRoot
Set-Location $root

Invoke-Checked { gh auth status 2>&1 | Out-Null } 'gh auth status'

if (-not $Tag) {
    $Tag = Get-LatestReleaseTag
}

$hostTriple = Get-HostTriple
$release = gh release view $Tag --json isPrerelease,name -q '.' | ConvertFrom-Json
$prereleaseFlag = if ($release.isPrerelease) { '--prerelease' } else { '' }

Write-Host "Release tag:     $Tag"
Write-Host "Release name:    $($release.name)"
Write-Host "Host target:     $hostTriple"
Write-Host "HEAD:            $(git rev-parse --short HEAD)"
Write-Host "Prerelease:      $($release.isPrerelease)"
Write-Host ""

Confirm-Release "Overwrite GitHub release '$Tag' with current HEAD?"

if (-not $SkipTagPush) {
    Write-Host "Moving tag $Tag to HEAD..."
    $commit = (git rev-parse HEAD).Trim()
    Invoke-Checked { git tag -f $Tag } "git tag -f $Tag"
    Invoke-Checked { git push -f origin "refs/tags/$Tag" } "git push tag"
    # Tag move updates the release; edit only refreshes prerelease metadata if needed.
    if ($prereleaseFlag) {
        Invoke-Checked { gh release edit $Tag --target $commit --prerelease } 'gh release edit'
    } else {
        Invoke-Checked { gh release edit $Tag --target $commit } 'gh release edit'
    }
}

if (-not $CiOnly) {
    if (-not $SkipBuild) {
        Write-Host 'Downloading models...'
        Invoke-Checked { cargo xtask models } 'cargo xtask models'
        Write-Host "Building release package for $hostTriple..."
        Invoke-Checked {
            cargo xtask package --version $Tag --target $hostTriple
        } 'cargo xtask package'
    }

    if (-not $SkipUpload) {
        $pattern = Join-Path $root "target/tundra-$Tag-$hostTriple.*"
        $assets = @(Get-ChildItem -Path $pattern -ErrorAction SilentlyContinue)
        if (-not $assets) {
            throw "No package found matching $pattern (build first or drop -SkipBuild)"
        }
        Write-Host "Uploading $($assets.Count) asset(s) with --clobber..."
        & gh release upload $Tag --clobber @($assets | ForEach-Object { $_.FullName })
        if ($LASTEXITCODE -ne 0) {
            throw 'gh release upload failed with exit code ' + $LASTEXITCODE
        }
    }
}

if ($Ci -or $CiOnly) {
    Write-Host 'Dispatching Release builds workflow (Linux + macOS)...'
    Invoke-Checked {
        gh workflow run release.yml -f "tag=$Tag"
    } 'gh workflow run'
    Write-Host 'CI run started. Watch: gh run list --workflow=release.yml'
}

Write-Host "Done. Release: https://github.com/$(gh repo view --json nameWithOwner -q .nameWithOwner)/releases/tag/$Tag"
