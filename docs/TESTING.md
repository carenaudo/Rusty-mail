# Synthetic testing and acceptance

Commands below use generated mail or loopback TLS servers. They do not access real accounts or transmit mail to internet recipients. Run from the repository root with the README prerequisites. Native Strawberry Perl must be on PATH for vendored OpenSSL.

## Routine checks

```powershell
npm ci
npm run check
npm run build
npm audit --audit-level=moderate
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-features
powershell -NoProfile -File scripts/Test-Scripts.ps1
cargo install cargo-deny --locked --version 0.20.2
cargo deny --manifest-path src-tauri/Cargo.toml --locked check advisories licenses sources
node scripts/generate-notices.mjs
```

Review notices and `reports/licenses.json` when dependencies change. Some Tauri transitive packages use MPL-2.0; scoped exceptions in `deny.toml` are intentional. Keep their notices/source links. Review advisories individually. Current low-severity npm development-tool findings appear in [validation](VALIDATION.md).

Run `npm run build` before generating notices: Vite records the actual frontend packages with rendered code in `reports/frontend-packages.json`. The generator rejects a stale lockfile inventory and includes embedded tslib attribution. Compiler-only npm dependencies are excluded from shipped notices. Supplemental copies for omitted Rust texts are in `third-party-licenses/` with provenance. Signed builds reject missing notices; automated checks still require release review.

The Rust suite covers encryption/reopen, wrong keys, encrypted WAL/FTS/drafts/attachments, account identity/search isolation, malformed MIME, attachment filenames/round trips, header injection, cache preservation, OAuth callback restrictions, and interrupted-send recovery. Loopback TLS IMAP covers custom CA trust and rejected untrusted certificates. Loopback SMTP covers accepted submission and disconnect after DATA before acknowledgement. These tests do not prove public-provider behavior or delivery.

## Conversation threading

`cargo test --locked --manifest-path src-tauri/Cargo.toml threading_tests` checks native Gmail/Graph mapping, raw-body preservation, account/provider isolation, missing and invalid IDs, chronological ordering, filtered summaries, folder-spanning membership, provider corrections/deletes, keyset page boundaries, the schema-7 checkpoint upgrade, IMAP subject normalization and isolation, blank/shortened subject handling, subject-group pagination, and the schema-8 cache backfill. These are synthetic fixtures; real provider integration remains unverified.

For the reader interaction fixture, run `npm run dev` and open `http://127.0.0.1:1420/tests/threading-reader.html`. Its title changes to `PASS: threading reader` after checking a single expanded message, lazy body loading, failed download/retry, read/reply targeting, and switching conversations during a slow download. IPC is mocked entirely in memory; no accounts or provider connections are used. Append `?imap` to run the same checks with IMAP subject keys and verify the grouping label. The fixture is outside the production entry point.

The conversation list groups the matching messages before pagination. Counts and unread/starred indicators describe matching cached messages; the reader includes all cached members across folders. Message operations remain individual. Conversation queries sort/group matching metadata, so prior message-list benchmark results do not establish conversation-list performance.

## HTML reading and image privacy

`cargo test --locked --manifest-path src-tauri/Cargo.toml mail_html::tests` covers MIME HTML/plaintext handling, cached-body preservation, list/conversation projection, cache eviction, local upgrades without a provider request or sync gate, CID ownership, byte/dimension limits, unsupported SVG/handlers, and private-address rejection before connecting.

Run `npm run dev` and open `http://127.0.0.1:1420/tests/html-reader.html`. The title becomes `PASS: HTML reader security` after checking preserved formatting/tables, inert parsing without automatic resource requests, default blocking of remote/CID/data images, sandbox/CSP restrictions, removal of active markup/attributes and unsafe links, explicit loading/hiding, consent reset, stale responses, plaintext fallback, and user-triggered external links. The fixture uses the same reader component and app-equivalent frame/image CSP. IPC/image results are synthetic; no real remote images are fetched.

The HTML/image changes passed 53 Rust tests (2 benchmark/soak tests ignored), Svelte checks, production frontend build, and the browser fixture. This does not establish a professional security audit or real-provider/WebView2 acceptance.

## Mailbox benchmarks

```powershell
New-Item -ItemType Directory -Force reports/performance | Out-Null
$env:RUSTYMAIL_BENCH_MESSAGES = '100000'
$env:RUSTYMAIL_BENCH_OUTPUT = Join-Path (Get-Location) 'reports/performance/benchmark-current-100000.json'
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-features mailbox_benchmark -- --ignored --nocapture
$env:RUSTYMAIL_BENCH_MESSAGES = '500000'
$env:RUSTYMAIL_BENCH_OUTPUT = Join-Path (Get-Location) 'reports/performance/benchmark-current-500000.json'
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-features mailbox_benchmark -- --ignored --nocapture
Remove-Item Env:RUSTYMAIL_BENCH_MESSAGES, Env:RUSTYMAIL_BENCH_OUTPUT
```

Workloads use SQLCipher, three accounts, 20,000 bodies, repeated page/search/read queries, and a 32 MiB SQLite cache. Reports identify the debug build. Results reflect this development machine and warmed queries, not the Windows 11 four-core/8 GiB reference machine or cold-start UI performance. Harness memory is not whole-app/WebView2 memory.

