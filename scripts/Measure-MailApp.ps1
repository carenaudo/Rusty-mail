[CmdletBinding()]
param([ValidateSet(100000,500000)][int]$Messages=100000)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$exe=Join-Path $root 'src-tauri/target/release/rusty-mail.exe'
$fixture=Join-Path $root ".release/benchmarks/conversations-$Messages.db"
$ready=[IO.Path]::ChangeExtension($fixture,'startup.json')
if(!(Test-Path -LiteralPath $fixture)){throw 'Generate the marked synthetic conversation fixture first'}
# An old binary would ignore the diagnostic flag and open the regular profile.
if(![Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe)).Contains('--synthetic-performance-profile')){throw 'Build the new diagnostic-capable binary first'}
if(Test-Path -LiteralPath $ready){Remove-Item -LiteralPath $ready}
$app=Start-Process -FilePath $exe -ArgumentList @('--synthetic-performance-profile',('"'+$fixture+'"')) -WindowStyle Hidden -PassThru
function Sample-Tree {
 $all=@(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId)
 $ids=[Collections.Generic.HashSet[int]]::new();[void]$ids.Add($app.Id)
 do { $added=$false;foreach($p in $all){if($ids.Contains([int]$p.ParentProcessId)-and $ids.Add([int]$p.ProcessId)){$added=$true}} }while($added)
 $processes=@(foreach($id in $ids){Get-Process -Id $id -ErrorAction SilentlyContinue})
 [pscustomobject]@{cpu_seconds=($processes|Measure-Object CPU -Sum).Sum;private_bytes=($processes|Measure-Object PrivateMemorySize64 -Sum).Sum;working_set_bytes=($processes|Measure-Object WorkingSet64 -Sum).Sum;processes=$processes.Count}
}
try {
 $deadline=[DateTime]::UtcNow.AddSeconds(90)
 while(!(Test-Path -LiteralPath $ready)){
  if($app.HasExited){throw "Synthetic app exited with $($app.ExitCode)"}
  if([DateTime]::UtcNow -gt $deadline){throw 'Synthetic list did not paint within 90 seconds'}
  Start-Sleep -Milliseconds 500
 }
 $startup=Get-Content -LiteralPath $ready -Raw|ConvertFrom-Json
 if($startup.rows -ne 100 -or !$startup.synthetic){throw 'Expected a painted 100-row synthetic list'}
 Start-Sleep -Seconds 10
 $before=Sample-Tree;$watch=[Diagnostics.Stopwatch]::StartNew()
 Start-Sleep -Seconds 30
 $after=Sample-Tree;$watch.Stop()
 $cpu=[Math]::Max(0,($after.cpu_seconds-$before.cpu_seconds)/$watch.Elapsed.TotalSeconds*100)
 $result=[ordered]@{measured_utc=[DateTime]::UtcNow.ToString('o');messages=$Messages;profile='release';executable_sha256=(Get-FileHash -LiteralPath $exe -Algorithm SHA256).Hash.ToLower();startup=$startup;settle_seconds=10;idle_seconds=$watch.Elapsed.TotalSeconds;idle_cpu_one_core_percent=$cpu;idle_cpu_machine_percent=$cpu/[Environment]::ProcessorCount;before=$before;after=$after;notes='Single synthetic local run; background provider sync disabled; private bytes are committed memory; summed working sets can double-count shared pages. Not a real-account or cold-boot measurement.'}
 $out=Join-Path $root "reports/performance/app-compose-$Messages.json"
 $result|ConvertTo-Json -Depth 5|Set-Content -LiteralPath $out -Encoding utf8
 Write-Output $out
} finally {
 if(!$app.HasExited){Stop-Process -Id $app.Id -ErrorAction SilentlyContinue}
}
