# Mail performance policy and measurements

Keep the product focused on mail. New features should reuse cached metadata, fetch bodies on demand, bound input/output and background concurrency, and avoid fetching images or attachments without a user action. Measure the affected paths before and after a feature. A passing unit suite does not establish responsiveness or idle cost.

## Conversation baseline, November 3, 2026

Windows x64, encrypted SQLCipher stores, the same debug test profile, 30 synthetic messages per provider thread across 3 accounts (10 members per account), and 20k cached text/HTML bodies. Each principal query uses 3 timed samples after warm-up; reported p95 is therefore the maximum of three, not a statistically strong tail estimate. Cached opening and member enumeration use 50 samples. No real mail or network is involved. Source hashes and exact values are retained in `reports/performance/conversations-before-compose-*.json` and `conversations-after-compose-final-*.json`.

| Action (p95 ms) | 100k before | 100k after | 500k before | 500k after |
| --- | ---: | ---: | ---: | ---: |
| First conversation page | 6303.70 | 171.25 | 28748.45 | 3396.99 |
| Next conversation page | 6321.52 | 168.99 | 30771.78 | 3326.39 |
| Scoped body search | 284.63 | 53.14 | 299.66 | 49.49 |
| Cached message read | 0.08 | 0.08 | 0.07 | 0.07 |
| Ten cached members | 1.04 | 1.05 | 0.85 | 1.25 |

Dense broad search failed in the baseline due to a stale FTS predicate after planner selection. It now succeeds at 191.80/3408.24 ms for 100k/500k. The schema-10 metadata projection avoids grouping full message JSON; a materialized page bounds payload decoding to 101 rows. Atomic triggers maintain the projection alongside message writes and skip redundant metadata rewrites when only body content changes. The tradeoff is extra encrypted disk space, indexes and metadata write work; this run does not quantify migration or ingest overhead. Grouped page/count queries still scan the matching metadata, so very large mailbox paging needs further optimization. These are local debug measurements, not a promise of instant 500k-message pages on every machine.

The same-machine regression gate allows at most `baseline p95 * 1.30 + 10 ms`, requires broad search to succeed, and rejects query errors. `reports/performance/compose-gate.json` passed. For subsequent features use `after-compose-final` as the baseline and repeat the same workload/build profile. Noise should trigger investigation and a controlled rerun, not weakening the limit. The gate compares only listed scenarios; it does not measure all performance.

## Reproduce

First retain the encrypted synthetic fixtures with the query benchmark described in TESTING.md. With `.release/benchmarks/synthetic-100000.db` and `synthetic-500000.db` closed, run:

```powershell
$env:RUSTYMAIL_PERF_LABEL='candidate'
$env:RUSTYMAIL_PERF_MESSAGES='100000'
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-features mail_performance::conversation_performance -- --ignored --nocapture
$env:RUSTYMAIL_PERF_MESSAGES='500000'
cargo test --locked --manifest-path src-tauri/Cargo.toml --all-features mail_performance::conversation_performance -- --ignored --nocapture
python scripts/check-mail-performance.py after-compose-final candidate
```

The first run copies and marks a separate `conversations-N.db`; never use real mail with its public test key. Do not run two workloads against the same fixture at once. Do not relink a Windows test binary while it is running. Timing excludes fixture preparation/migration and includes encrypted queries plus result decoding. Retain environment/build/source information with future measurements.

## Full app startup and idle

After building the release, `scripts/Measure-MailApp.ps1 -Messages 100000` (or 500000) launches only a verified synthetic fixture through `--synthetic-performance-profile`. The script checks that the executable supports the flag before launch, waits for 100 rows to paint, lets the process tree settle for 10 seconds, samples a 30-second idle interval, then stops its own native process. The isolated app skips the normal Credential Manager database-key path, single-instance activation, and background sync. It does not touch the normal mailbox. Never automate account setup or sending in this profile.

Reports in `reports/performance/app-compose-N.json` include executable hash, startup-to-painted-list time, tree CPU, committed private bytes and working set. Summed working sets can count shared pages repeatedly. These are initial synthetic release baselines for future comparisons, with warm OS caches possible; they do not establish cold-boot, real-provider idle behavior or a full-app soak. Normal launches keep the normal profile and worker behavior.
