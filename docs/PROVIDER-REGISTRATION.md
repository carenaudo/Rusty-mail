# Register and connect email providers

Last checked: October 29, 2026. This guide describes the code currently in this repository. Google/Microsoft use a **registered native desktop application and your system browser**. The client ID identifies the application; it is not your mailbox password. End users of a future published release should use the release's registration rather than create their own.

## Before starting

Use a dedicated test mailbox containing synthetic messages, not your primary inbox. Start Rusty Mail from the same PowerShell process where you set its environment variables. Close any existing Rusty Mail window first: the app permits one instance, and an existing process will retain its old environment. `.env.example` is an example, not an automatically loaded configuration file. Never use `VITE_` variables for credentials.

## Microsoft: Outlook.com, Hotmail and Microsoft 365

1. Sign in to [Microsoft Entra](https://entra.microsoft.com/). You need a directory where your account can register applications. If access is denied or no directory is available, ask your directory administrator; a personal mailbox alone does not guarantee application-registration permissions.
2. Open **Entra ID → App registrations → New registration**. Name it `Rusty Mail Development`.
3. Choose the audience that includes **any organizational directory and personal Microsoft accounts**. This matches Rusty Mail's `common` authority and supports work/school and Outlook/Hotmail accounts.
4. Register and copy **Application (client) ID** from Overview. Do not copy Object ID or Directory (tenant) ID.

These steps follow [Microsoft's application registration guide](https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app).

5. In **Authentication → Add a platform**, choose **Mobile and desktop applications**. Add the custom redirect URI:

   ```text
   http://localhost/oauth/callback
   ```

6. Enable **Allow public client flows** in Authentication's advanced settings and save. Do not create a client secret for this desktop client. Do not enable implicit access-token/ID-token grants.
7. In **API permissions → Add a permission → Microsoft Graph → Delegated permissions**, add `User.Read`, `Mail.ReadWrite`, `Mail.Send`, and `offline_access`. Organization policy may require administrator approval. Do not select application permissions: this app acts on behalf of the signed-in user.

The redirect's path is exact; the temporary port chosen by Rusty Mail is allowed by Microsoft's [localhost redirect matching rules](https://learn.microsoft.com/en-us/entra/identity-platform/reply-url). See also [desktop configuration](https://learn.microsoft.com/en-us/entra/identity-platform/scenario-desktop-app-configuration) and [Graph permissions](https://learn.microsoft.com/en-us/graph/permissions-reference).

```powershell
# Run these commands from the project root.
$env:RUSTYMAIL_MICROSOFT_CLIENT_ID = 'YOUR-APPLICATION-CLIENT-ID'
npm.cmd run tauri dev
```

In Settings, select **Continue with Microsoft**, complete Microsoft's browser login and consent, return to Rusty Mail, and select **Synchronize**. Test reading and moving a synthetic message before testing sending to another mailbox you control.

## Google: Gmail and Google Workspace

1. Open [Google Cloud Console](https://console.cloud.google.com/) and create or select a development project.
2. In **APIs & Services → Library**, enable the **Gmail API**.
3. Open **Google Auth Platform** (or the OAuth consent-screen area). Complete Branding and contact details. Select Internal only when you actually have a Workspace organization and only intend its users; otherwise use External.
4. In Audience, keep development/testing status and add the Gmail account you will use as a test user.
5. In Data Access, configure `https://www.googleapis.com/auth/gmail.modify`, the restricted scope used by this client for reading, mailbox actions, drafts and sending.
6. In Clients, create an OAuth client of type **Desktop app**. Copy its client ID. Keep the downloaded desktop credential file outside this repository.

Rusty Mail uses S256 PKCE and `http://127.0.0.1:<temporary-port>/oauth/callback`. A Desktop registration handles loopback redirects; do not create a Web application just to enter the development UI's port. If Google supplies a desktop client-secret value required by its token endpoint, configure it too. Google treats distributed desktop clients as public clients, so that value is not a confidential secret and PKCE remains essential. [Google native application flow](https://developers.google.com/identity/protocols/oauth2/native-app)

```powershell
$env:RUSTYMAIL_GOOGLE_CLIENT_ID = 'YOUR-ID.apps.googleusercontent.com'
# Only if required by the downloaded Desktop credentials:
$env:RUSTYMAIL_GOOGLE_CLIENT_SECRET = 'DESKTOP-CREDENTIAL-VALUE'
npm.cmd run tauri dev
```

Select **Continue with Google**, choose your listed test user, review access, then synchronize. Testing-mode authorizations can expire; reconnect when Google revokes or expires the refresh token. A public release needs the applicable OAuth verification process. Gmail data forwarded to a hosted model can introduce additional policy/security-assessment requirements; an API key alone does not resolve those requirements. [Gmail scopes](https://developers.google.com/workspace/gmail/api/auth/scopes), [restricted-scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification), [Workspace data policy](https://developers.google.com/workspace/workspace-api-user-data-developer-policy)

## iCloud

No developer application ID is required for the implemented IMAP/app-password route. On [account.apple.com](https://account.apple.com/), use **Sign-In and Security → App-Specific Passwords** to generate a password named `Rusty Mail Test`. Two-factor authentication is required. Use the app-specific password, never your primary Apple Account password. [Apple instructions](https://support.apple.com/en-us/102654)

In Rusty Mail, choose **Add iCloud, Fastmail or custom IMAP/SMTP**, select iCloud, and enter your email and the username Apple specifies for your mailbox. The preset uses `imap.mail.me.com:993` with implicit TLS and `smtp.mail.me.com:587` with required STARTTLS. Incoming and outgoing usernames can differ. Revoke the app-specific password on Apple's site when finished. [Apple server settings](https://support.apple.com/en-us/102525)

## Fastmail

Create an app password with Mail/IMAP/SMTP access in Fastmail's password/security settings. Use the Fastmail preset: `imap.fastmail.com:993`, `smtp.fastmail.com:587`, required STARTTLS for SMTP, and your full account username. Third-party protocol access depends on your plan. No application ID is needed for this route. [Server settings](https://www.fastmail.help/hc/en-us/articles/1500000278342-Server-names-and-ports), [third-party setup](https://www.fastmail.help/hc/en-us/articles/360058752834-Set-up-Fastmail-on-your-device)

## Yahoo and AOL: registration reference, integration pending

The browser OAuth connector is **not implemented** in Rusty Mail. Do not expect a Yahoo client ID to work in the Google/Microsoft fields. To prepare a future integration, create a Yahoo Developer application, request access for the needed mail scopes, and follow the provider's developer-access approval process. Login/OpenID access alone is not approval to access mail. Validate native/public-client PKCE and redirect support before implementing this connector; do not embed a confidential-client secret in a distributed app.

[Yahoo getting started](https://developer.yahoo.com/oauth2/guide/openid_connect/getting_started.html), [Yahoo developer access](https://senders.yahooinc.com/developer/developer-access/)

## Private IMAP/SMTP servers

Use the hostnames, ports and credentials supplied by the server administrator. Current support is implicit TLS for IMAP (normally 993), and required STARTTLS (normally 587) or implicit TLS (normally 465) for SMTP. Paste a private CA certificate in the advanced CA field if needed; the app keeps certificate and hostname validation enabled. The connect action validates IMAP login; SMTP credentials are exercised during sending. Generic custom-server OAuth and IMAP STARTTLS are not yet implemented.

Proton Mail requires its separate Bridge and eligible service plan. Bridge compatibility is not validated here, and its default IMAP STARTTLS setup is outside this client's current IMAP transport support. [Proton Bridge](https://proton.me/support/imap-smtp-and-pop3-setup)

## Troubleshooting

| Symptom | Check |
|---|---|
| `Set RUSTYMAIL_..._CLIENT_ID` | Close the existing app; set the variable in the shell that starts the new app. |
| Microsoft `AADSTS50011` | Mobile/Desktop platform and exact `/oauth/callback` path; do not use the frontend port 1420/1421 as the OAuth redirect. |
| Microsoft organization denies consent | Ask the tenant administrator; do not change to broader application permissions. |
| Google `access_denied` | Listed test user, consent-screen audience/status, enabled Gmail API and organization policy. |
| Refresh token rejected | Reconnect through the browser; password changes, policy changes or revoked access may invalidate authorization. |
| Browser callback fails | Let the attempt expire/cancel and retry. Do not copy authorization codes into chat or logs. |
| IMAP TLS fails | Correct hostname/port and certificate chain; provide the intended private CA instead of disabling validation. |

To set a client ID persistently for your Windows user, use `[Environment]::SetEnvironmentVariable('RUSTYMAIL_MICROSOFT_CLIENT_ID','YOUR-ID','User')`, then open a **new** terminal. Client IDs may be public. Mail passwords, refresh tokens, private signing keys and their passwords must remain private.
