//! Opt-in, encrypted synthetic conversation benchmarks. Never opens user mail.
use crate::{engine, open_store};
use sha2::{Digest, Sha256};
use std::time::Instant;
#[test]
#[ignore = "Measures 100k/500k synthetic encrypted conversation stores"]
fn conversation_performance() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let count: usize = std::env::var("RUSTYMAIL_PERF_MESSAGES")
        .unwrap_or("100000".into())
        .parse()
        .unwrap();
    assert!([100_000, 500_000].contains(&count));
    let path = root.join(format!(".release/benchmarks/conversations-{count}.db"));
    if !path.exists() {
        let source = root.join(format!(".release/benchmarks/synthetic-{count}.db"));
        assert!(
            !std::path::PathBuf::from(format!("{}-wal", source.display())).exists(),
            "Source must be a closed synthetic fixture"
        );
        std::fs::copy(source, &path).unwrap();
    }
    let db = open_store(&path, &"ba".repeat(32)).unwrap();
    engine::migrate(&db).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM mail", [], |r| r.get::<_, usize>(0))
            .unwrap(),
        count
    );
    assert_eq!(
        engine::state(&db, "synthetic-benchmark", "fixture-v1")
            .unwrap()
            .as_deref(),
        Some(count.to_string().as_str())
    );
    if engine::state(&db, "synthetic-benchmark", "conversation-fixture-v1")
        .unwrap()
        .is_none()
    {
        let tx = db.unchecked_transaction().unwrap();
        let mut q=tx.prepare("UPDATE mail SET payload=json_set(payload,'$.provider','google','$.thread_id',?1,'$.conversation_key',?2,'$.html_body',CASE WHEN received<20000 THEN '<p>Cached <strong>formatted</strong> message.</p>' ELSE '' END,'$.body_format_version',1) WHERE received>=?3 AND received<?4").unwrap();
        for i in 0..count.div_ceil(30) {
            let thread = format!("perf-{i}");
            let key = format!("native:{}", engine::identity("google", &thread));
            q.execute(rusqlite::params![thread, key, i * 30, (i + 1) * 30])
                .unwrap();
        }
        drop(q);
        engine::checkpoint(&tx, "synthetic-benchmark", "conversation-fixture-v1", "1").unwrap();
        tx.commit().unwrap();
    }
    let mut timings = serde_json::Map::new();
    fn measure(mut f: impl FnMut(), samples: usize) -> serde_json::Value {
        f();
        let mut times = Vec::new();
        for _ in 0..samples {
            let t = Instant::now();
            f();
            times.push(t.elapsed().as_secs_f64() * 1000.0)
        }
        times.sort_by(f64::total_cmp);
        serde_json::json!({"samples":samples,"p50_ms":times[times.len()/2],"p95_ms":times[(times.len()*95/100).min(times.len()-1)]})
    }
    timings.insert(
        "conversation_page".into(),
        measure(
            || {
                let p = engine::query_conversations(&db, "inbox", "", "", 0, None).unwrap();
                assert_eq!(p.messages.len(), 100);
                assert!(p.total < (count as i64));
            },
            3,
        ),
    );
    let anchor = engine::query_conversations(&db, "inbox", "", "", 0, None)
        .unwrap()
        .next_cursor
        .unwrap();
    timings.insert(
        "next_page".into(),
        measure(
            || {
                assert_eq!(
                    engine::query_conversations(&db, "inbox", "", "", 0, Some(&anchor))
                        .unwrap()
                        .messages
                        .len(),
                    100
                )
            },
            3,
        ),
    );
    timings.insert(
        "scoped_search".into(),
        measure(
            || {
                assert!(
                    engine::query_conversations(
                        &db,
                        "inbox",
                        "account-0",
                        "searchable phrase",
                        0,
                        None
                    )
                    .unwrap()
                    .total
                        > 0
                )
            },
            3,
        ),
    );
    match engine::query_conversations(&db, "inbox", "", "Project", 0, None) {
        Ok(_) => {
            timings.insert(
                "broad_search".into(),
                measure(
                    || {
                        assert!(
                            engine::query_conversations(&db, "inbox", "", "Project", 0, None)
                                .unwrap()
                                .total
                                > 0
                        )
                    },
                    7,
                ),
            );
        }
        Err(e) => {
            timings.insert("broad_search_error".into(), serde_json::Value::String(e));
        }
    }
    let id = engine::identity("account-0", "0");
    timings.insert(
        "cached_message".into(),
        measure(
            || {
                engine::get(&db, &id).unwrap();
            },
            50,
        ),
    );
    timings.insert(
        "conversation_members".into(),
        measure(
            || assert_eq!(engine::conversation(&db, &id).unwrap().len(), 10),
            50,
        ),
    );
    let inbox_page =
        || engine::query_conversations(&db, "inbox", "account-0", "", 0, None).unwrap();
    let expected = inbox_page();
    timings.insert(
        "account_inbox".into(),
        measure(|| assert_eq!(inbox_page().messages.len(), 100), 7),
    );
    db.execute_batch("SAVEPOINT unrelated_snooze").unwrap();
    db.execute(
        "INSERT INTO snoozes(id,wake) VALUES(?1,?2)",
        rusqlite::params![engine::identity("account-1", "1"), engine::now() + 3600],
    )
    .unwrap();
    let snoozed = inbox_page();
    assert_eq!(snoozed.total, expected.total);
    assert!(snoozed
        .messages
        .iter()
        .map(|message| &message.id)
        .eq(expected.messages.iter().map(|message| &message.id)));
    timings.insert(
        "account_inbox_unrelated_snooze".into(),
        measure(|| assert_eq!(inbox_page().messages.len(), 100), 7),
    );
    db.execute_batch("ROLLBACK TO unrelated_snooze; RELEASE unrelated_snooze")
        .unwrap();
    let label = std::env::var("RUSTYMAIL_PERF_LABEL").unwrap_or("current".into());
    let result = serde_json::json!({"label":label,"messages":count,"build":if cfg!(debug_assertions){"debug"}else{"release"},"encrypted":true,"fixture":"30 messages per provider thread across 3 accounts; 20k cached bodies and HTML","engine_sha256":hex::encode(Sha256::digest(include_bytes!("engine.rs"))),"measured_utc":chrono::Utc::now().to_rfc3339(),"timings":timings});
    let output = root.join(format!(
        "reports/performance/conversations-{label}-{count}.json"
    ));
    std::fs::write(&output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    println!("{}", result);
}

