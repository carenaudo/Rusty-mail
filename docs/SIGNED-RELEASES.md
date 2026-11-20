# Windows installers and signed updates

Last checked: October 29, 2026. This repository has a repeatable release build script and an optional official Tauri updater integration. **A production-signed release has not been created:** the publisher's certificate, updater key pair, hosting URL and release account are not configured.

The signed-build script also blocks unresolved dependency notices. The Vite build records packages with rendered code in `reports/frontend-packages.json`; notice generation checks that inventory against the lockfile. This excludes compiler-only npm tools instead of assuming every installed package ships. Supplemental upstream notices are preserved in `third-party-licenses/` with provenance. A passing automated check still requires release review of the generated notices.

## Two signatures with separate purposes

| Mechanism | Purpose | Required material |
|---|---|---|
| Windows Authenticode | Identify the publisher and detect changes to the EXE/installer | A code-signing certificate and an accessible private key, through a certificate-store/HSM or cloud signing provider |
| Tauri updater signature | Let an installed Rusty Mail verify a downloaded installer against its pinned trust key | A Tauri/minisign private signing key; only its public key is embedded in the app |

An updater signature does not establish Windows publisher trust. Authenticode alone does not satisfy Tauri's updater. Self-signed Authenticode certificates are useful only on explicitly configured test machines; do not distribute them as publicly trusted releases. Signing also does not guarantee the absence of SmartScreen prompts. [Tauri Windows signing](https://v2.tauri.app/distribute/sign/windows/)

## 1. Prepare the build machine

Install the Rust toolchain, Node/npm, Visual Studio C++ build tools and Windows SDK, WebView2, and native Strawberry Perl for vendored OpenSSL/SQLCipher. Ensure the SDK's SignTool and your signing provider's tools are available. Build from a reviewed checkout with committed lockfiles. Run:

```powershell
npm.cmd ci
npm.cmd run check
npm.cmd run build
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-features
powershell.exe -NoProfile -File scripts/Test-Scripts.ps1
```

The app, Cargo package and Tauri configuration versions must agree. Check dependency advisories/licenses and create notices before release, using [TESTING.md](TESTING.md). The default bundle is per-user Windows x86_64 NSIS. The installer downloads Microsoft's WebView2 bootstrapper only when needed; the shared runtime is excluded from the app-size budget.

## 2. Make an unsigned development installer

```powershell
powershell.exe -NoProfile -File scripts/Build-Release.ps1 -Mode Development
```

This produces an installer under `src-tauri/target/release/bundle/nsis/`, plus `.release/build-report.json`. The report records mode, version, size and SHA-256. This is explicitly an unsigned build with no active update channel. `-DebugBuild` is available for a quicker packaging smoke check; its size/performance is not a release measurement.

## 3. Configure Authenticode

Choose either a certificate available through your Windows certificate store or a cloud/hardware signing provider. Obtain and provision the certificate through the provider's instructions; do not assume a modern public certificate can be exported as a PFX.

For a compatible existing PFX, import it yourself with an interactive password prompt:

```powershell
$pfxPassword = Read-Host 'PFX password' -AsSecureString
Import-PfxCertificate -FilePath 'C:\Secure\publisher.pfx' `
  -CertStoreLocation Cert:\CurrentUser\My -Password $pfxPassword
Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
  Select-Object Subject, Thumbprint, NotAfter, HasPrivateKey
