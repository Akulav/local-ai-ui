# Bumps the app version everywhere it lives and opens a CHANGELOG entry.
#   scripts\bump-version.ps1            -> patch bump (0.2.0 -> 0.2.1)
#   scripts\bump-version.ps1 minor      -> 0.2.0 -> 0.3.0
#   scripts\bump-version.ps1 1.4.2      -> explicit version
# package.json is the source of truth (tauri.conf.json reads it); Cargo.toml is kept in sync.
param([string]$Part = "patch")
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot

$pkgPath = Join-Path $root "package.json"
$pkg = Get-Content $pkgPath -Raw
$current = [regex]::Match($pkg, '"version"\s*:\s*"([^"]+)"').Groups[1].Value
$v = [version]$current

switch -regex ($Part) {
    '^\d+\.\d+\.\d+$' { $new = $Part }
    '^major$' { $new = "$($v.Major + 1).0.0" }
    '^minor$' { $new = "$($v.Major).$($v.Minor + 1).0" }
    default   { $new = "$($v.Major).$($v.Minor).$($v.Build + 1)" }
}

# note: a Regex *instance* is needed for "replace only the first match" ([regex]::Replace has no count)
$pkg = ([regex]'("version"\s*:\s*")[^"]+(")').Replace($pkg, "`${1}$new`${2}", 1)
[IO.File]::WriteAllText($pkgPath, $pkg)

$cargoPath = Join-Path $root "src-tauri\Cargo.toml"
$cargo = Get-Content $cargoPath -Raw
$cargo = ([regex]'(?m)^(version\s*=\s*")[^"]+(")').Replace($cargo, "`${1}$new`${2}", 1)
[IO.File]::WriteAllText($cargoPath, $cargo)

$clPath = Join-Path $root "CHANGELOG.md"
$cl = Get-Content $clPath -Raw
$date = Get-Date -Format "yyyy-MM-dd"
$entry = "## [$new] - $date`n### Changed`n- `n`n"
$cl = ([regex]'## \[').Replace($cl, "$entry## [", 1)
[IO.File]::WriteAllText($clPath, $cl)

Write-Host "Version $current -> $new (package.json, Cargo.toml, CHANGELOG.md)"
Write-Host "Fill in CHANGELOG.md, then: git commit -am `"Release v$new`" ; git tag v$new"
