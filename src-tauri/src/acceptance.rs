//! Repeatable acceptance checks with synthetic data; no real mail is transmitted.
use crate::{
    engine::{self, Mail},
    providers,
};
use sha2::Digest;

#[test]
fn encrypted_wal_index_draft_and_attachment() {
    let path =
        std::env::temp_dir().join(format!("rustymail-acceptance-{}.db", rand::random::<u64>()));
    let key = "ac".repeat(32);
    let marker = "private-acceptance-marker-82496";
    {
        let db = crate::open_store(&path, &key).unwrap();
        engine::migrate(&db).unwrap();
        let m = Mail {
            id: "a".into(),
            account_id: "account-a".into(),
            remote_id: "uid-42".into(),
            folder: "inbox".into(),
            body: marker.into(),
            ..Default::default()
        };
        engine::put(&db, &m).unwrap();
        db.execute(
            "INSERT INTO files VALUES('f','a','private.txt','text/plain',?1,0)",
            [marker.as_bytes()],
        )
        .unwrap();
        db.execute("INSERT INTO saved_drafts(id,account_id,payload,updated) VALUES('draft-a','account-a',?1,0)", [marker])
            .unwrap();
        db.execute(
            "INSERT INTO outbox_mime VALUES('queued',?1)",
            [marker.as_bytes()],
        )
        .unwrap();
        for file in [
            &path,
            &std::path::PathBuf::from(format!("{}-wal", path.display())),
        ] {
            if file.exists() {
                assert!(!std::fs::read(file)
                    .unwrap()
                    .windows(marker.len())
                    .any(|w| w == marker.as_bytes()))
            }
        }
        assert_eq!(
            engine::query(&db, "inbox", "account-a", "82496", 0)
                .unwrap()
                .total,
            1
        );
    }
    let db = crate::open_store(&path, &key).unwrap();
    assert_eq!(engine::get(&db, "a").unwrap().body, marker);
    assert_eq!(
        db.query_row("SELECT raw FROM outbox_mime", [], |r| r
            .get::<_, Vec<u8>>(0))
            .unwrap(),
        marker.as_bytes()
    );
    assert_eq!(
        db.query_row("SELECT data FROM files", [], |r| r.get::<_, Vec<u8>>(0))
            .unwrap(),
        marker.as_bytes()
    );
    drop(db);
    let _ = std::fs::remove_file(path);
}
#[test]
fn mime_attachments_roundtrip_and_headers_cannot_inject() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    engine::migrate(&db).unwrap();
    db.execute_batch("CREATE TABLE accounts(id TEXT PRIMARY KEY,provider TEXT,email TEXT); INSERT INTO accounts VALUES('a','imap','sender@example.com'); INSERT INTO files VALUES('f','outgoing','../report.txt','text/plain',X'68656c6c6f',0); INSERT INTO outgoing_owners VALUES('f','a');").unwrap();
    let store = crate::Store(std::sync::Arc::new(std::sync::Mutex::new(db)));
    let mut draft = engine::Draft {
        account_id: "a".into(),
        to: "Recipient <recipient@example.com>".into(),
        subject: "A report".into(),
        body: "Hello".into(),
        attachments: vec!["f".into()],
        reply_message_id: None,
        ..Default::default()
    };
    let raw = providers::build_mime(&store, &draft, "job-42").unwrap();
    let db = store.0.lock().unwrap();
    let m = engine::parse_raw(
        &db,
        Mail {
            id: "message".into(),
            account_id: "a".into(),
            remote_id: "1".into(),
            ..Default::default()
        },
        &raw,
    )
    .unwrap();
    assert_eq!(m.attachments.len(), 1);
    assert!(!m.attachments[0].name.contains('/'));
    let data: Vec<u8> = db
        .query_row(
            "SELECT data FROM files WHERE message_id='message'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(data, b"hello");
    drop(db);
    draft.subject = "hello\r\nBcc: victim@example.com".into();
    assert!(providers::build_mime(&store, &draft, "job-42").is_err());
}
#[test]
fn cache_eviction_keeps_outgoing_files_and_selected_message() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    engine::migrate(&db).unwrap();
    for id in ["old", "selected"] {
        let m = Mail {
            id: id.into(),
            account_id: "a".into(),
            remote_id: id.into(),
            loaded: true,
            body: "cached text".into(),
            folder: "inbox".into(),
            ..Default::default()
        };
        engine::put(&db, &m).unwrap();
        db.execute("INSERT INTO mail_raw VALUES(?1,X'01020304',0)", [id])
            .unwrap();
    }
    db.execute("INSERT INTO files VALUES('draft','outgoing','draft.bin','application/octet-stream',X'01020304',0)",[]).unwrap();
    db.execute(
        "INSERT INTO files VALUES('incoming','old','file.bin','application/octet-stream',X'0102',0)",
        [],
    )
    .unwrap();
    assert_eq!(engine::trim_cache(&db, 4, "selected").unwrap(), 1);
    assert!(!engine::get(&db, "old").unwrap().loaded);
    assert!(engine::get(&db, "selected").unwrap().loaded);
    assert_eq!(
        db.query_row("SELECT count(*) FROM files WHERE id='draft'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(cache_bytes(&db), 4);
}

#[test]
#[ignore = "Runs a 100k or 500k encrypted synthetic mailbox performance workload"]
fn mailbox_benchmark() {
    let count = std::env::var("RUSTYMAIL_BENCH_MESSAGES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100_000)
        .clamp(1000, 500_000);
    let persistent = std::env::var("RUSTYMAIL_BENCH_KEEP").as_deref() == Ok("1");
    let path = if persistent {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".release/benchmarks");
        std::fs::create_dir_all(&directory).unwrap();
        directory.join(format!("synthetic-{count}.db"))
    } else {
        std::env::temp_dir().join(format!("rustymail-bench-{}.db", rand::random::<u64>()))
    };
    let reused = persistent && path.exists();
    let db = crate::open_store(&path, &"ba".repeat(32)).unwrap();
    engine::migrate(&db).unwrap();
    let start = std::time::Instant::now();
    if reused {
        assert_eq!(
            engine::state(&db, "synthetic-benchmark", "fixture-v1")
                .unwrap()
                .as_deref(),
            Some(count.to_string().as_str()),
            "Only reuse a verified synthetic fixture"
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM mail", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            count as i64
        );
    } else {
        let tx = db.unchecked_transaction().unwrap();
        for i in 0..count {
            let a = format!("account-{}", i % 3);
            let remote = i.to_string();
            let m = Mail {
                id: engine::identity(&a, &remote),
                account_id: a,
                remote_id: remote,
                account: format!("me{}@example.com", i % 3),
                sender: "Synthetic sender".into(),
                address: "sender@example.com".into(),
                subject: format!("Project report {}", i % 300),
                body: if i < 20_000 {
                    format!(
                        "Unique searchable phrase item {i}. {}",
                        "mail body ".repeat(80)
                    )
                } else {
                    String::new()
                },
                folder: "inbox".into(),
                received: i as i64,
                loaded: i < 20_000,
                ..Default::default()
            };
            engine::put(&tx, &m).unwrap();
        }
        tx.commit().unwrap();
        engine::checkpoint(&db, "synthetic-benchmark", "fixture-v1", &count.to_string()).unwrap();
    }
    let ingest = start.elapsed().as_millis();
    let mut page = Vec::new();
    let mut search = Vec::new();
    let mut read = Vec::new();
    let mut broad_search = Vec::new();
    let mut deep_page = Vec::new();
    let mut cursor_page = Vec::new();
    let mut snoozed_search = Vec::new();
    let id = engine::identity("account-0", "0");
    for _ in 0..50 {
        let t = std::time::Instant::now();
        assert_eq!(
            engine::query(&db, "inbox", "", "", 0)
                .unwrap()
                .messages
                .len(),
            100
        );
        page.push(t.elapsed().as_secs_f64() * 1000.);
        let t = std::time::Instant::now();
        assert!(
            engine::query(&db, "inbox", "account-0", "searchable phrase", 0)
                .unwrap()
                .total
                > 0
        );
        search.push(t.elapsed().as_secs_f64() * 1000.);
        let t = std::time::Instant::now();
        engine::get(&db, &id).unwrap();
        read.push(t.elapsed().as_secs_f64() * 1000.);
    }
    for _ in 0..5 {
        let t = std::time::Instant::now();
        assert_eq!(
            engine::query(&db, "inbox", "", "Project", 0).unwrap().total,
            count as i64
        );
        broad_search.push(t.elapsed().as_secs_f64() * 1000.);
        let t = std::time::Instant::now();
        assert_eq!(
            engine::query(&db, "inbox", "", "", count as i64 - 100)
                .unwrap()
                .messages
                .len(),
            100
        );
        deep_page.push(t.elapsed().as_secs_f64() * 1000.);
        let t = std::time::Instant::now();
        let cursor = engine::MailCursor {
            received: 100,
            id: engine::identity("account-1", "100"),
        };
        assert_eq!(
            engine::query_after(&db, "inbox", "", "", 0, Some(&cursor))
                .unwrap()
                .messages
                .len(),
            100
        );
        cursor_page.push(t.elapsed().as_secs_f64() * 1000.);
    }
    db.execute(
        "INSERT INTO snoozes VALUES(?1,?2)",
        rusqlite::params![id, engine::now() + 86400],
    )
    .unwrap();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        assert_eq!(
            engine::query(&db, "inbox", "", "Project", 0).unwrap().total,
            count as i64 - 1
        );
        snoozed_search.push(t.elapsed().as_secs_f64() * 1000.);
    }
    db.execute("DELETE FROM snoozes", []).unwrap();
    fn p95(v: &mut [f64]) -> f64 {
        v.sort_by(f64::total_cmp);
        v[(v.len() * 95 / 100).min(v.len() - 1)]
    }
    let result = serde_json::json!({"messages":count,"downloaded_bodies":count.min(20_000),"accounts":3,"build":"debug","encrypted":true,"schema_version":engine::SCHEMA_VERSION,"engine_source_sha256":hex::encode(sha2::Sha256::digest(include_bytes!("engine.rs"))),"measured_utc":engine::measured_utc(),"fixture_reused":reused,"fixture_retained":persistent,"samples":50,"broad_and_deep_samples":5,"ingest_ms":ingest,"page_p95_ms":p95(&mut page),"search_p95_ms":p95(&mut search),"read_p95_ms":p95(&mut read),"broad_search_p95_ms":p95(&mut broad_search),"deep_offset_page_p95_ms":p95(&mut deep_page),"deep_cursor_page_p95_ms":p95(&mut cursor_page),"broad_search_with_snooze_p95_ms":p95(&mut snoozed_search)});
    println!("{result}");
    let target = std::env::var("RUSTYMAIL_BENCH_OUTPUT")
        .unwrap_or_else(|_| format!("benchmark-{count}.json"));
    std::fs::write(target, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    drop(db);
    if !persistent {
        let _ = std::fs::remove_file(path);
    }
}

#[test]
#[ignore = "Continuous synthetic encrypted-store soak; run through scripts/Start-Soak.ps1"]
fn synthetic_soak() {
    let seconds = std::env::var("RUSTYMAIL_SOAK_SECONDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(72 * 3600)
        .clamp(1, 7 * 24 * 3600);
    let count = std::env::var("RUSTYMAIL_SOAK_MESSAGES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(100000)
        .clamp(100, 500000);
    let output = std::path::PathBuf::from(
        std::env::var("RUSTYMAIL_SOAK_OUTPUT")
            .expect("Use the soak supervisor script to select an isolated report directory"),
    );
    let path = std::env::temp_dir().join(format!(
        "rustymail-synthetic-soak-{}.db",
        rand::random::<u64>()
    ));
    let db = crate::open_store(&path, &"ca".repeat(32)).unwrap();
    engine::migrate(&db).unwrap();
    {
        let tx = db.unchecked_transaction().unwrap();
        for i in 0..count {
            let account = format!("synthetic-{}", i % 3);
            let remote = i.to_string();
            engine::put(
                &tx,
                &Mail {
                    id: engine::identity(&account, &remote),
                    account_id: account,
                    remote_id: remote,
                    subject: format!("Synthetic report {i}"),
                    sender: "Test sender".into(),
                    body: if i < 20000 {
                        "Synthetic searchable body".repeat(20)
                    } else {
                        String::new()
                    },
                    folder: "inbox".into(),
                    received: i as i64,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        tx.commit().unwrap();
    }
    let start = std::time::Instant::now();
    let mut iterations = 0u64;
    loop {
        for n in 0..10 {
            let i = (iterations as usize * 10 + n) % count;
            let account = format!("synthetic-{}", i % 3);
            let id = engine::identity(&account, &i.to_string());
            let mut mail = engine::get(&db, &id).unwrap();
            mail.starred = !mail.starred;
            engine::put(&db, &mail).unwrap();
        }
        assert!(!engine::query(&db, "inbox", "", "", 0)
            .unwrap()
            .messages
            .is_empty());
        assert!(
            engine::query(&db, "inbox", "synthetic-0", "Synthetic", 0)
                .unwrap()
                .total
                > 0
        );
        iterations += 1;
        let elapsed = start.elapsed().as_secs();
        let complete = elapsed >= seconds;
        let report = serde_json::json!({"status":if complete{"complete"}else{"running"},"synthetic_only":true,"messages":count,"accounts":3,"requested_seconds":seconds,"elapsed_seconds":elapsed,"iterations":iterations,"updated_utc":engine::measured_utc()});
        std::fs::write(&output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        if complete {
            break;
        }
        std::thread::sleep(
            std::time::Duration::from_secs(5)
                .min(std::time::Duration::from_secs(seconds - elapsed)),
        );
    }
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    let _ = std::fs::remove_file(path);
}

fn triage_store() -> crate::Store {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    engine::migrate(&db).unwrap();
    crate::Store(std::sync::Arc::new(std::sync::Mutex::new(db)))
}

fn triage_mail(id: &str, folder: &str, received: i64) -> Mail {
    Mail {
        id: id.into(),
        account_id: "a".into(),
        remote_id: id.into(),
        provider: "google".into(),
        thread_id: "thread".into(),
        folder: folder.into(),
        address: "sender@example.com".into(),
        subject: "Project notes".into(),
        unread: true,
        received,
        ..Default::default()
    }
}

fn inbox_scope() -> engine::MailScope {
    engine::MailScope {
        folder: "inbox".into(),
        account: "a".into(),
        search: String::new(),
        filters: None,
    }
}

#[test]
fn bulk_actions_apply_only_to_matching_members_of_selected_conversations() {
    let store = triage_store();
    {
        let db = store.0.lock().unwrap();
        for message in [
            triage_mail("newest", "inbox", 104),
            triage_mail("matching", "inbox", 103),
            triage_mail("sent", "sent", 106),
            triage_mail("too-old", "inbox", 100),
            Mail {
                subject: "Unrelated content".into(),
                ..triage_mail("wrong-search", "inbox", 105)
            },
            Mail {
                unread: false,
                ..triage_mail("read", "inbox", 107)
            },
            Mail {
                account_id: "b".into(),
                ..triage_mail("other-account", "inbox", 108)
            },
        ] {
            engine::put(&db, &message).unwrap();
        }
    }
    let scope = engine::MailScope {
        search: "project".into(),
        filters: Some(engine::MailFilters {
            unread: true,
            received_from: Some(102),
            ..Default::default()
        }),
        ..inbox_scope()
    };
    let row_ids = ["newest".into(), "newest".into()];
    assert_eq!(
        crate::live::queue_bulk_mutation_inner(&store, &scope, &row_ids, "archive").unwrap(),
        2
    );
    let db = store.0.lock().unwrap();
    for id in ["newest", "matching"] {
        assert_eq!(engine::get(&db, id).unwrap().folder, "archive");
    }
    for id in ["too-old", "wrong-search", "read", "other-account"] {
        assert_eq!(engine::get(&db, id).unwrap().folder, "inbox");
    }
    assert_eq!(engine::get(&db, "sent").unwrap().folder, "sent");
    assert_eq!(engine::jobs(&db).unwrap().len(), 2);
}

#[test]
fn bulk_actions_reject_stale_or_oversized_selections_and_roll_back_queue_failures() {
    let store = triage_store();
    {
        let db = store.0.lock().unwrap();
        engine::put(&db, &triage_mail("newer", "inbox", 2)).unwrap();
        engine::put(&db, &triage_mail("older", "inbox", 1)).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_bulk BEFORE INSERT ON jobs
            WHEN json_extract(new.payload,'$.id')='older'
            BEGIN SELECT raise(ABORT,'queue write failed'); END;",
        )
        .unwrap();
    }
    let run = |ids: &[String]| {
        crate::live::queue_bulk_mutation_inner(&store, &inbox_scope(), ids, "read")
    };
    assert!(run(&["older".into()])
        .unwrap_err()
        .contains("selection changed"));
    assert!(run(&["newer".into(), "missing".into()])
        .unwrap_err()
        .contains("selection changed"));
    assert!(run(&vec!["newer".into(); 101])
        .unwrap_err()
        .contains("100 conversations"));
    assert!(run(&["newer".into()])
        .unwrap_err()
        .contains("queue write failed"));
    {
        let db = store.0.lock().unwrap();
        assert!(engine::get(&db, "newer").unwrap().unread);
        assert!(engine::get(&db, "older").unwrap().unread);
        assert!(engine::jobs(&db).unwrap().is_empty());
        db.execute_batch("DROP TRIGGER fail_bulk").unwrap();
    }
    assert_eq!(run(&["newer".into()]).unwrap(), 2);
    assert_eq!(run(&["newer".into()]).unwrap(), 0);
    {
        let db = store.0.lock().unwrap();
        let tx = db.unchecked_transaction().unwrap();
        for n in 0..499 {
            engine::put(&tx, &triage_mail(&format!("member-{n}"), "inbox", 0)).unwrap();
        }
        tx.commit().unwrap();
    }
    assert!(run(&["newer".into()]).unwrap_err().contains("500 messages"));
    let db = store.0.lock().unwrap();
    assert_eq!(engine::jobs(&db).unwrap().len(), 2);
    assert!(engine::get(&db, "member-0").unwrap().unread);
}

#[tokio::test]
async fn a_missing_message_fails_its_job_and_leaves_the_next_job_runnable() {
    let store = triage_store();
    {
        let db = store.0.lock().unwrap();
        for id in ["first", "second"] {
            engine::put(&db, &triage_mail(id, "inbox", 1)).unwrap();
        }
    }
    for id in ["first", "second"] {
        crate::live::queue_mutation_inner(&store, id, "trash").unwrap();
        let db = store.0.lock().unwrap();
        engine::remove(&db, id).unwrap();
    }
    assert!(crate::live::process_jobs(&store).await.unwrap());
    assert!(crate::live::process_jobs(&store).await.unwrap());
    assert!(!crate::live::process_jobs(&store).await.unwrap());
    let db = store.0.lock().unwrap();
    let jobs = engine::jobs(&db).unwrap();
    assert_eq!(jobs.len(), 2);
    assert!(jobs.iter().all(|job| job.status == "failed"));
}

#[test]
fn active_job_pages_reach_failures_beyond_the_summary_limit() {
    let store = triage_store();
    let db = store.0.lock().unwrap();
    engine::put(&db, &triage_mail("message", "inbox", 1)).unwrap();
    for n in 0..105 {
        db.execute(
            "INSERT INTO jobs VALUES(?1,'a','mutation','{\"id\":\"message\"}',?2,'Failed',0,0)",
            rusqlite::params![n.to_string(), if n == 50 { "done" } else { "failed" }],
        )
        .unwrap();
    }
    assert_eq!(engine::jobs(&db).unwrap().len(), 100);
    let page = serde_json::to_value(engine::active_jobs(&db, 100).unwrap()).unwrap();
    let jobs = page["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 4);
    assert_eq!(jobs.last().unwrap()["id"], "0");
    assert_eq!(jobs.last().unwrap()["subject"], "Project notes");
    assert!(page["next_offset"].is_null());
}

#[test]
fn custom_mailboxes_scope_colliding_labels_and_keep_them_when_archiving() {
    let store = triage_store();
    {
        let db = store.0.lock().unwrap();
        for (id, account) in [("first", "a"), ("second", "b")] {
            let message = Mail {
                account_id: account.into(),
                labels: vec!["INBOX".into(), "project".into()],
                ..triage_mail(id, "inbox", 1)
            };
            engine::put(&db, &message).unwrap();
        }
        assert!(engine::query_conversations(&db, "mailbox:project", "", "", 0, None).is_err());
        let page =
            engine::query_conversations(&db, "mailbox:project", "a", "notes", 0, None).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.messages[0].id, "first");
    }
    let scope = engine::MailScope {
        folder: "mailbox:project".into(),
        ..inbox_scope()
    };
    assert_eq!(
        crate::live::queue_bulk_mutation_inner(&store, &scope, &["first".into()], "archive")
            .unwrap(),
        1
    );
    let db = store.0.lock().unwrap();
    assert_eq!(engine::get(&db, "first").unwrap().labels, ["project"]);
    assert_eq!(engine::get(&db, "second").unwrap().folder, "inbox");
    assert_eq!(
        engine::query_conversations(&db, "mailbox:project", "a", "", 0, None)
            .unwrap()
            .total,
        1
    );
}

fn cache_bytes(db: &rusqlite::Connection) -> i64 {
    db.query_row(
        "SELECT bytes FROM cache_size WHERE singleton=1",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn encrypted_cache_accounting_migrates_and_survives_rollback_and_reopen() {
    let path = std::env::temp_dir().join(format!("rustymail-cache-{}.db", rand::random::<u64>()));
    let key = "af".repeat(32);
    {
        let db = crate::open_store(&path, &key).unwrap();
        db.execute_batch(
            "CREATE TABLE mail_raw(id TEXT PRIMARY KEY,raw BLOB NOT NULL,accessed INTEGER NOT NULL);
             CREATE TABLE files(id TEXT PRIMARY KEY,message_id TEXT NOT NULL,name TEXT NOT NULL,
                 mime TEXT NOT NULL,data BLOB NOT NULL,created INTEGER NOT NULL);
             CREATE TABLE outgoing_owners(file_id TEXT PRIMARY KEY,account_id TEXT NOT NULL);
             INSERT INTO mail_raw VALUES('m',X'01020304',0);
             INSERT INTO files VALUES('in','m','in','text/plain',X'0102',0);
             INSERT INTO files VALUES('out','outgoing','out','text/plain',X'01020304',0);
             PRAGMA user_version=12;",
        )
        .unwrap();
        engine::migrate(&db).unwrap();
        assert_eq!(cache_bytes(&db), 6);
        db.execute_batch(
            "INSERT INTO mail_raw VALUES('m',X'01',1)
                 ON CONFLICT(id) DO UPDATE SET raw=excluded.raw,accessed=excluded.accessed;
             UPDATE files SET message_id='outgoing' WHERE id='in';
             UPDATE files SET message_id='m',data=X'010203' WHERE id='out';",
        )
        .unwrap();
        assert_eq!(cache_bytes(&db), 4);
        let tx = db.unchecked_transaction().unwrap();
        tx.execute("DELETE FROM mail_raw", []).unwrap();
        tx.execute("DELETE FROM files", []).unwrap();
        assert_eq!(cache_bytes(&tx), 0);
        tx.rollback().unwrap();
        assert_eq!(cache_bytes(&db), 4);
    }
    let db = crate::open_store(&path, &key).unwrap();
    engine::migrate(&db).unwrap();
    assert_eq!(cache_bytes(&db), 4);
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        engine::SCHEMA_VERSION
    );
    drop(db);
    std::fs::remove_file(path).unwrap();
}
