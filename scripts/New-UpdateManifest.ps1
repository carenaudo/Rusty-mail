[CmdletBinding()]
param([Parameter(Mandatory)][string]$Installer,[Parameter(Mandatory)][string]$DownloadUrl,[Parameter(Mandatory)][string]$Version,[string]$Notes='',[string]$OutputPath='.release/latest.json')
$ErrorActionPreference='Stop'
if($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$'){throw 'Supply a semantic release version'}
$uri=$null
if(-not[Uri]::TryCreate($DownloadUrl,[UriKind]::Absolute,[ref]$uri) -or $uri.Scheme -ne 'https' -or $uri.UserInfo){throw 'Installer URL must be HTTPS and contain no credentials'}
if(-not(Test-Path -LiteralPath $Installer)){throw 'Installer does not exist'}
if([IO.Path]::GetExtension($Installer) -ne '.exe'){throw 'This project publishes Windows x86_64 NSIS .exe installers'}
if((Get-AuthenticodeSignature -LiteralPath $Installer).Status -ne 'Valid'){throw 'Installer must pass Authenticode verification before creating a release manifest'}
$signaturePath=$Installer+'.sig'
if(-not(Test-Path -LiteralPath $signaturePath)){throw 'Missing Tauri .sig file; do not sign or modify the installer after this file is generated'}
$signature=(Get-Content -LiteralPath $signaturePath -Raw).Trim()
try{$null=[Convert]::FromBase64String($signature)}catch{throw 'Invalid updater signature encoding'}
$manifest=@{version=$Version;notes=$Notes;pub_date=[DateTime]::UtcNow.ToString('o');platforms=@{'windows-x86_64'=@{url=$DownloadUrl;signature=$signature}}}
$parent=Split-Path ([IO.Path]::GetFullPath($OutputPath)) -Parent
New-Item -ItemType Directory -Path $parent -Force|Out-Null
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath),($manifest|ConvertTo-Json -Depth 8),[Text.UTF8Encoding]::new($false))
Write-Output "Created $OutputPath. Verify the signature/version pairing and upload the unchanged installer and manifest yourself."

