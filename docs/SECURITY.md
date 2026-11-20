# Security model and remaining work

## Boundaries

Message text, MIME, remote identities, model responses, attachments, and network responses are untrusted. The bundled UI communicates with Rust through typed commands. Rust owns credentials, authentication, TLS connections, persistence, and operation validation. The browser preview contains synthetic fixtures only.

Threats considered include hostile mail, malicious servers or model endpoints, interrupted connections, and unsafe filenames. Malware running as the signed-in Windows user, altered application binaries, and compromised operating systems remain outside the protection provided by at-rest encryption.

## Implemented protections

- A random 256-bit SQLCipher key resides in Windows Credential Manager. Startup fails closed on unavailable cipher/key; database, WAL, FTS, drafts, raw MIME, and attachment BLOBs use encrypted storage. SQLite temporary storage stays in memory; the page cache is bounded to 32 MiB.
- Google/Microsoft OAuth uses the system browser, S256 PKCE, constant-time state comparison, constrained loopback callbacks, bounded reads, and expiry. Tokens/passwords remain in the OS credential store. HTTPS token/API requests disable redirects and bound responses.
- Provider URLs are fixed or validated, including Graph continuation URLs. TLS validates certificates; custom trust roots are explicit. IMAP requires TLS; SMTP STARTTLS cannot silently downgrade.
- Cached identities cannot change account or remote ID. Mail/index/membership writes and migrations are atomic. Exact hashed FTS scopes avoid tokenizer aliases. Queries parameterize SQL and bound inputs.
- Display headers and attachment metadata are bounded without retaining oversized string allocations; shortened headers are disclosed. Page and conversation projections omit HTML and full attachment/label arrays. The plaintext body remains the search/AI input. Untrusted HTML is parsed in an inert template, stripped of resource attributes, sanitized with DOMPurify using explicit tag/attribute and CSS-property allowlists, and rendered in a sandboxed `srcdoc` frame without scripts, same-origin access, forms, popups, or top navigation. The frame CSP denies all network connections and allows only validated raster data images. The app CSP allows its own reader frames while continuing to block remote image/script origins. Raw HTML is never inserted into the application document.
- Outgoing attachments belong to a sending account. Draft save/restore and MIME assembly enforce ownership. Native dialogs select/save bounded files; frontend paths cannot choose arbitrary files. Names reject path/control/device hazards; opening is explicit. Saved files become user-owned plaintext. Incoming MIME is limited to 25 MiB and 256 attachments; failed parsing/caching rolls back.
- Jobs persist before optimistic mutations. Queued sends store immutable MIME and job data atomically, with a 100 MiB snapshot budget. Undo stores only changed fields, preserves unrelated content, and checks newer dependent actions. Unresolved jobs stay visible beyond seven days. Interrupted sends become `uncertain` and are never automatically retried. Reconcile Sent before resubmission.
- AI requires a user request. Hosted destinations require consent per account and endpoint. Secrets remain in Rust. HTTPS is required except literal loopback HTTP; redirects/proxies are disabled. Input/output, timeout, and cancellation are bounded. Attachments and mailbox tools are excluded.
- A single desktop instance owns the store. Optional updates use baked-in HTTPS endpoints/public keys, signed-version verification, and block installation during pending mail operations. Generic updater/filesystem/shell capabilities are not exposed to the frontend.

## HTML reading and image approval

Images have no `src` in the reader until an explicit per-message action. Consent lives only in the mounted reader and resets on message/mode changes; late image responses cannot affect another selection. Merely parsing an email must not preload its original URLs. Even embedded CID/base64 images require the same action.

After approval, Rust permits up to 12 image sources, 1 MiB each and 4 MiB total, with a 25-second batch timeout. Public HTTPS downloads use certificate validation, checked and pinned DNS addresses, no environment proxy, no cookies/referrers/authorization, and no redirects. Private, loopback, link-local, mapped/reserved IP ranges and local hostnames are rejected. Raster signatures/dimensions are checked before conversion to data URLs (PNG/JPEG/GIF/WebP; dimensions <=8192 on either side and <=20 million pixels). SVG, HTTP images and other formats remain blocked. CID resolution reads only the selected message's cached MIME. Image bytes and approval are not persisted.

