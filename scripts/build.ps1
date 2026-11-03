# Lumen build script, called by run.bat.
# Installs whatever is missing (Node.js, Rust, MSVC C++ Build Tools), builds the release exe and
# drops it into dist\Lumen next to the portable LumenData folder (models/chats are never touched).
param([string]$Mode = "run")
# "Continue": native tools (cargo, rustup) write progress to stderr, which "Stop" would treat as fatal
# in Windows PowerShell 5.1. Failures are detected through $LASTEXITCODE and explicit checks instead.
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

function Step($msg) { Write-Host "`n==> $msg" -ForegroundColor Cyan }
function Ok($msg) { Write-Host "    $msg" -ForegroundColor Green }

# Makes freshly installed tools visible without opening a new terminal.
function Refresh-Path {
    $machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $user = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = "$env:USERPROFILE\.cargo\bin;$env:ProgramFiles\nodejs;$machine;$user"
}

function Need-Winget {
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
        throw "winget is not available. Install 'App Installer' from the Microsoft Store, then run again."
    }
}

function Winget-Install($id, $extra) {
    Need-Winget
    Write-Host "    installing $id via winget..." -ForegroundColor Yellow
    $wargs = @("install", "--id", $id, "-e", "--accept-package-agreements", "--accept-source-agreements") + $extra
    & winget @wargs
    # winget returns non-zero for "already installed"; the checks below are what matter
    Refresh-Path
}

function Has-MsvcTools {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $false }
    $p = & $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    return [bool]$p
}

Refresh-Path
trap { Write-Host "`nERROR: $_" -ForegroundColor Red; exit 1 }

if ($Mode -eq "clean") {
    Step "Cleaning build outputs"
    Remove-Item -Recurse -Force "src-tauri\target", "node_modules" -ErrorAction SilentlyContinue
    Ok "done (dist\Lumen\LumenData kept)"
    exit 0
}

# ---- toolchain -------------------------------------------------------------
Step "Checking build tools"

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    Winget-Install "OpenJS.NodeJS.LTS" @()
    if (-not (Get-Command node -ErrorAction SilentlyContinue)) { throw "Node.js install failed" }
}
Ok "Node.js $(node --version)"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Winget-Install "Rustlang.Rustup" @()
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) { throw "rustup install failed" }
}
# make sure a toolchain is actually selected (fresh rustup installs may have none)
& rustup default stable *> $null
& rustup target add x86_64-pc-windows-msvc *> $null
Ok "$(cargo --version)"

if (-not (Has-MsvcTools)) {
    Write-Host "    Visual Studio C++ Build Tools missing (needed by the Rust MSVC linker)." -ForegroundColor Yellow
    Write-Host "    This is a ~3-6 GB one-time install and needs administrator rights." -ForegroundColor Yellow
    Winget-Install "Microsoft.VisualStudio.2022.BuildTools" @("--override", "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended")
    if (-not (Has-MsvcTools)) { throw "C++ Build Tools install did not complete (was the UAC prompt declined?)" }
}
Ok "MSVC C++ Build Tools"

# ---- dependencies ----------------------------------------------------------
Step "Installing npm packages"
if (Test-Path "package-lock.json") { & npm ci --no-audit --no-fund } else { & npm install --no-audit --no-fund }
if ($LASTEXITCODE -ne 0) { throw "npm install failed" }

if ($Mode -eq "dev") {
    Step "Starting dev mode (Ctrl+C to stop)"
    & npx tauri dev
    exit $LASTEXITCODE
}

# ---- build -----------------------------------------------------------------
$version = (Get-Content package.json -Raw | ConvertFrom-Json).version
Step "Building Lumen v$version (release)"
& npx tauri build --no-bundle
if ($LASTEXITCODE -ne 0) { throw "cargo/tauri build failed" }

# ---- package ---------------------------------------------------------------
Step "Copying to dist\Lumen"
$dist = Join-Path $root "dist\Lumen"
New-Item -ItemType Directory -Force $dist | Out-Null
$exe = Join-Path $dist "Lumen.exe"
# a running copy locks the exe; close it (its job object also stops llama-server)
Get-Process -Name Lumen -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | ForEach-Object {
    Write-Host "    closing running Lumen..." -ForegroundColor Yellow
    $_ | Stop-Process -Force
    Start-Sleep -Milliseconds 800
}
Copy-Item "src-tauri\target\release\lumen.exe" $exe -Force
Ok "$exe"

if ($Mode -ne "build") {
    Step "Launching"
    Start-Process $exe
}
Write-Host "`nDone." -ForegroundColor Green
