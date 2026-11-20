param([ValidateRange(0.0001,168)][double]$Hours=72,[ValidateRange(100,500000)][int]$MessageCount=100000,[Parameter(Mandatory)][string]$ReportDirectory)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$directory=[IO.Path]::GetFullPath($ReportDirectory)
$allowed=[IO.Path]::GetFullPath((Join-Path $root 'reports/soak'))+[IO.Path]::DirectorySeparatorChar
if(-not $directory.StartsWith($allowed,[StringComparison]::OrdinalIgnoreCase)){throw 'Soak reports must stay inside reports/soak/<run>'}
New-Item -ItemType Directory -Path $directory -Force|Out-Null
$env:RUSTYMAIL_SOAK_SECONDS=[Math]::Ceiling($Hours*3600).ToString('0')
$env:RUSTYMAIL_SOAK_MESSAGES=$MessageCount.ToString()
$env:RUSTYMAIL_SOAK_OUTPUT=Join-Path $directory 'workload.json'
$start=[DateTime]::UtcNow
$summaryPath=Join-Path $directory 'summary.json'
$samples=Join-Path $directory 'resources.jsonl'
$manifest=Join-Path $root 'src-tauri/Cargo.toml'
$sourceFiles=@(Get-ChildItem -LiteralPath (Join-Path $root 'src-tauri/src') -File)+@(Get-ChildItem -LiteralPath (Join-Path $root 'src') -File)+@(Get-Item -LiteralPath $manifest,(Join-Path $root 'src-tauri/Cargo.lock'),(Join-Path $root 'src-tauri/tauri.conf.json'),(Join-Path $root 'package-lock.json'),$PSCommandPath)
$hashes=@($sourceFiles|ForEach-Object {@{path=$_.FullName;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}})
@{startedUtc=$start.ToString('o');os=[Environment]::OSVersion.VersionString;logicalProcessors=[Environment]::ProcessorCount;sources=$hashes}|ConvertTo-Json -Depth 5|Set-Content -LiteralPath (Join-Path $directory 'environment.json')
$process=Start-Process -FilePath (Get-Command cargo.exe).Source -ArgumentList @('test','--offline','--locked','--manifest-path',('"'+$manifest+'"'),'synthetic_soak','--','--ignored','--nocapture') -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput (Join-Path $directory 'stdout.log') -RedirectStandardError (Join-Path $directory 'stderr.log') -PassThru
$null=$process.Handle # Retain the process handle so Windows PowerShell preserves ExitCode after exit.
$maxBytes=0L;$previousCpu=0.0;$previousTime=$start;$cpuSamples=@();$baselineCaptured=$false;$maxGapSeconds=0.0
do{
 $process.Refresh()
 $all=Get-CimInstance Win32_Process
 $ids=@([int]$process.Id)
 do{$children=@($all|Where-Object {$_.ParentProcessId -in $ids -and $_.ProcessId -notin $ids}|ForEach-Object {[int]$_.ProcessId});$ids+=$children}while($children.Count)
 $live=@($ids|ForEach-Object {Get-Process -Id $_ -ErrorAction SilentlyContinue})
 $bytes=[long](($live|Measure-Object PrivateMemorySize64 -Sum).Sum)
 $cpu=[double](($live|Measure-Object CPU -Sum).Sum)
 $time=[DateTime]::UtcNow;$span=[Math]::Max(.001,($time-$previousTime).TotalSeconds)
 if($baselineCaptured){$maxGapSeconds=[Math]::Max($maxGapSeconds,$span)}
 $percent=100*[Math]::Max(0,$cpu-$previousCpu)/$span/[Environment]::ProcessorCount
 $previousCpu=$cpu;$previousTime=$time;$maxBytes=[Math]::Max($maxBytes,$bytes);if($baselineCaptured){$cpuSamples+=,$percent}else{$percent=0;$baselineCaptured=$true}
 @{utc=$time.ToString('o');privateBytes=$bytes;machineCpuPercent=$percent;processIds=$ids}|ConvertTo-Json -Compress|Add-Content -LiteralPath $samples
 @{status='running';syntheticOnly=$true;supervisorPid=$PID;cargoPid=$process.Id;startedUtc=$start.ToString('o');updatedUtc=$time.ToString('o');requestedHours=$Hours;messages=$MessageCount;peakPrivateBytes=$maxBytes;maxSamplingGapSeconds=$maxGapSeconds;continuityValid=($maxGapSeconds -le 60)}|ConvertTo-Json|Set-Content -LiteralPath $summaryPath
 if(-not $process.HasExited){Start-Sleep -Seconds 10}
}while(-not $process.HasExited)
$process.WaitForExit()
$workload=$null
if(Test-Path -LiteralPath $env:RUSTYMAIL_SOAK_OUTPUT){$workload=Get-Content -LiteralPath $env:RUSTYMAIL_SOAK_OUTPUT -Raw|ConvertFrom-Json}
$passed=$process.ExitCode -eq 0 -and $workload.status -eq 'complete' -and $workload.elapsed_seconds -ge [double]$env:RUSTYMAIL_SOAK_SECONDS -and $maxGapSeconds -le 60
@{status=if($passed){'passed'}else{'failed-or-interrupted'};syntheticOnly=$true;startedUtc=$start.ToString('o');finishedUtc=[DateTime]::UtcNow.ToString('o');requestedHours=$Hours;workload=$workload;exitCode=$process.ExitCode;peakPrivateBytes=$maxBytes;maxSamplingGapSeconds=$maxGapSeconds;continuityValid=($maxGapSeconds -le 60);averageSampledMachineCpuPercent=($cpuSamples|Measure-Object -Average).Average}|ConvertTo-Json -Depth 8|Set-Content -LiteralPath $summaryPath
if(-not $passed){exit 1}

