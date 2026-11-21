# Build DBLocal MSI on Windows (F-PKG-04).
#
# This script is intended for CI on windows-latest with WiX installed.
# It is NOT runnable on this Linux machine; it is kept here so the release
# workflow and Windows verification guide have a single source of truth.
#
# Usage:
#   pwsh packaging/msi/build-msi.ps1 -Version 0.1.0 -DblocalExe path\to\dblocal.exe -OutDir dist\msi
#
# Requires: WiX Toolset v3 (candle.exe/light.exe) on PATH, or set WIX env var.

[CmdletBinding()]
param(
  [Parameter(Mandatory = $false)][string]$Version = "",
  [Parameter(Mandatory = $true)][string]$DblocalExe,
  [string]$OutDir = "dist/msi"
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $DblocalExe)) {
  throw "DblocalExe not found: $DblocalExe"
}

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$wxs = Join-Path $scriptDir "dblocal.wxs"
if (-not (Test-Path $wxs)) { throw "Missing WiX source: $wxs" }

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$objDir = Join-Path $OutDir "obj"
New-Item -ItemType Directory -Force -Path $objDir | Out-Null

$candle = Get-Command candle.exe -ErrorAction SilentlyContinue
$light = Get-Command light.exe -ErrorAction SilentlyContinue
if (-not $candle -and $env:WIX) {
  $candle = Get-Command (Join-Path $env:WIX "bin/candle.exe") -ErrorAction SilentlyContinue
  $light = Get-Command (Join-Path $env:WIX "bin/light.exe") -ErrorAction SilentlyContinue
}
if (-not $candle -or -not $light) {
  throw "WiX candle.exe/light.exe not found. Install WiX Toolset v3 or set WIX."
}

# F2: explicit -Version is validated; default (no tag) becomes a snapshot.
# F1: prerelease '-' maps to '~' in package semantics, but WiX ProductVersion
# must be major.minor.build numeric; we keep raw for filename and strip for MSI.
$rawVersion = $Version
$isSnapshot = $false
if ([string]::IsNullOrWhiteSpace($rawVersion)) {
  $gitDesc = ""
  try { $gitDesc = (& git -C (Split-Path -Parent (Split-Path -Parent $scriptDir)) describe --tags --always 2>$null) } catch {}
  $gitDesc = "$gitDesc".Trim()
  $stripped = $gitDesc -replace '^v', ''
  if ($stripped -match '^[0-9]+\.[0-9]+\.[0-9]+([-~][0-9A-Za-z.]+)?$') {
    $rawVersion = $stripped
  } else {
    $short = ""
    try { $short = (& git -C (Split-Path -Parent (Split-Path -Parent $scriptDir)) rev-parse --short=12 HEAD 2>$null) } catch {}
    $short = "$short".Trim()
    if ([string]::IsNullOrWhiteSpace($short)) { $short = "unknown" }
    $rawVersion = "0.0.0~git$short"
    $isSnapshot = $true
    Write-Host "info: no SemVer tag describes HEAD; using snapshot package version $rawVersion"
  }
} else {
  $stripped = $rawVersion -replace '^v', ''
  if ($stripped -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([-~][0-9A-Za-z.]+)?$') {
    throw "invalid version '$rawVersion': expected MAJOR.MINOR.PATCH with optional [-~]prerelease (e.g. 0.2.0, 0.2.0-rc.1)"
  }
  $rawVersion = $stripped
}

$msiVersion = $rawVersion
# WiX ProductVersion must be major.minor.build; strip prerelease/snapshot metadata.
if ($msiVersion -match '^(\d+\.\d+\.\d+)') { $msiVersion = $Matches[1] }
else { $msiVersion = "0.0.0" }

$wixobj = Join-Path $objDir "dblocal.wixobj"
$msi = Join-Path $OutDir "dblocal-$rawVersion-windows-x86_64.msi"

& $candle.Source `
  -nologo `
  -arch x64 `
  "-dProductVersion=$msiVersion" `
  "-dDblocalExeSource=$DblocalExe" `
  -out $wixobj `
  $wxs
if ($LASTEXITCODE -ne 0) { throw "candle.exe failed with exit code $LASTEXITCODE" }

& $light.Source `
  -nologo `
  -ext WixUIExtension `
  -sval `
  -out $msi `
  $wixobj
if ($LASTEXITCODE -ne 0) { throw "light.exe failed with exit code $LASTEXITCODE" }

Write-Host "wrote $msi"
