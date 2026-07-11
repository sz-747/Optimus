<#
.SYNOPSIS
  Build the Optimus Tauri app, CLI, and NSIS installer.

.DESCRIPTION
  Thin wrapper over the Tauri CLI. Installs frontend dependencies, then runs tauri build:
    1. beforeBuildCommand builds the Vite frontend into ui/dist,
    2. Cargo builds the GUI and optimus CLI binaries,
    3. NSIS bundles both binaries and downloads WebView2 during installation when required.

  Output: target\<profile>\bundle\nsis\*-setup.exe

.PARAMETER Debug
  Build the debug binary and installer instead of the release bundle.
#>
[CmdletBinding()]
param(
    [switch]$Debug
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Push-Location $repo
try {
    Write-Host '==> npm ci' -ForegroundColor Cyan
    npm ci
    if ($LASTEXITCODE -ne 0) { throw "npm ci failed ($LASTEXITCODE)" }

    $tauriArgs = @('run', 'tauri', '--', 'build')
    if ($Debug) { $tauriArgs += '--debug' }

    Write-Host "==> npm $($tauriArgs -join ' ')" -ForegroundColor Cyan
    npm @tauriArgs
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }

    $profile = if ($Debug) { 'debug' } else { 'release' }
    Write-Host "==> done - installer under target\$profile\bundle\nsis\" -ForegroundColor Green
}
finally {
    Pop-Location
}
