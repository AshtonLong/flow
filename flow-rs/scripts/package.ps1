<#
.SYNOPSIS
  Builds Flow in release mode and produces the installer and its update feed:
  dist\Flow-Setup-<version>.exe and dist\latest.yml (the electron-builder
  format the updater reads).
.PARAMETER SkipBuild
  Package the existing target\release\Flow.exe.
#>
param([switch] $SkipBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent          # flow-rs
$repo = Split-Path $root -Parent                  # the repository
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Packaging Flow $version"

if (-not $SkipBuild) {
  cargo build --release -p flow --bin Flow
  if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}

$stage = Join-Path $root 'dist\stage'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item target\release\Flow.exe $stage
& (Join-Path $PSScriptRoot 'stage-engine.ps1') -Dest $stage

# transcribe.dll is built with MSVC: ship its runtime next to it (app-local deployment),
# so machines without the Visual C++ Redistributable still run local models.
$crt = @('msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')
$redist = Get-ChildItem 'C:\Program Files\Microsoft Visual Studio\*\*\VC\Redist\MSVC\*\x64\Microsoft.VC14*.CRT' -Directory -ErrorAction SilentlyContinue |
  Sort-Object FullName -Descending | Select-Object -First 1
foreach ($dll in $crt) {
  $from = if ($redist) { Join-Path $redist.FullName $dll } else { Join-Path $env:SystemRoot "System32\$dll" }
  if (Test-Path $from) { Copy-Item $from (Join-Path $stage "engine\$dll") } else { Write-Warning "$dll not found; local models need the VC++ runtime" }
}

$resources = New-Item -ItemType Directory -Force (Join-Path $stage 'resources')
Copy-Item (Join-Path $repo 'resources\catalog.json'), (Join-Path $repo 'resources\catalog.json.sig') $resources
Copy-Item -Recurse (Join-Path $repo 'resources\icons') $resources
Copy-Item (Join-Path $repo 'LICENSE') $stage
New-Item -ItemType Directory -Force (Join-Path $stage 'licenses') | Out-Null
Copy-Item (Join-Path $root 'resources\fonts\OFL.txt') (Join-Path $stage 'licenses\Recursive-OFL.txt')
Copy-Item (Join-Path $root 'resources\icons\lucide\LICENSE') (Join-Path $stage 'licenses\lucide-LICENSE.txt')

# NSIS: on PATH, in its default folder, or under NSIS_HOME.
$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
foreach ($candidate in @("$env:NSIS_HOME\makensis.exe", 'C:\Program Files (x86)\NSIS\makensis.exe', 'C:\Program Files\NSIS\makensis.exe')) {
  if (-not $makensis -and $candidate -and (Test-Path $candidate)) { $makensis = $candidate }
}
if (-not $makensis) { throw 'makensis not found: install NSIS 3 or set NSIS_HOME' }

$dist = Join-Path $root 'dist'
$installerName = "Flow-Setup-$version.exe"
$installer = Join-Path $dist $installerName
Push-Location (Join-Path $root 'installer')
try {
  & $makensis /V2 "/DVERSION=$version" "/DSTAGE=$stage" "/DOUTFILE=$installer" flow.nsi
  if ($LASTEXITCODE -ne 0) { throw 'makensis failed' }
} finally { Pop-Location }

# The update feed, in the format electron-updater (and Flow's updater) reads.
$bytes = [IO.File]::ReadAllBytes($installer)
$sha512 = [Convert]::ToBase64String([Security.Cryptography.SHA512]::Create().ComputeHash($bytes))
$date = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ss.fffZ')
@"
version: $version
files:
  - url: $installerName
    sha512: $sha512
    size: $($bytes.Length)
path: $installerName
sha512: $sha512
releaseDate: '$date'
"@ | Set-Content -Encoding utf8NoBOM (Join-Path $dist 'latest.yml') -ErrorAction SilentlyContinue
if (-not (Test-Path (Join-Path $dist 'latest.yml'))) {
  # Windows PowerShell 5 has no utf8NoBOM.
  [IO.File]::WriteAllText((Join-Path $dist 'latest.yml'), "version: $version`nfiles:`n  - url: $installerName`n    sha512: $sha512`n    size: $($bytes.Length)`npath: $installerName`nsha512: $sha512`nreleaseDate: '$date'`n")
}
$mb = [math]::Round($bytes.Length / 1MB, 1)
Write-Host "Built $installerName ($mb MB) and latest.yml in $dist"
