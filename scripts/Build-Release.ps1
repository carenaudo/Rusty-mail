[CmdletBinding()]
param(
 [ValidateSet('Development','Signed')][string]$Mode='Development',
 [string]$UpdatePublicKeyFile,
 [string]$UpdateEndpoint,
 [string]$CertificateThumbprint,
 [string]$TimestampUrl,
 [string]$SignCommand,
 [switch]$DebugBuild,
 [switch]$ValidateOnly
)
$ErrorActionPreference='Stop'
$root=Split-Path $PSScriptRoot -Parent
Set-Location -LiteralPath $root
$package=Get-Content package.json -Raw|ConvertFrom-Json
$config=Get-Content src-tauri/tauri.conf.json -Raw|ConvertFrom-Json
if($package.version -ne $config.version){throw 'package.json and tauri.conf.json versions must match'}
$cargoText=Get-Content src-tauri/Cargo.toml -Raw
if($cargoText -notmatch ('(?m)^version\s*=\s*"'+[regex]::Escape($package.version)+'"')){throw 'Cargo package version must match package.json'}
$overlay=@{bundle=@{active=$true;targets=@('nsis');createUpdaterArtifacts=$false}}
if($Mode -eq 'Signed'){
 if(-not $env:TAURI_SIGNING_PRIVATE_KEY){throw 'Set TAURI_SIGNING_PRIVATE_KEY in this process. Do not put private keys in this repository.'}
 if(-not $UpdatePublicKeyFile -or -not(Test-Path -LiteralPath $UpdatePublicKeyFile)){throw 'Supply the public .pub key file'}
 $publicKey=(Get-Content -LiteralPath $UpdatePublicKeyFile -Raw).Trim()
 try{$decoded=[Convert]::FromBase64String($publicKey)}catch{throw 'Updater public key must be Tauri signer output, not a path or placeholder'}
 if($decoded.Length -lt 40){throw 'Invalid updater public key'}
 $uri=$null
 if(-not[Uri]::TryCreate($UpdateEndpoint,[UriKind]::Absolute,[ref]$uri) -or $uri.Scheme -ne 'https' -or $uri.UserInfo -or $uri.Fragment){throw 'Supply your HTTPS update-manifest URL, without credentials or fragments'}
 if([bool]$CertificateThumbprint -eq [bool]$SignCommand){throw 'Supply exactly one Authenticode method: CertificateThumbprint or SignCommand'}
 $windows=@{}
 if($CertificateThumbprint){
  if($CertificateThumbprint -notmatch '^[A-Fa-f0-9]{40}$'){throw 'Use the 40-hex certificate thumbprint from the Windows certificate store'}
  $timestamp=$null
  if(-not[Uri]::TryCreate($TimestampUrl,[UriKind]::Absolute,[ref]$timestamp) -or $timestamp.Scheme -notin @('http','https')){throw 'Supply the certificate provider timestamp URL'}
  if(-not $ValidateOnly){$cert=Get-Item -LiteralPath "Cert:\CurrentUser\My\$CertificateThumbprint";if(-not $cert.HasPrivateKey -or $cert.NotAfter -lt [DateTime]::UtcNow){throw 'Signing certificate must have an available private key and must not be expired'}}
  $windows=@{certificateThumbprint=$CertificateThumbprint;digestAlgorithm='sha256';timestampUrl=$TimestampUrl;tsp=$true}
 }else{$windows=@{signCommand=$SignCommand}}
 $overlay.bundle.windows=$windows
 $overlay.bundle.createUpdaterArtifacts=$true
 $overlay.plugins=@{updater=@{pubkey=$publicKey;endpoints=@($UpdateEndpoint);requireSignedVersion=$true;windows=@{installMode='basicUi'}}}
 $notices=Join-Path $root 'reports/licenses.json'
 if(-not(Test-Path -LiteralPath $notices)){throw 'Generate and review dependency notices before a signed release'}
 $unresolved=@((Get-Content -LiteralPath $notices -Raw|ConvertFrom-Json)|Where-Object {$_.licenseFiles.Count -eq 0})
 if($unresolved.Count){throw ('Complete missing license notices before a signed release: '+(($unresolved|ForEach-Object {$_.name+'@'+$_.version}) -join ', '))}
}
if($DebugBuild -and $Mode -eq 'Signed'){throw 'Signed release builds must use the release profile'}
if($ValidateOnly){Write-Output "Release configuration validated ($Mode). No build, key generation, signing, or publication performed.";return}
$out=Join-Path $root '.release'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$overlayPath=Join-Path $out 'tauri.override.json'
[IO.File]::WriteAllText($overlayPath,($overlay|ConvertTo-Json -Depth 12),[Text.UTF8Encoding]::new($false))
$buildArguments=@('run','tauri','--','build','--config',$overlayPath,'--bundles','nsis')
if($Mode -eq 'Signed'){$buildArguments+=@('--features','signed-updates')}
if($DebugBuild){$buildArguments+='--debug'}
& npm.cmd @buildArguments
if($LASTEXITCODE -ne 0){throw "Tauri build failed ($LASTEXITCODE)"}
$profile=if($DebugBuild){'debug'}else{'release'}
$app=Join-Path $root "src-tauri/target/$profile/rusty-mail.exe"
$installers=@(Get-ChildItem -LiteralPath (Join-Path $root "src-tauri/target/$profile/bundle/nsis") -Filter '*.exe' | Where-Object {$_.Name -like "*$($package.version)*"})
if($installers.Count -ne 1){throw 'Expected exactly one installer for the current version; inspect the output directory'}
if($Mode -eq 'Signed'){
 foreach($file in @($app,$installers[0].FullName)){
  $signature=Get-AuthenticodeSignature -LiteralPath $file
  if($signature.Status -ne 'Valid'){throw "Authenticode verification failed: $file"}
  if($CertificateThumbprint -and $signature.SignerCertificate.Thumbprint -ne $CertificateThumbprint){throw 'Unexpected Authenticode signer'}
 }
 if(-not(Test-Path -LiteralPath ($installers[0].FullName+'.sig'))){throw 'Missing Tauri updater signature'}
}
$report=@{version=$package.version;mode=$Mode;profile=$profile;installer=$installers[0].Name;installerBytes=$installers[0].Length;sha256=(Get-FileHash -LiteralPath $installers[0].FullName -Algorithm SHA256).Hash;createdUtc=[DateTime]::UtcNow.ToString('o')}
$report.noticesSha256=(Get-FileHash -LiteralPath (Join-Path $root 'THIRD_PARTY_NOTICES.md') -Algorithm SHA256).Hash
$report.authenticodeStatus=(Get-AuthenticodeSignature -LiteralPath $installers[0].FullName).Status.ToString()
$report.sourceHashes=@(Get-ChildItem -LiteralPath (Join-Path $root 'src-tauri/src') -File|Sort-Object Name|ForEach-Object {@{file=$_.Name;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash}})
$report|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $out 'build-report.json')
Write-Output "Built $($installers[0].FullName). Mode: $Mode. Nothing was uploaded or published."