## Continuous store soak

```powershell
powershell -NoProfile -File scripts/Start-Soak.ps1 -Hours 72 -MessageCount 100000
```

The command prints a unique report directory and supervisor PID. A hidden synthetic process updates messages/indexes, performs queries, and records process-tree private bytes/CPU. It does not run provider sync or a WebView. Keep the computer awake and powered on. Reports stay local and excluded from source control. `environment.json` records source hashes and OS/processor context.

```powershell
Get-Content reports/soak/<run>/summary.json
Get-Content reports/soak/<run>/workload.json
Get-Content reports/soak/<run>/resources.jsonl -Tail 3
```

Only `status: passed` after the requested duration and final integrity check is a completed run. A disappeared process or stale `running` report is not a pass. Sampling gaps over 60 seconds invalidate continuity. Interruptions, sleep, crashes, and source changes require a fresh run for the intended build. To stop, inspect the supervisor/cargo/test PIDs and stop those exact processes. No automatic restart occurs. Interrupted workloads may leave synthetic `rustymail-synthetic-soak-*.db` files in the OS temporary directory.

## Remaining integration matrix

Before daily use, test Gmail/Microsoft system-browser login, refresh expiry/rotation, revoked consent, offline restart, expired history/delta, and rate limits. Test IMAP UIDVALIDITY changes, folder moves, duplicate identifiers across accounts, TLS failures, and lost SMTP acknowledgement. Reconcile provider Sent mail before resolving uncertain submissions. Verify draft recovery, reply headers, attachment byte equality, large-message limits, and no duplicate sends after restart. Use controlled test recipients; sending to other people requires explicit authorization.

For hosted AI, use a test HTTPS endpoint to confirm denied consent makes no request, changed accounts/destinations require consent, cancellation works, redirects fail, attachments stay excluded, truncation is disclosed, and secrets never reach frontend/logs. Test prompt injection and invented deadlines manually. Endpoint-validation unit tests do not cover this matrix.

For release acceptance, measure cold launch, reader/search p95 through IPC, and settled idle private bytes/CPU for the entire release app/WebView2 tree on the reference machine with 100k messages. Record hardware, Windows/WebView2 versions, build hash, cache state, and interval. Targets: launch ≤2 s, read ≤100 ms, search ≤150 ms, idle private bytes ≤250 MiB, idle CPU <1%, installer ≤40 MiB excluding shared WebView2. Complete keyboard-only/Narrator checks and a separate 72-hour full-app sync soak.

Use [the signing guide](SIGNED-RELEASES.md) for valid/wrong-key/tampered/rollback update tests and installer verification. An unsigned development installer is not a signed release or an updater acceptance result.

## Repeatable query-only measurements

Optionally set `RUSTYMAIL_BENCH_KEEP=1` before the benchmark commands. The harness retains verified synthetic fixtures in `.release/benchmarks/` and reuses them on later runs. Reports explicitly label reuse and include the engine-source hash. On reuse, `ingest_ms` measures verification, not a new import. These fixtures use a public test key and must never contain real mail. Unset the variable afterward. Default runs use disposable temporary databases.

The benchmark now covers first pages, scoped body search, all-message subject search, legacy deep offsets, cursor seeks, and a broad search with an active snooze. Read the sample counts: broad/deep scenarios use five samples; principal queries use fifty. See SCALING-SECURITY.md for current limits and initial-load comparisons.

The AI network tests use only a loopback HTTP listener and fake key while exercising the hosted-consent branch. They verify no connection on missing/mismatched consent, approved request contents and limits, attachment/tool exclusion, redirect refusal, response limits/error privacy, and in-flight revocation. Real-provider HTTPS/policy and frontend consent interaction remain separate checks.
Soak process-tree samples can include compilation and initial seeding; do not interpret their peak as idle app memory. The current engine also bounds incoming metadata, discloses shortened headers, and rejects downloads above 256 attachments atomically. These cases are covered in hardening_tests.rs.

## Reply, forwarding and conversations

`cargo test --locked --manifest-path src-tauri/Cargo.toml --all-features` covers Reply-To, sent follow-ups, own addresses and aliases, deduplication, quoted commas, Bcc-only delivery, SMTP envelope/header separation, Gmail queue snapshots, Graph recipient verification, cross-account source rejection, changed subjects, bounded references, explicit forwarding copies and rollback, projection triggers, dense search, and extreme/tied dates. Tests use synthetic data and local servers, never send to real providers.

Run `npm run dev` and open `/tests/compose-reader.html`, `/tests/threading-reader.html`, `/tests/threading-reader.html?imap`, and `/tests/html-reader.html`. Each self-running fixture sets its title and root `data-test-result` to passed after assertions. Composition checks reviewed recipients, draft persistence, account changes, explicit attachment copies, delayed-copy races and queue arguments. Browser mocks are not live-provider acceptance.

See [PERFORMANCE.md](PERFORMANCE.md) for benchmark reproduction and release startup/idle measurements.
