# Scaling and security review — October 29, 2026

This pass uses synthetic mail and local TLS fixtures only. Original code remains MIT licensed. It improves encrypted-store query performance and strengthens application checks; it is not a public-release security certification.

## Measured results

Windows 11 development machine: Ryzen 5 4600H, 6 cores/12 threads, approximately 31.37 GiB RAM. SQLCipher uses a 32 MiB page cache. Workloads contain three accounts and 20,000 downloaded bodies. These are debug engine measurements, not end-to-end IPC/WebView or reference-machine acceptance.

| Workload | First page p95 | Scoped body search p95 | Broad subject search p95 | Deep cursor page p95 |
|---|---:|---:|---:|---:|
| 100k messages | 1.84 ms | 31.10 ms | 53.10 ms | 3.10 ms |
| 500k messages | 1.73 ms | 31.76 ms | 213.09 ms | 3.58 ms |

Cached reads measured approximately 0.13 ms. Broad searches with an active snooze measured 45.07/210.88 ms. Main timings use 50 warmed samples; broad/deep/snooze scenarios use five, whose reported p95 is the maximum sample. Reports include schema version, engine-source SHA-256, timestamp, and fixture reuse status:

- `reports/performance/benchmark-v6-100000.json`
- `reports/performance/benchmark-v6-500000.json`

The previous 500k baseline measured 329.88 ms for a page and 1,136.82 ms for scoped body search. An intermediate expanded test exposed a 5.86-second broad-search case; the final query design reduces it to about 213 ms. That still exceeds 150 ms at the stress size. Legacy offset pagination near the end remains slow (about 827 ms); the desktop now uses cursor paging instead.

Initial ingestion increased from approximately 21/125 seconds to 45/221 seconds at 100k/500k. See `benchmark-v6-initial-*.json`; timings were collected on a shared development machine and are not controlled throughput certification. Final reports reuse verified fixtures, so their `ingest_ms` is fixture-validation time, not ingestion throughput. Batch-write efficiency remains work to do.

## Scaling changes

SQLite-maintained counters replace full mailbox recounts for standard folders and account scopes. Triggers update counts atomically with inserts, moves, and deletes. Full-text indexes contain hash-derived account/folder scope tokens, which avoid tokenizer aliases between distinct identifiers. Search text matches sender, subject, and body fields rather than internal scope tokens.

Sparse searches start from FTS hits; dense searches use the date index and FTS membership checks. Counts come directly from the scoped FTS index when supported. Active snoozes subtract a small indexed exclusion set, rather than forcing a full recount. Only the page's message payloads are decoded.

The UI uses descending `(received, id)` cursor seeks, with a local back stack and one-row lookahead. New arrivals and deleted anchors do not shift the remaining reading queue. Cached body reads return before waiting for the network synchronization gate. Regression tests hold that gate while reading an offline message.

## Security and integrity changes

| Boundary | Fix | Synthetic evidence |
|---|---|---|
| Remote identity | An existing cached ID cannot change account or provider identity. | Cross-account/remote-ID overwrite attempts fail without changing mail or search. |
| Derived storage | Mail/index/membership writes and schema migrations use composable savepoints. | Injected write failures roll back rows, counters and search; failed upgrades retain the old version. |
| Attachments | Outgoing files belong to a sending account; draft save, restore and MIME assembly check ownership. | A different account cannot queue another account's file. |
| Outbox | Queueing persists the exact MIME bytes and job in one transaction, with a 100 MiB snapshot budget. | Later file changes/deletion leave queued bytes unchanged; failed queue insertion leaves no orphan snapshot. |
| Restart/cancel | Ambiguous sends remain uncertain; cancelled/reviewed/accepted sends release their snapshot. | Restart never automatically resends; cancellation submits nothing. |
| Undo | Cancellation restores only the affected field, preserves later unrelated changes, and checks newer dependent operations. | Undo preserves read flags and downloaded content; dependent operations must be cancelled in reverse order. |
| Ordering | Ready jobs use insertion order. | Mutation state tests cover overlapping operations and confirmed later changes. |
| AI | Consent remains account/endpoint scoped; revocation invalidates active request generations and is rechecked before submission. Header input is bounded alongside the body. | Scope and loopback request tests verify consent enforcement, no redirect, bounded inputs/responses, attachment exclusion, and active revocation. Real hosted-provider TLS/policy and frontend interaction still need validation. |
| Filenames/IPC | Filename sanitization handles standard Windows device names and direction controls. Unused demo IPC and its separate AI path were removed from the desktop backend. | Filename cases and native/frontend build checks pass. |

AI revocation cannot recall data already transmitted. The frontend send button also guards against repeated clicks while queueing and captures one draft snapshot. Switching sending accounts clears selected attachments so they must be deliberately reattached for the new account.

## Upgrade behavior

Storage schema 6 rebuilt scope/search/order indexes once during upgrade. Schema 7 adds an account/provider/thread lookup index and clears Gmail/Graph checkpoints once to populate native threading metadata on sync. Schema 8 backfills account-scoped conversation keys, including IMAP subject groups, and indexes their lookup. Conversation grouping sorts matching metadata before pagination; the historical message-query timings above do not measure the conversation list. Migrations are atomic and newer unknown schemas are rejected. Large upgrades can take longer than a normal cached launch; migration progress UI and power-loss testing remain open.

Old queued sends that lack immutable MIME snapshots require review/requeue rather than silent rebuilding. Their draft payload remains in encrypted storage. Previously running sends remain uncertain and must be reconciled against provider Sent mail. Legacy attachment ownership is recovered only when stored references agree on one account; otherwise reattach the file.

## Validation and remaining work

The native all-features suite passes **35 tests**, with benchmark/soak tests run separately. Frontend checks pass with zero errors/warnings. A fresh 72-hour synthetic store soak is running in `reports/soak/20261029-175824-d7fb3b`; it has not passed. Earlier frozen builds were superseded after engine changes, not counted as successful.

Next scaling targets are broad 500k searches, initial import throughput, starred/custom-folder queries, and representative MIME/body distributions. Complete full-app memory/CPU/cold-launch measurements on the reference machine. Security work still includes real-provider failure matrices, real hosted-provider HTTPS/policy and frontend consent checks, account removal/revocation, power-loss and malicious-MIME corpora, staged updater tamper/rollback testing, and independent release review.

Design references: [SQLite join planning](https://www.sqlite.org/optoverview.html), [FTS5](https://www.sqlite.org/fts5.html), and [transactional counters through triggers](https://www.sqlite.org/lang_createtrigger.html).

Additional hardening bounds displayed subject/sender/address/date/category headers without retaining their original oversized allocations, and discloses shortened headers in the reader. Page data excludes full attachment/label arrays. MIME caching rejects more than 256 attachments and rolls back all partial files; the original cached message survives. Raw MIME remains encrypted when successfully downloaded. Mutation undo stores only changed fields, not duplicate message bodies. Pending/failed/uncertain jobs are queried ahead of completed history, so unresolved older sends do not disappear after seven days.

The final AI suite simulates the hosted-consent branch over loopback HTTP using a fake API key. It confirms no connection on absent or mismatched consent, correct selected content bounds, no attachments/tools, no redirect following, bounded responses, hidden provider error bodies, and cancellation on revocation. It does not call a real hosted model or prove its data-retention policy.

Some transport scheduling is still serialized: an active sync can delay a new download or send, although cached reading now bypasses that gate. Adversarial IMAP-server buffering and full-app resource budgets remain review items. Soak reports measure cargo/test children and can include compilation/seeding; their peak is not a settled idle app/WebView measurement.