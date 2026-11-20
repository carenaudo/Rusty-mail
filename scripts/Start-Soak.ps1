[CmdletBinding()]
param([ValidateRange(0.0001,168)][double]$Hours=72,[ValidateRange(100,500000)][int]$MessageCount=100000)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
$directory=Join-Path $root ('reports/soak/'+[DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss')+'-'+[Guid]::NewGuid().ToString('N').Substring(0,6))
New-Item -ItemType Directory -Force -Path $directory|Out-Null
$script=Join-Path $PSScriptRoot 'Run-Soak.ps1'
$hoursText=$Hours.ToString([Globalization.CultureInfo]::InvariantCulture)
$arguments="-NoProfile -File `"$script`" -Hours $hoursText -MessageCount $MessageCount -ReportDirectory `"$directory`""
$process=Start-Process -FilePath powershell.exe -ArgumentList $arguments -WorkingDirectory $root -WindowStyle Hidden -RedirectStandardOutput (Join-Path $directory 'supervisor.stdout.log') -RedirectStandardError (Join-Path $directory 'supervisor.stderr.log') -PassThru
@{supervisorPid=$process.Id;reportDirectory=$directory;requestedHours=$Hours;syntheticOnly=$true}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $directory 'launch.json')
Write-Output "Synthetic soak started. Supervisor PID: $($process.Id). Reports: $directory. It has not passed until summary.json says passed."