```

Record the intended certificate's thumbprint and the timestamp URL recommended by its provider. Do not commit the PFX or put its password on a command line. The build script configures SHA-256 and RFC 3161 timestamping. For nonexportable/cloud keys, supply `-SignCommand` instead, using a signing command already tested with your provider. Tauri substitutes `%1` with the file path. [SignTool reference](https://learn.microsoft.com/en-us/windows/win32/seccrypto/signtool), [timestamping](https://learn.microsoft.com/en-us/windows/win32/seccrypto/time-stamping-authenticode-signatures)

## 4. Generate the updater key pair

Keep the private key **outside the repository**, with an encrypted offline backup and restricted release access. Generate once, not for each release:

```powershell
$keyDirectory = Join-Path $env:USERPROFILE '.rusty-mail-signing'
New-Item -ItemType Directory -Force -Path $keyDirectory
npm.cmd run tauri -- signer generate -w (Join-Path $keyDirectory 'updater.key')
```

Follow the CLI's password prompt. Keep `updater.key` private; `updater.key.pub` is public. Losing the private key prevents normal updates to clients trusting that key. Rotate keys through an update still trusted by the previous key before retiring it. Never paste private keys into chat, build logs, issue trackers or frontend environment variables. [Tauri updater signing](https://v2.tauri.app/plugin/updater/)

## 5. Build both signatures

Set the signing key and its password in the **build process**, using your secret manager or interactive input. `.env` files are not loaded by this script or by Tauri's signer. For a local session:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY = Join-Path $keyDirectory 'updater.key'
$secret = Read-Host 'Updater key password' -AsSecureString
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = [Net.NetworkCredential]::new('', $secret).Password

powershell.exe -NoProfile -File scripts/Build-Release.ps1 -Mode Signed `
  -UpdatePublicKeyFile (Join-Path $keyDirectory 'updater.key.pub') `
  -UpdateEndpoint 'https://YOUR-HOST/releases/latest.json' `
  -CertificateThumbprint 'YOUR-40-HEX-CERTIFICATE-THUMBPRINT' `
  -TimestampUrl 'https://YOUR-CERTIFICATE-PROVIDER/TIMESTAMP'

Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY_PASSWORD
Remove-Item Env:\TAURI_SIGNING_PRIVATE_KEY
```

Replace all placeholders. With a cloud provider, replace `-CertificateThumbprint` and `-TimestampUrl` with your trusted `-SignCommand`. Do not pass both methods.

The script writes a local ignored configuration overlay, enables the `signed-updates` Cargo feature, embeds the **public key contents** and your HTTPS endpoint, and enables updater artifact signatures. It uses the pinned updater's `requireSignedVersion=true`: the installer signature must bind the version announced in the manifest. The installed CLI's `tauri build` does this automatically. If signing manually, use `npm.cmd run tauri -- signer sign --app-version VERSION FILE`.

The build fails if Authenticode verification is not `Valid`, the selected signer is wrong, or an updater `.sig` file is missing. No script uploads or publishes a release. Never modify or re-sign an installer after its updater `.sig` is generated: that changes its bytes and invalidates the updater signature.

## 6. Create and publish the update manifest

Build version 0.1.1 or higher when testing an update from 0.1.0. Use the exact final installer and its adjacent `.sig`:

```powershell
powershell.exe -NoProfile -File scripts/New-UpdateManifest.ps1 `
  -Installer 'src-tauri\target\release\bundle\nsis\ACTUAL-INSTALLER-NAME.exe' `
  -DownloadUrl 'https://YOUR-HOST/releases/0.1.1/ACTUAL-INSTALLER-NAME.exe' `
  -Version '0.1.1' -Notes 'Describe the changes.'
```

This creates `.release/latest.json` with the `windows-x86_64` platform entry. Upload the unchanged versioned installer first; verify the HTTPS download and checksum; then publish the manifest at the baked-in update endpoint. Avoid mutable installer URLs. The manifest carries the detached signature; a public `.sig` download may also be useful for independent verification. Keep the previous signed installer for recovery.

## 7. Verify before publication

```powershell
signtool.exe verify /pa /all /v 'PATH-TO-rusty-mail.exe'
signtool.exe verify /pa /all /v 'PATH-TO-setup.exe'
Get-FileHash 'PATH-TO-setup.exe' -Algorithm SHA256
```

In an isolated Windows test user or VM, install N, launch it, and use **Settings → Check for signed updates** against staging N+1. Review the offered version and choose **Download, verify and install**. Windows installation closes the app. Confirm mail-cache/draft retention after reopening. The app refuses installation while mail operations are pending/running. Test a corrupted installer, wrong key, substituted version, invalid HTTPS endpoint, interrupted download and unavailable endpoint: none should execute an unverified installer. A failed signature test is not a reason to disable verification.

No update is checked or installed automatically. Development builds report that no signed update channel is configured. Frontend commands cannot choose arbitrary update URLs or trust keys.

## CI and release gates

Keep production signing in a protected CI environment with manual approval. Use short-lived cloud signing credentials where available. Do not expose signing secrets to pull-request builds, untrusted branches or fork workflows. Run the same build script on the release runner, then upload artifacts for review; publishing the installer/manifest remains a distinct release action. Record source revision, toolchain, lockfile hashes, dependency review, signer identity, timestamp, updater public-key fingerprint and artifact SHA-256 with each release.

Blocking items before a public release: real-provider workflow validation, hardware-appropriate performance/soak results, outstanding security findings, third-party notices, an available trusted certificate/key, staging update verification, a published privacy policy and applicable provider OAuth approval. Do not mark any item passed merely because the associated script exists.