#[test]
#[ignore = "Measures synthetic inbox projection write overhead"]
fn inbox_write_performance() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let path = root.join(".release/benchmarks/conversations-100000.db");
    let db = open_store(&path, &"ba".repeat(32)).unwrap();
    assert_eq!(
        engine::state(&db, "synthetic-benchmark", "conversation-fixture-v1")
            .unwrap()
            .as_deref(),
        Some("1")
    );
    let mut values = Vec::new();
    for disabled in [true, false] {
        let mut samples = Vec::new();
        for _ in 0..3 {
            db.execute_batch("SAVEPOINT perf_write").unwrap();
            if disabled {
                db.execute_batch("DROP TRIGGER inbox_summary_insert; DROP TRIGGER inbox_summary_update; DROP TRIGGER inbox_summary_delete;").unwrap();
            }
            let start = Instant::now();
            db.execute("UPDATE mail_groups SET unread=1-unread WHERE id IN (SELECT id FROM mail_groups WHERE folder='inbox' LIMIT 1000)",[]).unwrap();
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            db.execute_batch("ROLLBACK TO perf_write; RELEASE perf_write")
                .unwrap();
        }
        samples.sort_by(f64::total_cmp);
        values.push(samples);
    }
    let output = serde_json::json!({"fixture":"1000 synthetic metadata flag updates, rolled back", "samples":3,"without_summary_ms":values[0],"with_summary_ms":values[1]});
    std::fs::write(
        root.join("reports/performance/inbox-write-v11.json"),
        serde_json::to_vec_pretty(&output).unwrap(),
    )
    .unwrap();
    println!("{output}");
}