Sender-supplied links have no navigation handlers inside the frame. The reader lists actual destinations outside it; an explicit click can open only HTTP(S) or a constrained `mailto:` URI in the user's external application. Scripts, event handlers, forms, embedded frames/objects, SVG/MathML, external styles/fonts, CSS URLs, relative resources, `srcset`, and custom protocol handlers are removed. Inline presentation uses a narrow subset of text/color/spacing properties. Large HTML bodies (>1,000,000 UTF-8 bytes) fall back to text. Downloaded older MIME is reparsed locally rather than silently fetching mail again.

Loading remote images can disclose an open event and IP address to the sender; approval does not make a sender trustworthy. This is explained next to the image action. Real WebView2/provider corpora and independent security review remain needed. Browser image decoding still depends on the installed WebView2 runtime.

References: [DOMPurify](https://github.com/cure53/DOMPurify), [sandboxed srcdoc](https://developer.mozilla.org/en-US/docs/Web/API/HTMLIFrameElement/srcdoc).

## Evidence and limitations

See [validation](VALIDATION.md) for executed tests. Dependency advisories/license checks are automated, with exact third-party notices and scoped MPL exceptions. These checks do not constitute a professional security audit. Hosted model policy compliance must be established before real data use; destination consent alone does not establish permitted use.

Real OAuth/token rotation, Graph/Gmail resets, rate limits, tenant policies, sleep/wake, and server interoperability need integration testing. Known-marker encryption tests do not prove every storage or crash-dump path is protected. Credential-store failure, key loss, power loss during migration, account removal, renderer compromise, Narrator, and wider hostile MIME/IMAP-server corpora still require review.

There is no encrypted backup/key-recovery workflow. Losing the database key loses local drafts/content. Local cancellation restores only the affected field and rejects out-of-order undo; provider failure/move reconciliation still needs integration testing. SMTP Sent-folder copy and provider draft synchronization are unfinished. AI output can invent facts or follow hostile content despite prompting; review it.

Before public distribution, complete the [testing matrix](TESTING.md), applicable provider verification, publisher signing, staged updater tamper/rollback tests, privacy documentation, and an independent release security review. Never put private signing keys, passwords, tokens, or real mailbox fixtures in this repository.

The synthetic AI network suite verifies that missing/wrong-account/changed-endpoint consent opens no connection; approved requests bound headers/body and omit attachments/tools; redirects and oversized responses fail; provider error bodies stay out of errors; revocation cancels an active generation. These tests simulate the hosted decision path over loopback HTTP with fake keys. They do not validate a real hosted provider, TLS deployment, retention policy, or recall of data already sent. Unused demo persistence and separate AI commands were removed from the desktop IPC surface.

## Reply and forwarding boundaries

Incoming Bcc is never copied into recipient metadata or forwarded headers. Reply all excludes connected accounts and explicitly configured own addresses, deduplicates addresses, prefers Reply-To, and stops automatic generation when recipient metadata was truncated. Aliases affect exclusions only and do not authorize sending as another address. Message-ID and reference headers are validated and bounded; references retain the root and recent ancestors. Recipient fields reject controls and exceed neither 100 total recipients nor 8 KiB per field.

Queued MIME and provider context are frozen together. The backend rederives reply context from a source in the same account; frontend provider IDs are discarded. Gmail receives threadId only for a valid parent Message-ID and compatible subject. Microsoft creates a native reply draft, replaces To/Cc/Bcc with the reviewed lists, verifies the returned address sets, then submits it. SMTP uses a separate envelope and omits the Bcc header; provider MIME APIs receive Bcc for routing. Submission failures retain the existing uncertain-send review policy.

Forwarding adds no original attachments automatically. Selected cached attachments are copied transactionally to new account-owned outgoing IDs; missing, foreign or excessive selections roll back. Original message removal cannot delete those outgoing copies. Incoming HTML and images remain under the reader's existing controls; composition quotes bounded plaintext and never automatically loads images.

The explicit synthetic performance CLI uses a fixed public fixture key, verifies its marker, size and account namespace read-only before opening it for profiling, skips Credential Manager database-key access and background sync, and bypasses single-instance activation. It is for generated test data only; the public fixture key must never protect real mail.
