use crate::{
    engine::{self, Mail},
    providers,
};
use rusqlite::Connection;
use serde_json::json;

fn db() -> Connection {
    let db = Connection::open_in_memory().unwrap();
    engine::migrate(&db).unwrap();
    db
}
fn mail(account: &str, provider: &str, remote: &str, thread: &str, received: i64) -> Mail {
    Mail {
        id: engine::identity(account, remote),
        account_id: account.into(),
        remote_id: remote.into(),
        provider: provider.into(),
        thread_id: thread.into(),
        subject: "Same subject".into(),
        folder: "inbox".into(),
        received,
        ..Default::default()
    }
}
#[test]
fn native_ids_survive_mapping_storage_and_raw_download() {
    let db = db();
    let g = providers::gmail_mail(
        "a",
        "a@example.com",
        &json!({"id":"g","threadId":"opaque/ID+="}),
    )
    .unwrap();
    let m = providers::graph_mail(
        "a",
        "a@example.com",
        "sent",
        &json!({"id":"m","conversationId":"opaque/ID+="}),
    )
    .unwrap();
    for msg in [&g, &m] {
        engine::put(&db, msg).unwrap();
        let parsed = engine::parse_raw(
            &db,
            msg.clone(),
            b"From: A <a@example.com>\r\nSubject: Same subject\r\nMessage-ID: <original@example.com>\r\n\r\nHello",
        )
        .unwrap();
        assert_eq!(parsed.thread_id, "opaque/ID+=");
        engine::put(&db, msg).unwrap();
        let refreshed = engine::get(&db, &msg.id).unwrap();
        assert_eq!(refreshed.message_id, "<original@example.com>");
        assert!(refreshed.loaded);
        assert_eq!(refreshed.body, "Hello");
        assert_eq!(refreshed.thread_id, parsed.thread_id);
        assert_eq!(engine::get(&db, &msg.id).unwrap().provider, msg.provider);
    }
    assert_eq!(engine::conversation(&db, &g.id).unwrap().len(), 1);
    assert_eq!(engine::conversation(&db, &m.id).unwrap().len(), 1);
}
#[test]
fn grouping_is_scoped_and_missing_or_invalid_ids_stay_separate() {
    let db = db();
    let cases = [
        ("a", "google", "1", "t"),
        ("a", "google", "2", "t"),
        ("b", "google", "3", "t"),
        ("a", "microsoft", "4", "t"),
        ("a", "google", "5", ""),
        ("a", "google", "6", ""),
        ("a", "imap", "7", "t"),
        ("a", "imap", "8", "t"),
        ("a", "google", "9", " "),
        ("a", "google", "10", " "),
    ];
    for (a, p, r, t) in cases {
        let mut m = mail(a, p, r, t, 10);
        if p == "imap" {
            m.subject = format!("Separate subject {r}");
        }
        engine::put(&db, &m).unwrap();
    }
    assert_eq!(
        engine::query_conversations(&db, "all", "", "", 0, None)
            .unwrap()
            .total,
        9
    );
    let members = engine::conversation(&db, &engine::identity("a", "1")).unwrap();
    assert_eq!(members.len(), 2);
    assert!(members[0].id < members[1].id); // stable ordering at identical timestamps
    let huge = "x".repeat(8193);
    for r in ["11", "12"] {
        engine::put(&db, &mail("a", "google", r, &huge, 20)).unwrap();
    }
    assert_eq!(
        engine::conversation(&db, &engine::identity("a", "11"))
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn filters_summary_and_membership_updates_are_consistent() {
    let db = db();
    let mut first = mail("a", "google", "first", "t", 1);
    first.unread = true;
    first.starred = true;
    first.body = "needle".into();
    let mut last = mail("a", "google", "last", "t", 2);
    last.folder = "sent".into();
    engine::put(&db, &first).unwrap();
    engine::put(&db, &last).unwrap();
    let all = engine::query_conversations(&db, "all", "a", "", 0, None).unwrap();
    assert_eq!(all.total, 1);
    assert_eq!(all.messages[0].id, last.id);
    assert!(all.messages[0].unread && all.messages[0].starred);
    assert_eq!(all.messages[0].conversation_count, 2);
    for (folder, search) in [("inbox", ""), ("all", "needle"), ("starred", "")] {
        let page = engine::query_conversations(&db, folder, "a", search, 0, None).unwrap();
        assert_eq!(page.messages[0].id, first.id);
        assert_eq!(
            engine::conversation(&db, &page.messages[0].id)
                .unwrap()
                .len(),
            2
        );
    }
    // Provider correction moves membership immediately; deletes leave no ghosts.
    last.thread_id = "different".into();
    engine::put(&db, &last).unwrap();
    assert_eq!(
        engine::query_conversations(&db, "all", "a", "", 0, None)
            .unwrap()
            .total,
        2
    );
    engine::remove(&db, &last.id).unwrap();
    assert_eq!(engine::conversation(&db, &first.id).unwrap().len(), 1);
}
#[test]
fn conversations_are_collapsed_before_keyset_pagination() {
    let db = db();
    for i in 0..105 {
        for j in 0..2 {
            engine::put(
                &db,
                &mail("a", "microsoft", &format!("{i}-{j}"), &format!("t{i}"), i),
            )
            .unwrap();
        }
    }
    let first = engine::query_conversations(&db, "inbox", "", "", 0, None).unwrap();
    assert_eq!(first.total, 105);
    assert_eq!(first.messages.len(), 100);
    let second =
        engine::query_conversations(&db, "inbox", "", "", 0, first.next_cursor.as_ref()).unwrap();
    assert_eq!(second.total, 105);
    assert_eq!(second.messages.len(), 5);
    assert!(second.next_cursor.is_none());
    for m in &second.messages {
        assert!(!first.messages.iter().any(|f| f.thread_id == m.thread_id));
    }
}
#[test]
fn upgrade_restarts_provider_checkpoints_once_and_keeps_legacy_cache() {
    let db = db();
    let legacy = mail("a", "", "old", "", 1);
    engine::put(&db, &legacy).unwrap();
    db.execute_batch("PRAGMA user_version=6").unwrap();
    for key in [
        "history",
        "page",
        "bootstrap_history",
        "delta:inbox",
        "uid:INBOX",
    ] {
        engine::checkpoint(&db, "a", key, "saved").unwrap();
    }
    engine::migrate(&db).unwrap();
    assert!(engine::state(&db, "a", "history").unwrap().is_none());
    assert!(engine::state(&db, "a", "delta:inbox").unwrap().is_none());
    assert_eq!(
        engine::state(&db, "a", "uid:INBOX").unwrap().as_deref(),
        Some("saved")
    );
    assert_eq!(engine::conversation(&db, &legacy.id).unwrap().len(), 1);
    engine::checkpoint(&db, "a", "history", "new").unwrap();
    engine::migrate(&db).unwrap();
    assert_eq!(
        engine::state(&db, "a", "history").unwrap().as_deref(),
        Some("new")
    );
}

#[test]
fn subject_normalization_handles_replies_forwards_and_empty_titles() {
    for (subject, expected) in [
        ("  Re: FW: fwd: RE[2]:  Project   update ", "project update"),
        ("RE : Re: Café\tplans", "café plans"),
        ("Reunion: Tomorrow", "reunion: tomorrow"),
        ("[List] Project update", "[list] project update"),
        ("Re: (No Subject)", ""),
        ("  ", ""),
        ("Re: Fwd:", ""),
        ("Re[x]: Project", "re[x]: project"),
    ] {
        assert_eq!(engine::normalized_subject(subject), expected);
    }
}

#[test]
fn imap_subject_groups_span_folders_but_never_accounts_or_native_threads() {
    let db = db();
    for (account, provider, id, thread, subject, folder) in [
        ("a", "imap", "1", "", "Project update", "inbox"),
        ("a", "imap", "2", "", "Re:  PROJECT   UPDATE", "sent"),
        ("a", "imap", "3", "", "Fwd: Re: Project update", "archive"),
        ("b", "imap", "4", "", "Project update", "inbox"),
        ("a", "google", "5", "native1", "Project update", "inbox"),
        ("a", "google", "6", "native2", "Project update", "inbox"),
        ("a", "microsoft", "7", "native1", "Project update", "inbox"),
        ("a", "google", "8", "", "Project update", "inbox"),
    ] {
        let mut m = mail(account, provider, id, thread, id.parse().unwrap());
        m.subject = subject.into();
        m.folder = folder.into();
        engine::put(&db, &m).unwrap();
    }
    let members = engine::conversation(&db, &engine::identity("a", "2")).unwrap();
    assert_eq!(
        members
            .iter()
            .map(|m| m.remote_id.as_str())
            .collect::<Vec<_>>(),
        vec!["1", "2", "3"]
    );
    assert!(members[0].conversation_key.starts_with("subject:"));
    assert_eq!(
        engine::query_conversations(&db, "all", "", "", 0, None)
            .unwrap()
            .total,
        6
    );
    let inbox = engine::query_conversations(&db, "inbox", "a", "project", 0, None).unwrap();
    let row = inbox
        .messages
        .iter()
        .find(|m| m.provider == "imap")
        .unwrap();
    assert_eq!(row.conversation_count, 1);
    assert_eq!(engine::conversation(&db, &row.id).unwrap().len(), 3);
    let mut changed = members[1].clone();
    changed.subject = "Another topic".into();
    engine::put(&db, &changed).unwrap();
    assert_eq!(engine::conversation(&db, &changed.id).unwrap().len(), 1);
    assert_eq!(engine::conversation(&db, &members[0].id).unwrap().len(), 2);
    for (i, subject) in ["", "(No subject)", "Re:", " ", "No subject"]
        .into_iter()
        .enumerate()
    {
        let mut m = mail("a", "imap", &format!("empty{i}"), "", 20);
        m.subject = subject.into();
        engine::put(&db, &m).unwrap();
        assert!(engine::get(&db, &m.id).unwrap().conversation_key.is_empty());
        assert_eq!(engine::conversation(&db, &m.id).unwrap().len(), 1);
    }
    let mut long = mail("a", "imap", "long1", "", 30);
    long.subject = "a".repeat(5000);
    engine::put(&db, &long).unwrap();
    long.id = engine::identity("a", "long2");
    long.remote_id = "long2".into();
    engine::put(&db, &long).unwrap();
    assert_eq!(engine::conversation(&db, &long.id).unwrap().len(), 1);
}

#[test]
fn legacy_imap_cache_is_backfilled_without_resetting_sync() {
    let db = db();
    db.execute_batch("CREATE TABLE accounts(id TEXT PRIMARY KEY,provider TEXT,email TEXT); INSERT INTO accounts VALUES('a','imap','a@example.com');").unwrap();
    for (id, subject) in [("old1", "Project update"), ("old2", "Re: PROJECT UPDATE")] {
        let mut m = mail("a", "", id, "", 1);
        m.subject = subject.into();
        engine::put(&db, &m).unwrap();
        db.execute("UPDATE mail SET payload=json_remove(payload,'$.provider','$.conversation_key') WHERE id=?1",[&m.id]).unwrap();
    }
    db.execute_batch("PRAGMA user_version=7;").unwrap();
    engine::checkpoint(&db, "a", "uidvalidity:INBOX", "123").unwrap();
    engine::checkpoint(&db, "google", "history", "keep").unwrap();
    engine::migrate(&db).unwrap();
    engine::migrate(&db).unwrap();
    assert_eq!(
        engine::query_conversations(&db, "all", "a", "", 0, None)
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        engine::conversation(&db, &engine::identity("a", "old1"))
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        engine::state(&db, "a", "uidvalidity:INBOX")
            .unwrap()
            .as_deref(),
        Some("123")
    );
    assert_eq!(
        engine::state(&db, "google", "history").unwrap().as_deref(),
        Some("keep")
    );
}

#[test]
fn subject_groups_are_collapsed_before_pagination() {
    let db = db();
    for i in 0..103 {
        for j in 0..2 {
            let mut m = mail("a", "imap", &format!("{i}-{j}"), "", i);
            m.subject = format!("{}Topic {i}", if j == 0 { "" } else { "Re: " });
            engine::put(&db, &m).unwrap();
        }
    }
    let first = engine::query_conversations(&db, "all", "", "", 0, None).unwrap();
    assert_eq!(first.total, 103);
    assert_eq!(first.messages.len(), 100);
    let second =
        engine::query_conversations(&db, "all", "", "", 0, first.next_cursor.as_ref()).unwrap();
    assert_eq!(second.messages.len(), 3);
    assert!(second.next_cursor.is_none());
    for m in second.messages {
        assert!(!first
            .messages
            .iter()
            .any(|f| f.conversation_key == m.conversation_key));
    }
}

#[test]
fn grouped_representatives_cover_negative_extreme_and_tied_timestamps() {
    let db = db();
    for (remote, received) in [
        ("a", i64::MIN),
        ("b", -20),
        ("c", -1),
        ("d", 0),
        ("e", i64::MAX),
        ("f", i64::MAX),
    ] {
        engine::put(&db, &mail("a", "google", remote, "one", received)).unwrap();
    }
    let expected = ["e", "f"]
        .map(|r| engine::identity("a", r))
        .into_iter()
        .max()
        .unwrap();
    let page = engine::query_conversations(&db, "all", "", "", 0, None).unwrap();
    assert_eq!(page.messages.len(), 1);
    assert_eq!(page.messages[0].id, expected);
    assert_eq!(page.messages[0].received, i64::MAX);
    engine::remove(&db, &expected).unwrap();
    let members = engine::conversation(&db, &engine::identity("a", "a")).unwrap();
    assert_eq!(members.len(), 5);
}
#[test]
fn grouped_dense_search_snoozes_and_starred_views_match_reference_rows() {
    let db = db();
    for i in 0..1200 {
        let mut m = mail(
            if i % 2 == 0 { "a" } else { "b" },
            "google",
            &format!("dense-{i}"),
            &format!("t{}", i / 6),
            i,
        );
        m.subject = "Broad matching term".into();
        m.starred = i % 9 == 0;
        engine::put(&db, &m).unwrap();
    }
    let p = engine::query_conversations(&db, "inbox", "", "Broad", 0, None).unwrap();
    assert_eq!(p.total, 400);
    assert_eq!(p.messages.len(), 100);
    assert_eq!(
        engine::query_conversations(&db, "inbox", "a", "Broad", 0, None)
            .unwrap()
            .total,
        200
    );
    let id = engine::identity("a", "dense-0");
    db.execute(
        "INSERT INTO snoozes VALUES(?1,?2)",
        rusqlite::params![id, engine::now() + 86400],
    )
    .unwrap();
    assert_eq!(
        engine::query_conversations(&db, "inbox", "", "Broad", 0, None)
            .unwrap()
            .total,
        400
    );
    assert_eq!(
        engine::query_conversations(&db, "snoozed", "", "Broad", 0, None)
            .unwrap()
            .total,
        1
    );
    assert!(
        engine::query_conversations(&db, "starred", "", "Broad", 0, None)
            .unwrap()
            .messages
            .iter()
            .all(|m| m.starred)
    );
    // Projection triggers cover raw provider moves/deletes as well as domain writes.
    db.execute("UPDATE mail SET folder='archive' WHERE id=?1", [&id])
        .unwrap();
    assert_eq!(
        engine::query_conversations(&db, "archive", "", "", 0, None)
            .unwrap()
            .total,
        0
    ); // snoozed until explicitly opened
    db.execute("DELETE FROM snoozes", []).unwrap();
    assert_eq!(
        engine::query_conversations(&db, "archive", "", "", 0, None)
            .unwrap()
            .total,
        1
    );
    db.execute("DELETE FROM mail WHERE id=?1", [&id]).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM mail_groups WHERE id=?1", [&id], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn search_total_is_independent_of_cursor_for_dense_and_sparse_results() {
    let db = db();
    for index in 0..1200 {
        let mut message = mail(
            "a",
            "google",
            &format!("match-{index:04}"),
            &format!("thread-{}", index / 2),
            i64::MIN,
        );
        if index < 1050 {
            message.subject = if index < 2 { "Broad Rare" } else { "Broad" }.into();
        }
        engine::put(&db, &message).unwrap();
    }
    for (term, total, matching) in [("Broad", 525, 1050), ("Rare", 1, 2)] {
        let page = engine::query_conversations(&db, "inbox", "a", term, 0, None).unwrap();
        assert_eq!(page.total, total);
        let newest = (0..matching)
            .map(|index| engine::identity("a", &format!("match-{index:04}")))
            .max()
            .unwrap();
        assert_eq!(page.messages[0].id, newest);
        assert_eq!(page.messages[0].conversation_count, 2);
        let exhausted = engine::MailCursor {
            received: i64::MIN,
            id: String::new(),
        };
        let page =
            engine::query_conversations(&db, "inbox", "a", term, 0, Some(&exhausted)).unwrap();
        assert_eq!(page.total, total);
        assert!(page.messages.is_empty() && page.next_cursor.is_none());
    }
}

#[test]
fn snoozed_inbox_reorders_affected_threads_and_removes_empty_ones() {
    let db = db();
    for i in 0..220 {
        engine::put(
            &db,
            &mail("a", "google", &format!("m-{i:03}"), &format!("t{i}"), i),
        )
        .unwrap();
    }
    let mut hidden_latest = mail("a", "google", "latest", "t4", 221);
    hidden_latest.unread = true;
    hidden_latest.starred = true;
    engine::put(&db, &hidden_latest).unwrap();
    let other = mail("b", "google", "other", "t4", 222);
    engine::put(&db, &other).unwrap();
    let mut archived = mail("a", "google", "archived", "t4", 223);
    archived.folder = "archive".into();
    engine::put(&db, &archived).unwrap();
    let read = |offset, cursor| {
        engine::query_conversations(&db, "inbox", "a", "", offset, cursor).unwrap()
    };
    let initial = read(0, None);
    for id in [&other.id, &archived.id] {
        db.execute(
            "INSERT INTO snoozes VALUES(?1,?2)",
            rusqlite::params![id, engine::now() + 3600],
        )
        .unwrap();
    }
    assert_eq!(read(0, None).messages[0].id, initial.messages[0].id);
    for id in [hidden_latest.id, engine::identity("a", "m-219")] {
        db.execute(
            "INSERT INTO snoozes VALUES(?1,?2)",
            rusqlite::params![id, engine::now() + 3600],
        )
        .unwrap();
    }
    let page = read(0, None);
    assert_eq!(page.total, 219);
    assert_eq!(page.messages[0].remote_id, "m-218");
    let cursor_page = read(0, page.next_cursor.as_ref());
    let offset_page = read(100, None);
    assert_eq!(
        cursor_page
            .messages
            .iter()
            .map(|m| &m.id)
            .collect::<Vec<_>>(),
        offset_page
            .messages
            .iter()
            .map(|m| &m.id)
            .collect::<Vec<_>>()
    );
    let tail = read(0, cursor_page.next_cursor.as_ref());
    let restored = tail
        .messages
        .iter()
        .find(|m| m.remote_id == "m-004")
        .unwrap();
    assert_eq!(restored.conversation_count, 1);
    assert!(!restored.unread && !restored.starred);
    assert_eq!(tail.total, 219);
}

#[test]
fn inbox_summary_matches_grouped_query_after_mutations_and_rollback() {
    let db = db();
    let mut items = Vec::new();
    for i in 0..240 {
        let mut m = mail(
            if i % 2 == 0 { "a" } else { "b" },
            "google",
            &i.to_string(),
            &format!("thread-{}", i / 6),
            (i / 4) as i64,
        );
        m.unread = i % 3 == 0;
        m.starred = i % 7 == 0;
        engine::put(&db, &m).unwrap();
        items.push(m);
    }
    let compare = |account: &str| {
        let force_grouping = engine::MailFilters {
            received_from: Some(i64::MIN),
            ..Default::default()
        };
        let grouped = |cursor| {
            engine::query_filtered_conversations(
                &db,
                "inbox",
                account,
                "",
                Some(&force_grouping),
                0,
                cursor,
            )
            .unwrap()
        };
        let fast = engine::query_conversations(&db, "inbox", account, "", 0, None).unwrap();
        let slow = grouped(None);
        assert_eq!(
            serde_json::to_value(&fast.messages).unwrap(),
            serde_json::to_value(&slow.messages).unwrap()
        );
        assert_eq!(fast.total, slow.total);
        if let (Some(a), Some(b)) = (&fast.next_cursor, &slow.next_cursor) {
            let fast_next =
                engine::query_conversations(&db, "inbox", account, "", 0, Some(a)).unwrap();
            let slow_next = grouped(Some(b));
            assert_eq!(
                serde_json::to_value(&fast_next.messages).unwrap(),
                serde_json::to_value(&slow_next.messages).unwrap()
            );
        }
    };
    compare("");
    compare("a");
    compare("b");
    items[4].folder = "archive".into();
    engine::put(&db, &items[4]).unwrap();
    items[6].unread = !items[6].unread;
    items[6].starred = !items[6].starred;
    engine::put(&db, &items[6]).unwrap();
    engine::remove(&db, &items[9].id).unwrap();
    compare("");
    compare("a");
    db.execute_batch("SAVEPOINT test_rollback").unwrap();
    engine::remove(&db, &items[10].id).unwrap();
    db.execute_batch("ROLLBACK TO test_rollback; RELEASE test_rollback")
        .unwrap();
    compare("");
}

#[test]
fn filters_group_only_matching_messages_and_page_with_the_same_count() {
    let db = db();
    for n in 0..102 {
        let mut message = mail("a", "google", &format!("m-{n}"), &format!("t-{n}"), 100 + n);
        message.unread = true;
        message.address = "Alice+tag@EXAMPLE.COM".into();
        engine::put(&db, &message).unwrap();
    }
    let cases = [
        (
            "same-thread",
            "a",
            "inbox",
            "t-101",
            200,
            true,
            "alice+tag@example.com",
        ),
        (
            "sent",
            "a",
            "sent",
            "t-101",
            202,
            true,
            "alice+tag@example.com",
        ),
        (
            "already-read",
            "a",
            "inbox",
            "t-101",
            202,
            false,
            "alice+tag@example.com",
        ),
        (
            "token-alias",
            "a",
            "inbox",
            "t-101",
            202,
            true,
            "alice.tag@example.com",
        ),
        (
            "wrong-account",
            "b",
            "inbox",
            "t-101",
            202,
            true,
            "alice+tag@example.com",
        ),
        (
            "lower-bound",
            "a",
            "inbox",
            "t-101",
            99,
            true,
            "alice+tag@example.com",
        ),
        (
            "upper-bound",
            "a",
            "inbox",
            "t-101",
            203,
            true,
            "alice+tag@example.com",
        ),
    ];
    for (remote, account, folder, thread, received, unread, address) in cases {
        let mut message = mail(account, "google", remote, thread, received);
        message.folder = folder.into();
        message.unread = unread;
        message.address = address.into();
        // Display names also enter the FTS sender column.
        message.sender = "alice+tag@example.com".into();
        engine::put(&db, &message).unwrap();
    }
    let filters = engine::MailFilters {
        unread: true,
        sender: "  ALICE+tag@example.com  ".into(),
        received_from: Some(100),
        received_before: Some(203),
    };
    let query = |cursor| {
        engine::query_filtered_conversations(
            &db,
            "inbox",
            "a",
            "same subject",
            Some(&filters),
            0,
            cursor,
        )
        .unwrap()
    };
    let first = query(None);
    assert_eq!(first.total, 102);
    assert_eq!(first.messages.len(), 100);
    assert_eq!(first.messages[0].id, engine::identity("a", "m-101"));
    assert_eq!(first.messages[0].conversation_count, 2);
    assert!(first.messages[0].unread);
    let last = query(first.next_cursor.as_ref());
    assert_eq!(last.total, 102);
    assert_eq!(last.messages.len(), 2);
    assert_eq!(last.messages[1].received, 100);
    assert!(last.next_cursor.is_none());
    assert!(first
        .messages
        .iter()
        .all(|m| !last.messages.iter().any(|n| m.id == n.id)));
}

#[test]
fn sender_filter_matches_unicode_case_and_only_the_sender_address() {
    let db = db();
    let mut message = mail("a", "google", "one", "", 100);
    message.address = "Éva@EXAMPLE.COM".into();
    engine::put(&db, &message).unwrap();
    let mut other = mail("a", "google", "two", "", 101);
    other.address = "else@example.com".into();
    other.account = "éva@example.com".into();
    engine::put(&db, &other).unwrap();
    let filters = engine::MailFilters {
        sender: "éVA@example.com".into(),
        ..Default::default()
    };
    let page = engine::query_filtered_conversations(&db, "inbox", "", "", Some(&filters), 0, None)
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.messages[0].id, message.id);
}
