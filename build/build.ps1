<#
.SYNOPSIS
  Build the optimus engine (Rust cdylib) and the WinUI 3 app (plan §8 U2 / §9.3).

.DESCRIPTION
  1. cargo build the engine -> engine\target\<profile>\optimus_engine.dll
     (this also runs the csbindgen build.rs that regenerates NativeMethods.g.cs).
  2. dotnet build (or publish) the app; the csproj stages optimus_engine.dll next to Optimus.exe.

  cargo is invoked from its known install location because it is frequently not on PATH.

.PARAMETER Configuration
  Debug (default) or Release.

.PARAMETER Rid
  Runtime identifier for the .NET build/publish (win-x64 default; win-arm64 supported).

.PARAMETER Publish
  Run `dotnet publish` instead of `dotnet build` (verifies the unpackaged self-contained
  publish path — plan risk §9.1 #3).

.PARAMETER EngineOnly
  Build only the Rust engine and stop (useful for regenerating bindings).

.PARAMETER VerifyMigration
  Run the legacy regression tests plus the Tauri shell and frontend checks after a normal build.
#>
[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug',
    [string]$Rid = 'win-x64',
    [switch]$Publish,
    [switch]$EngineOnly,
    [switch]$VerifyMigration
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$engineDir = Join-Path $repo 'engine'
$appProj = Join-Path $repo 'app\Optimus.App.csproj'

# Locate cargo (commonly not on PATH).
$cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
if (-not $cargo) {
    $candidate = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path $candidate) { $cargo = $candidate }
}
if (-not $cargo) {
    throw "cargo not found on PATH or in %USERPROFILE%\.cargo\bin. Install Rust or fix PATH."
}

$cargoArgs = @('build')
if ($Configuration -eq 'Release') { $cargoArgs += '--release' }

Write-Host "==> cargo $($cargoArgs -join ' ')  (cwd: $engineDir)" -ForegroundColor Cyan
Push-Location $engineDir
try {
    & $cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}
finally {
    Pop-Location
}

if ($EngineOnly) {
    Write-Host "==> engine built; skipping app (-EngineOnly)" -ForegroundColor Green
    return
}

# The csproj's BuildOptimusEngine target would rebuild the engine too; skip it here since we
# just built it, so the .NET step doesn't shell out to cargo a second time.
$verb = if ($Publish) { 'publish' } else { 'build' }
$dotnetArgs = @($verb, $appProj, '-c', $Configuration, '-r', $Rid, '-p:SkipOptimusEngineBuild=true')

Write-Host "==> dotnet $($dotnetArgs -join ' ')" -ForegroundColor Cyan
& dotnet @dotnetArgs
if ($LASTEXITCODE -ne 0) { throw "dotnet $verb failed ($LASTEXITCODE)" }

# Build the optimus CLI (plan Phase 4 U4) alongside the app so `optimus.exe` ships with it.
# On publish, the CLI goes self-contained single-file: the installer payload (p6 U3) must run
# on machines with no .NET runtime, same guarantee the app gets from SelfContained above.
$cliProj = Join-Path $repo 'cli\Optimus.Cli.csproj'
$cliArgs = @($verb, $cliProj, '-c', $Configuration)
if ($Publish) {
    $cliArgs += @('-r', $Rid, '--self-contained', 'true', '-p:PublishSingleFile=true')
}

Write-Host "==> dotnet $($cliArgs -join ' ')" -ForegroundColor Cyan
& dotnet @cliArgs
if ($LASTEXITCODE -ne 0) { throw "dotnet $verb (cli) failed ($LASTEXITCODE)" }

if ($VerifyMigration) {
    $testsProj = Join-Path $repo 'tests\Optimus.Core.Tests.csproj'

    Write-Host "==> cargo test -p optimus_engine" -ForegroundColor Cyan
    Push-Location $repo
    try {
        & $cargo test -p optimus_engine
        if ($LASTEXITCODE -ne 0) { throw "cargo test failed ($LASTEXITCODE)" }
    }
    finally {
        Pop-Location
    }

    Write-Host "==> cargo fmt -p optimus-shell -- --check" -ForegroundColor Cyan
    Push-Location $repo
    try {
        & $cargo fmt -p optimus-shell -- --check
        if ($LASTEXITCODE -ne 0) { throw "Tauri shell formatting check failed ($LASTEXITCODE)" }
    }
    finally {
        Pop-Location
    }

    Write-Host "==> dotnet test $testsProj" -ForegroundColor Cyan
    & dotnet test $testsProj -c $Configuration -r $Rid -p:SkipOptimusEngineBuild=true
    if ($LASTEXITCODE -ne 0) { throw "dotnet test failed ($LASTEXITCODE)" }

    Write-Host "==> npm --prefix frontend test" -ForegroundColor Cyan
    & npm --prefix (Join-Path $repo 'frontend') test
    if ($LASTEXITCODE -ne 0) { throw "frontend test failed ($LASTEXITCODE)" }

    Write-Host "==> cargo check -p optimus-shell" -ForegroundColor Cyan
    Push-Location $repo
    try {
        & $cargo check -p optimus-shell
        if ($LASTEXITCODE -ne 0) { throw "Tauri shell check failed ($LASTEXITCODE)" }
    }
    finally {
        Pop-Location
    }
}

Write-Host "==> done ($Configuration / $Rid / $verb)" -ForegroundColor Green
