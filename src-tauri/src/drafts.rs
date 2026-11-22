use crate::{compose, engine::Draft, outgoing, providers, Store};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

const PAGE_SIZE: i64 = 50;

#[derive(Serialize)]
pub struct DraftSummary {
    id: String,
    account_id: String,
    subject: String,
    to: String,
    updated: i64,
}

#[derive(Serialize)]
pub struct DraftPage {
    drafts: Vec<DraftSummary>,
    next_offset: Option<i64>,
}

#[tauri::command]
pub fn list_live_drafts(
    store: tauri::State<Store>,
    account_id: String,
    offset: i64,
) -> Result<DraftPage, String> {
    providers::db(&store, |db| list(db, &account_id, offset))
}

pub fn list(db: &rusqlite::Connection, account: &str, offset: i64) -> Result<DraftPage, String> {
    let offset = offset.max(0);
    let mut query = db
        .prepare(
            "SELECT id,account_id,updated,
          CASE WHEN json_valid(payload) THEN substr(json_extract(payload,'$.subject'),1,240) END,
          CASE WHEN json_valid(payload) THEN substr(json_extract(payload,'$.to'),1,240) END
         FROM saved_drafts WHERE queued_job_id IS NULL AND (?1='' OR account_id=?1)
         ORDER BY updated DESC,id DESC LIMIT ?2 OFFSET ?3",
        )
        .map_err(|e| e.to_string())?;
    let mut drafts = query
        .query_map(params![account, PAGE_SIZE + 1, offset], |row| {
            Ok(DraftSummary {
                id: row.get(0)?,
                account_id: row.get(1)?,
                updated: row.get(2)?,
                subject: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                to: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let next_offset = (drafts.len() > PAGE_SIZE as usize).then(|| offset + PAGE_SIZE);
    drafts.truncate(PAGE_SIZE as usize);
    Ok(DraftPage {
        drafts,
        next_offset,
    })
}

#[tauri::command]
pub fn save_live_draft(store: tauri::State<Store>, id: String, draft: Draft) -> Result<(), String> {
    save(&store, &id, &draft)
}

pub fn save(store: &Store, id: &str, draft: &Draft) -> Result<(), String> {
    outgoing::validate_draft_id(id)?;
    compose::validate_draft_storage(draft)?;
    providers::account(store, &draft.account_id)?;
    providers::db(store, |db| {
        let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
        let old = attachment_ids(&tx, id)?;
        for attachment in &draft.attachments {
            let owned: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM files f JOIN outgoing_owners o ON o.file_id=f.id
                 LEFT JOIN outgoing_staged s ON s.file_id=f.id
                 WHERE f.id=?1 AND f.message_id='outgoing' AND o.account_id=?2
                   AND (s.file_id IS NULL OR s.draft_id=?3))",
                    params![attachment, draft.account_id, id],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !owned {
                return Err(
                    "Draft attachment does not belong to this sending account; reattach it".into(),
                );
            }
        }
        let changed = tx.execute(
            "INSERT INTO saved_drafts(id,account_id,payload,updated) VALUES(?1,?2,?3,?4)
             ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id,payload=excluded.payload,updated=excluded.updated
             WHERE saved_drafts.queued_job_id IS NULL",
            params![id, draft.account_id, serde_json::to_string(draft).map_err(|e| e.to_string())?,
                chrono::Utc::now().timestamp_millis()],
        ).map_err(|e| e.to_string())?;
        if changed == 0 {
            return Err("This draft is already queued for sending".into());
        }
        outgoing::unstage_files(&tx, id, &draft.attachments)?;
        let detached: Vec<_> = old
            .into_iter()
            .filter(|file| !draft.attachments.contains(file))
            .collect();
        outgoing::reclaim_files(&tx, &detached)?;
        tx.commit().map_err(|e| e.to_string())
    })
}

fn attachment_ids(db: &rusqlite::Connection, id: &str) -> Result<Vec<String>, String> {
    let files: Option<String> = db.query_row(
        "SELECT CASE WHEN json_valid(payload) THEN json_extract(payload,'$.attachments') END FROM saved_drafts WHERE id=?1",
        [id], |row| row.get::<_, Option<String>>(0),
    )
        .optional().map_err(|e| e.to_string())?.flatten();
    Ok(files
        .as_deref()
        .and_then(|ids| serde_json::from_str(ids).ok())
        .unwrap_or_default())
}

#[tauri::command]
pub fn load_live_draft(store: tauri::State<Store>, id: String) -> Result<Draft, String> {
    providers::db(&store, |db| load(db, &id))
}

pub fn load(db: &rusqlite::Connection, id: &str) -> Result<Draft, String> {
    let raw: String = db
        .query_row(
            "SELECT payload FROM saved_drafts WHERE id=?1 AND queued_job_id IS NULL",
            [id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or("This draft is no longer available")?;
    serde_json::from_str(&raw).map_err(|_| "This draft could not be read".into())
}

#[tauri::command]
pub fn discard_live_draft(store: tauri::State<Store>, id: String) -> Result<(), String> {
    providers::db(&store, |db| discard(db, &id))
}

pub fn discard(db: &rusqlite::Connection, id: &str) -> Result<(), String> {
    outgoing::validate_draft_id(id)?;
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    if tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM saved_drafts WHERE id=?1 AND queued_job_id IS NOT NULL)",
            [id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|e| e.to_string())?
    {
        return Err("This draft is already queued for sending".into());
    }
    let mut before = attachment_ids(&tx, id)?;
    let staged = outgoing::staged_files(&tx, id)?;
    tx.execute("DELETE FROM saved_drafts WHERE id=?1", [id])
        .map_err(|e| e.to_string())?;
    before.extend(outgoing::unstage_files(&tx, id, &staged)?);
    outgoing::reclaim_files(&tx, &before)?;
    tx.commit().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{engine, live};
    use std::sync::{Arc, Mutex};

    const FIRST: &str = "00000000000000000000000000000001";
    const SECOND: &str = "00000000000000000000000000000002";

    fn store() -> Store {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        engine::migrate(&db).unwrap();
        db.execute_batch(
            "CREATE TABLE accounts(id TEXT PRIMARY KEY,provider TEXT,email TEXT);
             INSERT INTO accounts VALUES('a','google','a@example.com'),('b','imap','b@example.com');
             INSERT INTO files VALUES('file-a','outgoing','note.txt','text/plain',X'616263',0);
             INSERT INTO outgoing_owners VALUES('file-a','a');",
        )
        .unwrap();
        Store(Arc::new(Mutex::new(db)))
    }

    fn draft() -> Draft {
        Draft {
            account_id: "a".into(),
            to: "recipient@example.com".into(),
            subject: "Draft subject".into(),
            body: "Draft body".into(),
            attachments: vec!["file-a".into()],
            ..Default::default()
        }
    }

    #[test]
    fn outgoing_file_survives_a_shared_draft_and_is_removed_with_its_last_reference() {
        let store = store();
        let file = {
            let db = store.0.lock().unwrap();
            outgoing::store_file(
                &db,
                "a",
                Some(FIRST),
                "shared.txt".into(),
                "text/plain".into(),
                vec![1],
            )
            .unwrap()
            .id
        };
        let shared = Draft {
            attachments: vec![file.clone()],
            ..draft()
        };
        assert!(save(&store, SECOND, &shared).is_err());
        save(&store, FIRST, &shared).unwrap();
        save(&store, SECOND, &shared).unwrap();
        {
            let db = store.0.lock().unwrap();
            discard(&db, FIRST).unwrap();
            let exists: bool = db
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM files WHERE id=?1)",
                    [&file],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists);
        }
        save(
            &store,
            SECOND,
            &Draft {
                attachments: Vec::new(),
                ..draft()
            },
        )
        .unwrap();
        let db = store.0.lock().unwrap();
        let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM files WHERE id=?1 UNION SELECT 1 FROM outgoing_owners WHERE file_id=?1)", [&file], |row| row.get(0)).unwrap();
        assert!(!exists);
        assert!(load(&db, SECOND).unwrap().attachments.is_empty());
    }

    #[test]
    fn migration_keeps_existing_content_and_attachment_references() {
        let store = store();
        let db = store.0.lock().unwrap();
        db.execute(
            "INSERT INTO live_drafts VALUES('a',?1)",
            [serde_json::to_string(&draft()).unwrap()],
        )
        .unwrap();
        db.pragma_update(None, "user_version", 11).unwrap();
        engine::migrate(&db).unwrap();
        engine::migrate(&db).unwrap();
        let page = list(&db, "a", 0).unwrap();
        assert_eq!(page.drafts.len(), 1);
        let migrated = load(&db, &page.drafts[0].id).unwrap();
        assert_eq!(migrated.subject, "Draft subject");
        assert_eq!(migrated.attachments, ["file-a"]);
        assert!(list(&db, "b", 0).unwrap().drafts.is_empty());
    }

    #[test]
    fn drafts_are_independent_and_account_moves_validate_attachments() {
        let store = store();
        let first = draft();
        let second = Draft {
            subject: "Other draft".into(),
            ..draft()
        };
        save(&store, FIRST, &first).unwrap();
        save(&store, SECOND, &second).unwrap();
        let mut moved = Draft {
            account_id: "b".into(),
            ..first
        };
        assert!(save(&store, FIRST, &moved).is_err());
        moved.attachments.clear();
        save(&store, FIRST, &moved).unwrap();
        let db = store.0.lock().unwrap();
        assert_eq!(list(&db, "a", 0).unwrap().drafts[0].id, SECOND);
        assert_eq!(list(&db, "b", 0).unwrap().drafts[0].id, FIRST);
        discard(&db, FIRST).unwrap();
        assert!(load(&db, FIRST).is_err());
        assert_eq!(load(&db, SECOND).unwrap().subject, "Other draft");
    }

    #[tokio::test]
    async fn sending_transitions_are_atomic_and_preserve_a_recoverable_draft() {
        let store = store();
        let draft = draft();
        save(&store, FIRST, &draft).unwrap();
        {
            let db = store.0.lock().unwrap();
            db.execute_batch("CREATE TRIGGER stop_send BEFORE INSERT ON jobs BEGIN SELECT raise(ABORT,'job write failed'); END;").unwrap();
        }
        assert!(live::queue_send_with_draft(&store, draft.clone(), Some(FIRST)).is_err());
        {
            let db = store.0.lock().unwrap();
            assert_eq!(list(&db, "", 0).unwrap().drafts.len(), 1);
            db.execute_batch("DROP TRIGGER stop_send").unwrap();
        }
        let changed = Draft {
            subject: "Unsaved change".into(),
            ..draft.clone()
        };
        assert!(live::queue_send_with_draft(&store, changed, Some(FIRST)).is_err());
        let job = live::queue_send_with_draft(&store, draft.clone(), Some(FIRST)).unwrap();
        assert!(save(&store, FIRST, &draft).is_err());
        {
            let db = store.0.lock().unwrap();
            assert!(list(&db, "", 0).unwrap().drafts.is_empty());
            assert!(discard(&db, FIRST).is_err());
        }
        live::cancel_job_inner(&store, &job).unwrap();
        {
            let db = store.0.lock().unwrap();
            assert_eq!(load(&db, FIRST).unwrap().attachments, ["file-a"]);
        }
        let job = live::queue_send_with_draft(&store, draft, Some(FIRST)).unwrap();
        {
            let db = store.0.lock().unwrap();
            db.execute("UPDATE jobs SET due=0 WHERE id=?1", [&job])
                .unwrap();
            db.execute("DELETE FROM outbox_mime WHERE job_id=?1", [&job])
                .unwrap();
        }
        assert!(live::process_jobs(&store).await.unwrap());
        let db = store.0.lock().unwrap();
        assert_eq!(load(&db, FIRST).unwrap().body, "Draft body");
    }
}
