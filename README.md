# Rusty Mail

> **Personal project:** Rusty Mail is my personal project, built to keep email simple and focused without adding lots of features unrelated to email. I shape it to suit my preferences and develop it with ChatGPT Codex. I also conduct security audits with Codex and other AI-assisted tools, including Copilot, Claude Code, and Antigravity. If you're looking for an established email client, consider Thunderbird, Betterbird, Outlook, or another option.

Rusty Mail is an early-stage Windows email client built with Rust, Tauri, and Svelte. It brings multiple inboxes into one conversation view, with search, drafts, and an encrypted local mail cache. Most of the code was written with AI assistance.

**New to Rusty Mail?** Start with the [illustrated user manual](docs/USER-MANUAL.md).

![Light-mode inbox with downloaded-mail search, a conversation list, and a formatted message](docs/screenshots/inbox-and-reader.png)

## What you can do

- Connect Gmail, Microsoft mail, or an IMAP/SMTP account such as iCloud or Fastmail. [Provider setup](docs/PROVIDER-REGISTRATION.md)
- Search downloaded mail, narrow results by unread status, sender, or date, and organize messages with stars, archive, and local snooze.
- Reply to a specific message in a conversation, save a draft, and cancel a queued send during the 10-second delay.
- Read formatted email or plain text, with confirmation before loading images, saving attachments, or opening external links.

![Light-mode reply composer with an editable recipient, subject, message, and draft controls](docs/screenshots/compose-reply.png)

Both screenshots show synthetic mail.

## Run on Windows

If you have the Windows x64 release bundle, extract the ZIP and run `rusty-mail.exe`, or run the NSIS setup executable. Rusty Mail uses Microsoft Edge WebView2.

To build from source, install Node.js, Rust, the Microsoft C++ build tools, WebView2, and native Strawberry Perl for vendored OpenSSL. Run these commands from the repository root.

```powershell
npm ci
npm run tauri dev
```

`npm run dev` runs the sample-only browser preview. Source and development builds need a desktop OAuth registration for Gmail and Microsoft sign-in. [Provider setup](docs/PROVIDER-REGISTRATION.md)

## Project documentation

- [Testing and known limits](docs/TESTING.md)
- [Security and local storage](docs/SECURITY.md)
- [Signed release builds](docs/SIGNED-RELEASES.md)

## License

Rusty Mail’s original code and assets use the MIT License. Dependencies have their own licenses. [MIT License](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.md)
