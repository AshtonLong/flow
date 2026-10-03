# Measures the packaged app against the footprint budgets in SPEC.md:
# cold start to tray-ready, idle memory and idle CPU. Run after `pnpm dist`.
$ErrorActionPreference = 'Stop'
$exe = Join-Path $PSScriptRoot '..\release\win-unpacked\Flow.exe' | Resolve-Path
$env:ELECTRON_RUN_AS_NODE = $null

$profileDir = Join-Path $env:TEMP ('flow-idle-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $profileDir | Out-Null
[IO.File]::WriteAllText((Join-Path $profileDir 'config.toml'), "version = 1`n[general]`nonboarded = true`n")
$env:FLOW_USER_DATA = $profileDir
$log = Join-Path $profileDir 'logs\flow.log'

$watch = [Diagnostics.Stopwatch]::StartNew()
Start-Process -FilePath $exe -ArgumentList '--hidden' | Out-Null
while ($watch.ElapsedMilliseconds -lt 15000) {
  if ((Test-Path $log) -and (Select-String -Path $log -Pattern 'tray ready' -Quiet)) { break }
  Start-Sleep -Milliseconds 25
}
"Cold start to tray-ready: $($watch.ElapsedMilliseconds) ms"

Start-Sleep -Seconds 12
$ids = Get-CimInstance Win32_Process -Filter "Name='Flow.exe'" |
  Where-Object { $_.ExecutablePath -eq $exe.Path } |
  ForEach-Object { $_.ProcessId }
$procs = $ids | ForEach-Object { Get-Process -Id $_ }
$working = ($procs | Measure-Object -Property WorkingSet64 -Sum).Sum
$private = ($procs | Measure-Object -Property PrivateMemorySize64 -Sum).Sum
"Processes: $($procs.Count)"
"Idle memory: $([math]::Round($working / 1MB)) MB working set, $([math]::Round($private / 1MB)) MB private"

$before = ($procs | ForEach-Object { $_.TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
Start-Sleep -Seconds 10
$after = ($ids | ForEach-Object { (Get-Process -Id $_).TotalProcessorTime.TotalMilliseconds } | Measure-Object -Sum).Sum
"Idle CPU: $([math]::Round($after - $before)) ms of CPU time over 10 s"

$ids | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
Start-Sleep -Seconds 1
[IO.Directory]::Delete($profileDir, $true)
