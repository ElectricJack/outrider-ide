#Requires -Version 5.1
param(
    [switch]$Release,
    [string]$ProjectRoot
)

$ErrorActionPreference = "Stop"

if (-not $ProjectRoot) {
    $ProjectRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
}

Write-Host "==> Project root: $ProjectRoot"
Write-Host "==> Checking prerequisites..."

# --- rustup ---
if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    Write-Host "    Installing rustup..."
    $RustupInit = "$env:TEMP\rustup-init.exe"
    Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $RustupInit
    & $RustupInit -y --default-toolchain stable
    # Add to current session PATH
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
}
Write-Host "    rustc: $(rustc --version)"

# --- MSVC / Windows SDK ---
$VsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $VsWhere) {
    $VsPath = & $VsWhere -latest -property installationPath 2>$null
    if ($VsPath) {
        Write-Host "    VS Build Tools: $VsPath"
    }
} else {
    Write-Host "    WARNING: Visual Studio Build Tools not found."
    Write-Host "    Install 'Desktop development with C++' from:"
    Write-Host "    https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022"
    Write-Host "    (cargo build will fail at link time without MSVC)"
}

# --- Build ---
$CargoArgs = @("build")
if ($Release) {
    $CargoArgs += "--release"
}

Write-Host ""
Write-Host "==> Building outrider ($( if ($Release) { 'release' } else { 'debug' } ))..."
Push-Location $ProjectRoot
try {
    & cargo @CargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

$Profile = if ($Release) { "release" } else { "debug" }
$ExePath = Join-Path $ProjectRoot "target\$Profile\outrider.exe"
if (Test-Path $ExePath) {
    Write-Host ""
    Write-Host "==> Build complete: $ExePath"
} else {
    Write-Host ""
    Write-Host "==> Build finished but .exe not found at expected path."
    Write-Host "    Check target\$Profile\ for output."
}
