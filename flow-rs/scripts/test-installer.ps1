<#
.SYNOPSIS
  Installs the sandbox installer into a temporary folder, dictates once with the
  installed Flow.exe (test mode, fake microphone, its own settings folder), then
  uninstalls and checks that nothing is left behind. A real Flow install, its
  shortcut, uninstall entry and settings are not touched.
.PARAMETER SkipBuild
  Package the existing target\release\Flow.exe.
#>
param([switch] $SkipBuild)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent          # flow-rs
$repo = Split-Path $root -Parent

& (Join-Path $PSScriptRoot 'package.ps1') -Sandbox -SkipBuild:$SkipBuild
$installer = Get-ChildItem (Join-Path $root 'dist') -Filter 'Flow-Setup-*-sandbox.exe' | Select-Object -First 1
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\computer.flow.dictation.sandbox'
$shortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Flow (sandbox).lnk'
$dir = Join-Path ([IO.Path]::GetTempPath()) "flow-install-test-$PID"
$userData = Join-Path ([IO.Path]::GetTempPath()) "flow-install-test-data-$PID"
$failures = @()
function Check([bool] $ok, [string] $what) {
  if ($ok) { Write-Host "  ok    $what" } else { Write-Host "  FAIL  $what"; $script:failures += $what }
}

Write-Host "Installing into $dir"
$timer = [Diagnostics.Stopwatch]::StartNew()
# NSIS wants /D last and unquoted.
Start-Process $installer.FullName -ArgumentList '/S', "/D=$dir" -Wait
Write-Host "  installed in $($timer.ElapsedMilliseconds) ms"
foreach ($file in 'Flow.exe', 'Uninstall Flow.exe', 'engine\transcribe.dll', 'engine\vcruntime140.dll', 'resources\catalog.json', 'resources\catalog.json.sig', 'LICENSE') {
  Check (Test-Path (Join-Path $dir $file)) "installed $file"
}
Check ((Get-ItemProperty $key -ErrorAction SilentlyContinue).InstallLocation -eq $dir) 'uninstall entry points at the install folder'
Check (Test-Path $shortcut) 'Start-menu shortcut'
$size = (Get-ChildItem $dir -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("  {0:N1} MB on disk" -f ($size / 1MB))

Write-Host 'Dictating with the installed Flow.exe'
New-Item -ItemType Directory -Force $userData | Out-Null
[IO.File]::WriteAllText((Join-Path $userData 'config.toml'), "version = 1`n`n[general]`nonboarded = true`n")
$info = New-Object Diagnostics.ProcessStartInfo (Join-Path $dir 'Flow.exe'), '--hidden'
$info.UseShellExecute = $false
$info.RedirectStandardInput = $true
$info.RedirectStandardOutput = $true
$info.EnvironmentVariables.Remove('ELECTRON_RUN_AS_NODE')
$info.EnvironmentVariables['FLOW_TEST'] = '1'
$info.EnvironmentVariables['FLOW_USER_DATA'] = $userData
$info.EnvironmentVariables['FLOW_MODELS_DIR'] = Join-Path $env:LOCALAPPDATA 'Flow\models'
$info.EnvironmentVariables['FLOW_FAKE_MIC'] = Join-Path $repo 'tests\fixtures\jfk.wav'
$flow = [Diagnostics.Process]::Start($info)
function Wait-Line([string] $type, [int] $seconds) {
  $deadline = (Get-Date).AddSeconds($seconds)
  while ((Get-Date) -lt $deadline) {
    $task = $flow.StandardOutput.ReadLineAsync()
    if (-not $task.Wait([int] [math]::Max(1, ($deadline - (Get-Date)).TotalMilliseconds))) { return $null }
    if ($null -eq $task.Result) { return $null }
    $line = $task.Result | ConvertFrom-Json -ErrorAction SilentlyContinue
    if ($line -and $line.type -eq $type) { return $line }
  }
  return $null
}
try {
  Check ($null -ne (Wait-Line 'ready' 20)) 'starts in the tray'
  $flow.StandardInput.WriteLine('hotkey hold-start')
  Start-Sleep -Milliseconds 400
  $flow.StandardInput.WriteLine('hotkey hold-end')
  $insert = Wait-Line 'insert' 90
  Check ($insert -and $insert.text -match 'ask not what your country can do for you') "transcribes with the local model: $($insert.text)"
  $flow.StandardInput.WriteLine('quit')
  Check ($flow.WaitForExit(5000)) 'quits on request'
} finally {
  if (-not $flow.HasExited) { $flow.Kill() }
  Remove-Item -Recurse -Force $userData -ErrorAction SilentlyContinue
}

Write-Host 'Uninstalling'
# The uninstaller copies itself to %TEMP% and returns at once; wait for the folder to go.
Start-Process (Join-Path $dir 'Uninstall Flow.exe') -ArgumentList '/currentuser', '/S' -Wait
$deadline = (Get-Date).AddSeconds(30)
while ((Test-Path $dir) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
Check (-not (Test-Path $dir)) 'install folder removed'
Check (-not (Test-Path $key)) 'uninstall entry removed'
Check (-not (Test-Path $shortcut)) 'shortcut removed'

if ($failures.Count) { throw "$($failures.Count) installer check(s) failed" }
Write-Host 'Installer test passed'
