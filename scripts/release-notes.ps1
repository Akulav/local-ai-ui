# Prints the CHANGELOG.md section for one version (used as the GitHub release body).
#   scripts\release-notes.ps1 0.2.0            -> writes to stdout
#   scripts\release-notes.ps1 0.2.0 notes.md   -> writes to a file
# Fails if the version has no section or the section is still the empty template from bump-version.
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$OutFile
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$changelog = [IO.File]::ReadAllText((Join-Path $root "CHANGELOG.md"))

$v = [regex]::Escape($Version.TrimStart("v"))
$m = [regex]::Match($changelog, "(?ms)^## \[$v\][^\r\n]*\r?\n(.*?)(?=^## \[|\z)")
if (-not $m.Success) { throw "CHANGELOG.md has no section for version $Version" }

$body = $m.Groups[1].Value.Trim()
# a section containing only headings / empty bullets means nobody wrote the notes yet
$content = ($body -split "`n" | Where-Object { $_.Trim() -notmatch '^(#+ .*|-\s*)?$' })
if (-not $content) { throw "CHANGELOG.md section for $Version is empty - describe the release first" }

$body += "`n`n---`n**Windows x64.** Portable: unzip and run ``Lumen.exe`` (models and chats are stored in ``LumenData`` next to it). Installer: run the ``-setup.exe``.`n`nMade by Akulav."
if ($OutFile) { [IO.File]::WriteAllText($OutFile, $body) } else { $body }
