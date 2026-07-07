# Measure Tracium's runtime footprint (CPU + RAM) on Windows, RELEASE build.
# Windows counterpart of bench.sh. NOT yet validated on Windows — see issue
# "Windows testing". Run from the repo root in PowerShell.
#
#   pnpm build ; cargo build --release -p tracium
#   ./scripts/bench.ps1 -SampleSecs 20 -SettleSecs 25
param([int]$SampleSecs = 20, [int]$SettleSecs = 25)

$bin = "target/release/tracium.exe"
if (-not (Test-Path $bin)) { Write-Error "$bin not found — build release first"; exit 1 }

Write-Host "launching $bin and settling ${SettleSecs}s…"
$app = Start-Process $bin -PassThru
Start-Sleep -Seconds $SettleSecs

# Tracium + its WebView2 (msedgewebview2) helper processes.
$names = @("tracium", "msedgewebview2")
$procs = Get-Process | Where-Object { $names -contains $_.ProcessName }

$ramMB = [math]::Round((($procs | Measure-Object WorkingSet64 -Sum).Sum) / 1MB, 0)

$c1 = ($procs | Measure-Object -Property TotalProcessorTime -Sum).Sum.TotalSeconds
Start-Sleep -Seconds $SampleSecs
$procs = Get-Process | Where-Object { $names -contains $_.ProcessName }
$c2 = ($procs | Measure-Object -Property TotalProcessorTime -Sum).Sum.TotalSeconds
$cpuPct = [math]::Round((($c2 - $c1) / $SampleSecs) * 100, 2)

Write-Host "================ Tracium footprint (release, idle, window open) ================"
Write-Host ("processes      : {0} (app + WebView2 helpers)" -f $procs.Count)
Write-Host ("RAM (WorkingSet): {0} MB" -f $ramMB)
Write-Host ("CPU idle       : {0}% of one core over {1}s" -f $cpuPct, $SampleSecs)
Write-Host "================================================================================"
Write-Host "NOTE: Windows uses the WebView2 runtime (not WebKitGTK); numbers differ from Linux."

$app | Stop-Process -ErrorAction SilentlyContinue
