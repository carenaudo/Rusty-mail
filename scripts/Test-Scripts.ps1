$ErrorActionPreference='Stop'
Get-ChildItem -LiteralPath $PSScriptRoot -Filter '*.ps1'|ForEach-Object {
 $tokens=$null;$parseErrors=$null
 $null=[System.Management.Automation.Language.Parser]::ParseFile($_.FullName,[ref]$tokens,[ref]$parseErrors)
 if($parseErrors.Count){throw ($parseErrors|Out-String)}
}
& (Join-Path $PSScriptRoot 'Build-Release.ps1') -ValidateOnly
if($LASTEXITCODE){throw 'Release preflight failed'}
function Assert-Rejected([scriptblock]$Action,[string]$Expected){
 $rejected=$false
 try{& $Action}catch{if($_.Exception.Message -notlike ('*'+$Expected+'*')){throw};$rejected=$true}
 if(-not $rejected){throw "Expected rejection: $Expected"}
}
$release=Join-Path $PSScriptRoot 'Build-Release.ps1'
$manifest=Join-Path $PSScriptRoot 'New-UpdateManifest.ps1'
$previousKey=$env:TAURI_SIGNING_PRIVATE_KEY
try{
 $env:TAURI_SIGNING_PRIVATE_KEY=$null
 Assert-Rejected {& $release -Mode Signed -ValidateOnly} 'TAURI_SIGNING_PRIVATE_KEY'
}finally{$env:TAURI_SIGNING_PRIVATE_KEY=$previousKey}
Assert-Rejected {& $manifest -Installer 'nonexistent.exe' -DownloadUrl 'http://updates.example/installer.exe' -Version '1.0.0'} 'HTTPS'
Assert-Rejected {& $manifest -Installer 'nonexistent.exe' -DownloadUrl 'https://updates.example/installer.exe' -Version 'invalid'} 'semantic'
Assert-Rejected {& $manifest -Installer 'nonexistent.exe' -DownloadUrl 'https://user:password@updates.example/installer.exe' -Version '1.0.0'} 'credentials'
Write-Output 'PowerShell scripts parse; development preflight and signed-release/manifest rejection checks pass.'
