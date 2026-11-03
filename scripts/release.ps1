# Cuts a release: verifies everything the Release workflow will check, then tags and pushes.
# GitHub Actions (.github/workflows/release.yml) builds the exe + installer and publishes the release.
#
#   1. npm run version:bump [patch|minor|major|x.y.z]   (bumps package.json + Cargo.toml, adds CHANGELOG entry)
#   2. describe the release in CHANGELOG.md, commit it
#   3. npm run release                                    (this script)
param([switch]$NoPush)
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
trap { Write-Host "`nERROR: $_" -ForegroundColor Red; exit 1 }

# git may not be on PATH (GitHub Desktop ships its own copy)
$git = (Get-Command git -ErrorAction SilentlyContinue).Source
if (-not $git) {
    $git = Get-ChildItem "$env:LOCALAPPDATA\GitHubDesktop" -Recurse -Filter git.exe -ErrorAction SilentlyContinue |
        Where-Object FullName -like '*\cmd\git.exe' | Sort-Object LastWriteTime -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $git) { throw "git not found (install Git or GitHub Desktop)" }

$version = (Get-Content package.json -Raw | ConvertFrom-Json).version
$cargo = ([regex]'(?m)^version\s*=\s*"([^"]+)"').Match([IO.File]::ReadAllText("$root\src-tauri\Cargo.toml")).Groups[1].Value
if ($version -ne $cargo) { throw "package.json ($version) and Cargo.toml ($cargo) differ - use npm run version:bump" }
$tag = "v$version"

# release notes must exist and be filled in (the workflow uses them as the release body)
& "$PSScriptRoot\release-notes.ps1" -Version $version | Out-Null

if (& $git status --porcelain) { throw "working tree has uncommitted changes - commit them first" }
if (& $git tag --list $tag) { throw "tag $tag already exists - bump the version first" }

$branch = (& $git rev-parse --abbrev-ref HEAD).Trim()
Write-Host "Tagging $tag on $branch ($((& $git rev-parse --short HEAD).Trim()))" -ForegroundColor Cyan
& $git tag -a $tag -m "Lumen $tag"
if ($LASTEXITCODE -ne 0) { throw "git tag failed" }

if ($NoPush) {
    Write-Host "Tag created locally. Push with: git push origin $branch $tag" -ForegroundColor Yellow
    exit 0
}
& $git push origin $branch $tag
if ($LASTEXITCODE -ne 0) { throw "git push failed (the tag exists locally; push it once the problem is fixed)" }
Write-Host "`nPushed $tag. The Release workflow is now building it:" -ForegroundColor Green
Write-Host "  https://github.com/Akulav/local-ai-ui/actions"
