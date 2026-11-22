//! Regression scenarios for security boundaries and derived-data consistency.
use crate::{
    engine::{self, Draft, Mail},
    live, providers, Store,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

fn db() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    engine::migrate(&db).unwrap();
    db
}
#[tokio::test]
async fn cached_read_does_not_wait_for_a_network_operation() {
    let db = db();
    let mut m = mail("cached", "a", "inbox");
    m.loaded = true;
    m.body = "Available offline".into();
    engine::put(&db, &m).unwrap();
    let store = Store(Arc::new(Mutex::new(db)));
    let runtime = live::Runtime::default();
    let _network = runtime.gate.lock().await;
    let loaded = tokio::time::timeout(
        std::time::Duration::from_millis(200),
        live::get_mail_inner(&store, &runtime, "cached"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(loaded.body, m.body);
}
#[test]
fn dense_and_sparse_searches_preserve_exact_account_and_folder_scopes() {
    let db = db();
    let tx = db.unchecked_transaction().unwrap();
    for i in 0..2400 {
        engine::put(
            &tx,
            &mail(
                &format!("scope-{i}"),
                if i % 2 == 0 { "a-b" } else { "a b" },
                "inbox",
            ),
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let first = engine::query_after(&db, "inbox", "", "marker", 0, None).unwrap();
    let anchor = first.next_cursor.clone().unwrap();
    let mut seen = first
        .messages
        .iter()
        .map(|m| m.id.clone())
        .collect::<Vec<_>>();
    // Removing the anchor and receiving newer mail must not repeat or skip the
    // rest of the original queue, even when timestamps tie.
    engine::remove(&db, &anchor.id).unwrap();
    let mut newest = mail("new-arrival", "a-b", "inbox");
    newest.received = 999;
    engine::put(&db, &newest).unwrap();
    let mut cursor = Some(anchor);
    while let Some(anchor) = cursor {
        let p = engine::query_after(&db, "inbox", "", "marker", 0, Some(&anchor)).unwrap();
        seen.extend(p.messages.iter().map(|m| m.id.clone()));
        cursor = p.next_cursor;
    }
    assert_eq!(seen.len(), 2400);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 2400);
    assert!(!seen.iter().any(|id| id == "new-arrival"));
    // Restore the fixture for the scope matrix below.
    engine::remove(&db, "new-arrival").unwrap();
    let old_id = first.messages.last().unwrap().id.clone();
    let old = first.messages.last().unwrap();
    engine::put(&db, &mail(&old_id, &old.account_id, "inbox")).unwrap();
    for account in ["", "a-b", "a b"] {
        for folder in ["all", "inbox"] {
            let p = engine::query(&db, folder, account, "marker", 0).unwrap();
            assert_eq!(p.total, if account.is_empty() { 2400 } else { 1200 });
            assert_eq!(p.messages.len(), 100);
            assert!(p
                .messages
                .iter()
                .all(|m| account.is_empty() || m.account_id == account));
        }
    }
    let mut upper = engine::get(&db, "scope-0").unwrap();
    upper.folder = "INBOX".into();
    upper.labels = vec!["INBOX".into()];
    engine::put(&db, &upper).unwrap();
    assert_eq!(
        engine::query(&db, "inbox", "a-b", "marker", 0)
            .unwrap()
            .total,
        1199
    );
    assert_eq!(
        engine::query(&db, "INBOX", "a-b", "marker", 0)
            .unwrap()
            .total,
        1
    );
    assert_eq!(engine::query(&db, "all", "", "a-b", 0).unwrap().total, 0);
}
fn mail(id: &str, account: &str, folder: &str) -> Mail {
    Mail {
        id: id.into(),
        account_id: account.into(),
        remote_id: id.into(),
        subject: "Searchable marker".into(),
        folder: folder.into(),
        ..Default::default()
    }
}
fn sending_store() -> Store {
    let db = db();
    db.execute_batch("CREATE TABLE accounts(id TEXT PRIMARY KEY,provider TEXT,email TEXT); INSERT INTO accounts VALUES('a','imap','a@example.com'),('b','imap','b@example.com'); INSERT INTO files VALUES('attachment','outgoing','test.txt','text/plain',X'68656c6c6f',0); INSERT INTO outgoing_owners VALUES('attachment','a');").unwrap();
    Store(Arc::new(Mutex::new(db)))
}
fn draft() -> Draft {
    Draft {
        account_id: "a".into(),
        to: "receiver@example.com".into(),
        subject: "Original subject".into(),
        body: "Original body".into(),
        attachments: vec!["attachment".into()],
        reply_message_id: None,
        ..Default::default()
    }
}

#[test]
fn account_identity_and_failed_writes_cannot_corrupt_mail_or_search() {
    let db = db();
    let original = mail("same-id", "a", "inbox");
    engine::put(&db, &original).unwrap();
    for mut forged in [mail("same-id", "b", "trash"), mail("same-id", "a", "trash")] {
        if forged.account_id == "a" {
            forged.remote_id = "different-remote".into();
        }
        assert!(engine::put(&db, &forged).is_err());
    }
    db.execute_batch("CREATE TRIGGER fail_membership BEFORE INSERT ON memberships WHEN new.mailbox='blocked' BEGIN SELECT RAISE(ABORT,'fixture storage failure'); END;").unwrap();
    let mut changed = original.clone();
    changed.subject = "Should roll back".into();
    changed.folder = "trash".into();
    changed.labels = vec!["blocked".into()];
    assert!(engine::put(&db, &changed).is_err());
    assert_eq!(
        engine::get(&db, "same-id").unwrap().subject,
        original.subject
    );
    assert_eq!(
        engine::query(&db, "inbox", "a", "marker", 0).unwrap().total,
        1
    );
    assert_eq!(engine::query(&db, "trash", "a", "", 0).unwrap().total, 0);
    assert_eq!(engine::query(&db, "all", "b", "", 0).unwrap().total, 0);
}

#[test]
fn counts_match_rows_after_moves_deletes_rollback_and_snooze() {
    let db = db();
    for i in 0..150 {
        let mut m = mail(
            &format!("id-{i:03}"),
            if i % 2 == 0 { "a" } else { "b" },
            "inbox",
        );
        m.received = (i % 7) as i64;
        engine::put(&db, &m).unwrap();
    }
    let first = engine::query(&db, "inbox", "", "", 0).unwrap();
    let second = engine::query(&db, "inbox", "", "", 100).unwrap();
    assert_eq!(first.total, 150);
    assert_eq!(second.messages.len(), 50);
    let mut ids = first
        .messages
        .iter()
        .chain(&second.messages)
        .map(|m| m.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 150);
    for i in 0..30 {
        let id = format!("id-{i:03}");
        if i % 3 == 0 {
            engine::remove(&db, &id).unwrap();
        } else {
            let mut m = engine::get(&db, &id).unwrap();
            m.folder = "archive".into();
            engine::put(&db, &m).unwrap();
        }
    }
    {
        let tx = db.unchecked_transaction().unwrap();
        engine::remove(&tx, "id-149").unwrap();
        tx.rollback().unwrap();
    }
    for account in ["", "a", "b"] {
        for folder in ["all", "inbox", "archive", "trash"] {
            let expected:i64=db.query_row("SELECT count(*) FROM mail WHERE (?1='' OR account_id=?1) AND (?2='all' OR folder=?2)",rusqlite::params![account,folder],|r|r.get(0)).unwrap();
            assert_eq!(
                engine::query(&db, folder, account, "", 0).unwrap().total,
                expected
            );
            assert_eq!(
                engine::query(&db, folder, account, "marker", 0)
                    .unwrap()
                    .total,
                expected
            );
        }
    }
    db.execute(
        "INSERT INTO snoozes VALUES('id-149',?1)",
        [engine::now() + 3600],
    )
    .unwrap();
    assert_eq!(engine::query(&db, "inbox", "b", "", 0).unwrap().total, 59);
    assert_eq!(
        engine::query(&db, "inbox", "b", "marker", 0).unwrap().total,
        59
    );
    assert_eq!(
        engine::query(&db, "snoozed", "b", "marker", 0)
            .unwrap()
            .total,
        1
    );
    db.execute("UPDATE snoozes SET wake=0", []).unwrap();
    assert_eq!(engine::query(&db, "inbox", "b", "", 0).unwrap().total, 60);
}

fn downgrade_fixture(db: &Connection) {
    db.execute_batch("DROP TRIGGER mail_counts_insert; DROP TRIGGER mail_counts_delete; DROP TRIGGER mail_counts_move; DROP TABLE mail_counts; DROP TABLE outgoing_owners; DROP TABLE outbox_mime; DROP INDEX snoozes_wake; DROP INDEX jobs_created; PRAGMA user_version=3;").unwrap();
}

#[test]
fn migration_is_atomic_and_old_sends_require_review() {
    let db = db();
    engine::put(&db, &mail("m", "a", "inbox")).unwrap();
    downgrade_fixture(&db);
    db.execute_batch("INSERT INTO live_drafts VALUES('a','malformed JSON'); INSERT INTO jobs VALUES('old','a','send','{}','pending','',0,0);").unwrap();
    assert!(engine::migrate(&db).is_err());
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='mail_counts'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT status FROM jobs", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "pending"
    );
    db.execute("DELETE FROM live_drafts", []).unwrap();
    engine::migrate(&db).unwrap();
    assert_eq!(engine::query(&db, "inbox", "a", "", 0).unwrap().total, 1);
    assert_eq!(
        db.query_row("SELECT status FROM jobs", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "failed"
    );
    db.execute_batch("PRAGMA user_version=99").unwrap();
    assert!(engine::migrate(&db).is_err());
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        99
    );
}

#[test]
fn attachment_ownership_and_outbox_snapshot_survive_changes_and_restart() {
    let store = sending_store();
    let mut d = draft();
    d.account_id = "b".into();
    assert!(live::queue_send_inner(&store, d).is_err());
    let id = live::queue_send_inner(&store, draft()).unwrap();
    let queued = live::queued_mime(&store, &id).unwrap();
    {
        let db = store.0.lock().unwrap();
        db.execute("UPDATE files SET data=X'676f6f64627965'", [])
            .unwrap();
    }
    assert_eq!(live::queued_mime(&store, &id).unwrap(), queued);
    assert_ne!(
        providers::build_mime(&store, &draft(), &id).unwrap(),
        queued
    );
    {
        let db = store.0.lock().unwrap();
        db.execute("DELETE FROM files", []).unwrap();
        db.execute("UPDATE jobs SET status='running' WHERE id=?1", [&id])
            .unwrap();
        engine::migrate(&db).unwrap();
        assert_eq!(
            db.query_row("SELECT status FROM jobs", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "uncertain"
        );
    }
    assert_eq!(live::queued_mime(&store, &id).unwrap(), queued);
    assert!(queued
        .windows(b"Original body".len())
        .any(|w| w == b"Original body"));
}

#[test]
fn outbox_insert_failure_leaves_no_job_or_snapshot() {
    let store = sending_store();
    store.0.lock().unwrap().execute_batch("CREATE TRIGGER reject_job BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'fixture disk failure'); END;").unwrap();
    assert!(live::queue_send_inner(&store, draft()).is_err());
    let db = store.0.lock().unwrap();
    for table in ["jobs", "outbox_mime"] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
#[test]
fn cancelling_one_action_preserves_later_fields_and_downloaded_content() {
    let db = db();
    let mut m = mail("actions", "a", "inbox");
    m.unread = true;
    engine::put(&db, &m).unwrap();
    let store = Store(Arc::new(Mutex::new(db)));
    let archive = live::queue_mutation_inner(&store, "actions", "archive").unwrap();
    {
        let db = store.0.lock().unwrap();
        let payload: String = db
            .query_row("SELECT payload FROM jobs WHERE id=?1", [&archive], |r| {
                r.get(0)
            })
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert!(value["original"].get("body").is_none());
    }
    let read = live::queue_mutation_inner(&store, "actions", "read").unwrap();
    {
        let db = store.0.lock().unwrap();
        let mut current = engine::get(&db, "actions").unwrap();
        current.body = "Newly downloaded content".into();
        current.loaded = true;
        engine::put(&db, &current).unwrap();
    }
    live::cancel_job_inner(&store, &archive).unwrap();
    {
        let db = store.0.lock().unwrap();
        let current = engine::get(&db, "actions").unwrap();
        assert_eq!(current.folder, "inbox");
        assert!(!current.unread);
        assert_eq!(current.body, "Newly downloaded content");
        assert!(current.loaded);
    }
    live::cancel_job_inner(&store, &read).unwrap();
    let star = live::queue_mutation_inner(&store, "actions", "star").unwrap();
    let unstar = live::queue_mutation_inner(&store, "actions", "unstar").unwrap();
    assert!(live::cancel_job_inner(&store, &star).is_err());
    assert_eq!(
        store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT status FROM jobs WHERE id=?1", [&star], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "pending"
    );
    live::cancel_job_inner(&store, &unstar).unwrap();
    live::cancel_job_inner(&store, &star).unwrap();
    let archive = live::queue_mutation_inner(&store, "actions", "archive").unwrap();
    let trash = live::queue_mutation_inner(&store, "actions", "trash").unwrap();
    {
        let db = store.0.lock().unwrap();
        db.execute("UPDATE jobs SET status='done' WHERE id=?1", [trash])
            .unwrap();
        crate::reconcile::confirm_action(&db, "actions", "trash", None).unwrap();
    }
    live::cancel_job_inner(&store, &archive).unwrap();
    assert_eq!(
        engine::get(&store.0.lock().unwrap(), "actions")
            .unwrap()
            .folder,
        "trash"
    );
}
#[test]
fn cancelling_send_removes_its_snapshot_without_submitting() {
    let store = sending_store();
    let id = live::queue_send_inner(&store, draft()).unwrap();
    live::cancel_job_inner(&store, &id).unwrap();
    assert!(live::queued_mime(&store, &id).is_err());
    let attachment_retained: bool = store
        .0
        .lock()
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE id='attachment'
                 UNION SELECT 1 FROM outgoing_owners WHERE file_id='attachment')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!attachment_retained);
    assert_eq!(
        store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT status FROM jobs WHERE id=?1", [id], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "cancelled"
    );
}

#[test]
fn filenames_do_not_name_windows_devices_or_hide_direction_changes() {
    for name in ["CON", "con.txt", "NUL.bin", "COM1.txt", "lpt9", "AUX "] {
        assert!(engine::safe_filename(name).starts_with('_'));
    }
    assert_eq!(
        engine::safe_filename("report\u{202e}txt.exe"),
        "report_txt.exe"
    );
    assert_eq!(engine::safe_filename("../  "), "_");
}
#[test]
fn oversized_headers_are_bounded_without_retaining_their_allocations() {
    let db = db();
    let subject = "é".repeat(10000);
    let sender = "Name".repeat(1000);
    let raw=format!("From: {sender} <sender@example.com>\r\nSubject: {subject}\r\nContent-Type: text/plain\r\n\r\nReadable body");
    let m = engine::parse_raw(&db, mail("headers", "a", "inbox"), raw.as_bytes()).unwrap();
    assert!(m.headers_shortened);
    assert!(m.subject.len() <= 4096);
    assert!(m.sender.len() <= 1024);
    assert!(m.subject.capacity() <= 4096);
    let cached = engine::get(&db, "headers").unwrap();
    assert!(cached.headers_shortened);
    assert!(cached.subject.len() <= 4096);
    let original: Vec<u8> = db
        .query_row("SELECT raw FROM mail_raw WHERE id='headers'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(original, raw.as_bytes());
    let p = engine::query(&db, "inbox", "a", "", 0).unwrap();
    assert!(p.messages[0].subject.len() <= 4096);
    assert!(p.messages[0].subject.capacity() <= 4096);
}
#[test]
fn excessive_mime_attachments_do_not_leave_partial_cached_files() {
    let db = db();
    let original = engine::parse_raw(
        &db,
        mail("mime-limit", "a", "inbox"),
        b"Subject: original\r\nContent-Type: text/plain\r\n\r\noriginal body",
    )
    .unwrap();
    let mut raw="Subject: replacement\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=parts\r\n\r\n".to_string();
    for i in 0..=engine::MAX_ATTACHMENTS {
        raw.push_str(&format!("--parts\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=file-{i}.bin\r\n\r\nx\r\n"));
    }
    raw.push_str("--parts--\r\n");
    assert!(engine::parse_raw(&db, original, raw.as_bytes())
        .unwrap_err()
        .contains("attachment"));
    assert_eq!(engine::get(&db, "mime-limit").unwrap().subject, "original");
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM files WHERE message_id='mime-limit'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let p = engine::query(&db, "inbox", "a", "original", 0).unwrap();
    assert_eq!(p.total, 1);
    assert_eq!(
        engine::query(&db, "inbox", "a", "replacement", 0)
            .unwrap()
            .total,
        0
    );
}
#[test]
fn old_unresolved_sends_remain_visible_and_newer_pending_actions_come_first() {
    let db = db();
    db.execute(
        "INSERT INTO jobs VALUES('old','a','send','{}','uncertain','Check Sent',0,0)",
        [],
    )
    .unwrap();
    for i in 0..150 {
        db.execute(
            "INSERT INTO jobs VALUES(?1,'a','mutation','{}','done','',0,?2)",
            rusqlite::params![format!("done-{i}"), engine::now()],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO jobs VALUES('new','a','mutation','{}','pending','Queued archive',0,?1)",
        [engine::now()],
    )
    .unwrap();
    let jobs = engine::jobs(&db).unwrap();
    assert_eq!(jobs[0].id, "new");
    assert_eq!(jobs[1].id, "old");
}
