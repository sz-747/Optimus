<#
.SYNOPSIS
  Build the Optimus Tauri app — the NSIS installer + bundled binary (plan §8).

.DESCRIPTION
  Thin wrapper over the Tauri CLI. Installs frontend deps, then runs `tauri build`, which:
    1. runs beforeBuildCommand (vite build ui -> ui/dist),
    2. cargo-builds the release `optimus-shell` binary (windows_subsystem = windows: no console),
    3. bundles the per-user NSIS installer with the WebView2 bootstrapper.
  The `tauri` CLI locates shell\tauri.conf.json from the repo root.

  Output: target\release\bundle\nsis\*-setup.exe

.PARAMETER Debug
  Build the debug binary (keeps the console) instead of the release bundle.
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

    Write-Host '==> done — installer under target\release\bundle\nsis\' -ForegroundColor Green
}
finally {
    Pop-Location
}
