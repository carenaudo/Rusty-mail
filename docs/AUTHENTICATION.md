# Account authentication

Follow [Provider application registration](PROVIDER-REGISTRATION.md) for portal instructions, redirect URIs, scopes, and troubleshooting. No real accounts are needed for the [synthetic test suite](TESTING.md).

Google and Microsoft sign-in use the operating system's default browser. Their passwords are never entered into Rusty Mail. Rust generates S256 PKCE and fresh state, binds an ephemeral IPv4 loopback listener, opens the official authorization endpoint, validates callback host/path/state, and exchanges the single-use code over HTTPS. Callback reads are bounded; attempts expire after three minutes and can be cancelled. The frontend receives account metadata only.

Tokens and mail passwords stay in Windows Credential Manager. Account metadata, server configuration, messages, indexes, drafts, and attachments use SQLCipher. Refresh-token renewal and rotation are implemented in Rust. Native applications are public clients: Microsoft does not require a client secret. Google desktop credential files can include a value required by its token exchange; it does not replace PKCE or turn the app into a confidential client.

Set `RUSTYMAIL_GOOGLE_CLIENT_ID` and/or `RUSTYMAIL_MICROSOFT_CLIENT_ID` in the shell that launches the desktop app. If applicable, set `RUSTYMAIL_GOOGLE_CLIENT_SECRET`. `.env.example` documents variables; dotenv files are not automatically loaded.

Google requests `gmail.modify` with offline access. Microsoft requests delegated `offline_access`, `User.Read`, `Mail.ReadWrite`, and `Mail.Send` through the `common` authority. Tenant restrictions and provider testing/verification requirements still apply.

Custom servers use explicit incoming/outgoing settings with validated TLS and optional custom CA PEM. IMAP currently requires implicit TLS; SMTP supports implicit TLS or required STARTTLS. iCloud and Fastmail presets accept provider-supported app passwords. Yahoo/AOL OAuth, generic IMAP OAuth, and Proton Bridge compatibility are not implemented. Do not substitute a primary provider password for an unsupported browser-login flow.

Implemented authorization and refresh code is not evidence of successful provider integration. Real-account OAuth, refresh expiry/revocation, organization consent, and end-to-end delivery still require validation. Complete account disconnection/revocation management is pending.
