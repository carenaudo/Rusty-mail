# Validation — October 29, 2026

Testing uses synthetic data and loopback servers only. No real mailbox authorization, internet sending, or hosted inference was exercised.

## Executed checks

- Frontend check: zero errors/warnings. Production build: JavaScript 115.80 kB / 39.64 kB gzip; CSS 12.74 kB / 3.47 kB gzip.
- Native all-features suite: **35 passed, 2 ignored**. Ignored benchmark/soak workloads are executed separately. Default and updater-enabled configurations compile.
- TLS IMAP custom-CA acceptance and untrusted-certificate rejection; TLS SMTP acceptance and lost acknowledgement pass with local servers.
- Encryption/reopen/wrong key, known plaintext absence in DB/WAL, encrypted FTS/drafts/attachments/outbox, MIME attachment equality, header-injection rejection, filenames, cache preservation, and uncertain-send restart behavior pass.
- New regressions cover immutable account identity, attachment ownership, atomic migration/write/queue failures, immutable MIME snapshots, field-preserving undo, ordered cancellation, exact hashed FTS scopes, and cursor paging across tied timestamps/new arrivals/deleted anchors.
- Cached reading succeeds while the network gate is held. AI consent cannot transfer between accounts/endpoints. Loopback AI request tests verify consent enforcement, request bounds, attachment/tool exclusion, redirects, response limits, error privacy, and revocation. Real hosted-provider HTTPS/policy, frontend consent interaction, and OAuth renewal remain integration gates.
- Packaging scripts parse and development preflight/negative signing-manifest checks pass. An unsigned x64 NSIS release installer builds at approximately 4.1 MiB; installation/uninstallation and production signing remain untested. Artifact metadata is in `.release/build-report.json` and `reports/performance/installer-development.json`.
- Pinned cargo-deny advisories/licenses/sources checks pass. Complete notices cover shipped frontend code, Windows Rust dependencies, bundled C, and tslib, with scoped MPL exceptions. This is not an independent legal/security release review.
- npm audit: three low-severity development-tool findings; no moderate/high/critical findings or automatic fix reported by the recorded audit.

## Current encrypted engine measurements

Windows 11, Ryzen 5 4600H (6 cores/12 threads), approximately 31.37 GiB RAM; debug builds with a 32 MiB SQLite cache, three accounts and 20k bodies.

| Workload | Page p95 | Cached read p95 | Scoped search p95 | Broad search p95 | Deep cursor page p95 |
|---|---:|---:|---:|---:|---:|
| 100k | 1.84 ms | 0.13 ms | 31.10 ms | 53.10 ms | 3.10 ms |
| 500k | 1.73 ms | 0.13 ms | 31.76 ms | 213.09 ms | 3.58 ms |

Reports: `reports/performance/benchmark-v6-{100000,500000}.json`. Engine hashes match the current source. These reuse verified synthetic fixtures and report validation time, not initial ingestion time, in `ingest_ms`. Initial loads measured approximately 45/221 seconds and are retained separately as `benchmark-v6-initial-*.json`.

Main query timings use 50 warmed samples; broad/deep/snooze scenarios use five. This development machine exceeds the proposed four-core/8 GiB reference specification. These are engine measurements, not IPC/WebView timings or acceptance on the reference machine. Broad 500k search and initial ingestion remain scaling targets. See [the detailed review](SCALING-SECURITY.md) for baseline comparisons and tradeoffs.

## Soak

The schema-6 supervisor smoke run passed (`reports/soak/20261029-175558-34dc4a`). A fresh **72-hour, 100k-message store soak is running** in `reports/soak/20261029-175824-d7fb3b`. It has not passed. Consult live `summary.json`, `workload.json`, and `resources.jsonl`; sampling gaps over 60 seconds invalidate continuity. Prior runs were superseded after engine changes; no partial run counts as a 72-hour pass.

This tests encrypted persistence/index operations and harness resources, not provider synchronization or the app/WebView2 process tree.

## Remaining release gates

Real Gmail/Microsoft authorization, token expiry/rotation/revocation, API rate limits/resets, IMAP UIDVALIDITY/folder recovery, SMTP interoperability and Sent copies, sleep/wake, hosted-AI request consent, and actual delivery need integration testing. Account disconnection, provider drafts, generic IMAP OAuth, custom Graph folders, and richer composition remain incomplete. Gmail/Microsoft native conversation grouping and IMAP subject grouping have synthetic coverage; real-account validation remains open.

Full-app cold launch, reference-machine IPC latency, idle process-tree memory/CPU, Narrator/keyboard audit, hostile MIME corpus, power loss, a full-app 72-hour sync soak, installer behavior, signed updates/tamper tests, and independent release review remain open. No production-signed release has been published.

Follow [provider registration](PROVIDER-REGISTRATION.md), [testing](TESTING.md), [security](SECURITY.md), and [signing](SIGNED-RELEASES.md). Upgrades rebuild indexes atomically; older pending sends without immutable snapshots require review/requeue.

Header-memory bounds and excessive-attachment rollback tests also pass. Compilation can appear in soak resource peaks; no idle-memory or CPU acceptance claim is made from those samples.

## HTML reader update — November 3, 2026

The desktop reader now preserves MIME HTML and offers sanitized formatting in an opaque-origin, scriptless sandbox. Images default to blocked, including CID and base64 content; per-message approval is required for the bounded native image loader. The frame itself cannot access the network or IPC. Plain text remains the search and AI input.

Executed checks for this change: 53 Rust tests passed (2 benchmark/soak tests ignored), Svelte check and frontend production build passed, and the browser HTML security fixture passed. The fixture checks no automatic resource preloads, active-content stripping, formatting, image loading/hiding, consent reset and stale responses, plaintext fallback, and explicit external links. Rust coverage includes image URL/address checks, byte/dimension limits, CID scoping and local cache upgrades. See [security details](SECURITY.md) and [reproduction steps](TESTING.md). Real-provider/WebView2 and independent security acceptance remain open.

## Composition and query update — November 3, 2026

Reply all, Forward and editable To/Cc/Bcc now have synthetic coverage, including own-address exclusions, Reply-To, sent follow-ups, bounded references, SMTP Bcc privacy, frozen queue context, Gmail thread hints, Graph recipient verification, explicit forwarding copies and account changes. Cached MIME upgrades recipient metadata locally on opening. Schema 9 stores explicit own aliases and schema 10 maintains compact conversation metadata transactionally.

Executed: 67 Rust tests passed, 3 opt-in measurements ignored; Svelte check has zero errors/warnings; frontend production build passed. Compose, Gmail/IMAP conversation reader and secure HTML browser fixtures passed. The same-machine encrypted 100k/500k conversation gate passed; broad search previously failed and now succeeds. See PERFORMANCE.md and raw reports for values, sample counts and limitations. Full-app synthetic release startup/idle results are recorded separately when executed. Real provider delivery, replies received into the expected server thread, SMTP server variations, Sent copy behavior, and provider-draft cleanup remain open integration acceptance.
