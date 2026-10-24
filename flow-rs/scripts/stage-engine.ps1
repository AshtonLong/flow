<#
.SYNOPSIS
  Puts the prebuilt speech engine (transcribe.cpp 0.3.0: transcribe.dll, the
  ggml CPU and Vulkan backends and their licences) into <Dest>\engine.
.EXAMPLE
  scripts\stage-engine.ps1 -Dest target\debug
#>
param(
  [Parameter(Mandatory)] [string] $Dest,
  [string] $Version = '0.3.0'
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Dest | Out-Null
$Dest = (Resolve-Path -LiteralPath $Dest).Path
$engine = Join-Path $Dest 'engine'
if (Test-Path (Join-Path $engine 'transcribe.dll')) { Write-Host "Engine already staged in $engine"; return }
$work = Join-Path ([IO.Path]::GetTempPath()) "flow-stage-engine-$Version"
New-Item -ItemType Directory -Force $work | Out-Null
Push-Location $work
try {
  $tgz = Get-ChildItem -Filter 'transcribe-cpp-win32-x64-cpu-vulkan-*.tgz' | Select-Object -First 1
  if (-not $tgz) {
    npm pack "@transcribe-cpp/win32-x64-cpu-vulkan@$Version" | Out-Null
    $tgz = Get-ChildItem -Filter 'transcribe-cpp-win32-x64-cpu-vulkan-*.tgz' | Select-Object -First 1
  }
  if (Test-Path package) { Remove-Item -Recurse -Force package }
  # Windows tar (bsdtar); a GNU tar earlier on PATH reads "C:" as a remote host.
  & (Join-Path $env:SystemRoot 'System32\tar.exe') -xzf $tgz.Name
  New-Item -ItemType Directory -Force $engine | Out-Null
  Copy-Item package\*.dll $engine
  if (Test-Path package\licenses) { Copy-Item -Recurse -Force package\licenses (Join-Path $engine 'licenses') }
  Copy-Item package\contract.json $engine
} finally {
  Pop-Location
}
Write-Host "Engine $Version staged in $engine"
